//! Unix-domain-socket servers for a `Tap`: the NDJSON command protocol and the raw-TTY protocol.

use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;

use crate::net::{UnixListener, UnixStream};
use crate::tap::LockExt;
use crate::{Command, CommandResult, HumanPriority, ServeOptions, Tap, WritePolicy, snapshot};

/// Chunks, not bytes: at the 8 KiB read size `forward_output` uses, this bounds a stalled sink
/// to ~2 MiB of buffered output before it is dropped rather than allowed to block everyone.
const TTY_SINK_QUEUE_CAPACITY: usize = 256;

/// One NDJSON command line; a client that never sends a newline would otherwise grow the
/// holder's memory without bound.
const MAX_COMMAND_LINE_BYTES: u64 = 1024 * 1024;

/// Per listener; each connection costs at least one thread.
const MAX_CONNECTIONS: usize = 64;

/// Decrements the live-connection count when its handler thread ends, panic or not.
struct ConnectionSlot(Arc<AtomicUsize>);

impl ConnectionSlot {
    fn try_acquire(live: &Arc<AtomicUsize>) -> Option<Self> {
        if live.fetch_add(1, Ordering::AcqRel) >= MAX_CONNECTIONS {
            live.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        Some(ConnectionSlot(live.clone()))
    }
}

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(crate) fn serve(tap: Arc<Tap>, path: &Path, opts: ServeOptions) -> io::Result<()> {
    // Set explicitly either way so a second `serve` with another policy can't leave a stale
    // `true`; only `Exclusive` + `Participant` lets a keystroke revoke a remote holder.
    let participant =
        matches!(opts.write_policy, WritePolicy::Exclusive { human_priority: HumanPriority::Participant });
    tap.human_revokes_lock.store(participant, Ordering::Relaxed);

    accept_loop(tap, path, move |tap, stream| serve_connection(tap, stream, opts))
}

pub(crate) fn serve_tty(tap: Arc<Tap>, path: &Path, policy: WritePolicy) -> io::Result<()> {
    accept_loop(tap, path, move |tap, stream| serve_tty_connection(tap, stream, policy))
}

/// Binds `path` (removing a stale socket first) and runs a thread per accepted connection, up
/// to `MAX_CONNECTIONS`; excess connections are dropped.
fn accept_loop(
    tap: Arc<Tap>,
    path: &Path,
    handler: impl Fn(&Tap, UnixStream) + Send + Sync + 'static,
) -> io::Result<()> {
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    let live = Arc::new(AtomicUsize::new(0));
    let handler = Arc::new(handler);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let Some(slot) = ConnectionSlot::try_acquire(&live) else { continue };
            let (tap, handler) = (tap.clone(), handler.clone());
            thread::spawn(move || {
                let _slot = slot;
                handler(&tap, stream)
            });
        }
    });
    Ok(())
}

impl Tap {
    /// Queues a redraw of the current screen and registers the sink for every later chunk, both
    /// under the lock `forward_output` holds while applying and broadcasting a chunk. The redraw
    /// is the first item in a fresh channel, so it always precedes live output. The blocking
    /// writes happen on a dedicated thread, never under that lock.
    fn attach_tty_sink(&self, id: u64, sink: Box<dyn Write + Send>) {
        let (tx, rx) = mpsc::sync_channel::<Arc<[u8]>>(TTY_SINK_QUEUE_CAPACITY);
        {
            let mut state = self.shared.state.lock_unpoisoned();
            let snap = snapshot::build(&state.term, state.bytes_seen, self.title.current_title());
            let _ = tx.try_send(snapshot::redraw_sequence(&snap).into());
            state.tty_sinks.push((id, tx));
        }
        thread::spawn(move || {
            let mut sink = sink;
            for chunk in rx {
                if sink.write_all(&chunk).is_err() || sink.flush().is_err() {
                    break;
                }
            }
        });
    }

    fn detach_tty_sink(&self, id: u64) {
        self.shared.state.lock_unpoisoned().tty_sinks.retain(|(sink_id, _)| *sink_id != id);
    }
}

/// Counts this NDJSON connection in `Tap::open_connections` for as long as it lives, so the holder
/// can linger briefly for a client that is waiting on the child's exit.
struct OpenConnection<'a>(&'a Tap);

impl<'a> OpenConnection<'a> {
    fn new(tap: &'a Tap) -> Self {
        tap.open_connections.fetch_add(1, Ordering::AcqRel);
        OpenConnection(tap)
    }
}

impl Drop for OpenConnection<'_> {
    fn drop(&mut self) {
        self.0.open_connections.fetch_sub(1, Ordering::AcqRel);
    }
}

fn serve_connection(tap: &Tap, stream: UnixStream, opts: ServeOptions) {
    let _open = OpenConnection::new(tap);
    let id = tap.next_connection_id.fetch_add(1, Ordering::Relaxed);
    ::log::debug!("connection {id}: accepted");
    let Ok(reader_stream) = stream.try_clone() else { return };
    let mut reader = io::BufReader::new(reader_stream);
    let mut writer = stream;
    let mut raw_line = Vec::new();
    loop {
        raw_line.clear();
        let read = io::BufRead::read_until(&mut (&mut reader).take(MAX_COMMAND_LINE_BYTES + 1), b'\n', &mut raw_line);
        if matches!(read, Ok(0) | Err(_)) {
            break;
        }
        if raw_line.len() as u64 > MAX_COMMAND_LINE_BYTES && !raw_line.ends_with(b"\n") {
            ::log::debug!("connection {id}: command line over {MAX_COMMAND_LINE_BYTES} bytes, closing");
            break;
        }
        let line = String::from_utf8_lossy(&raw_line);
        if line.trim().is_empty() {
            continue;
        }
        let Some(result) = handle_line(tap, id, opts, &line) else { continue };
        if let CommandResult::Error { message } = &result {
            ::log::debug!("connection {id}: rejected - {message}");
        }
        let Ok(text) = serde_json::to_string(&result) else { continue };
        if writeln!(writer, "{text}").is_err() {
            break;
        }
    }
    ::log::debug!("connection {id}: disconnected");
    // However the connection ends, it must not keep the exclusive lock.
    tap.exclusive_lock.release_if_held_by(id);
}

/// Parses, authorizes and executes one command line; `None` means there is nothing to report.
fn handle_line(tap: &Tap, id: u64, opts: ServeOptions, line: &str) -> Option<CommandResult> {
    let error = |message: String| Some(CommandResult::Error { message });
    let cmd = match serde_json::from_str::<Command>(line) {
        Ok(cmd) => cmd,
        Err(e) => return error(format!("bad command: {e}")),
    };
    if let Err(denied) = cmd.authorize(opts.write_policy, opts.resize_owned_by_terminal) {
        return error(denied.to_string());
    }
    match cmd {
        Command::AcquireWrite => {
            let acquired = tap.exclusive_lock.try_acquire(id);
            ::log::debug!("connection {id}: acquire_write -> {acquired}");
            Some(CommandResult::WriteAccess { acquired })
        }
        Command::ReleaseWrite => {
            tap.exclusive_lock.release_if_held_by(id);
            ::log::debug!("connection {id}: released write access");
            Some(CommandResult::WriteAccess { acquired: false })
        }
        // `authorize` can't know which connection is asking, so whether this one holds the
        // lock is checked here.
        Command::Send { .. } | Command::SendKey { .. } | Command::Resize { .. }
            if !tap.exclusive_lock.permits(opts.write_policy, id) =>
        {
            error("write access not held - send acquire_write first".to_string())
        }
        _ => match cmd.execute(tap) {
            Ok(result) => result,
            Err(e) => error(e.to_string()),
        },
    }
}

/// A TTY client is a live output sink plus a writer: every byte it sends is gated by `policy`
/// through the same `ExclusiveLock` a JSON client's `Send` uses.
fn serve_tty_connection(tap: &Tap, stream: UnixStream, policy: WritePolicy) {
    let id = tap.next_connection_id.fetch_add(1, Ordering::Relaxed);
    ::log::debug!("tty connection {id}: accepted");
    let Ok(mut reader) = stream.try_clone() else { return };
    // Before the redraw is queued, so a client that has seen the screen has already been counted.
    tap.touch_activity();
    tap.attach_tty_sink(id, Box::new(stream));

    let mut buf = [0u8; 4096];
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        if tap.exclusive_lock.permits(policy, id) && tap.send_bytes(&buf[..n]).is_err() {
            break;
        }
    }
    ::log::debug!("tty connection {id}: disconnected");
    tap.detach_tty_sink(id);
    tap.exclusive_lock.release_if_held_by(id);
}
