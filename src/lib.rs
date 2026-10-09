//! `vtee`: spawn a child in a pty, forward its bytes raw to our terminal, and keep an
//! `alacritty_terminal` emulator watching a copy so the screen can be queried and driven over
//! sockets (`docs/architecture.md`).

mod command;
mod keys;
mod log;
pub mod net;
mod rawmode;
mod serve;
mod session;
pub mod snapshot;
mod sockets;
mod tap;

pub use command::{Command, CommandResult, Denied, HumanPriority, WritePolicy};
pub use log::{RawLog, replay};
pub use rawmode::RawModeGuard;
#[cfg(unix)]
pub use rawmode::run_tty_guard;
pub use serve::ServeOptions;
pub use session::{
    SessionPaths, SessionRecord, default_log_path, session_is_live, session_names, session_paths, session_record,
    write_session_record,
};
pub use snapshot::{Cell, Color, Cursor, CursorShape, Snapshot, redraw_sequence};
#[cfg(windows)]
pub use tap::restore_default_ctrl_c;
pub use tap::{SpawnOptions, Tap};

pub(crate) use tap::{ObserverListener, Size};
