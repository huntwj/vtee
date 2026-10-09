//! Unix-domain sockets on both platforms: `std`'s on Unix, `uds_windows` (AF_UNIX, Windows 10
//! 1803+) on Windows. Same API either way, so the servers and clients are written once.

#[cfg(unix)]
pub use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(windows)]
pub use uds_windows::{UnixListener, UnixStream};
