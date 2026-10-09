//! `Tap`: a child spawned in a pty, its output forwarded raw to our stdout (requirement 7) while
//! a copy feeds an `alacritty_terminal` emulator that stays a passive observer.

use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::{Config, Term, TermMode};
use portable_pty::{Child, CommandBuilder, ExitStatus, MasterPty, PtySize, native_pty_system};
use vte::ansi::Processor;

use crate::serve::ExclusiveLock;
use crate::{RawLog, Snapshot, keys, snapshot};

/// Upper bound on a requested terminal size; the emulator allocates cols x rows cells, so an
/// unbounded remote resize could otherwise OOM the whole session.
const MAX_TERMINAL_DIMENSION: u16 = 1000;

/// A panic on one thread poisons every lock it held, and plain `unwrap()` would then make every
/// later `snapshot()` panic too. The data behind these locks is consistent between statements,
/// so recovering the guard is safe.
pub(crate) trait LockExt<T> {
    fn lock_unpoisoned(&self) -> MutexGuard<'_, T>;
}

impl<T> LockExt<T> for Mutex<T> {
    fn lock_unpoisoned(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Captures title changes via `alacritty_terminal`'s event callback, the only way to read a
/// title back out of `Term`.
#[derive(Clone)]
pub(crate) struct ObserverListener {
    title: Arc<Mutex<Option<String>>>,
}

impl ObserverListener {
    pub(crate) fn new() -> Self {
        ObserverListener { title: Arc::new(Mutex::new(None)) }
    }

    pub(crate) fn current_title(&self) -> Option<String> {
        self.title.lock_unpoisoned().clone()
    }
}

impl EventListener for ObserverListener {
    fn send_event(&self, event: Event) {
        match event {
            Event::Title(t) => *self.title.lock_unpoisoned() = Some(t),
            Event::ResetTitle => *self.title.lock_unpoisoned() = None,
            _ => {}
        }
    }
}

/// The `Dimensions` impl `alacritty_terminal` wants. Scrollback comes from the grid's own
/// history, so total and screen lines are equal.
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

/// Everything behind one lock so a snapshot is a consistent (state, offset) pair, and so
/// "snapshot the screen and register for future chunks" is atomic against `forward_output`'s
/// "apply a chunk and broadcast it".
///
/// Each TTY sink is a bounded channel drained by its own writer thread: writing to the sink
/// directly under this lock would let one client that stops reading block every other client,
/// `snapshot()`, and the pty. A sink whose queue fills up is dropped instead.
pub(crate) struct TapState {
    pub(crate) term: Term<ObserverListener>,
    pub(crate) bytes_seen: u64,
    pub(crate) tty_sinks: Vec<(u64, SyncSender<Arc<[u8]>>)>,
}

/// Paired with the `Mutex` so `wait_for_text` can block instead of polling: `forward_output`
/// notifies after every chunk it applies.
pub(crate) struct Shared {
    pub(crate) state: Mutex<TapState>,
    pub(crate) changed: Condvar,
}

type SharedState = Arc<Shared>;
type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

/// How to spawn a [`Tap`]. `forward_input`/`forward_output` are independent: the interactive
/// case sets both, the batch CLI neither, and a session that lets a human watch while a remote
/// client drives the pty sets only `forward_output`.
pub struct SpawnOptions<'a> {
    pub cols: u16,
    pub rows: u16,
    pub log_path: Option<&'a Path>,
    /// Forward our own stdin to the child (a human's keystrokes, requirement 7).
    pub forward_input: bool,
    /// Forward the child's raw output to our own stdout (a human's view, requirement 7).
    pub forward_output: bool,
    /// Extra environment variables for the child, layered on top of our inherited environment.
    pub env: &'a [(String, String)],
}

impl Default for SpawnOptions<'_> {
    fn default() -> Self {
        SpawnOptions { cols: 80, rows: 24, log_path: None, forward_input: true, forward_output: true, env: &[] }
    }
}

/// A running tap: a child in a pty, an emulator watching a copy of its output, and optional
/// forwarding to our own stdin/stdout.
pub struct Tap {
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    writer: SharedWriter,
    // Mutex because `Arc<Tap>` is shared across server threads, so every field must be `Sync`.
    child: Mutex<Box<dyn Child + Send + Sync>>,
    pub(crate) shared: SharedState,
    pub(crate) title: ObserverListener,
    // Receiver is `Send` but not `Sync`.
    output_done: Mutex<Receiver<()>>,
    // The three below are only read by the Unix socket servers.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(crate) exclusive_lock: Arc<ExclusiveLock>,
    // Set by `Tap::serve` after the stdin-forwarding thread is already running, hence not a
    // constructor argument.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(crate) human_revokes_lock: Arc<AtomicBool>,
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(crate) next_connection_id: Arc<AtomicU64>,
}

impl Tap {
    /// Spawn `program args...` in a pty per `opts`, and start forwarding immediately.
    pub fn spawn(program: &str, args: &[String], opts: SpawnOptions) -> io::Result<Self> {
        // alacritty_terminal panics on a zero-sized grid.
        let cols = opts.cols.max(1);
        let rows = opts.rows.max(1);

        // ConPTY's `openpty()` was observed to hang when the process has no console. Gated on
        // `forward_output` because a new console window is wrong for --headless/--script.
        #[cfg(windows)]
        if opts.forward_output {
            unsafe {
                winapi::um::consoleapi::AllocConsole();
            }
        }

        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).map_err(to_io_error)?;

        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);
        if let Ok(cwd) = std::env::current_dir() {
            cmd.cwd(cwd);
        }
        for (key, value) in opts.env {
            cmd.env(key, value);
        }
        let child = pair.slave.spawn_command(cmd).map_err(to_io_error)?;
        // With our slave handle closed, the master sees EOF once the child's copy closes too.
        drop(pair.slave);

        let title = ObserverListener::new();
        let term = Term::new(Config::default(), &Size { cols: cols as usize, rows: rows as usize }, title.clone());
        let shared: SharedState = Arc::new(Shared {
            state: Mutex::new(TapState { term, bytes_seen: 0, tty_sinks: Vec::new() }),
            changed: Condvar::new(),
        });

        let log = opts.log_path.map(|p| RawLog::create(p, cols, rows)).transpose()?;

        let mut reader = pair.master.try_clone_reader().map_err(to_io_error)?;
        let (done_tx, output_done) = mpsc::channel();
        {
            let shared = shared.clone();
            let write_raw = opts.forward_output;
            thread::spawn(move || {
                forward_output(&mut reader, shared, log, write_raw);
                let _ = done_tx.send(());
            });
        }

        let exclusive_lock = Arc::new(ExclusiveLock::default());
        let human_revokes_lock = Arc::new(AtomicBool::new(false));

        let writer: SharedWriter = Arc::new(Mutex::new(pair.master.take_writer().map_err(to_io_error)?));
        if opts.forward_input {
            let writer = writer.clone();
            let exclusive_lock = exclusive_lock.clone();
            let human_revokes_lock = human_revokes_lock.clone();
            thread::spawn(move || forward_input(writer, exclusive_lock, human_revokes_lock));
        }

        Ok(Tap {
            master: Arc::new(Mutex::new(pair.master)),
            writer,
            child: Mutex::new(child),
            shared,
            title,
            output_done: Mutex::new(output_done),
            exclusive_lock,
            human_revokes_lock,
            next_connection_id: Arc::new(AtomicU64::new(1)),
        })
    }

    /// Resize the child's pty and the emulator together so they never disagree.
    pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if cols > MAX_TERMINAL_DIMENSION || rows > MAX_TERMINAL_DIMENSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "terminal size {cols}x{rows} exceeds the {MAX_TERMINAL_DIMENSION}x{MAX_TERMINAL_DIMENSION} limit"
                ),
            ));
        }
        self.master
            .lock_unpoisoned()
            .resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(to_io_error)?;
        self.shared.state.lock_unpoisoned().term.resize(Size { cols: cols as usize, rows: rows as usize });
        Ok(())
    }

    /// Block until the child exits, then give the output thread a short grace period to drain.
    /// ConPTY doesn't EOF the master reader when the child exits, so this can't wait for that.
    /// Holds `child`'s lock while blocked; concurrent callers should use `clone_killer()`
    /// instead (as `watch_sigterm` does).
    pub fn wait(&self) -> io::Result<ExitStatus> {
        let status = self.child.lock_unpoisoned().wait().map_err(to_io_error)?;
        let _ = self.output_done.lock_unpoisoned().recv_timeout(Duration::from_millis(500));
        Ok(status)
    }

    /// Terminate the child. Safe on an already-exited child.
    pub fn kill(&self) {
        let _ = self.child.lock_unpoisoned().kill();
    }

    /// The full current state, atomic with respect to the output-forwarding thread.
    pub fn snapshot(&self) -> Snapshot {
        let state = self.shared.state.lock_unpoisoned();
        snapshot::build(&state.term, state.bytes_seen, self.title.current_title())
    }

    pub fn bytes_seen(&self) -> u64 {
        self.shared.state.lock_unpoisoned().bytes_seen
    }

    /// Whether application-cursor-key mode (DECCKM) is active, which picks the escape form
    /// `send_key` uses for arrows/Home/End.
    pub fn app_cursor_mode(&self) -> bool {
        self.shared.state.lock_unpoisoned().term.mode().contains(TermMode::APP_CURSOR)
    }

    /// Write raw bytes to the child's input. Interactive stdin forwarding shares this lock, so
    /// writes never interleave.
    pub fn send_bytes(&self, bytes: &[u8]) -> io::Result<()> {
        let mut w = self.writer.lock_unpoisoned();
        w.write_all(bytes)?;
        w.flush()
    }

    pub fn send_text(&self, text: &str) -> io::Result<()> {
        self.send_bytes(text.as_bytes())
    }

    /// Send a named key (`src/keys.rs`), resolved against the current cursor-key mode.
    pub fn send_key(&self, key: &str) -> io::Result<()> {
        let bytes = keys::resolve(key, self.app_cursor_mode())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("unknown key {key:?}")))?;
        self.send_bytes(&bytes)
    }

    /// Block until the screen text contains `text` or `timeout` elapses; returns whether it
    /// matched and the snapshot at that moment. Wakes on new output via a `Condvar`.
    pub fn wait_for_text(&self, text: &str, timeout: Duration) -> (bool, Snapshot) {
        let deadline = Instant::now() + timeout;
        let mut guard = self.shared.state.lock_unpoisoned();
        loop {
            let snap = snapshot::build(&guard.term, guard.bytes_seen, self.title.current_title());
            if snap.lines.iter().any(|l| l.contains(text)) {
                return (true, snap);
            }
            let now = Instant::now();
            if now >= deadline {
                return (false, snap);
            }
            let (g, _) =
                self.shared.changed.wait_timeout(guard, deadline - now).unwrap_or_else(PoisonError::into_inner);
            guard = g;
        }
    }

    /// Propagate the real terminal's resizes (`SIGWINCH`) to the pty and emulator. Unix only
    /// (requirement 8); a script drives resizes explicitly instead.
    pub fn watch_resize(&self) {
        #[cfg(unix)]
        {
            use signal_hook::consts::SIGWINCH;
            use signal_hook::iterator::Signals;

            let master = self.master.clone();
            let shared = self.shared.clone();
            let Ok(mut signals) = Signals::new([SIGWINCH]) else { return };
            thread::spawn(move || {
                for _ in signals.forever() {
                    // A 0x0 report is a transient non-size (seen under `docker run -t`), not a
                    // resize to nothing; skip it rather than clamp to a visible 1x1.
                    if let Ok((cols, rows)) = crossterm::terminal::size() {
                        if cols == 0 || rows == 0 {
                            continue;
                        }
                        let _ =
                            master.lock_unpoisoned().resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
                        shared.state.lock_unpoisoned().term.resize(Size { cols: cols as usize, rows: rows as usize });
                    }
                }
            });
        }
    }

    /// Catch `SIGTERM`: restore the terminal mode, kill the child, exit. Uses `clone_killer`
    /// because the thread that owns the `Tap` may be blocked in `wait()`. Unix only.
    pub fn watch_sigterm(&self) {
        #[cfg(unix)]
        {
            use signal_hook::consts::SIGTERM;
            use signal_hook::iterator::Signals;

            let mut killer = self.child.lock_unpoisoned().clone_killer();
            let Ok(mut signals) = Signals::new([SIGTERM]) else { return };
            thread::spawn(move || {
                if signals.forever().next().is_some() {
                    let _ = crossterm::terminal::disable_raw_mode();
                    let _ = killer.kill();
                    std::process::exit(143); // 128 + SIGTERM
                }
            });
        }
    }
}

/// Safety net for early returns after the child was spawned. Does not run through
/// `std::process::exit`, so callers that exit while a `Tap` is alive must kill it first.
impl Drop for Tap {
    fn drop(&mut self) {
        let _ = self.child.lock_unpoisoned().kill();
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
            let mut state = shared.state.lock_unpoisoned();
            parser.advance(&mut state.term, chunk);
            state.bytes_seen += n as u64;
            // `try_send` never blocks: a sink with a full queue (stalled reader) or a gone
            // writer thread is dropped here.
            if !state.tty_sinks.is_empty() {
                let shared_chunk: Arc<[u8]> = chunk.into();
                state.tty_sinks.retain(|(_, tx)| tx.try_send(shared_chunk.clone()).is_ok());
            }
            shared.changed.notify_all();
        }
        // std's Mutex isn't fair: under a flooding child this loop re-takes the lock before a
        // waiting `snapshot()` caller ever gets scheduled, starving it.
        thread::yield_now();
    }
}

fn forward_input(writer: SharedWriter, exclusive_lock: Arc<ExclusiveLock>, human_revokes_lock: Arc<AtomicBool>) {
    let mut stdin = io::stdin();
    let mut buf = [0u8; 4096];
    loop {
        let n = match stdin.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        // A human's typing is never delayed by `Exclusive`: write first, and only then decide
        // whether the keystroke also revokes a remote holder (`HumanPriority`).
        {
            let mut w = writer.lock_unpoisoned();
            if w.write_all(&buf[..n]).is_err() {
                break;
            }
            let _ = w.flush();
        }
        if human_revokes_lock.load(Ordering::Relaxed) && exclusive_lock.revoke() {
            ::log::debug!("human keystroke revoked the remote write lock");
        }
    }
}

fn to_io_error(e: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::other(e)
}
