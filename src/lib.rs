//! v0 "tap": spawn a child in a pty, forward its bytes to our own stdout/stdin raw and
//! unmodified (requirement 7, transparent observer mode), while a copy of the child's output
//! also feeds an `alacritty_terminal` emulator so its state can be inspected. The emulator is a
//! passive observer and never alters what is forwarded.

mod log;
pub mod snapshot;

pub use log::{RawLog, replay};
pub use snapshot::{Cell, Color, Cursor, CursorShape, Snapshot};

use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::{Config, Term};
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

type SharedState = Arc<Mutex<TapState>>;

/// A running tap: a child spawned in a pty, forwarded both ways, with a copy of its output
/// also feeding an in-memory terminal emulator.
pub struct Tap {
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    child: Box<dyn Child + Send + Sync>,
    state: SharedState,
    title: ObserverListener,
    output_done: Receiver<()>,
}

impl Tap {
    /// Spawn `program args...` in a pty of the given size, and start forwarding immediately:
    /// child output -> our stdout + the emulator (+ `log_path`, if given), our stdin -> child
    /// input.
    pub fn spawn(program: &str, args: &[String], cols: u16, rows: u16, log_path: Option<&Path>) -> io::Result<Self> {
        // A 0x0 (or 0-in-one-dimension) size is never meaningful and alacritty_terminal
        // panics on a zero-sized grid rather than erroring, so this is a hard floor rather
        // than something callers can opt out of.
        let cols = cols.max(1);
        let rows = rows.max(1);

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
        let state: SharedState = Arc::new(Mutex::new(TapState { term, bytes_seen: 0 }));

        let log = log_path.map(|p| RawLog::create(p, cols, rows)).transpose()?;

        let mut reader = pair.master.try_clone_reader().map_err(to_io_error)?;
        let (done_tx, output_done) = mpsc::channel();
        {
            let state = state.clone();
            thread::spawn(move || {
                forward_output(&mut reader, state, log);
                let _ = done_tx.send(());
            });
        }

        let mut writer = pair.master.take_writer().map_err(to_io_error)?;
        thread::spawn(move || forward_input(&mut writer));

        let master = Arc::new(Mutex::new(pair.master));
        Ok(Tap { master, child, state, title, output_done })
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
        self.state.lock().unwrap().term.resize(Size { cols: cols as usize, rows: rows as usize });
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

    /// The full current state (cells, attributes, cursor, modes, offset - requirement 3 and
    /// 4), atomically with respect to the output-forwarding thread.
    pub fn snapshot(&self) -> Snapshot {
        let state = self.state.lock().unwrap();
        snapshot::build(&state.term, state.bytes_seen, self.title.current_title())
    }

    pub fn bytes_seen(&self) -> u64 {
        self.state.lock().unwrap().bytes_seen
    }

    /// Watch for the real terminal being resized and propagate it to the child pty and the
    /// emulator, so all three stay the same size (a "detail that bites" from HANDOFF.md). Unix
    /// only for now (SIGWINCH); requirement 8 defers Windows (ConPTY) to later.
    #[cfg(unix)]
    pub fn watch_resize(&self) {
        use signal_hook::consts::SIGWINCH;
        use signal_hook::iterator::Signals;

        let master = self.master.clone();
        let state = self.state.clone();
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
                    state.lock().unwrap().term.resize(Size { cols: cols as usize, rows: rows as usize });
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

fn forward_output(reader: &mut Box<dyn Read + Send>, state: SharedState, mut log: Option<RawLog>) {
    let mut stdout = io::stdout();
    let mut parser = Processor::<vte::ansi::StdSyncHandler>::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let chunk = &buf[..n];
        if stdout.write_all(chunk).is_err() {
            break;
        }
        let _ = stdout.flush();
        if let Some(log) = log.as_mut() {
            let _ = log.write_event(chunk);
        }
        {
            let mut state = state.lock().unwrap();
            parser.advance(&mut state.term, chunk);
            state.bytes_seen += n as u64;
        }
    }
}

fn forward_input(writer: &mut Box<dyn Write + Send>) {
    let mut stdin = io::stdin();
    let mut buf = [0u8; 4096];
    loop {
        let n = match stdin.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        if writer.write_all(&buf[..n]).is_err() {
            break;
        }
        let _ = writer.flush();
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
