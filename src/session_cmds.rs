//! `vtee attach`, `ls` and `kill`: clients of the named sessions other invocations hold.

use std::path::Path;
use std::process::ExitCode;

use crate::output::fail;

/// `attach [name]`: relay our terminal to a session's raw-TTY socket. A pure client; the raw
/// protocol has no way to report the wrapped program's exit code, so this succeeds whenever the
/// session ends.
pub fn run_attach(name: Option<&str>) -> ExitCode {
    let name = match name {
        Some(n) => n.to_string(),
        None => match resolve_default_session() {
            Ok(n) => n,
            Err(message) => return fail(exitcode::USAGE, message),
        },
    };
    match vtee::session_paths(&name) {
        Ok(paths) => attach(&name, &paths.tty_sock),
        Err(e) => fail(exitcode::OSERR, format!("couldn't resolve session {name:?}: {e}")),
    }
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

/// Bare `attach`, like tmux's `attach-session`: the only live session, else the most recently
/// active. Activity is approximated by the pidfile's mtime.
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
    live.sort_by_key(|(_, mtime)| *mtime);
    live.pop().map(|(name, _)| name).ok_or_else(|| "no live sessions to attach to".to_string())
}

/// `ls`: each session with a pidfile, marked `live` (its socket answers) or `stale`.
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
        let pid = vtee::session_paths(&name).ok().and_then(|p| std::fs::read_to_string(&p.pidfile).ok());
        let status = if live { "live" } else { "stale" };
        match pid {
            Some(pid) => println!("{name}\tpid={}\t{status}", pid.trim()),
            None => println!("{name}\t{status}"),
        }
    }
    ExitCode::SUCCESS
}

/// `kill <name>`: signal the holder and remove its files. The files are removed here because the
/// holder's `SIGTERM` handler exits without running destructors. A holder is signalled only if
/// its socket answers: a stale pidfile may name a PID that has since been reused.
pub fn run_kill(name: &str) -> ExitCode {
    let paths = match vtee::session_paths(name) {
        Ok(p) => p,
        Err(e) => return fail(exitcode::OSERR, format!("couldn't resolve session {name:?}: {e}")),
    };
    let pidfile_existed = paths.pidfile.exists();
    match vtee::session_is_live(name) {
        Ok(true) => {
            if let Some(pid) = std::fs::read_to_string(&paths.pidfile).ok().and_then(|s| s.trim().parse::<i32>().ok())
                && pid > 1
            {
                send_sigterm(pid);
            }
        }
        // A leftover pidfile just gets cleaned up; no pidfile at all means there was never a session.
        Ok(false) if pidfile_existed => {}
        Ok(false) => return fail(exitcode::NOHOST, format!("no session named {name:?}")),
        Err(e) => return fail(exitcode::OSERR, format!("couldn't check session {name:?}: {e}")),
    }
    let _ = std::fs::remove_file(&paths.sock);
    let _ = std::fs::remove_file(&paths.tty_sock);
    let _ = std::fs::remove_file(&paths.pidfile);
    ExitCode::SUCCESS
}

#[cfg(unix)]
fn send_sigterm(pid: i32) {
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
}

/// No graceful signal exists for a process without a console of ours, so terminate it outright;
/// ConPTY then reaps the wrapped program. `kill` removes the session files itself.
#[cfg(windows)]
fn send_sigterm(pid: i32) {
    const PROCESS_TERMINATE: u32 = 0x0001;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
        fn TerminateProcess(process: *mut std::ffi::c_void, exit_code: u32) -> i32;
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    }
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid as u32);
        if !handle.is_null() {
            TerminateProcess(handle, 1);
            CloseHandle(handle);
        }
    }
}
