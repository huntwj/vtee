//! Unix-domain sockets on both platforms: `std`'s on Unix, `uds_windows` (AF_UNIX, Windows 10
//! 1803+) on Windows. Same API either way, so the servers and clients are written once.
//!
//! Access control follows tmux (`docs/guide/security.md`): a socket only its owner can open
//! (`bind_private`) and a check of every connecting process's uid (`peer_is_trusted`).

use std::io;
use std::path::Path;

#[cfg(unix)]
pub use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(windows)]
pub use uds_windows::{UnixListener, UnixStream};

/// Binds a listening socket that only its owner can connect to: mode `0600`, whatever the
/// process umask. The umask is tightened around `bind` rather than the file being `chmod`ed
/// afterwards, so there is no moment where the socket exists with looser permissions. On
/// Windows the per-user profile directory is the boundary and this is a plain bind.
pub fn bind_private(path: &Path) -> io::Result<UnixListener> {
    #[cfg(unix)]
    {
        use std::sync::{Mutex, PoisonError};

        // The umask is process-wide: serialise changes so two binds can't restore each other's.
        static UMASK: Mutex<()> = Mutex::new(());
        let _serialised = UMASK.lock().unwrap_or_else(PoisonError::into_inner);
        // SAFETY: umask only swaps the process's file-creation mask and cannot fail.
        let previous = unsafe { libc::umask(0o177) };
        let bound = UnixListener::bind(path);
        // SAFETY: as above; puts back the mask saved just before.
        unsafe { libc::umask(previous) };
        bound
    }
    #[cfg(windows)]
    {
        UnixListener::bind(path)
    }
}

/// Whether the process on the other end of `stream` may talk to this session: the same user, or
/// root, exactly as tmux decides (its server allows its own uid and uid 0, and refuses everyone
/// else). A peer whose identity can't be read is refused. On Windows there is no peer check yet;
/// see `docs/guide/security.md`.
pub fn peer_is_trusted(stream: &UnixStream) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: geteuid has no preconditions.
        let own = unsafe { libc::geteuid() };
        peer_uid(stream).is_some_and(|peer| uid_allowed(peer, own))
    }
    #[cfg(windows)]
    {
        let _ = stream;
        true
    }
}

#[cfg(unix)]
fn uid_allowed(peer: u32, own: u32) -> bool {
    peer == own || peer == 0
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn peer_uid(stream: &UnixStream) -> Option<u32> {
    use std::os::fd::AsRawFd;

    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `cred` and `len` are valid for the call, and `len` holds the size of `cred`.
    let status = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    (status == 0).then_some(cred.uid)
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
fn peer_uid(stream: &UnixStream) -> Option<u32> {
    use std::os::fd::AsRawFd;

    let (mut uid, mut gid) = (0, 0);
    // SAFETY: both out-pointers are valid for the call.
    let status = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    (status == 0).then_some(uid)
}

/// Other Unixes: no portable way to read the peer's uid here, so the directory and socket modes
/// are the only protection.
#[cfg(all(
    unix,
    not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))
))]
fn peer_uid(_stream: &UnixStream) -> Option<u32> {
    // SAFETY: geteuid has no preconditions.
    Some(unsafe { libc::geteuid() })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("vtee-net-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_same_user_and_root_are_allowed_everyone_else_is_not() {
        assert!(uid_allowed(1000, 1000));
        assert!(uid_allowed(0, 1000));
        assert!(!uid_allowed(1001, 1000));
        assert!(uid_allowed(0, 0));
        assert!(!uid_allowed(1000, 0));
    }

    #[test]
    fn a_connection_from_this_process_is_trusted() {
        let dir = scratch("trusted");
        let path = dir.join("s.sock");
        let listener = bind_private(&path).unwrap();
        let client = UnixStream::connect(&path).unwrap();
        let (server_side, _) = listener.accept().unwrap();
        assert!(peer_is_trusted(&server_side));
        drop(client);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A plain `bind` under the usual umask (022) would give 0755; `tests/security.rs` repeats
    /// this under umask 000 through the real binary.
    #[test]
    fn a_private_socket_is_mode_0600() {
        use std::os::unix::fs::PermissionsExt;

        let dir = scratch("mode");
        let path = dir.join("s.sock");
        let _listener = bind_private(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "socket mode was {mode:o}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
