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
    writer: BufWriter<File>,
    start: Instant,
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
        Ok(RawLog { writer, start: Instant::now() })
    }

    /// Record one chunk of the child's output. Non-UTF8 bytes are replaced lossily - the same
    /// limitation `avt`'s own `feed_str`-only API has (see docs/decisions.md); acceptable for a
    /// debugging/replay log, not for the forwarded bytes themselves (which this never touches).
    pub fn write_event(&mut self, data: &[u8]) -> io::Result<()> {
        let t = self.start.elapsed().as_secs_f64();
        let text = String::from_utf8_lossy(data);
        let event = serde_json::json!([t, "o", text]);
        writeln!(self.writer, "{event}")?;
        self.writer.flush()
    }
}

/// Rebuild a snapshot by replaying a log written by [`RawLog`], ignoring its timestamps
/// entirely. `cols`/`rows` must match the size the log was recorded at - a replay at the wrong
/// size would reproduce a different (wrong) layout, not an error, so callers should read the
/// header themselves if they don't already know the size.
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
