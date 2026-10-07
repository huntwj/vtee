//! v0 "tap": spawn a child in a pty, forward its bytes to our own stdout/stdin raw and
//! unmodified (requirement 7, transparent observer mode), while a copy of the child's output
//! also feeds an `alacritty_terminal` emulator so its state can be inspected. The emulator is a
//! passive observer and never alters what is forwarded. `SpawnOptions::interactive = false`
//! turns that off entirely for the batch-script case (`docs/roadmap.md`), where the caller
//! drives everything through `Command`s instead of a human at a real terminal.

mod command;
mod keys;
mod log;
pub mod snapshot;

pub use command::{Command, CommandResult};
pub use log::{RawLog, replay};
pub use snapshot::{Cell, Color, Cursor, CursorShape, Snapshot};

use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::{Config, Term, TermMode};
use portable_pty::{Child, CommandBuilder, ExitStatus, MasterPty, PtySize, native_pty_system};
use vte::ansi::Processor;

/// Captures title changes (requirement 3) via `alacritty_terminal`'s event callback, which is
/// the only way to read a title back out - `Term` keeps no public getter for it. Everything
/// else state-related is read straight off `Term`/`Grid` in `snapshot::build`.
#[derive(Clone)]
pub struct ObserverListener {
    title: Arc<Mutex<Option<String>>>,
}

impl ObserverListener {
    pub(crate) fn new() -> Self {
        ObserverListener { title: Arc::new(Mutex::new(None)) }
    }

    pub(crate) fn current_title(&self) -> Option<String> {
        self.title.lock().unwrap().clone()
    }
}

impl EventListener for ObserverListener {
    fn send_event(&self, event: Event) {
        match event {
            Event::Title(t) => *self.title.lock().unwrap() = Some(t),
            Event::ResetTitle => *self.title.lock().unwrap() = None,
            _ => {}
        }
    }
}

/// Size handed to `alacritty_terminal`, which wants a `Dimensions` impl rather than a plain pair
/// of numbers. `total_lines`/`screen_lines` are equal because scrollback comes from the grid's
/// own history (see `snapshot::build`), not from keeping extra lines in the visible area.
#[derive(Clone, Copy)]
pub(crate) struct Size {
    pub(crate) cols: usize,
    pub(crate) rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// `term` and `bytes_seen` are locked together so a snapshot is always a consistent (state,
/// offset) pair - requirement 4's "a query never sees a half-drawn frame," extended to mean the
/// reported offset never disagrees with the state it's reported alongside.
struct TapState {
    term: Term<ObserverListener>,
    bytes_seen: u64,
}

/// Paired with the `Mutex` so `wait_for_text` can block efficiently instead of polling:
/// `forward_output` notifies after every chunk it applies.
struct Shared {
    state: Mutex<TapState>,
    changed: Condvar,
}

type SharedState = Arc<Shared>;
type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

/// How to spawn a [`Tap`]. `interactive` is the one switch between this project's two modes
/// (`docs/roadmap.md`): a human at a real terminal (forward both ways, raw), or a script
/// driving `Command`s with nothing forwarded automatically.
pub struct SpawnOptions<'a> {
    pub cols: u16,
    pub rows: u16,
    pub log_path: Option<&'a Path>,
    pub interactive: bool,
}

impl Default for SpawnOptions<'_> {
    fn default() -> Self {
        SpawnOptions { cols: 80, rows: 24, log_path: None, interactive: true }
    }
}

/// A running tap: a child spawned in a pty, with a copy of its output feeding an in-memory
/// terminal emulator, and (when `SpawnOptions::interactive`) forwarded both ways to our own
/// real stdin/stdout.
pub struct Tap {
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    writer: SharedWriter,
    child: Box<dyn Child + Send + Sync>,
    shared: SharedState,
    title: ObserverListener,
    output_done: Receiver<()>,
}

impl Tap {
    /// Spawn `program args...` in a pty per `opts`, and start forwarding immediately.
    pub fn spawn(program: &str, args: &[String], opts: SpawnOptions) -> io::Result<Self> {
        // A 0x0 (or 0-in-one-dimension) size is never meaningful and alacritty_terminal
        // panics on a zero-sized grid rather than erroring, so this is a hard floor rather
        // than something callers can opt out of.
        let cols = opts.cols.max(1);
        let rows = opts.rows.max(1);

        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).map_err(to_io_error)?;

        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);
        if let Ok(cwd) = std::env::current_dir() {
            cmd.cwd(cwd);
        }
        let child = pair.slave.spawn_command(cmd).map_err(to_io_error)?;
        // Our process doesn't need its own handle to the slave side once the child has it;
        // dropping it means the master sees EOF once the child's copy of the slave closes too.
        drop(pair.slave);

        let title = ObserverListener::new();
        let term = Term::new(Config::default(), &Size { cols: cols as usize, rows: rows as usize }, title.clone());
        let shared: SharedState =
            Arc::new(Shared { state: Mutex::new(TapState { term, bytes_seen: 0 }), changed: Condvar::new() });

        let log = opts.log_path.map(|p| RawLog::create(p, cols, rows)).transpose()?;

        let mut reader = pair.master.try_clone_reader().map_err(to_io_error)?;
        let (done_tx, output_done) = mpsc::channel();
        {
            let shared = shared.clone();
            let write_raw = opts.interactive;
            thread::spawn(move || {
                forward_output(&mut reader, shared, log, write_raw);
                let _ = done_tx.send(());
            });
        }

        let writer: SharedWriter = Arc::new(Mutex::new(pair.master.take_writer().map_err(to_io_error)?));
        if opts.interactive {
            let writer = writer.clone();
            thread::spawn(move || forward_input(writer));
        }

        let master = Arc::new(Mutex::new(pair.master));
        Ok(Tap { master, writer, child, shared, title, output_done })
    }

    /// Resize the child's pty and the emulator together, so they never disagree about the
    /// screen size (a "detail that bites" from HANDOFF.md's design notes).
    pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        let cols = cols.max(1);
        let rows = rows.max(1);
        self.master
            .lock()
            .unwrap()
            .resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(to_io_error)?;
        self.shared.state.lock().unwrap().term.resize(Size { cols: cols as usize, rows: rows as usize });
        Ok(())
    }

    /// Block until the child exits, then give the output-forwarding thread a brief grace
    /// period to drain. On a real Unix pty, closing the slave delivers EOF to the master
    /// reader right away and this returns almost immediately. On Windows, ConPTY is known to
    /// not EOF the master reader just because the child exited, so this never blocks forever
    /// waiting for that - it waits briefly, then moves on regardless (requirement 8 defers a
    /// proper fix to the Windows port).
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        let status = self.child.wait().map_err(to_io_error)?;
        let _ = self.output_done.recv_timeout(Duration::from_millis(500));
        Ok(status)
    }

    /// Terminate the child. Used by the batch CLI, which doesn't assume a script leaves the
    /// program in a state where it'll exit on its own - deterministic over hoping a `wait()`
    /// eventually returns. Safe to call on an already-exited child (the underlying error is
    /// swallowed, not propagated, since "already gone" isn't a failure from a caller's
    /// perspective).
    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }

    /// The full current state (cells, attributes, cursor, modes, offset - requirement 3 and
    /// 4), atomically with respect to the output-forwarding thread.
    pub fn snapshot(&self) -> Snapshot {
        let state = self.shared.state.lock().unwrap();
        snapshot::build(&state.term, state.bytes_seen, self.title.current_title())
    }

    pub fn bytes_seen(&self) -> u64 {
        self.shared.state.lock().unwrap().bytes_seen
    }

    /// Whether application-cursor-key mode (DECCKM, `?1`) is active, so `send_key` can pick
    /// the right escape form for arrows/Home/End (`src/keys.rs`).
    pub fn app_cursor_mode(&self) -> bool {
        self.shared.state.lock().unwrap().term.mode().contains(TermMode::APP_CURSOR)
    }

    /// Write raw bytes to the child's input. Safe to call alongside interactive stdin
    /// forwarding - both go through the same lock, so one call's bytes can't interleave with
    /// another's (`docs/roadmap.md`, "Input injection"; *when* an agent should be allowed to
    /// inject while a human is also typing is still an open policy question, not a
    /// data-corruption risk).
    pub fn send_bytes(&self, bytes: &[u8]) -> io::Result<()> {
        let mut w = self.writer.lock().unwrap();
        w.write_all(bytes)?;
        w.flush()
    }

    pub fn send_text(&self, text: &str) -> io::Result<()> {
        self.send_bytes(text.as_bytes())
    }

    /// Send a named key (`src/keys.rs`), resolved against the terminal's current
    /// application-cursor-mode state.
    pub fn send_key(&self, key: &str) -> io::Result<()> {
        let bytes = keys::resolve(key, self.app_cursor_mode())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("unknown key {key:?}")))?;
        self.send_bytes(&bytes)
    }

    /// Block until the screen's text contains `text` or `timeout` elapses; returns whether it
    /// matched, and the snapshot that was true of at that moment. The scoped-down
    /// wait-for-predicate TinyFugue actually asked for (`docs/roadmap.md`): "text appears,"
    /// not an arbitrary predicate over the full `Snapshot`. Wakes on new output via a
    /// `Condvar` rather than polling.
    pub fn wait_for_text(&self, text: &str, timeout: Duration) -> (bool, Snapshot) {
        let deadline = Instant::now() + timeout;
        let mut guard = self.shared.state.lock().unwrap();
        loop {
            let snap = snapshot::build(&guard.term, guard.bytes_seen, self.title.current_title());
            if snap.lines.iter().any(|l| l.contains(text)) {
                return (true, snap);
            }
            let now = Instant::now();
            if now >= deadline {
                return (false, snap);
            }
            let (g, _) = self.shared.changed.wait_timeout(guard, deadline - now).unwrap();
            guard = g;
        }
    }

    /// Watch for the real terminal being resized and propagate it to the child pty and the
    /// emulator, so all three stay the same size (a "detail that bites" from HANDOFF.md). Unix
    /// only for now (SIGWINCH); requirement 8 defers Windows (ConPTY) to later. Only meaningful
    /// in interactive mode - a script drives resizes explicitly via `Command::Resize`.
    #[cfg(unix)]
    pub fn watch_resize(&self) {
        use signal_hook::consts::SIGWINCH;
        use signal_hook::iterator::Signals;

        let master = self.master.clone();
        let shared = self.shared.clone();
        let Ok(mut signals) = Signals::new([SIGWINCH]) else { return };
        thread::spawn(move || {
            for _ in signals.forever() {
                // A 0x0 report is a transient non-size (seen under `docker run -t` before a
                // real size is negotiated), not an actual resize to nothing - skip it rather
                // than clamping to 1x1, which would be a real, visible, wrong resize.
                if let Ok((cols, rows)) = crossterm::terminal::size() {
                    if cols == 0 || rows == 0 {
                        continue;
                    }
                    let _ = master.lock().unwrap().resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
                    shared.state.lock().unwrap().term.resize(Size { cols: cols as usize, rows: rows as usize });
                }
            }
        });
    }

    #[cfg(not(unix))]
    pub fn watch_resize(&self) {
        // TODO(requirement 8): Windows resize watching (ConPTY has no SIGWINCH equivalent;
        // needs polling console size or a console resize event).
    }
}

/// A safety net, not the primary cleanup path: whatever causes a `Tap` to be dropped -
/// including an early return or a `?` after the child was spawned - this stops the child from
/// being silently orphaned. Harmless if the child already exited (the error from killing an
/// already-dead process is swallowed, same as `kill()`). This only runs on a normal drop, not
/// through `std::process::exit()` (which skips destructors entirely) - callers that might call
/// `std::process::exit` while a `Tap` is still alive need to drop or kill it explicitly first.
impl Drop for Tap {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn forward_output(reader: &mut Box<dyn Read + Send>, shared: SharedState, mut log: Option<RawLog>, write_raw: bool) {
    let mut stdout = io::stdout();
    let mut parser = Processor::<vte::ansi::StdSyncHandler>::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let chunk = &buf[..n];
        if write_raw {
            if stdout.write_all(chunk).is_err() {
                break;
            }
            let _ = stdout.flush();
        }
        if let Some(log) = log.as_mut() {
            let _ = log.write_event(chunk);
        }
        {
            let mut state = shared.state.lock().unwrap();
            parser.advance(&mut state.term, chunk);
            state.bytes_seen += n as u64;
            shared.changed.notify_all();
        }
    }
}

fn forward_input(writer: SharedWriter) {
    let mut stdin = io::stdin();
    let mut buf = [0u8; 4096];
    loop {
        let n = match stdin.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let mut w = writer.lock().unwrap();
        if w.write_all(&buf[..n]).is_err() {
            break;
        }
        let _ = w.flush();
    }
}

fn to_io_error(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

/// Puts our own controlling terminal into raw mode for the life of the guard, so keystrokes
/// reach the child unmodified (requirement 7), and restores it on drop - including on panic
/// unwind, so a bug in vtee doesn't leave the human's shell broken. A no-op when stdin isn't
/// a real terminal (CI/scripted use, requirement 1), since raw mode doesn't apply there.
pub struct RawModeGuard {
    active: bool,
}

impl RawModeGuard {
    pub fn new() -> Self {
        use std::io::IsTerminal;
        let active = io::stdin().is_terminal() && crossterm::terminal::enable_raw_mode().is_ok();
        RawModeGuard { active }
    }
}

impl Default for RawModeGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = crossterm::terminal::disable_raw_mode();
        }
    }
}
