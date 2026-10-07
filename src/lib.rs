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
    // A `Mutex`, not a plain field, like everything else here that needs to work from more
    // than one thread: `Tap::serve` (observation, `docs/roadmap.md`) shares a `Tap` across an
    // accept loop and per-connection threads, none of which can assume exclusive access.
    child: Mutex<Box<dyn Child + Send + Sync>>,
    shared: SharedState,
    title: ObserverListener,
    // `Receiver` is `Send` but not `Sync` - wrapped for the same reason `child` is: `Arc<Tap>`
    // (observation, `Tap::serve`) needs every field to be `Sync`, not just usable from one
    // thread at a time the way a bare `Receiver` is.
    output_done: Mutex<Receiver<()>>,
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
        let child = Mutex::new(child);
        let output_done = Mutex::new(output_done);
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
    /// Blocks holding `child`'s lock for as long as the child runs - fine today since nothing
    /// else needs that lock while this is in flight (`watch_sigterm` deliberately uses its own
    /// `clone_killer()` handle instead of going through here, exactly to avoid this), but a
    /// reason to keep reaching for `clone_killer()` rather than this method for anything that
    /// might run concurrently with a blocked `wait()` in the future.
    pub fn wait(&self) -> io::Result<ExitStatus> {
        let status = self.child.lock().unwrap().wait().map_err(to_io_error)?;
        let _ = self.output_done.lock().unwrap().recv_timeout(Duration::from_millis(500));
        Ok(status)
    }

    /// Terminate the child. Used by the batch CLI, which doesn't assume a script leaves the
    /// program in a state where it'll exit on its own - deterministic over hoping a `wait()`
    /// eventually returns. Safe to call on an already-exited child (the underlying error is
    /// swallowed, not propagated, since "already gone" isn't a failure from a caller's
    /// perspective).
    pub fn kill(&self) {
        let _ = self.child.lock().unwrap().kill();
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

    /// Catch `SIGTERM` and clean up before exiting: restore the real terminal's mode if it was
    /// changed (a safe no-op if it wasn't - `crossterm::disable_raw_mode` already checks),
    /// kill the child, then exit. Unlike `SIGKILL` (see `RawModeGuard`'s watchdog), `SIGTERM` is
    /// catchable, so no second process is needed here - this just makes sure something actually
    /// catches it, since the default disposition (terminate, no cleanup) would otherwise leave
    /// the terminal in whatever state raw mode left it in. Uses `ChildKiller::clone_killer`
    /// rather than `Tap::kill`, specifically so this can run from its own thread without needing
    /// `&mut Tap` - the thread that owns that may be blocked in `Tap::wait()` when the signal
    /// arrives.
    #[cfg(unix)]
    pub fn watch_sigterm(&self) {
        use signal_hook::consts::SIGTERM;
        use signal_hook::iterator::Signals;

        let mut killer = self.child.lock().unwrap().clone_killer();
        let Ok(mut signals) = Signals::new([SIGTERM]) else { return };
        thread::spawn(move || {
            if signals.forever().next().is_some() {
                let _ = crossterm::terminal::disable_raw_mode();
                let _ = killer.kill();
                std::process::exit(143); // 128 + SIGTERM, the conventional shell-visible code
            }
        });
    }

    #[cfg(not(unix))]
    pub fn watch_sigterm(&self) {
        // TODO(requirement 8): Windows has no SIGTERM; the nearest equivalent is a console
        // control event, not yet handled.
    }

    /// Observation (`docs/roadmap.md`, "connect and watch"): bind a Unix domain socket at
    /// `path` and, for each connection, read `Command`s as newline-delimited JSON - the exact
    /// same format the batch CLI reads from a script file - and write one `CommandResult` line
    /// back per `Snapshot`/`WaitFor`/`WaitMs`. Read-only by design right now: anything that
    /// isn't (`Command::is_read_only`) gets a `CommandResult::Error` instead of ever reaching
    /// `execute`, since a watcher injecting input is stage 3 ("full duplex"), not this stage.
    ///
    /// Takes `Arc<Tap>` rather than `&self`, because the accept loop and every per-connection
    /// handler need to outlive this call and run concurrently with whatever the caller does
    /// next (`Tap::wait()` in `main.rs`'s case) - an ordinary borrow couldn't outlive that.
    /// Binding happens synchronously, so a bad path fails loudly here rather than silently in
    /// a background thread; removes a stale socket file at `path` first, since `UnixListener::
    /// bind` otherwise fails if a previous run left one behind (e.g. after a `SIGKILL`).
    #[cfg(unix)]
    pub fn serve(tap: Arc<Tap>, path: &Path) -> io::Result<()> {
        let _ = std::fs::remove_file(path);
        let listener = std::os::unix::net::UnixListener::bind(path)?;
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let tap = tap.clone();
                thread::spawn(move || serve_connection(&tap, stream));
            }
        });
        Ok(())
    }

    #[cfg(not(unix))]
    pub fn serve(_tap: Arc<Tap>, _path: &Path) -> io::Result<()> {
        // TODO(requirement 8): named pipes are the Windows equivalent of a Unix domain socket;
        // not yet done.
        Err(io::Error::new(io::ErrorKind::Unsupported, "observation (--listen) needs Unix"))
    }
}

#[cfg(unix)]
fn serve_connection(tap: &Tap, stream: std::os::unix::net::UnixStream) {
    let Ok(reader_stream) = stream.try_clone() else { return };
    let reader = io::BufReader::new(reader_stream);
    let mut writer = stream;
    for line in io::BufRead::lines(reader) {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let result = match serde_json::from_str::<Command>(&line) {
            Err(e) => CommandResult::Error { message: format!("bad command: {e}") },
            Ok(cmd) if !cmd.is_read_only() => CommandResult::Error {
                message: "write commands aren't available yet - observation is read-only (docs/roadmap.md)".to_string(),
            },
            Ok(cmd) => match cmd.execute(tap) {
                Ok(Some(result)) => result,
                Ok(None) => continue, // WaitMs: nothing to report, same as the batch CLI
                Err(e) => CommandResult::Error { message: e.to_string() },
            },
        };
        let Ok(text) = serde_json::to_string(&result) else { continue };
        if writeln!(writer, "{text}").is_err() {
            break;
        }
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
        let _ = self.child.lock().unwrap().kill();
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

#[cfg(unix)]
type WatchdogHandle = std::os::unix::net::UnixStream;
#[cfg(not(unix))]
type WatchdogHandle = ();

/// Puts our own controlling terminal into raw mode for the life of the guard, so keystrokes
/// reach the child unmodified (requirement 7), and restores it on drop - including on panic
/// unwind, so a bug in vtee doesn't leave the human's shell broken. A no-op when stdin isn't
/// a real terminal (CI/scripted use, requirement 1), since raw mode doesn't apply there.
///
/// Also spawns a watchdog process (Unix; `requirement 8` defers Windows) that restores the
/// terminal even if this process is killed with `SIGKILL`, which cannot be caught - no handler,
/// no exception, a Unix guarantee, not a gap in this code. The technique: a `UnixStream::pair()`
/// socketpair, one end given to the watchdog as its stdin, the other (`_watchdog` below) held
/// here for the guard's lifetime. When this process dies for *any* reason - including `-9` -
/// the OS closes all its file descriptors, including its end of that pair, with no cooperation
/// needed from the dying process; the watchdog's blocking read on its stdin returns EOF, and it
/// restores the terminal and exits. The same thing happens on a clean drop, just not because of
/// a crash - harmless, restoring already-sane settings is a no-op in practice.
///
/// Measured, not assumed, that this can't just call `crossterm::terminal::disable_raw_mode()`
/// from the watchdog: that function only works by remembering the pre-raw termios in a
/// process-local static that `enable_raw_mode` populated - the watchdog is a different process
/// that never called `enable_raw_mode`, so its copy of that static is empty, and
/// `disable_raw_mode` silently no-ops instead of touching the terminal. First version of this
/// did exactly that and the terminal measurably stayed raw after a real `SIGKILL` test. Fixed by
/// capturing the real termios ourselves (`libc::tcgetattr`) before enabling raw mode, and
/// sending those bytes to the watchdog over the same socket, so it can restore the exact prior
/// state independently of crossterm's in-process memory.
pub struct RawModeGuard {
    active: bool,
    _watchdog: Option<WatchdogHandle>,
}

impl RawModeGuard {
    pub fn new() -> Self {
        use std::io::IsTerminal;
        let is_tty = io::stdin().is_terminal();
        #[cfg(unix)]
        let original_termios = if is_tty { capture_termios() } else { None };
        let active = is_tty && crossterm::terminal::enable_raw_mode().is_ok();
        #[cfg(unix)]
        let _watchdog = if active { spawn_tty_watchdog(original_termios) } else { None };
        #[cfg(not(unix))]
        let _watchdog = None;
        RawModeGuard { active, _watchdog }
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

#[cfg(unix)]
fn capture_termios() -> Option<libc::termios> {
    unsafe {
        let mut termios: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(libc::STDIN_FILENO, &mut termios) == 0 { Some(termios) } else { None }
    }
}

#[cfg(unix)]
fn spawn_tty_watchdog(original_termios: Option<libc::termios>) -> Option<WatchdogHandle> {
    use std::os::fd::OwnedFd;
    use std::process::Stdio;

    let original_termios = original_termios?;
    let (mut ours, theirs) = std::os::unix::net::UnixStream::pair().ok()?;

    // Sent first, read first (`run_tty_guard`) - the watchdog needs this before it can do
    // anything, so write it before the dead-man's-switch end of this socket is even relevant.
    let bytes: &[u8] = unsafe {
        std::slice::from_raw_parts(
            (&original_termios as *const libc::termios).cast::<u8>(),
            std::mem::size_of::<libc::termios>(),
        )
    };
    ours.write_all(bytes).ok()?;

    let exe = std::env::current_exe().ok()?;
    std::process::Command::new(exe)
        .arg("--internal-tty-guard")
        .stdin(Stdio::from(OwnedFd::from(theirs)))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    Some(ours)
}

/// The watchdog process's own entry point. `main.rs` calls this immediately, before anything
/// else, when invoked with `--internal-tty-guard` - the hidden argument `spawn_tty_watchdog`
/// passes when it re-execs the same binary. Reads the termios snapshot `spawn_tty_watchdog`
/// sent first, then blocks on the same stdin until it sees EOF (see `RawModeGuard`'s doc comment
/// for why that's the right signal and why this can't just call `crossterm::disable_raw_mode`),
/// and applies that snapshot to `/dev/tty` with `libc::tcsetattr`.
///
/// `/dev/tty` is opened *immediately*, before waiting for EOF, and the resulting fd is held
/// rather than reopened afterward - measured, not assumed, that the reopen-after approach
/// doesn't work: opening `/dev/tty` after the main process (the session leader) has died
/// returns `ENXIO`, because POSIX disassociates the controlling terminal from a session once
/// its leader exits, even though the watchdog itself is still alive and part of that session.
/// An already-open fd from before that happens keeps working for `tcsetattr` regardless.
///
/// Also measured, not assumed: an earlier version without the `SIGHUP` guard below lost a
/// different race and never got this far at all. The watchdog inherits the same controlling
/// terminal and (unless moved) the same foreground process group as the main process, and when
/// a session leader holding a controlling terminal dies, the kernel sends `SIGHUP` to that group
/// - which killed the watchdog itself, by default, before it ever saw the pipe's EOF.
/// Registering `SIGHUP` with `signal_hook` (even though nothing reads from the resulting
/// `Signals`) replaces that default disposition, the same mechanism `watch_resize`/
/// `watch_sigterm` use for their own signals - keeping the registration alive for the rest of
/// this function is what matters, not consuming it.
#[cfg(unix)]
pub fn run_tty_guard() -> ! {
    let _sighup_guard = signal_hook::iterator::Signals::new([signal_hook::consts::SIGHUP]);
    let tty = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty");

    let mut stdin = io::stdin();
    let mut termios_bytes = [0u8; std::mem::size_of::<libc::termios>()];
    if stdin.read_exact(&mut termios_bytes).is_ok() {
        let mut buf = [0u8; 1];
        let _ = stdin.read(&mut buf); // blocks until EOF (clean drop or the main process dying)

        if let Ok(tty) = tty {
            use std::os::fd::AsRawFd;
            let termios: libc::termios = unsafe { std::ptr::read(termios_bytes.as_ptr().cast()) };
            unsafe {
                libc::tcsetattr(tty.as_raw_fd(), libc::TCSANOW, &termios);
            }
        }
    }
    std::process::exit(0);
}
