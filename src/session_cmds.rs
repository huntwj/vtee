//! `vtee attach`, `ls` and `kill`: clients of the named sessions other invocations hold.

use std::io::IsTerminal;
use std::path::Path;
use std::process::ExitCode;

use crate::output::fail;

/// `attach [name]`: relay our terminal to a session's raw-TTY socket. A pure client; the raw
/// protocol has no way to report the wrapped program's exit code, so this succeeds whenever the
/// session ends.
pub fn run_attach(name: Option<&str>) -> ExitCode {
    // The session's screen is replayed as terminal escape codes; into a pipe or file that is noise.
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return fail(
            exitcode::USAGE,
            "attach needs a terminal: stdin and stdout must both be terminals (use --connect for scripts)",
        );
    }
    let name = match name {
        Some(n) => n.to_string(),
        None => match resolve_default_session() {
            Ok(n) => n,
            Err(message) => return fail(exitcode::USAGE, message),
        },
    };
    let paths = match vtee::session_paths(&name) {
        Ok(p) => p,
        Err(e) => return fail(exitcode::OSERR, format!("couldn't resolve session {name:?}: {e}")),
    };
    let tty_sock = match vtee::session_record(&name) {
        Ok(Some(record)) => record.tty_sock,
        Ok(None) => paths.tty_sock,
        Err(e) => return fail(exitcode::OSERR, format!("couldn't read session {name:?}: {e}")),
    };
    attach(&name, &tty_sock)
}

fn attach(name: &str, tty_sock: &Path) -> ExitCode {
    use std::io::{Read, Write};

    let stream = match vtee::net::UnixStream::connect(tty_sock) {
        Ok(s) => s,
        Err(e) => return fail(exitcode::NOHOST, format!("failed to attach to {name:?}: {e}")),
    };
    let _raw_guard = vtee::RawModeGuard::new();

    let Ok(reader_stream) = stream.try_clone() else {
        return fail(
            exitcode::OSERR,
            format!("attach session {name:?} ended with an error: couldn't clone the socket"),
        );
    };
    let mut writer = stream;
    let reader_thread = std::thread::spawn(move || {
        let mut reader = reader_stream;
        let mut stdout = std::io::stdout();
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if stdout.write_all(&buf[..n]).is_err() {
                        break;
                    }
                    let _ = stdout.flush();
                }
            }
        }
    });

    // Raw reads, not `.lines()`: control bytes and escape sequences must pass through untouched.
    let mut stdin = std::io::stdin();
    let mut buf = [0u8; 4096];
    loop {
        let n = match stdin.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        if writer.write_all(&buf[..n]).is_err() {
            break;
        }
    }
    // `shutdown`, not `drop`: the cloned fd would keep the socket open (see `client::run_connect`).
    let _ = writer.shutdown(std::net::Shutdown::Write);
    let _ = reader_thread.join();
    ExitCode::SUCCESS
}

/// Bare `attach`, like tmux's `attach-session`: the only live session, else the one attached to
/// most recently. A holder bumps its record's mtime on every attach (and wrote it when it started),
/// so the newest mtime wins; equal times fall back to the name.
fn resolve_default_session() -> Result<String, String> {
    let names = vtee::session_names().map_err(|e| format!("couldn't list sessions: {e}"))?;
    let mut live = Vec::new();
    for name in names {
        if vtee::session_is_live(&name).unwrap_or(false) {
            let mtime = vtee::session_paths(&name)
                .ok()
                .and_then(|p| std::fs::metadata(&p.pidfile).ok())
                .and_then(|m| m.modified().ok())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            live.push((name, mtime));
        }
    }
    live.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)));
    live.pop().map(|(name, _)| name).ok_or_else(|| "no live sessions to attach to".to_string())
}

/// `ls`: each session with a pidfile, marked `live` (its recorded socket answers) or `stale`. A
/// socket that isn't the conventional one (`--listen=<path>`) is shown too.
pub fn run_ls() -> ExitCode {
    let names = match vtee::session_names() {
        Ok(n) => n,
        Err(e) => return fail(exitcode::OSERR, format!("couldn't list sessions: {e}")),
    };
    if names.is_empty() {
        println!("no sessions");
        return ExitCode::SUCCESS;
    }
    for name in names {
        let live = vtee::session_is_live(&name).unwrap_or(false);
        let record = vtee::session_record(&name).ok().flatten();
        let conventional_sock = vtee::session_paths(&name).ok().map(|p| p.sock);
        let status = if live { "live" } else { "stale" };
        let mut line = name;
        if let Some(pid) = record.as_ref().and_then(|r| r.pid) {
            line.push_str(&format!("\tpid={pid}"));
        }
        line.push_str(&format!("\t{status}"));
        if let Some(record) = &record
            && Some(&record.sock) != conventional_sock.as_ref()
        {
            line.push_str(&format!("\tsock={}", record.sock.display()));
        }
        println!("{line}");
    }
    ExitCode::SUCCESS
}

/// `kill <name>`: signal the holder and remove its files, including the sockets its record names.
/// The files are removed here because the holder's `SIGTERM` handler exits without running
/// destructors. A holder is signalled only if the recorded socket answers: a stale pidfile may name
/// a PID that has since been reused.
pub fn run_kill(name: &str) -> ExitCode {
    let paths = match vtee::session_paths(name) {
        Ok(p) => p,
        // A name that can't be a session here (invalid, or too long for the socket path) can't be
        // a live one: the same answer as any other session that doesn't exist.
        Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => {
            return fail(exitcode::NOHOST, format!("no session named {name:?}: {e}"));
        }
        Err(e) => return fail(exitcode::OSERR, format!("couldn't resolve session {name:?}: {e}")),
    };
    let record = match vtee::session_record(name) {
        Ok(r) => r,
        Err(e) => return fail(exitcode::OSERR, format!("couldn't read session {name:?}: {e}")),
    };
    match vtee::session_is_live(name) {
        Ok(true) => {
            if let Some(pid) = record.as_ref().and_then(|r| r.pid)
                && pid > 1
            {
                stop_holder(pid, &paths.pidfile);
            }
        }
        // A leftover pidfile just gets cleaned up; no pidfile at all means there was never a session.
        Ok(false) if record.is_some() => {}
        Ok(false) => return fail(exitcode::NOHOST, format!("no session named {name:?}")),
        Err(e) => return fail(exitcode::OSERR, format!("couldn't check session {name:?}: {e}")),
    }
    let (sock, tty_sock) = match record {
        Some(r) => (r.sock, r.tty_sock),
        None => (paths.sock, paths.tty_sock),
    };
    remove_socket_file(&sock);
    remove_socket_file(&tty_sock);
    let _ = std::fs::remove_file(&paths.pidfile);
    ExitCode::SUCCESS
}

/// A stale record's socket path may since have been reused for something else, so only an actual
/// socket is removed (`crate::net::path_is_socket`, checked on both platforms).
fn remove_socket_file(path: &Path) {
    if vtee::net::path_is_socket(path) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(unix)]
fn stop_holder(pid: i32, _pidfile: &Path) {
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
}

/// Seconds between 1601-01-01 (FILETIME epoch) and 1970-01-01.
#[cfg(any(windows, test))]
const FILETIME_UNIX_EPOCH_SECS: u64 = 11_644_473_600;

/// The holder writes its pidfile after it starts, so a process created *after* the pidfile was last
/// written cannot be the holder: its PID was reused. `creation` is a FILETIME (100 ns ticks since
/// 1601).
#[cfg(any(windows, test))]
fn created_after(creation: u64, pidfile_mtime: std::time::SystemTime) -> bool {
    use std::time::{Duration, UNIX_EPOCH};
    let since_1601 = Duration::new(creation / 10_000_000, (creation % 10_000_000) as u32 * 100);
    match since_1601.checked_sub(Duration::from_secs(FILETIME_UNIX_EPOCH_SECS)) {
        Some(since_unix) => UNIX_EPOCH + since_unix > pidfile_mtime,
        None => false,
    }
}

/// No graceful signal exists for a process without a console of ours, so terminate it outright;
/// ConPTY then reaps the wrapped program. `kill` removes the session files itself. A process
/// younger than the pidfile is a reused PID, never the holder, and is left alone.
#[cfg(windows)]
fn stop_holder(pid: i32, pidfile: &Path) {
    use std::ffi::c_void;

    const PROCESS_TERMINATE: u32 = 0x0001;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn TerminateProcess(process: *mut c_void, exit_code: u32) -> i32;
        fn GetProcessTimes(
            process: *mut c_void,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid as u32);
        if handle.is_null() {
            return;
        }
        let (mut creation, mut exit, mut kernel, mut user) =
            (FileTime::default(), FileTime::default(), FileTime::default(), FileTime::default());
        let mtime = std::fs::metadata(pidfile).and_then(|m| m.modified());
        let known_young = match mtime {
            Ok(mtime) if GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) != 0 => {
                created_after((u64::from(creation.high) << 32) | u64::from(creation.low), mtime)
            }
            // Without both times the age can't be checked; the socket answered, so go ahead.
            _ => false,
        };
        if !known_young {
            TerminateProcess(handle, 1);
        }
        CloseHandle(handle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn filetime(unix: Duration) -> u64 {
        (unix.as_secs() + FILETIME_UNIX_EPOCH_SECS) * 10_000_000 + u64::from(unix.subsec_nanos()) / 100
    }

    #[test]
    fn a_process_older_than_the_pidfile_is_the_holder() {
        let mtime = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        assert!(!created_after(filetime(Duration::from_secs(1_799_999_990)), mtime));
        assert!(!created_after(filetime(Duration::from_secs(1_800_000_000)), mtime));
    }

    #[test]
    fn a_process_younger_than_the_pidfile_has_a_reused_pid() {
        let mtime = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        assert!(created_after(filetime(Duration::from_secs(1_800_000_001)), mtime));
    }

    #[test]
    fn a_creation_time_before_the_unix_epoch_is_not_young() {
        assert!(!created_after(0, UNIX_EPOCH));
    }
}
