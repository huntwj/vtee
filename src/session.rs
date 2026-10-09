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

/// `%LOCALAPPDATA%\vtee`: already private to the user by the profile's ACL. There is no fallback
/// to the temp dir, which can be shared (a service's is `C:\Windows\Temp`). Kept short because
/// AF_UNIX paths cap at 107 bytes. An existing entry must be a plain directory, not a symlink or
/// junction.
#[cfg(windows)]
fn default_session_dir() -> io::Result<PathBuf> {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).filter(|p| p.is_absolute()).ok_or_else(|| {
        io::Error::other("%LOCALAPPDATA% isn't set to an absolute path - no private place for sessions")
    })?;
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

/// Why Windows would not accept `name` as a file stem, if it wouldn't. Pure so it is tested on every
/// platform; only enforced on Windows, where `:` opens an NTFS alternate data stream and device
/// names (`CON`, `NUL`, ...) are special even with an extension.
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_name_problem(name: &str) -> Option<&'static str> {
    if name.chars().any(|c| c.is_control() || "<>:\"|?*".contains(c)) {
        return Some("contains a control character or one of < > : \" | ? *");
    }
    if name.starts_with([' ', '.']) || name.ends_with([' ', '.']) {
        return Some("starts or ends with a space or a dot");
    }
    let stem = name.split('.').next().unwrap_or(name).trim_end().to_ascii_uppercase();
    let numbered = |prefix: &str| {
        stem.strip_prefix(prefix).is_some_and(|n| n.len() == 1 && matches!(n.as_bytes()[0], b'1'..=b'9'))
    };
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") || numbered("COM") || numbered("LPT");
    reserved.then_some("is a reserved Windows device name")
}

/// A name must be exactly one plain path component, otherwise `join` would escape the session
/// directory. `session_paths` is the only place a name becomes a path.
fn valid_session_name(name: &str) -> Result<(), String> {
    let one_component = !name.is_empty()
        && matches!(Path::new(name).components().collect::<Vec<_>>().as_slice(), [Component::Normal(_)]);
    if !one_component {
        return Err("no \"/\", no \".\"/\"..\", not empty".to_string());
    }
    #[cfg(windows)]
    if let Some(problem) = windows_name_problem(name) {
        return Err(problem.to_string());
    }
    Ok(())
}

pub fn session_paths(name: &str) -> io::Result<SessionPaths> {
    if let Err(why) = valid_session_name(name) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{name:?} isn't a valid session name: {why}")));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_rejects_stream_wildcard_and_control_characters() {
        for bad in ["ab:c", "a*b", "a?b", "a<b", "a>b", "a\"b", "a|b", "a\u{1}b", "a\tb"] {
            assert!(windows_name_problem(bad).is_some(), "{bad:?}");
        }
    }

    #[test]
    fn windows_rejects_leading_and_trailing_space_or_dot() {
        for bad in [" a", "a ", ".a", "a.", "a. "] {
            assert!(windows_name_problem(bad).is_some(), "{bad:?}");
        }
    }

    #[test]
    fn windows_rejects_device_names_with_or_without_extension() {
        for bad in ["CON", "con", "Nul", "aux.x", "PRN.tar.gz", "COM1", "lpt9", "com3.txt"] {
            assert!(windows_name_problem(bad).is_some(), "{bad:?}");
        }
    }

    #[test]
    fn windows_accepts_ordinary_names() {
        for good in ["tf", "my-session_2", "a.b", "COM0", "COM10", "CONSOLE", "lpt", "x.con"] {
            assert_eq!(windows_name_problem(good), None, "{good:?}");
        }
    }

    #[test]
    fn every_platform_rejects_separators_dots_and_empty() {
        for bad in ["", ".", "..", "a/b", "../x", "/abs"] {
            assert!(valid_session_name(bad).is_err(), "{bad:?}");
        }
        assert!(valid_session_name("ok-name").is_ok());
    }
}
