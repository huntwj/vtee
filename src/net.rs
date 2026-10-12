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

/// Whether `path` currently names a socket file, without following symlinks. Used before removing
/// a path that might not be ours to remove: a stale socket left by a dead session is safe to
/// clear, but an ordinary file - a typo in `--listen=<path>`, or a path someone else cares about -
/// never is. On Windows, `uds_windows` implements AF_UNIX sockets as reparse points tagged
/// `IO_REPARSE_TAG_AF_UNIX` (`0x80000023`, confirmed with `fsutil reparsepoint query` against a
/// real bound socket); a symlink or junction is also a reparse point but a different tag, so
/// checking the attribute alone also passes those through as "a socket" - an accepted, narrow
/// over-approximation (an ordinary file, which is what actually needs protecting, is never a
/// reparse point at all).
pub fn path_is_socket(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_socket())
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        std::fs::symlink_metadata(path).is_ok_and(|m| m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
    }
}

/// Whether the process on the other end of `stream` may talk to this session: the same user, or
/// root, exactly as tmux decides (its server allows its own uid and uid 0, and refuses everyone
/// else). A peer whose identity can't be read is refused. On Windows the peer is the process
/// behind the socket (`SIO_AF_UNIX_GETPEERPID`), and the same rule applies to its user SID, with
/// SYSTEM standing in for root.
pub fn peer_is_trusted(stream: &UnixStream) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: geteuid has no preconditions.
        let own = unsafe { libc::geteuid() };
        peer_uid(stream).is_some_and(|peer| uid_allowed(peer, own))
    }
    #[cfg(windows)]
    {
        windows_peer::trusted(stream)
    }
}

#[cfg(windows)]
mod windows_peer {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawSocket;
    use std::ptr::{null, null_mut};

    use super::UnixStream;

    /// `_WSAIOR(IOC_VENDOR, 256)` from `afunix.h`.
    const SIO_AF_UNIX_GETPEERPID: u32 = 0x5800_0100;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const TOKEN_QUERY: u32 = 0x0008;
    const TOKEN_USER: u32 = 1;
    /// `S-1-5-18`, the local SYSTEM account: the nearest thing Windows has to uid 0.
    const SYSTEM_SID: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 5, 18, 0, 0, 0];

    #[link(name = "ws2_32")]
    unsafe extern "system" {
        fn WSAIoctl(
            socket: usize,
            code: u32,
            input: *const c_void,
            input_len: u32,
            output: *mut c_void,
            output_len: u32,
            returned: *mut u32,
            overlapped: *mut c_void,
            completion: *mut c_void,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn GetCurrentProcess() -> *mut c_void;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn OpenProcessToken(process: *mut c_void, access: u32, token: *mut *mut c_void) -> i32;
        fn GetTokenInformation(token: *mut c_void, class: u32, info: *mut c_void, len: u32, needed: *mut u32) -> i32;
        fn GetLengthSid(sid: *mut c_void) -> u32;
    }

    pub(super) fn trusted(stream: &UnixStream) -> bool {
        let Some(pid) = peer_pid(stream) else {
            // Older Windows without the ioctl: the per-user profile directory is the boundary.
            ::log::debug!("peer pid unavailable; relying on the directory permissions");
            return true;
        };
        let own = own_user_sid();
        // SAFETY: the handle is closed below, whatever the outcome.
        let peer_handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if peer_handle.is_null() {
            return false;
        }
        let peer = process_user_sid(peer_handle);
        // SAFETY: closes the handle opened just above.
        unsafe { CloseHandle(peer_handle) };
        match (own, peer) {
            (Some(own), Some(peer)) => sid_allowed(&peer, &own),
            _ => false,
        }
    }

    pub(super) fn sid_allowed(peer: &[u8], own: &[u8]) -> bool {
        peer == own || peer == SYSTEM_SID
    }

    pub(super) fn own_user_sid() -> Option<Vec<u8>> {
        // SAFETY: the pseudo-handle from GetCurrentProcess needs no closing.
        process_user_sid(unsafe { GetCurrentProcess() })
    }

    pub(super) fn peer_pid(stream: &UnixStream) -> Option<u32> {
        let mut pid = 0u32;
        let mut returned = 0u32;
        // SAFETY: `pid` and `returned` outlive the call and the output size is that of `pid`.
        let status = unsafe {
            WSAIoctl(
                stream.as_raw_socket() as usize,
                SIO_AF_UNIX_GETPEERPID,
                null(),
                0,
                (&mut pid as *mut u32).cast(),
                size_of::<u32>() as u32,
                &mut returned,
                null_mut(),
                null_mut(),
            )
        };
        // This ioctl leaves `returned` at 0 even on success (measured on Windows 11), so the status
        // and a non-zero pid are what count.
        (status == 0 && pid != 0).then_some(pid)
    }

    /// The raw bytes of the user SID of `process`, so two can be compared.
    fn process_user_sid(process: *mut c_void) -> Option<Vec<u8>> {
        let mut token = null_mut();
        // SAFETY: `token` is a valid out-pointer.
        if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
            return None;
        }
        let sid = token_user_sid(token);
        // SAFETY: closes the token opened above.
        unsafe { CloseHandle(token) };
        sid
    }

    fn token_user_sid(token: *mut c_void) -> Option<Vec<u8>> {
        let mut needed = 0u32;
        // SAFETY: a zero-length query only reports the size needed.
        unsafe { GetTokenInformation(token, TOKEN_USER, null_mut(), 0, &mut needed) };
        if needed == 0 {
            return None;
        }
        // 8-byte alignment: the buffer starts with a TOKEN_USER holding a pointer.
        let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
        // SAFETY: the buffer is at least `needed` bytes and suitably aligned.
        let filled = unsafe { GetTokenInformation(token, TOKEN_USER, buffer.as_mut_ptr().cast(), needed, &mut needed) };
        if filled == 0 {
            return None;
        }
        // SAFETY: on success the buffer starts with a TOKEN_USER whose first field is a pointer
        // to the SID, stored inside the same buffer.
        let sid = unsafe { *(buffer.as_ptr() as *const *mut c_void) };
        // SAFETY: `sid` points at a valid SID inside `buffer`, which is still alive.
        let len = unsafe { GetLengthSid(sid) } as usize;
        // SAFETY: GetLengthSid reported `len` bytes at `sid`.
        Some(unsafe { std::slice::from_raw_parts(sid as *const u8, len) }.to_vec())
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    fn connected_pair(name: &str) -> (UnixStream, UnixStream, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("vtee-net-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let listener = bind_private(&dir.join("s.sock")).unwrap();
        let client = UnixStream::connect(dir.join("s.sock")).unwrap();
        let (server_side, _) = listener.accept().unwrap();
        (client, server_side, dir)
    }

    #[test]
    fn this_process_resolves_to_a_user_sid() {
        let sid = windows_peer::own_user_sid();
        assert!(sid.as_ref().is_some_and(|s| s.len() >= 12), "no SID: {sid:?}");
    }

    /// Without this the trust test below could pass on the "ioctl unavailable" branch.
    #[test]
    fn the_peer_pid_ioctl_reports_the_client_process() {
        let (_client, server_side, dir) = connected_pair("pid");
        assert_eq!(windows_peer::peer_pid(&server_side), Some(std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_same_user_and_system_are_allowed_everyone_else_is_not() {
        let me = [1, 5, 0, 0, 0, 0, 0, 5, 21, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 0xe8, 3, 0, 0];
        let someone_else = {
            let mut s = me;
            s[27] = 4;
            s
        };
        let system = [1, 1, 0, 0, 0, 0, 0, 5, 18, 0, 0, 0];
        assert!(windows_peer::sid_allowed(&me, &me));
        assert!(windows_peer::sid_allowed(&system, &me));
        assert!(!windows_peer::sid_allowed(&someone_else, &me));
    }

    #[test]
    fn a_connection_from_this_process_is_trusted() {
        let (_client, server_side, dir) = connected_pair("trusted");
        assert!(peer_is_trusted(&server_side));
        let _ = std::fs::remove_dir_all(&dir);
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
