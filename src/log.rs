//! Raw byte log and replay (requirement 5). The log is asciicast v2 (the format `ht` and
//! `avt` already use): a header line, then one `[time, "o", text]` event per chunk written.
//! Replaying it feeds the same bytes into a fresh emulator with no timing involved, which is
//! what makes "emulator state after these bytes" a stable golden rather than a live capture.

use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::Path;
use std::time::Instant;

use alacritty_terminal::term::{Config, Term};
use vte::ansi::Processor;

use crate::snapshot::{self, Snapshot};
use crate::{ObserverListener, Size};

pub struct RawLog {
    // Flushed after every event in write_event/finish, by design: a holder killed mid-session
    // (SIGKILL, a crash) must leave a replayable prefix of the log on disk, not lose whatever sat
    // unflushed in the BufWriter's own buffer. The BufWriter still earns its keep by coalescing
    // write_event's own multiple small writes (the timestamp, "o", the text) into one syscall.
    writer: BufWriter<File>,
    start: Instant,
    /// A UTF-8 sequence's leading bytes, held back from the end of one chunk because they were
    /// incomplete, waiting for the rest in the next one. The child's output is read and logged
    /// in fixed-size chunks with no regard for character boundaries, so a character can (and in
    /// practice does) split across two chunks; converting each chunk to text independently would
    /// turn one real character into up to two replacement characters, corrupting the log in a way
    /// `replay` would then reproduce as a *different* screen than the live session actually showed.
    pending: Vec<u8>,
}

impl RawLog {
    pub fn create(path: &Path, cols: u16, rows: u16) -> io::Result<Self> {
        let mut writer = BufWriter::new(File::create(path)?);
        let header = serde_json::json!({
            "version": 2,
            "width": cols,
            "height": rows,
            "timestamp": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        });
        writeln!(writer, "{header}")?;
        Ok(RawLog { writer, start: Instant::now(), pending: Vec::new() })
    }

    /// Record one chunk of the child's output. A UTF-8 sequence split between this chunk and the
    /// next is reassembled rather than corrupted (see `pending`); genuinely invalid bytes are
    /// still replaced lossily, which is fine for a replay log. The forwarded bytes themselves are
    /// never touched either way - only what lands in the log.
    pub fn write_event(&mut self, data: &[u8]) -> io::Result<()> {
        let text = self.convert(data);
        if text.is_empty() {
            return Ok(());
        }
        let t = self.start.elapsed().as_secs_f64();
        let event = serde_json::json!([t, "o", text]);
        writeln!(self.writer, "{event}")?;
        self.writer.flush()
    }

    /// `data`, prefixed with any leftover from the last call, converted to text. A UTF-8 sequence
    /// still incomplete at the very end is held back in `pending` instead of being emitted (call
    /// [`finish`](Self::finish) to flush it lossily once no more bytes are coming); a genuinely
    /// invalid byte sequence becomes one U+FFFD, the same as `String::from_utf8_lossy`.
    fn convert(&mut self, data: &[u8]) -> String {
        let mut input = std::mem::take(&mut self.pending);
        input.extend_from_slice(data);
        let mut out = String::with_capacity(input.len());
        let mut rest = input.as_slice();
        loop {
            match std::str::from_utf8(rest) {
                Ok(valid) => {
                    out.push_str(valid);
                    break;
                }
                Err(e) => {
                    let valid_up_to = e.valid_up_to();
                    // SAFETY/correctness: `valid_up_to` is exactly the boundary `from_utf8`
                    // validated, so this prefix is always valid UTF-8.
                    out.push_str(std::str::from_utf8(&rest[..valid_up_to]).unwrap());
                    match e.error_len() {
                        // A real invalid byte sequence, not just a truncated one: replace it and
                        // keep going with whatever follows it in this same chunk.
                        Some(len) => {
                            out.push('\u{FFFD}');
                            rest = &rest[valid_up_to + len..];
                        }
                        // Ran out of bytes mid-sequence: it may complete in the next chunk.
                        None => {
                            self.pending.extend_from_slice(&rest[valid_up_to..]);
                            break;
                        }
                    }
                }
            }
        }
        out
    }

    /// Flushes any held-back partial UTF-8 sequence lossily, once no more bytes are coming (the
    /// child exited with less than a full character trailing its last chunk). A no-op otherwise.
    pub fn finish(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let text = String::from_utf8_lossy(&std::mem::take(&mut self.pending)).into_owned();
        let t = self.start.elapsed().as_secs_f64();
        let event = serde_json::json!([t, "o", text]);
        writeln!(self.writer, "{event}")?;
        self.writer.flush()
    }
}

/// Rebuild a snapshot by replaying a log written by [`RawLog`], ignoring its timestamps
/// entirely. `cols`/`rows` must match the size the log was recorded at - a replay at the wrong
/// size would reproduce a different (wrong) layout, not an error, so callers should read the
/// header themselves if they don't already know the size. Unlike `Tap::spawn`/`Tap::resize`,
/// `cols`/`rows` here are not bounded by `MAX_TERMINAL_DIMENSION`: a replay is a one-shot,
/// caller-controlled reconstruction, not a live session whose resource use an unbounded remote
/// resize could otherwise run away with - and the test suite's own conformance tests replay a
/// synthetic, deliberately oversized grid (one row per measured character) to tabulate character
/// widths efficiently, with no terminal and no resize involved at all.
pub fn replay(path: &Path, cols: u16, rows: u16) -> io::Result<Snapshot> {
    let cols = cols.max(1);
    let rows = rows.max(1);
    let file = BufReader::new(File::open(path)?);
    let mut lines = file.lines();

    lines.next().transpose()?; // header, not needed for replay itself

    let listener = ObserverListener::new();
    let mut term = Term::new(Config::default(), &Size { cols: cols as usize, rows: rows as usize }, listener.clone());
    let mut parser = Processor::<vte::ansi::StdSyncHandler>::new();
    let mut byte_offset: u64 = 0;

    for line in lines {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let event: serde_json::Value =
            serde_json::from_str(&line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let (Some("o"), Some(text)) = (event.get(1).and_then(|v| v.as_str()), event.get(2).and_then(|v| v.as_str()))
        else {
            continue;
        };
        let bytes = text.as_bytes();
        parser.advance(&mut term, bytes);
        byte_offset += bytes.len() as u64;
    }

    Ok(snapshot::build(&term, byte_offset, listener.current_title()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(path: &Path) -> Vec<String> {
        let text = std::fs::read_to_string(path).unwrap();
        text.lines()
            .skip(1) // header
            .map(|line| {
                let v: serde_json::Value = serde_json::from_str(line).unwrap();
                v.get(2).and_then(|t| t.as_str()).unwrap().to_string()
            })
            .collect()
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("vtee-log-test-{name}-{}.cast", std::process::id()))
    }

    /// A 4-byte character ("🎉", `F0 9F 8E 89`) split across two chunks, as a real pty read
    /// could split it, must come back as one character, not two replacement characters.
    #[test]
    fn a_character_split_across_two_chunks_is_reassembled() {
        let path = scratch("split");
        let mut log = RawLog::create(&path, 80, 24).unwrap();
        let emoji = "🎉".as_bytes();
        log.write_event(&[b"before:", &emoji[..2]].concat()).unwrap();
        log.write_event(&[&emoji[2..], b":after"].concat()).unwrap();
        log.finish().unwrap();

        assert_eq!(events(&path), vec!["before:", "🎉:after"]);
        let _ = std::fs::remove_file(&path);
    }

    /// A character split across three chunks (one byte each) is still reassembled, and nothing
    /// is emitted for a chunk that contributed only a partial character.
    #[test]
    fn a_character_split_across_three_chunks_emits_nothing_until_complete() {
        let path = scratch("split3");
        let mut log = RawLog::create(&path, 80, 24).unwrap();
        let emoji = "🎉".as_bytes();
        log.write_event(&emoji[..1]).unwrap();
        log.write_event(&emoji[1..2]).unwrap();
        log.write_event(&emoji[2..]).unwrap();

        assert_eq!(events(&path), vec!["🎉"]);
        let _ = std::fs::remove_file(&path);
    }

    /// A genuinely invalid byte (not just a truncated character) is still replaced, matching
    /// `String::from_utf8_lossy`.
    #[test]
    fn a_genuinely_invalid_byte_is_replaced_not_held_back() {
        let path = scratch("invalid");
        let mut log = RawLog::create(&path, 80, 24).unwrap();
        log.write_event(b"a\xffb").unwrap();

        assert_eq!(events(&path), vec!["a\u{FFFD}b"]);
        let _ = std::fs::remove_file(&path);
    }

    /// A character left incomplete at the very end (the child exited mid-write) is flushed
    /// lossily by `finish`, rather than silently lost. The complete, non-dependent part of the
    /// same chunk ("end:") is written immediately, without waiting for `finish`.
    #[test]
    fn finish_flushes_a_trailing_incomplete_character_lossily() {
        let path = scratch("trailing");
        let mut log = RawLog::create(&path, 80, 24).unwrap();
        let emoji = "🎉".as_bytes();
        log.write_event(&[b"end:", &emoji[..2]].concat()).unwrap();
        assert_eq!(events(&path), vec!["end:"], "the complete prefix must not wait for the incomplete tail");
        log.finish().unwrap();

        let want = String::from_utf8_lossy(&emoji[..2]).into_owned();
        assert_eq!(events(&path), vec!["end:".to_string(), want]);
        let _ = std::fs::remove_file(&path);
    }
}
