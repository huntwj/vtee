//! Named sessions: where their sockets and pidfiles live, and how to find them.

use std::io;
use std::path::{Component, Path, PathBuf};

/// A named session's addresses, all inside the hardened per-user session directory: the NDJSON
/// command socket, the raw-TTY socket, and the pidfile `ls`/`kill` use.
pub struct SessionPaths {
    pub sock: PathBuf,
    pub tty_sock: PathBuf,
    pub pidfile: PathBuf,
}

/// `$TMPDIR/vtee-<uid>/`, created `0700`. Access control is the private directory rather than
/// socket-file permissions (as in tmux). An existing directory is verified, not trusted: it must
/// be a plain directory (not a symlink), owned by us, with no group/other access.
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
            // Another process may win the race to create it; re-verify either way.
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

/// `%LOCALAPPDATA%i-mux` (temp if unset): already private to the user by the profile's ACL.
/// Kept short because AF_UNIX paths cap at 107 bytes. An existing entry must be a plain
/// directory, not a symlink or junction.
#[cfg(windows)]
fn default_session_dir() -> io::Result<PathBuf> {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let dir = base.join("vtee");
    match std::fs::symlink_metadata(&dir) {
        Ok(meta) if !meta.is_dir() || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 => {
            return Err(io::Error::other(format!(
                "{} exists and isn't a plain directory - refusing to use it as the default session dir",
                dir.display()
            )));
        }
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => match std::fs::create_dir_all(&dir) {
            Ok(()) => return default_session_dir(),
            Err(e) => return Err(e),
        },
        Err(e) => return Err(e),
    }
    Ok(dir)
}

/// A name must be exactly one plain path component, otherwise `join` would escape the session
/// directory. `session_paths` is the only place a name becomes a path.
fn valid_session_name(name: &str) -> bool {
    !name.is_empty() && matches!(Path::new(name).components().collect::<Vec<_>>().as_slice(), [Component::Normal(_)])
}

pub fn session_paths(name: &str) -> io::Result<SessionPaths> {
    if !valid_session_name(name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name:?} isn't a valid session name - no \"/\", no \".\"/\"..\", not empty"),
        ));
    }
    let dir = default_session_dir()?;
    #[cfg(windows)]
    {
        // The longest file is `<name>.tty.sock`; AF_UNIX addresses cap at 107 bytes.
        let longest = dir.join(format!("{name}.tty.sock")).as_os_str().len();
        if longest > 107 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("session name {name:?} is too long: the socket path would be {longest} bytes (limit 107)"),
            ));
        }
    }
    Ok(SessionPaths {
        sock: dir.join(format!("{name}.sock")),
        tty_sock: dir.join(format!("{name}.tty.sock")),
        pidfile: dir.join(format!("{name}.pid")),
    })
}

/// Whether a session is live. Connects to its socket rather than trusting the pidfile or the
/// socket file's existence, since a killed session leaves both behind.
pub fn session_is_live(name: &str) -> io::Result<bool> {
    connects(&session_paths(name)?.sock)
}

fn connects(sock: &Path) -> io::Result<bool> {
    match crate::net::UnixStream::connect(sock) {
        Ok(_) => Ok(true),
        Err(e) if matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused) => Ok(false),
        Err(e) => Err(e),
    }
}

/// Every session name with a pidfile; stale ones are included, so check each with
/// `session_is_live`.
pub fn session_names() -> io::Result<Vec<String>> {
    let dir = default_session_dir()?;
    let mut names = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "pid")
            && let Some(name) = path.file_stem().and_then(|s| s.to_str())
        {
            names.push(name.to_string());
        }
    }
    Ok(names)
}

/// Where diagnostic logging goes while a terminal is showing the child: beside the sockets.
pub fn default_log_path() -> io::Result<PathBuf> {
    Ok(default_session_dir()?.join(format!("{}.log", std::process::id())))
}
