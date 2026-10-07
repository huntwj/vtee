//! v0 "tap": spawn a child in a pty, forward its bytes to our own stdout/stdin raw and
//! unmodified (requirement 7, transparent observer mode), while a copy of the child's output
//! also feeds an `alacritty_terminal` emulator so its state can be inspected. The emulator is a
//! passive observer and never alters what is forwarded.

use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::{Config, Term};
use portable_pty::{Child, CommandBuilder, ExitStatus, MasterPty, PtySize, native_pty_system};
use vte::ansi::Processor;

#[derive(Clone)]
pub struct NullListener;

impl EventListener for NullListener {
    fn send_event(&self, _event: Event) {}
}

/// Size handed to `alacritty_terminal`, which wants a `Dimensions` impl rather than a plain pair
/// of numbers. `total_lines`/`screen_lines` are equal because v0 keeps no emulator-side
/// scrollback of its own (requirement-3 scrollback is deferred to the snapshot API step).
#[derive(Clone, Copy)]
struct Size {
    cols: usize,
    rows: usize,
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

pub type SharedTerm = Arc<Mutex<Term<NullListener>>>;

/// A running tap: a child spawned in a pty, forwarded both ways, with a copy of its output
/// also feeding an in-memory terminal emulator.
pub struct Tap {
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    child: Box<dyn Child + Send + Sync>,
    term: SharedTerm,
    bytes_seen: Arc<AtomicU64>,
    output_done: Receiver<()>,
}

impl Tap {
    /// Spawn `program args...` in a pty of the given size, and start forwarding immediately:
    /// child output -> our stdout + the emulator, our stdin -> child input.
    pub fn spawn(program: &str, args: &[String], cols: u16, rows: u16) -> io::Result<Self> {
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

        let term = Arc::new(Mutex::new(Term::new(
            Config::default(),
            &Size { cols: cols as usize, rows: rows as usize },
            NullListener,
        )));
        let bytes_seen = Arc::new(AtomicU64::new(0));

        let mut reader = pair.master.try_clone_reader().map_err(to_io_error)?;
        let (done_tx, output_done) = mpsc::channel();
        {
            let term = term.clone();
            let bytes_seen = bytes_seen.clone();
            thread::spawn(move || {
                forward_output(&mut reader, term, bytes_seen);
                let _ = done_tx.send(());
            });
        }

        let mut writer = pair.master.take_writer().map_err(to_io_error)?;
        thread::spawn(move || forward_input(&mut writer));

        let master = Arc::new(Mutex::new(pair.master));
        Ok(Tap { master, child, term, bytes_seen, output_done })
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
        self.term.lock().unwrap().resize(Size { cols: cols as usize, rows: rows as usize });
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

    pub fn bytes_seen(&self) -> u64 {
        self.bytes_seen.load(Ordering::Relaxed)
    }

    /// Watch for the real terminal being resized and propagate it to the child pty and the
    /// emulator, so all three stay the same size (a "detail that bites" from HANDOFF.md). Unix
    /// only for now (SIGWINCH); requirement 8 defers Windows (ConPTY) to later.
    #[cfg(unix)]
    pub fn watch_resize(&self) {
        use signal_hook::consts::SIGWINCH;
        use signal_hook::iterator::Signals;

        let master = self.master.clone();
        let term = self.term.clone();
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
                    term.lock().unwrap().resize(Size { cols: cols as usize, rows: rows as usize });
                }
            }
        });
    }

    #[cfg(not(unix))]
    pub fn watch_resize(&self) {
        // TODO(requirement 8): Windows resize watching (ConPTY has no SIGWINCH equivalent;
        // needs polling console size or a console resize event).
    }

    /// Plain-text rows of the current screen. A stand-in for the real snapshot API (cells,
    /// attributes, cursor, modes - HANDOFF.md step 3), kept here only so v0 is independently
    /// checkable without that API existing yet.
    pub fn snapshot_lines(&self) -> Vec<String> {
        let term = self.term.lock().unwrap();
        let grid = term.grid();
        (0..grid.screen_lines())
            .map(|r| {
                let line = alacritty_terminal::index::Line(r as i32);
                (0..grid.columns())
                    .map(|c| grid[alacritty_terminal::index::Point::new(line, alacritty_terminal::index::Column(c))].c)
                    .collect::<String>()
            })
            .collect()
    }
}

fn forward_output(reader: &mut Box<dyn Read + Send>, term: SharedTerm, bytes_seen: Arc<AtomicU64>) {
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
        {
            let mut term = term.lock().unwrap();
            parser.advance(&mut *term, chunk);
        }
        bytes_seen.fetch_add(n as u64, Ordering::Relaxed);
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
