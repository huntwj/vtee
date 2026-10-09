//! `vtee`: spawn a child in a pty, forward its bytes raw to our terminal, and keep an
//! `alacritty_terminal` emulator watching a copy so the screen can be queried and driven over
//! sockets (`docs/architecture.md`).

mod command;
mod keys;
mod log;
mod rawmode;
mod serve;
mod session;
pub mod snapshot;
#[cfg(unix)]
mod sockets;
mod tap;

pub use command::{Command, CommandResult, Denied, HumanPriority, WritePolicy};
pub use log::{RawLog, replay};
pub use rawmode::RawModeGuard;
#[cfg(unix)]
pub use rawmode::run_tty_guard;
pub use serve::ServeOptions;
pub use session::{SessionPaths, default_log_path, session_is_live, session_names, session_paths};
pub use snapshot::{Cell, Color, Cursor, CursorShape, Snapshot, redraw_sequence};
pub use tap::{SpawnOptions, Tap};

pub(crate) use tap::{ObserverListener, Size};

#[cfg(not(unix))]
pub(crate) fn unsupported(what: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Unsupported, format!("{what} needs Unix"))
}
