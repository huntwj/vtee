//! `Tap`: a child spawned in a pty, its output forwarded raw to our stdout (requirement 7) while
//! a copy feeds an `alacritty_terminal` emulator that stays a passive observer.

use std::collections::VecDeque;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
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
pub const MAX_TERMINAL_DIMENSION: u16 = 1000;

/// The one place `Tap::spawn` and `Tap::resize` both check a requested size against: below 1x1
/// (`alacritty_terminal` panics on a zero-sized grid) or above `MAX_TERMINAL_DIMENSION` on either
/// side. `replay` deliberately does not use this - see its own doc comment.
pub(crate) fn check_terminal_size(cols: u16, rows: u16) -> io::Result<()> {
    if cols == 0 || rows == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("terminal size {cols}x{rows} is below the 1x1 minimum"),
        ));
    }
    if cols > MAX_TERMINAL_DIMENSION || rows > MAX_TERMINAL_DIMENSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("terminal size {cols}x{rows} exceeds the {MAX_TERMINAL_DIMENSION}x{MAX_TERMINAL_DIMENSION} limit"),
        ));
    }
    Ok(())
}

/// How often `wait`/`wait_exit` poll the child; also the worst-case added latency to noticing an exit.
const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// The longest `wait` holds the holder open after the child exits for connected clients.
const CLIENT_LINGER: Duration = Duration::from_millis(300);

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
/// title back out of `Term`, and (for a tap that is the child's only terminal) the replies the
/// emulator wants sent back to the child for its queries - `ESC[6n` cursor-position reports,
/// device attributes, mode reports - which `Term` emits as `Event::PtyWrite`.
#[derive(Clone)]
pub(crate) struct ObserverListener {
    title: Arc<Mutex<Option<String>>>,
    /// `None` when something else (a human's real terminal) answers the child's queries, so the
    /// emulator's own answers are dropped instead of queued forever.
    replies: Option<Arc<Mutex<Vec<String>>>>,
}

impl ObserverListener {
    /// A listener that only tracks the title; query replies are discarded.
    pub(crate) fn new() -> Self {
        ObserverListener { title: Arc::new(Mutex::new(None)), replies: None }
    }

    /// A listener that also queues the emulator's query replies for [`Self::take_replies`].
    pub(crate) fn answering_queries() -> Self {
        ObserverListener { title: Arc::new(Mutex::new(None)), replies: Some(Arc::new(Mutex::new(Vec::new()))) }
    }

    pub(crate) fn current_title(&self) -> Option<String> {
        self.title.lock_unpoisoned().clone()
    }

    /// Replies queued since the last call, in the order the queries were parsed.
    pub(crate) fn take_replies(&self) -> Vec<String> {
        match &self.replies {
            Some(replies) => std::mem::take(&mut *replies.lock_unpoisoned()),
            None => Vec::new(),
        }
    }
}

impl EventListener for ObserverListener {
    fn send_event(&self, event: Event) {
        match event {
            Event::Title(t) => *self.title.lock_unpoisoned() = Some(t),
            Event::ResetTitle => *self.title.lock_unpoisoned() = None,
            Event::PtyWrite(text) => {
                if let Some(replies) = &self.replies {
                    replies.lock_unpoisoned().push(text);
                }
            }
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
/// Each TTY sink is a [`SinkQueue`] drained by its own writer thread: writing to the sink
/// directly under this lock would let one client that stops reading block every other client,
/// `snapshot()`, and the pty. A client that falls behind gets a redraw instead (see `SinkQueue`).
pub(crate) struct TapState {
    pub(crate) term: Term<ObserverListener>,
    pub(crate) bytes_seen: u64,
    pub(crate) tty_sinks: Vec<(u64, Arc<SinkQueue>)>,
}

/// How much unread output one attached client may have queued before it is resynchronised.
const SINK_QUEUE_BYTES: usize = 2 * 1024 * 1024;

/// Output waiting to be written to one attached terminal client, bounded in bytes.
///
/// A client that falls behind must neither block the session nor be left with a frozen screen. A
/// stalled reader and a client the scheduler starved for a few milliseconds look the same from
/// here, and the second is common on a loaded machine. So when the backlog would pass the limit it is
/// thrown away and replaced by one redraw of the current screen: the client skips the output in
/// between and carries on from a correct picture, whichever of the two it is. A client that never
/// reads costs at most the limit in memory.
pub(crate) struct SinkQueue {
    state: Mutex<SinkQueueState>,
    ready: Condvar,
}

struct SinkQueueState {
    chunks: VecDeque<Arc<[u8]>>,
    bytes: usize,
    /// Length of the last redraw queued: a huge screen's redraw must not itself keep tripping the limit.
    redraw_len: usize,
    resyncs: u64,
    closed: bool,
}

impl SinkQueue {
    /// A queue whose first item is `redraw`, the screen as of attaching.
    pub(crate) fn new(redraw: Vec<u8>) -> Arc<SinkQueue> {
        let redraw_len = redraw.len();
        let chunks = VecDeque::from([Arc::<[u8]>::from(redraw)]);
        let state = SinkQueueState { chunks, bytes: redraw_len, redraw_len, resyncs: 0, closed: false };
        Arc::new(SinkQueue { state: Mutex::new(state), ready: Condvar::new() })
    }

    /// Queues `chunk`, or, if that would pass the limit, replaces the whole backlog with
    /// `redraw()` (which must reflect the screen after `chunk`). Never blocks. Returns `false` once
    /// the writer has gone away, so the caller can drop the sink.
    pub(crate) fn push(&self, chunk: &Arc<[u8]>, redraw: impl FnOnce() -> Vec<u8>) -> bool {
        let mut q = self.state.lock_unpoisoned();
        if q.closed {
            return false;
        }
        if q.bytes + chunk.len() > SINK_QUEUE_BYTES.max(2 * q.redraw_len) {
            let redraw: Arc<[u8]> = redraw().into();
            q.chunks.clear();
            q.bytes = redraw.len();
            q.redraw_len = redraw.len();
            q.chunks.push_back(redraw);
            q.resyncs += 1;
            ::log::debug!("a terminal client fell behind; resynchronised it ({} so far)", q.resyncs);
        } else {
            q.bytes += chunk.len();
            q.chunks.push_back(chunk.clone());
        }
        self.ready.notify_one();
        true
    }

    /// The next item, waiting for one; `None` once the queue is closed and empty.
    pub(crate) fn pop(&self) -> Option<Arc<[u8]>> {
        let mut q = self.state.lock_unpoisoned();
        loop {
            if let Some(chunk) = q.chunks.pop_front() {
                q.bytes -= chunk.len();
                return Some(chunk);
            }
            if q.closed {
                return None;
            }
            q = self.ready.wait(q).unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Wakes the writer so it can exit, and makes later pushes report that the sink is gone.
    pub(crate) fn close(&self) {
        self.state.lock_unpoisoned().closed = true;
        self.ready.notify_all();
    }
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
    /// Forward our own stdin to the child (a human's keystrokes, requirement 7). A caller that
    /// wants a real terminal's keystrokes must put it in raw mode *before* spawning: the stdin
    /// thread's first read starts immediately, and on Windows a read begun in cooked (line
    /// input) mode stays cooked, swallowing the terminal's reply to ConPTY's own startup
    /// cursor query (`docs/decisions.md`, 2026-10-08).
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
    // Remembered once seen, so every later `exit_status` agrees with the first.
    exit: Mutex<Option<ExitStatus>>,
    pub(crate) exclusive_lock: Arc<ExclusiveLock>,
    // Set by `Tap::serve` after the stdin-forwarding thread is already running, hence not a
    // constructor argument.
    pub(crate) human_revokes_lock: Arc<AtomicBool>,
    pub(crate) next_connection_id: Arc<AtomicU64>,
    // NDJSON clients currently connected; `wait` lingers for them after the child exits.
    pub(crate) open_connections: AtomicUsize,
    // The session record, whose mtime is bumped on every attach so bare `attach` can tell which
    // session was used last.
    activity_file: Mutex<Option<PathBuf>>,
}

impl Tap {
    /// Spawn `program args...` in a pty per `opts`, and start forwarding immediately.
    pub fn spawn(program: &str, args: &[String], opts: SpawnOptions) -> io::Result<Self> {
        check_terminal_size(opts.cols, opts.rows)?;
        let (cols, rows) = (opts.cols, opts.rows);

        // Who answers the child's terminal queries (`ESC[6n` and friends)? A human's terminal
        // does, when it sees the child's output *and* its answers come back through our stdin.
        // Otherwise - --script, --headless, --no-stdin, or either side redirected to a pipe -
        // nobody would, and a child that waits for the reply hangs. On Windows that child is
        // ConPTY itself: created with `PSEUDOCONSOLE_INHERIT_CURSOR` (as `portable-pty` does)
        // it issues `ESC[6n` at startup and blocks everything, including the real child's
        // output, until a `ESC[row;colR` arrives (`docs/decisions.md`, 2026-10-08).
        let terminal_answers =
            opts.forward_input && opts.forward_output && io::stdin().is_terminal() && io::stdout().is_terminal();
        let answer_queries = !terminal_answers;

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

        let title = if answer_queries { ObserverListener::answering_queries() } else { ObserverListener::new() };
        let term = Term::new(Config::default(), &Size { cols: cols as usize, rows: rows as usize }, title.clone());
        let shared: SharedState = Arc::new(Shared {
            state: Mutex::new(TapState { term, bytes_seen: 0, tty_sinks: Vec::new() }),
            changed: Condvar::new(),
        });

        let log = opts.log_path.map(|p| RawLog::create(p, cols, rows)).transpose()?;

        let writer: SharedWriter = Arc::new(Mutex::new(pair.master.take_writer().map_err(to_io_error)?));

        let mut reader = pair.master.try_clone_reader().map_err(to_io_error)?;
        let (done_tx, output_done) = mpsc::channel();
        {
            let shared = shared.clone();
            let write_raw = opts.forward_output;
            let replies = answer_queries.then(|| (title.clone(), writer.clone()));
            thread::spawn(move || {
                forward_output(&mut reader, shared, log, write_raw, replies);
                let _ = done_tx.send(());
            });
        }

        let exclusive_lock = Arc::new(ExclusiveLock::default());
        let human_revokes_lock = Arc::new(AtomicBool::new(false));

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
            exit: Mutex::new(None),
            exclusive_lock,
            human_revokes_lock,
            next_connection_id: Arc::new(AtomicU64::new(1)),
            open_connections: AtomicUsize::new(0),
            activity_file: Mutex::new(None),
        })
    }

    /// Resize the child's pty and the emulator together so they never disagree.
    pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        check_terminal_size(cols, rows)?;
        self.master
            .lock_unpoisoned()
            .resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(to_io_error)?;
        self.shared.state.lock_unpoisoned().term.resize(Size { cols: cols as usize, rows: rows as usize });
        Ok(())
    }

    /// The child's exit status if it has exited, without blocking. Safe to call from any thread
    /// while another is in `wait`, `wait_exit` or `kill`: the child's lock is only held for the
    /// instant of each poll.
    pub fn exit_status(&self) -> io::Result<Option<ExitStatus>> {
        if let Some(status) = self.exit.lock_unpoisoned().clone() {
            return Ok(Some(status));
        }
        let polled = self.child.lock_unpoisoned().try_wait().map_err(to_io_error)?;
        if let Some(status) = &polled {
            *self.exit.lock_unpoisoned() = Some(status.clone());
        }
        Ok(polled)
    }

    /// Wait up to `timeout` for the child to exit: `Some` with its status, or `None` on timeout.
    /// After an exit it gives the output thread a short grace period to drain, so a `snapshot`
    /// taken next includes the child's last output. ConPTY doesn't EOF the master reader when
    /// the child exits, so that can't be waited for directly.
    pub fn wait_exit(&self, timeout: Duration) -> io::Result<Option<ExitStatus>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.exit_status()? {
                let _ = self.output_done.lock_unpoisoned().recv_timeout(Duration::from_millis(500));
                return Ok(Some(status));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            thread::sleep(remaining.min(EXIT_POLL_INTERVAL));
        }
    }

    /// Block until the child exits (see `wait_exit` for the output grace period). Polls instead
    /// of blocking in the OS wait, so it never keeps `kill` or `exit_status` waiting on the
    /// child's lock. If socket clients are connected it then lingers up to `CLIENT_LINGER` for
    /// them to disconnect, so one blocked in `wait_exit` still receives its answer before the
    /// holder process ends. With no clients it returns at once.
    pub fn wait(&self) -> io::Result<ExitStatus> {
        let status = loop {
            if let Some(status) = self.wait_exit(Duration::from_secs(3600))? {
                break status;
            }
        };
        let deadline = Instant::now() + CLIENT_LINGER;
        while self.open_connections.load(Ordering::Acquire) > 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        Ok(status)
    }

    /// Names the file whose modification time marks this session's last attach: every raw-TTY
    /// client that connects bumps it, so bare `vtee attach` can pick the session used last.
    pub fn set_activity_file(&self, path: &Path) {
        *self.activity_file.lock_unpoisoned() = Some(path.to_path_buf());
    }

    /// Best effort: a session whose record can't be touched is still a working session.
    pub(crate) fn touch_activity(&self) {
        let Some(path) = self.activity_file.lock_unpoisoned().clone() else { return };
        if let Ok(file) = std::fs::OpenOptions::new().write(true).open(path) {
            let _ = file.set_modified(std::time::SystemTime::now());
        }
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
        self.wait_for(text, timeout, false)
    }

    /// Like [`Tap::wait_for_text`], but matches against logical lines: rows joined across soft wraps
    /// (`Snapshot::wrapped_rows`), so text the program printed on one line matches however narrow the
    /// screen is. A hard line break still separates two lines. The wrap blank a wide character leaves at
    /// the end of a row is not part of the joined text.
    pub fn wait_for_joined_text(&self, text: &str, timeout: Duration) -> (bool, Snapshot) {
        self.wait_for(text, timeout, true)
    }

    fn wait_for(&self, text: &str, timeout: Duration, join_wrapped: bool) -> (bool, Snapshot) {
        let deadline = Instant::now() + timeout;
        let mut guard = self.shared.state.lock_unpoisoned();
        loop {
            // Only the row text is needed to check for a match - building the full `Snapshot`
            // (every cell's colour and attributes) on every wake, for every waiter, just to throw
            // it away on every iteration that doesn't match, is wasted work under the same lock
            // the pty reader needs. It's built for real exactly once below, only once this call
            // is actually about to return.
            let found = if join_wrapped {
                snapshot::logical_lines(&guard.term).iter().any(|l| l.contains(text))
            } else {
                snapshot::row_lines(&guard.term).iter().any(|l| l.contains(text))
            };
            let now = Instant::now();
            if found || now >= deadline {
                let snap = snapshot::build(&guard.term, guard.bytes_seen, self.title.current_title());
                return (found, snap);
            }
            let (g, _) =
                self.shared.changed.wait_timeout(guard, deadline - now).unwrap_or_else(PoisonError::into_inner);
            guard = g;
        }
    }

    /// Propagate the real terminal's resizes to the pty and emulator: `SIGWINCH` on Unix, a
    /// console-size poll on Windows (which has no such signal). A script drives resizes
    /// explicitly instead.
    ///
    /// `observed` is the terminal size the caller had already accounted for when it chose the
    /// spawn size (not the spawn size itself, which `--cols/--rows` may override). A terminal that
    /// has changed since then is applied immediately, so a resize between the two reads is not
    /// lost; later changes are followed as they happen.
    pub fn watch_resize(&self, observed: (u16, u16)) {
        let master = self.master.clone();
        let shared = self.shared.clone();
        // A 0x0 report is a transient non-size (seen under `docker run -t`), not a resize to
        // nothing; skip it rather than clamp to a visible 1x1.
        let apply = move |cols: u16, rows: u16| {
            if cols == 0 || rows == 0 {
                return;
            }
            let _ = master.lock_unpoisoned().resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
            shared.state.lock_unpoisoned().term.resize(Size { cols: cols as usize, rows: rows as usize });
        };

        #[cfg(unix)]
        {
            use signal_hook::consts::SIGWINCH;
            use signal_hook::iterator::Signals;

            let Ok(mut signals) = Signals::new([SIGWINCH]) else { return };
            // Registered first, so a resize after this check can't slip past both.
            if let Ok(now) = crossterm::terminal::size()
                && now != observed
            {
                apply(now.0, now.1);
            }
            thread::spawn(move || {
                for _ in signals.forever() {
                    if let Ok((cols, rows)) = crossterm::terminal::size() {
                        apply(cols, rows);
                    }
                }
            });
        }
        #[cfg(windows)]
        {
            // Only a *change* from what the caller had accounted for counts, so a `--cols/--rows`
            // override isn't undone while the terminal stays put.
            let mut last = observed;
            thread::spawn(move || {
                loop {
                    match crossterm::terminal::size() {
                        Ok(size) if size != last => {
                            last = size;
                            apply(size.0, size.1);
                        }
                        _ => {}
                    }
                    thread::sleep(Duration::from_millis(200));
                }
            });
        }
        #[cfg(not(any(unix, windows)))]
        drop((apply, observed));
    }

    /// Catch `SIGTERM` and `SIGHUP`: restore the terminal mode, kill the child, exit. `SIGHUP` is
    /// included because this is exactly what the kernel sends a foreground process when its
    /// controlling terminal closes, and its default disposition (like `SIGKILL`'s) runs no
    /// destructors at all. Uses `clone_killer` because the thread that owns the `Tap` may be
    /// blocked in `wait()`. On Windows the trigger is a console close, break or shutdown event;
    /// session files are left for `ls`/`kill` to clean.
    pub fn watch_sigterm(&self) {
        self.watch_sigterm_removing(&[]);
    }

    /// As [`watch_sigterm`](Self::watch_sigterm), and on Unix also delete `files` (a session's pid
    /// record and sockets) before exiting: `process::exit` skips destructors, so an external
    /// `kill <pid>` or a closed terminal would otherwise leave them behind for `ls` to show as
    /// stale. Ignored on Windows.
    pub fn watch_sigterm_removing(&self, files: &[PathBuf]) {
        #[cfg(not(unix))]
        let _ = files;
        #[cfg(unix)]
        {
            use signal_hook::consts::{SIGHUP, SIGTERM};
            use signal_hook::iterator::Signals;

            let mut killer = self.child.lock_unpoisoned().clone_killer();
            let Ok(mut signals) = Signals::new([SIGTERM, SIGHUP]) else { return };
            let files = files.to_vec();
            thread::spawn(move || {
                if let Some(signal) = signals.forever().next() {
                    for file in &files {
                        let _ = std::fs::remove_file(file);
                    }
                    let _ = crossterm::terminal::disable_raw_mode();
                    let _ = killer.kill();
                    std::process::exit(128 + signal);
                }
            });
        }
        #[cfg(windows)]
        {
            static KILLER: std::sync::OnceLock<Mutex<Box<dyn portable_pty::ChildKiller + Send + Sync>>> =
                std::sync::OnceLock::new();
            static FILES: std::sync::OnceLock<Vec<PathBuf>> = std::sync::OnceLock::new();
            const CTRL_BREAK_EVENT: u32 = 1;
            const CTRL_CLOSE_EVENT: u32 = 2;
            const CTRL_SHUTDOWN_EVENT: u32 = 6;

            // Runs on a thread Windows creates for the event; the process is killed shortly after
            // it returns, so the child must be killed before then.
            unsafe extern "system" fn on_ctrl(event: u32) -> i32 {
                if !matches!(event, CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT | CTRL_SHUTDOWN_EVENT) {
                    return 0;
                }
                if let Some(files) = FILES.get() {
                    for file in files {
                        let _ = std::fs::remove_file(file);
                    }
                }
                let _ = crossterm::terminal::disable_raw_mode();
                crate::rawmode::restore_console_modes();
                if let Some(killer) = KILLER.get() {
                    let _ = killer.lock_unpoisoned().kill();
                }
                std::process::exit(143);
            }

            let _ = FILES.set(files.to_vec());
            if KILLER.set(Mutex::new(self.child.lock_unpoisoned().clone_killer())).is_ok() {
                unsafe {
                    SetConsoleCtrlHandler(Some(on_ctrl), 1);
                }
            }
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

/// `replies`: when this tap is the child's only terminal, the listener whose queued query
/// replies get written back to the child through the pty writer after each chunk is parsed.
fn forward_output(
    reader: &mut Box<dyn Read + Send>,
    shared: SharedState,
    mut log: Option<RawLog>,
    write_raw: bool,
    replies: Option<(ObserverListener, SharedWriter)>,
) {
    let mut stdout = io::stdout();
    // Mutable: a broken pipe on our own stdout (`vtee prog | head`, once `head` has read enough
    // and closes its end) stops *this* copy, not the whole session - the emulator, the log and
    // every socket client must keep going, and the pty must keep being drained so the child
    // doesn't block once its pty buffer fills.
    let mut write_raw = write_raw;
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
                write_raw = false;
            }
            let _ = stdout.flush();
        }
        if let Some(log) = log.as_mut() {
            let _ = log.write_event(chunk);
        }
        let attach_client_answers = {
            let mut state = shared.state.lock_unpoisoned();
            parser.advance(&mut state.term, chunk);
            state.bytes_seen += n as u64;
            // `push` never blocks. A client that fell behind is resynchronised with a redraw
            // (built only then); a sink whose writer thread is gone is dropped here.
            if !state.tty_sinks.is_empty() {
                let shared_chunk: Arc<[u8]> = chunk.into();
                let TapState { term, bytes_seen, tty_sinks } = &mut *state;
                tty_sinks.retain(|(_, queue)| {
                    queue.push(&shared_chunk, || snapshot::redraw_sequence(&snapshot::build(term, *bytes_seen, None)))
                });
            }
            shared.changed.notify_all();
            !state.tty_sinks.is_empty()
        };
        // Outside the state lock: `send_bytes` callers take only the writer lock, and a stalled
        // pty writer must never hold up `snapshot()`.
        if let Some((listener, writer)) = &replies {
            let pending = listener.take_replies();
            // An attached terminal sees this chunk too and answers it itself; a second reply
            // from us would land in the child's input as stray keystrokes.
            // Known limitation: this assumes the attached client really is a terminal. A
            // pipe-driven client, or a dead sink whose writer thread hasn't exited yet, answers
            // nothing, so a child waiting on a reply then waits until a real terminal attaches.
            if !pending.is_empty() && !attach_client_answers {
                let mut w = writer.lock_unpoisoned();
                for reply in pending {
                    if w.write_all(reply.as_bytes()).is_err() {
                        break;
                    }
                }
                let _ = w.flush();
            }
        }
        // std's Mutex isn't fair: under a flooding child this loop re-takes the lock before a
        // waiting `snapshot()` caller ever gets scheduled, starving it.
        thread::yield_now();
    }
    if let Some(log) = log.as_mut() {
        let _ = log.finish();
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

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetConsoleCtrlHandler(handler: Option<unsafe extern "system" fn(u32) -> i32>, add: i32) -> i32;
}

/// Windows only. A parent started with `CREATE_NEW_PROCESS_GROUP` (a CI runner, an agent
/// harness, `start /b`) hands every descendant an "ignore CTRL+C" attribute, so a child spawned by
/// [`Tap::spawn`] never sees Ctrl-C. Call this before spawning to get a terminal's behaviour. The
/// CLI does; the library leaves a host process's console settings alone.
#[cfg(windows)]
pub fn restore_default_ctrl_c() {
    // (NULL, FALSE) restores normal CTRL+C processing for this process and its future children.
    // Failing (no console at all) is harmless: there is no Ctrl-C to handle either.
    unsafe {
        SetConsoleCtrlHandler(None, 0);
    }
}

fn to_io_error(e: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::other(e)
}

#[cfg(test)]
mod sink_queue_tests {
    use super::*;

    fn chunk(bytes: &[u8]) -> Arc<[u8]> {
        Arc::from(bytes)
    }

    fn drain(queue: &SinkQueue) -> Vec<Vec<u8>> {
        queue.close();
        std::iter::from_fn(|| queue.pop()).map(|c| c.to_vec()).collect()
    }

    #[test]
    fn output_within_the_limit_keeps_its_order_after_the_first_redraw() {
        let queue = SinkQueue::new(b"R0".to_vec());
        assert!(queue.push(&chunk(b"a"), || unreachable!("no resync below the limit")));
        assert!(queue.push(&chunk(b"b"), || unreachable!("no resync below the limit")));
        assert_eq!(drain(&queue), vec![b"R0".to_vec(), b"a".to_vec(), b"b".to_vec()]);
    }

    #[test]
    fn a_client_that_falls_behind_gets_one_redraw_instead_of_the_backlog() {
        let queue = SinkQueue::new(b"R0".to_vec());
        let big = Arc::<[u8]>::from(vec![b'x'; SINK_QUEUE_BYTES]);
        assert!(queue.push(&big, || b"R1".to_vec()), "overflow resyncs, it does not drop the sink");
        assert!(queue.push(&chunk(b"after"), || unreachable!("the queue is small again")));
        assert_eq!(drain(&queue), vec![b"R1".to_vec(), b"after".to_vec()]);
    }

    #[test]
    fn a_stalled_reader_costs_bounded_memory_and_never_blocks_the_producer() {
        let queue = SinkQueue::new(Vec::new());
        let piece = chunk(&[b'y'; 8192]);
        for _ in 0..10_000 {
            assert!(queue.push(&piece, || b"R".to_vec()));
        }
        let queued: usize = drain(&queue).iter().map(Vec::len).sum();
        assert!(queued <= SINK_QUEUE_BYTES, "queued {queued} bytes, limit {SINK_QUEUE_BYTES}");
    }

    #[test]
    fn a_redraw_larger_than_the_limit_does_not_resync_on_every_chunk() {
        let queue = SinkQueue::new(Vec::new());
        let big = Arc::<[u8]>::from(vec![b'x'; SINK_QUEUE_BYTES + 1]);
        assert!(queue.push(&big, || vec![b'R'; 3 * SINK_QUEUE_BYTES]));
        let mut calls = 0;
        for _ in 0..100 {
            assert!(queue.push(&chunk(b"z"), || {
                calls += 1;
                Vec::new()
            }));
        }
        assert_eq!(calls, 0, "small chunks after a large redraw must not trigger more redraws");
    }

    #[test]
    fn a_closed_queue_reports_the_sink_gone_and_wakes_a_waiting_writer() {
        let queue = SinkQueue::new(Vec::new());
        assert!(queue.pop().is_some(), "the initial (empty) redraw");
        let waiter = {
            let queue = queue.clone();
            thread::spawn(move || queue.pop())
        };
        thread::sleep(Duration::from_millis(50));
        queue.close();
        assert!(waiter.join().unwrap().is_none(), "close wakes a blocked pop with nothing left");
        assert!(!queue.push(&chunk(b"late"), Vec::new));
    }
}
