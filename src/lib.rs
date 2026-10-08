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

pub use command::{Command, CommandResult, HumanPriority, WritePolicy};
pub use log::{RawLog, replay};
pub use snapshot::{Cell, Color, Cursor, CursorShape, Snapshot, redraw_sequence};

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
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
///
/// `tty_sinks` lives here too, not in a separate `Mutex`, deliberately: a new raw-TTY client
/// (`Tap::serve_tty`) needs "take a redraw snapshot of right now" and "start receiving every
/// chunk from now on" to be one atomic step relative to `forward_output` - two separate locks
/// would leave a real window where a chunk lands in neither the snapshot nor the live feed (or
/// in both). One lock serializing "update state and broadcast this chunk" against "snapshot
/// current state and register for future chunks" closes that window by construction.
///
/// Each entry is a bounded channel, not the sink itself: confirmed by an adversarial review that
/// writing straight to the sink here - blocking `write_all` under this very lock - lets one
/// stalled TTY client (a backgrounded terminal, a slow reader, a hostile one that just never
/// reads) wedge the lock indefinitely, which stalls every other sink, `snapshot()`, and
/// eventually the pty itself. A dedicated writer thread per sink owns the actual blocking
/// `write_all`; this lock only ever does a bounded, non-blocking `try_send` (`forward_output`),
/// so a sink that falls behind gets dropped instead of blocking everyone else.
struct TapState {
    term: Term<ObserverListener>,
    bytes_seen: u64,
    tty_sinks: Vec<(u64, SyncSender<Vec<u8>>)>,
}

/// Bytes, not chunk count: at the 8KiB read-buffer size `forward_output` uses, this bounds a
/// single stalled sink to ~2MiB of buffered output before it's dropped rather than let it block
/// everything else.
const TTY_SINK_QUEUE_CAPACITY: usize = 256;

/// Paired with the `Mutex` so `wait_for_text` can block efficiently instead of polling:
/// `forward_output` notifies after every chunk it applies.
struct Shared {
    state: Mutex<TapState>,
    changed: Condvar,
}

type SharedState = Arc<Shared>;
type SharedWriter = Arc<Mutex<Box<dyn Write + Send>>>;

/// Backs `WritePolicy::Exclusive` (`docs/roadmap.md`, "Full duplex" stage 3b): at most one
/// connection ID may hold write access at a time. A field, not a per-connection value, because
/// it has to be shared across the accept loop's per-connection threads *and* (when
/// `HumanPriority::Participant`) the interactive stdin-forwarding thread - created once per
/// `Tap`, inert (never checked) unless `Tap::serve` is actually called with `Exclusive`.
#[derive(Default)]
struct ExclusiveLock {
    holder: Mutex<Option<u64>>,
}

impl ExclusiveLock {
    fn try_acquire(&self, id: u64) -> bool {
        let mut holder = self.holder.lock().unwrap();
        match *holder {
            Some(_) => false,
            None => {
                *holder = Some(id);
                true
            }
        }
    }

    fn release_if_held_by(&self, id: u64) {
        let mut holder = self.holder.lock().unwrap();
        if *holder == Some(id) {
            *holder = None;
        }
    }

    fn is_held_by(&self, id: u64) -> bool {
        *self.holder.lock().unwrap() == Some(id)
    }

    /// `HumanPriority::Participant`: a keystroke always revokes whoever currently holds it,
    /// human or not - reacting to a real event (a byte arrived) rather than inferring "idle
    /// enough" the way `WritePolicy`'s rejected idle-based option would have. Returns whether
    /// anyone was actually holding it, so a caller logging this can skip the common "nobody
    /// held it anyway" case instead of logging every single keystroke.
    fn revoke(&self) -> bool {
        self.holder.lock().unwrap().take().is_some()
    }
}

/// How to spawn a [`Tap`]. `forward_input`/`forward_output` are independent switches
/// (`docs/roadmap.md`, "Full duplex" stage 3b, mode (d)): the usual interactive case sets both,
/// the batch CLI sets neither, and a session that wants a human to *watch* without being able
/// to *type* (so a remote `--connect` client has the pty to itself) sets only `forward_output`.
pub struct SpawnOptions<'a> {
    pub cols: u16,
    pub rows: u16,
    pub log_path: Option<&'a Path>,
    /// Forward our own stdin to the child (a human's keystrokes, requirement 7).
    pub forward_input: bool,
    /// Forward the child's raw output to our own stdout (a human's view, requirement 7).
    pub forward_output: bool,
    /// Extra/overriding environment variables for the child (requirement 6, "Deterministic" -
    /// `TERM` is just another entry here, not a separate mechanism). Applied on top of our own
    /// inherited environment (`portable_pty::CommandBuilder::env`'s own default), not in place
    /// of it - an empty slice changes nothing from today's "inherits ours" behaviour.
    pub env: &'a [(String, String)],
}

impl Default for SpawnOptions<'_> {
    fn default() -> Self {
        SpawnOptions { cols: 80, rows: 24, log_path: None, forward_input: true, forward_output: true, env: &[] }
    }
}

/// A running tap: a child spawned in a pty, with a copy of its output feeding an in-memory
/// terminal emulator, and (per `SpawnOptions::forward_input`/`forward_output`) forwarded to our
/// own real stdin/stdout.
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
    exclusive_lock: Arc<ExclusiveLock>,
    // Set by `Tap::serve` when (and only when) it's called with `WritePolicy::Exclusive {
    // human_priority: HumanPriority::Participant }` - read by the stdin-forwarding thread
    // (spawned well before `serve` is ever called, so this can't just be a constructor
    // argument) to decide whether to revoke `exclusive_lock` after each chunk it forwards.
    human_revokes_lock: Arc<AtomicBool>,
    next_connection_id: Arc<AtomicU64>,
}

impl Tap {
    /// Spawn `program args...` in a pty per `opts`, and start forwarding immediately.
    pub fn spawn(program: &str, args: &[String], opts: SpawnOptions) -> io::Result<Self> {
        // A 0x0 (or 0-in-one-dimension) size is never meaningful and alacritty_terminal
        // panics on a zero-sized grid rather than erroring, so this is a hard floor rather
        // than something callers can opt out of.
        let cols = opts.cols.max(1);
        let rows = opts.rows.max(1);

        // Measured, not a precaution added on spec (docs/decisions.md): `openpty()` just below
        // was observed to hang when the calling process had no console of its own - a Windows
        // ConPTY quirk, independent of the CREATE_NEW_PROCESS_GROUP bug this project already
        // patches around (`vendor/portable-pty/README-vtee-patch.md`). A no-op failure
        // (ignored) if this process already has a console, which is the common case - but a
        // real side effect (a new, visible console window) if it doesn't, so this is gated on
        // `forward_output`: only when something's actually meant to be shown to a human on a
        // real screen, never for `--headless`/`--script`, where a surprise GUI window popping
        // up during otherwise-automated/CI usage would be exactly the wrong kind of surprise.
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
        // Our process doesn't need its own handle to the slave side once the child has it;
        // dropping it means the master sees EOF once the child's copy of the slave closes too.
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

        let master = Arc::new(Mutex::new(pair.master));
        let child = Mutex::new(child);
        let output_done = Mutex::new(output_done);
        let next_connection_id = Arc::new(AtomicU64::new(1));
        Ok(Tap {
            master,
            writer,
            child,
            shared,
            title,
            output_done,
            exclusive_lock,
            human_revokes_lock,
            next_connection_id,
        })
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

    /// Queues a redraw of the current screen and registers the sink to receive every chunk from
    /// now on, both under the one lock that also serializes `forward_output`'s own "apply this
    /// chunk, then broadcast it" step (`TapState::tty_sinks`'s doc comment) - so there is no
    /// window where a live chunk could be enqueued before, or interleaved with, the redraw
    /// that's supposed to precede it. The redraw is the first item placed in a fresh, empty
    /// channel, so it's always first out regardless of when the dedicated writer thread below
    /// gets scheduled.
    ///
    /// The actual blocking `write_all` happens on a dedicated thread, not under `self.shared`'s
    /// lock - see `TapState::tty_sinks`'s doc comment for why that split exists.
    fn attach_tty_sink(&self, id: u64, sink: Box<dyn Write + Send>) {
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(TTY_SINK_QUEUE_CAPACITY);
        {
            let mut state = self.shared.state.lock().unwrap();
            let snap = snapshot::build(&state.term, state.bytes_seen, self.title.current_title());
            // The channel was just created with spare capacity, so this can't actually be full.
            let _ = tx.try_send(snapshot::redraw_sequence(&snap));
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
        self.shared.state.lock().unwrap().tty_sinks.retain(|(sink_id, _)| *sink_id != id);
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

    /// Observation and (stage 3a) remote control (`docs/roadmap.md`, "connect and watch" /
    /// "Full duplex"): bind a Unix domain socket at `path` and, for each connection, read
    /// `Command`s as newline-delimited JSON - the exact same format the batch CLI reads from a
    /// script file - and write one `CommandResult` line back per `Snapshot`/`WaitFor`/
    /// `WaitMs`/(if `opts.write_policy` allows it) `Send`/`SendKey`/`Resize`. Every command is
    /// checked against `opts` via `Command::authorize` before ever reaching `execute` - a
    /// rejection becomes a `CommandResult::Error`, not a dropped connection, so a client can
    /// keep going after one denied command.
    ///
    /// Takes `Arc<Tap>` rather than `&self`, because the accept loop and every per-connection
    /// handler need to outlive this call and run concurrently with whatever the caller does
    /// next (`Tap::wait()` in `main.rs`'s case) - an ordinary borrow couldn't outlive that.
    /// Binding happens synchronously, so a bad path fails loudly here rather than silently in
    /// a background thread; removes a stale socket file at `path` first, since `UnixListener::
    /// bind` otherwise fails if a previous run left one behind (e.g. after a `SIGKILL`).
    #[cfg(unix)]
    pub fn serve(tap: Arc<Tap>, path: &Path, opts: ServeOptions) -> io::Result<()> {
        // Only `Exclusive { human_priority: Participant }` ever wants a keystroke to revoke a
        // remote holder (`forward_input`, spawned long before this call, reads this flag on
        // every chunk) - explicitly set either way rather than only ever turning it on, so a
        // second `serve` call with a different policy can't leave a stale `true` behind.
        let participant =
            matches!(opts.write_policy, WritePolicy::Exclusive { human_priority: HumanPriority::Participant });
        tap.human_revokes_lock.store(participant, Ordering::Relaxed);

        let _ = std::fs::remove_file(path);
        let listener = std::os::unix::net::UnixListener::bind(path)?;
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let tap = tap.clone();
                thread::spawn(move || serve_connection(&tap, stream, opts));
            }
        });
        Ok(())
    }

    #[cfg(not(unix))]
    pub fn serve(_tap: Arc<Tap>, _path: &Path, _opts: ServeOptions) -> io::Result<()> {
        // TODO(requirement 8): named pipes are the Windows equivalent of a Unix domain socket;
        // not yet done.
        Err(io::Error::new(io::ErrorKind::Unsupported, "observation (--listen) needs Unix"))
    }

    /// The raw-TTY peer protocol (`docs/architecture.md`): bidirectional byte forwarding, for a
    /// client that wants to *see and type* the way a real terminal would, rather than drive
    /// `Command`s. Same accept-loop shape as `serve` (and the same `Tap`, so the two protocols'
    /// writers share one `ExclusiveLock`/`WritePolicy` - a TTY client's keystrokes and a JSON
    /// client's `Send` are symmetric writers from here on, arbitrated identically). Several TTY
    /// clients may be attached at once; each gets its own entry in `tty_sinks` and its own
    /// live redraw-then-mirror of the screen.
    #[cfg(unix)]
    pub fn serve_tty(tap: Arc<Tap>, path: &Path, policy: WritePolicy) -> io::Result<()> {
        let _ = std::fs::remove_file(path);
        let listener = std::os::unix::net::UnixListener::bind(path)?;
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let tap = tap.clone();
                thread::spawn(move || serve_tty_connection(&tap, stream, policy));
            }
        });
        Ok(())
    }

    #[cfg(not(unix))]
    pub fn serve_tty(_tap: Arc<Tap>, _path: &Path, _policy: WritePolicy) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "attach (raw TTY) needs Unix"))
    }
}

/// tmux's own answer to "who can connect" (`docs/decisions.md`): not socket-file permissions,
/// but a private per-user directory that nothing else can even traverse into. Shared by
/// `default_socket_path` and `default_log_path` - one session's socket and its diagnostic log
/// live side by side, so they only need this check done once each.
///
/// Creates `$TMPDIR/vtee-<uid>/` with mode `0700` if it doesn't exist yet (the mode is passed
/// to the `mkdir` call itself, not set afterwards, so there's no window where it briefly exists
/// with the wrong permissions). If it already exists, verifies rather than trusts it - refuses
/// to use it at all if it isn't a plain directory (rejects a symlink the same way, since
/// `symlink_metadata` doesn't follow one), isn't owned by the current user, or has any
/// group/other permission bit set - the exact two checks that defeat a classic `/tmp`
/// symlink-or-pre-plant race, where another local user pre-creates the directory before this
/// one ever runs.
#[cfg(unix)]
fn default_session_dir() -> io::Result<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};

    let uid = unsafe { libc::getuid() };
    let dir = std::env::temp_dir().join(format!("vtee-{uid}"));

    match std::fs::symlink_metadata(&dir) {
        Ok(meta) if !meta.is_dir() => {
            return Err(io::Error::other(format!(
                "{} exists and isn't a plain directory - refusing to use it as the default session dir",
                dir.display()
            )));
        }
        Ok(meta) if meta.uid() != uid => {
            return Err(io::Error::other(format!(
                "{} isn't owned by the current user - refusing to use it as the default session dir",
                dir.display()
            )));
        }
        Ok(meta) if meta.permissions().mode() & 0o077 != 0 => {
            return Err(io::Error::other(format!(
                "{} has unsafe permissions (group/other access) - refusing to use it as the default session dir",
                dir.display()
            )));
        }
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            // Measured, not assumed (a real test failure surfaced this): two processes can
            // both see `NotFound` here and both race to create the directory - the loser gets
            // `AlreadyExists`, not a real problem (the winner created the exact same hardened
            // directory), so this re-verifies rather than propagating a transient race as a
            // hard failure. Anything *other* than `AlreadyExists` is a real error and still
            // propagates. The re-verification (recursing once) reuses the same checks above
            // rather than duplicating them - correct whether this call created the directory
            // or lost the race to one that already had.
            match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
            return default_session_dir();
        }
        Err(e) => return Err(e),
    }

    Ok(dir)
}

/// A named session's addresses - the NDJSON command socket, the raw-TTY socket
/// (`docs/architecture.md`, "two peer protocols"), and the pidfile `vtee ls`/`kill` use to
/// find and signal the holder process. All three share one name inside
/// `default_session_dir()`, replacing the old pid-based `<pid>.sock` naming now that a session
/// needs to be *findable* by something other than the pid of a process that might not be the
/// one you're thinking of (`vtee attach <name>`).
pub struct SessionPaths {
    pub sock: PathBuf,
    pub tty_sock: PathBuf,
    pub pidfile: PathBuf,
}

/// Confirmed, not assumed, by an adversarial review: without this check, `--name
/// '../../../tmp/evil'` (or an absolute path, or bare `.`/`..`) escaped `default_session_dir()`
/// entirely - `PathBuf::join` doesn't confine a value containing `/` to the directory it's
/// joined onto. A name must decompose into exactly one plain path component: no separators, no
/// `.`/`..`, nothing absolute. This is the actual security boundary for every caller
/// (`session_paths` is the one place a name becomes a filesystem path), not just the CLI's own
/// `--name` flag - fixing it here covers `attach`/`ls`/`kill` too, not only spawning.
fn valid_session_name(name: &str) -> bool {
    use std::path::Component;
    !name.is_empty() && matches!(Path::new(name).components().collect::<Vec<_>>().as_slice(), [Component::Normal(_)])
}

#[cfg(unix)]
pub fn session_paths(name: &str) -> io::Result<SessionPaths> {
    if !valid_session_name(name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name:?} isn't a valid session name - no \"/\", no \".\"/\"..\", not empty"),
        ));
    }
    let dir = default_session_dir()?;
    Ok(SessionPaths {
        sock: dir.join(format!("{name}.sock")),
        tty_sock: dir.join(format!("{name}.tty.sock")),
        pidfile: dir.join(format!("{name}.pid")),
    })
}

#[cfg(not(unix))]
pub fn session_paths(_name: &str) -> io::Result<SessionPaths> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "named sessions need Unix"))
}

/// Whether a session named `name` is currently live. Tries actually connecting to its NDJSON
/// socket rather than trusting the pidfile or the socket file's mere existence - a stale file
/// left by a killed session is exactly the case that must *not* look "alive" (the same reason
/// `Tap::serve` already removes a stale socket file before binding, rather than trusting it).
#[cfg(unix)]
pub fn session_is_live(name: &str) -> io::Result<bool> {
    let paths = session_paths(name)?;
    match std::os::unix::net::UnixStream::connect(&paths.sock) {
        Ok(_) => Ok(true),
        Err(e) if matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused) => Ok(false),
        Err(e) => Err(e),
    }
}

#[cfg(not(unix))]
pub fn session_is_live(_name: &str) -> io::Result<bool> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "named sessions need Unix"))
}

/// Every session name currently in `default_session_dir()` with a matching pidfile - the list
/// `vtee ls` reports, before each entry is checked with `session_is_live` for whether it's
/// actually still running or just a stale leftover.
#[cfg(unix)]
pub fn session_names() -> io::Result<Vec<String>> {
    let dir = default_session_dir()?;
    let mut names = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "pid")
            && let Some(name) = path.file_stem().and_then(|s| s.to_str())
        {
            names.push(name.to_string());
        }
    }
    Ok(names)
}

#[cfg(not(unix))]
pub fn session_names() -> io::Result<Vec<String>> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "named sessions need Unix"))
}

/// Where diagnostic logging goes when a real terminal is actively showing the child
/// (`docs/decisions.md`) - right beside the session's own socket, so attaching to one tells you
/// where to find the other. Same pid-naming and stale-file reasoning as `default_socket_path`;
/// unlike the socket, nothing re-opens this across runs, so no stale-file cleanup is needed.
#[cfg(unix)]
pub fn default_log_path() -> io::Result<PathBuf> {
    Ok(default_session_dir()?.join(format!("{}.log", std::process::id())))
}

#[cfg(not(unix))]
pub fn default_log_path() -> io::Result<PathBuf> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "a default diagnostic log path needs Unix"))
}

/// `Tap::serve`'s policy knobs. `Copy` and small on purpose - it's cloned into every
/// per-connection thread (`serve_connection`).
#[derive(Clone, Copy)]
pub struct ServeOptions {
    pub write_policy: WritePolicy,
    /// Whether the real terminal (if any) is actively managing the pty's size via
    /// `Tap::watch_resize` - see `Command::authorize`. `main.rs` sets this from the same
    /// `is_terminal()` check `RawModeGuard`/`watch_resize` already act on.
    pub resize_owned_by_terminal: bool,
}

impl Default for ServeOptions {
    fn default() -> Self {
        ServeOptions { write_policy: WritePolicy::ReadOnly, resize_owned_by_terminal: false }
    }
}

#[cfg(unix)]
fn serve_connection(tap: &Tap, stream: std::os::unix::net::UnixStream, opts: ServeOptions) {
    let id = tap.next_connection_id.fetch_add(1, Ordering::Relaxed);
    ::log::debug!("connection {id}: accepted");
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
            Ok(cmd) => match cmd.authorize(opts.write_policy, opts.resize_owned_by_terminal) {
                Err(message) => CommandResult::Error { message },
                // `authorize` only checked that `AcquireWrite`/`ReleaseWrite`/`Send`/`SendKey`/
                // `Resize` are structurally reachable under `Exclusive` - whether *this*
                // connection currently holds the lock is this function's job, since `authorize`
                // has no notion of which connection is asking.
                Ok(()) => match cmd {
                    Command::AcquireWrite => {
                        let acquired = tap.exclusive_lock.try_acquire(id);
                        ::log::debug!("connection {id}: acquire_write -> {acquired}");
                        CommandResult::WriteAccess { acquired }
                    }
                    Command::ReleaseWrite => {
                        tap.exclusive_lock.release_if_held_by(id);
                        ::log::debug!("connection {id}: released write access");
                        CommandResult::WriteAccess { acquired: false }
                    }
                    Command::Send { .. } | Command::SendKey { .. } | Command::Resize { .. }
                        if matches!(opts.write_policy, WritePolicy::Exclusive { .. })
                            && !tap.exclusive_lock.is_held_by(id) =>
                    {
                        CommandResult::Error { message: "write access not held - send acquire_write first".to_string() }
                    }
                    _ => match cmd.execute(tap) {
                        Ok(Some(result)) => result,
                        Ok(None) => continue, // WaitMs/Send/SendKey/Resize: nothing to report
                        Err(e) => CommandResult::Error { message: e.to_string() },
                    },
                },
            },
        };
        if let CommandResult::Error { message } = &result {
            ::log::debug!("connection {id}: rejected - {message}");
        }
        let Ok(text) = serde_json::to_string(&result) else { continue };
        if writeln!(writer, "{text}").is_err() {
            break;
        }
    }
    ::log::debug!("connection {id}: disconnected");
    // However this connection ends - a clean `release_write`, just closing it, a write
    // failure above - it must not be left holding the lock forever (`docs/roadmap.md`:
    // auto-released on disconnect). A no-op if it never held it, or already released it.
    tap.exclusive_lock.release_if_held_by(id);
}

/// The raw-TTY peer protocol's own connection handler (`Tap::serve_tty`): register as a live
/// output sink (redraw-then-mirror, `Tap::attach_tty_sink`), then treat every byte read from
/// the client as a write, authorized through the *same* `ExclusiveLock`/`WritePolicy` a JSON
/// client's `Send` goes through - a TTY client's keystrokes are a writer like any other, not a
/// privileged special case, now that there's no longer a single "the original human."
#[cfg(unix)]
fn serve_tty_connection(tap: &Tap, stream: std::os::unix::net::UnixStream, policy: WritePolicy) {
    let id = tap.next_connection_id.fetch_add(1, Ordering::Relaxed);
    ::log::debug!("tty connection {id}: accepted");
    let Ok(reader_stream) = stream.try_clone() else { return };
    // `stream` itself becomes the registered sink - see `attach_tty_sink`'s doc comment for
    // why the redraw and every later broadcast go through this *same* handle, via its own
    // dedicated writer thread.
    tap.attach_tty_sink(id, Box::new(stream));

    let mut reader = reader_stream;
    let mut buf = [0u8; 4096];
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let allowed = match policy {
            WritePolicy::ReadOnly => false,
            WritePolicy::Open => true,
            WritePolicy::Exclusive { .. } => tap.exclusive_lock.is_held_by(id),
        };
        if allowed && tap.send_bytes(&buf[..n]).is_err() {
            break;
        }
    }
    ::log::debug!("tty connection {id}: disconnected");
    tap.detach_tty_sink(id);
    tap.exclusive_lock.release_if_held_by(id);
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
            // Broadcast to every attached TTY client (`Tap::serve_tty`), under the same lock
            // that just applied this chunk to `term` - see `TapState::tty_sinks`'s doc comment
            // for why that atomicity matters. `try_send` only enqueues for that sink's own
            // writer thread - never blocks here - so a sink whose queue is full (its reader has
            // stalled) is dropped instead of wedging this lock, and with it every other sink,
            // `snapshot()`, and the pty itself. A sink whose writer thread already exited
            // (the client disconnected) reports the channel as disconnected and is dropped the
            // same way; `serve_tty_connection` also removes its own entry on the way out, so
            // this is a belt-and-suspenders cleanup for the same case.
            state.tty_sinks.retain(|(_, tx)| tx.try_send(chunk.to_vec()).is_ok());
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
        // A human's own typing is never delayed or blocked by `WritePolicy::Exclusive` - see
        // `HumanPriority`'s doc comment - so this writes first, unconditionally, and only
        // *afterward* decides whether that keystroke should also revoke a remote holder.
        {
            let mut w = writer.lock().unwrap();
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
///
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
            // `read_unaligned`, not `read`: `termios_bytes` is a `[u8; N]`, which the type
            // system only guarantees to be 1-byte aligned, while `libc::termios` likely needs
            // more (it's built from `c_int`/`tcflag_t` fields). `read` requires the source to
            // already be aligned for the target type; `read_unaligned` has identical semantics
            // without that requirement - this buffer's actual alignment isn't something to rely
            // on even though it has probably always happened to be enough in practice.
            let termios: libc::termios = unsafe { std::ptr::read_unaligned(termios_bytes.as_ptr().cast()) };
            unsafe {
                libc::tcsetattr(tty.as_raw_fd(), libc::TCSANOW, &termios);
            }
        }
    }
    std::process::exit(0);
}
