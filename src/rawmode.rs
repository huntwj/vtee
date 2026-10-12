//! Raw mode for our own terminal, plus a watchdog process that restores it even if we are
//! `SIGKILL`'d.

use std::io;

/// Puts our controlling terminal in raw mode for the guard's lifetime so keystrokes reach the
/// child unmodified (requirement 7), restoring it on drop and on panic unwind. A no-op when
/// stdin isn't a terminal.
///
/// On Windows it also turns on the console's virtual-terminal flags (`ENABLE_VIRTUAL_TERMINAL_INPUT` on
/// stdin, `ENABLE_VIRTUAL_TERMINAL_PROCESSING` on stdout) so escape sequences in the child's output
/// are interpreted rather than printed and special keys reach the child as escape sequences. Terminals
/// like Windows Terminal already do this; a legacy `conhost` window does not. The original modes are
/// restored on drop and by the console control handler.
///
/// On Unix it also spawns a watchdog process that restores the terminal after `SIGKILL`. The
/// watchdog's stdin is one end of a socketpair; we hold the other end, so when we die for any
/// reason the OS closes it and the watchdog sees EOF. The watchdog can't call
/// `crossterm::disable_raw_mode`, which relies on process-local state, so we capture the original
/// termios ourselves and send it over the socket first.
pub struct RawModeGuard {
    active: bool,
    #[cfg(unix)]
    _watchdog: Option<std::os::unix::net::UnixStream>,
}

impl RawModeGuard {
    pub fn new() -> Self {
        use std::io::IsTerminal;
        let is_tty = io::stdin().is_terminal();
        #[cfg(unix)]
        let original_termios = if is_tty { capture_termios() } else { None };
        #[cfg(windows)]
        if is_tty {
            console::save_originals();
        }
        let active = is_tty && crossterm::terminal::enable_raw_mode().is_ok();
        #[cfg(windows)]
        if active {
            console::enable_vt();
        }
        RawModeGuard {
            active,
            #[cfg(unix)]
            _watchdog: if active { spawn_tty_watchdog(original_termios) } else { None },
        }
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
            #[cfg(windows)]
            console::restore();
        }
    }
}

/// Puts back the console modes saved by the guard. For a path that exits the process without
/// running destructors (a console control handler, or a client ending itself once its peer is
/// gone while another thread is blocked reading stdin with no portable way to interrupt it). A
/// no-op if no guard is active.
#[cfg(windows)]
pub fn restore_console_modes() {
    console::restore();
}

#[cfg(windows)]
mod console {
    use std::sync::Mutex;

    type Handle = isize;
    const STD_INPUT_HANDLE: u32 = -10i32 as u32;
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const INVALID_HANDLE_VALUE: Handle = -1;
    const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(std_handle: u32) -> Handle;
        fn GetConsoleMode(handle: Handle, mode: *mut u32) -> i32;
        fn SetConsoleMode(handle: Handle, mode: u32) -> i32;
    }

    /// (handle, mode before we touched it, flag to add) for each console handle we changed.
    static SAVED: Mutex<Vec<(Handle, u32, u32)>> = Mutex::new(Vec::new());

    fn console_mode(std_handle: u32) -> Option<(Handle, u32)> {
        unsafe {
            let handle = GetStdHandle(std_handle);
            if handle == 0 || handle == INVALID_HANDLE_VALUE {
                return None;
            }
            let mut mode = 0u32;
            // Fails for a redirected handle, which is exactly when there is nothing to set.
            if GetConsoleMode(handle, &mut mode) == 0 { None } else { Some((handle, mode)) }
        }
    }

    /// Remembers the modes to restore. Called before raw mode changes the input mode.
    pub(super) fn save_originals() {
        let mut saved = SAVED.lock().unwrap_or_else(|e| e.into_inner());
        saved.clear();
        if let Some((h, mode)) = console_mode(STD_INPUT_HANDLE) {
            saved.push((h, mode, ENABLE_VIRTUAL_TERMINAL_INPUT));
        }
        if let Some((h, mode)) = console_mode(STD_OUTPUT_HANDLE) {
            saved.push((h, mode, ENABLE_VIRTUAL_TERMINAL_PROCESSING));
        }
    }

    /// Adds the VT flags on top of the *current* modes, so raw mode's own changes are kept.
    pub(super) fn enable_vt() {
        let saved = SAVED.lock().unwrap_or_else(|e| e.into_inner());
        for &(handle, _, flag) in saved.iter() {
            unsafe {
                let mut current = 0u32;
                if GetConsoleMode(handle, &mut current) != 0 {
                    SetConsoleMode(handle, current | flag);
                }
            }
        }
    }

    pub(super) fn restore() {
        let mut saved = SAVED.lock().unwrap_or_else(|e| e.into_inner());
        for &(handle, original, _) in saved.iter() {
            unsafe {
                SetConsoleMode(handle, original);
            }
        }
        saved.clear();
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
fn spawn_tty_watchdog(original_termios: Option<libc::termios>) -> Option<std::os::unix::net::UnixStream> {
    use std::io::Write;
    use std::os::fd::OwnedFd;
    use std::process::Stdio;

    let original_termios = original_termios?;
    let (mut ours, theirs) = std::os::unix::net::UnixStream::pair().ok()?;

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

/// The watchdog process's entry point (`main.rs` calls it for `--internal-tty-guard`): read the
/// termios sent by `spawn_tty_watchdog`, block until stdin hits EOF, then restore it.
///
/// - `/dev/tty` is opened up front and the fd held: reopening after the session leader dies
///   fails with `ENXIO`.
/// - Registering a `SIGHUP` handler keeps the watchdog alive when the kernel hangs up the
///   process group after the leader exits; the registration only needs to stay alive.
#[cfg(unix)]
pub fn run_tty_guard() -> ! {
    use std::io::Read;

    let _sighup_guard = signal_hook::iterator::Signals::new([signal_hook::consts::SIGHUP]);
    let tty = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty");

    let mut stdin = io::stdin();
    let mut termios_bytes = [0u8; std::mem::size_of::<libc::termios>()];
    if stdin.read_exact(&mut termios_bytes).is_ok() {
        let mut buf = [0u8; 1];
        let _ = stdin.read(&mut buf); // blocks until EOF (clean drop or the main process dying)

        if let Ok(tty) = tty {
            use std::os::fd::AsRawFd;
            // `read_unaligned`: a `[u8; N]` is only guaranteed 1-byte aligned, but termios needs more.
            let termios: libc::termios = unsafe { std::ptr::read_unaligned(termios_bytes.as_ptr().cast()) };
            unsafe {
                libc::tcsetattr(tty.as_raw_fd(), libc::TCSANOW, &termios);
            }
        }
    }
    std::process::exit(0);
}
