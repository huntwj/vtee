//! Non-interactive clients: the `--script` batch runner and the `--connect` observer.

use std::io::{BufRead, Write};
use std::path::Path;
use std::process::ExitCode;

use crate::output::fail;

pub struct ScriptOpts<'a> {
    pub log_path: Option<&'a Path>,
    pub script_path: &'a Path,
    pub text_mode: bool,
    pub cols: u16,
    pub rows: u16,
    pub env: &'a [(String, String)],
}

/// Run a newline-delimited-JSON script of `Command`s against a non-interactive tap, writing one
/// result line per `snapshot`/`wait_for`. Always kills the child at the end so a script can't
/// leave a test harness hanging.
pub fn run_script(program: &str, rest: &[String], opts: ScriptOpts) -> ExitCode {
    let spawn_opts = vtee::SpawnOptions {
        cols: opts.cols,
        rows: opts.rows,
        log_path: opts.log_path,
        forward_input: false,
        forward_output: false,
        env: opts.env,
    };

    let tap = match vtee::Tap::spawn(program, rest, spawn_opts) {
        Ok(t) => t,
        Err(e) => return fail(exitcode::OSERR, format!("failed to spawn {program}: {e}")),
    };

    tap.watch_sigterm();

    let code = run_script_body(&tap, opts.script_path, opts.text_mode);

    tap.kill();
    let _ = tap.wait();
    code
}

fn run_script_body(tap: &vtee::Tap, script_path: &Path, text_mode: bool) -> ExitCode {
    let script = match std::fs::File::open(script_path) {
        Ok(f) => f,
        Err(e) => return fail(exitcode::NOINPUT, format!("failed to open script {}: {e}", script_path.display())),
    };

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for (lineno, line) in std::io::BufReader::new(script).lines().enumerate() {
        let lineno = lineno + 1;
        let line = match line {
            Ok(l) => l,
            Err(e) => return fail(exitcode::IOERR, format!("error reading script line {lineno}: {e}")),
        };
        if line.trim().is_empty() {
            continue;
        }
        let cmd: vtee::Command = match serde_json::from_str(&line) {
            Ok(c) => c,
            Err(e) => return fail(exitcode::DATAERR, format!("bad command on script line {lineno}: {e}")),
        };
        match cmd.execute(tap) {
            Ok(Some(result)) => {
                if text_mode {
                    print_text(&mut out, &result);
                } else {
                    match serde_json::to_string(&result) {
                        Ok(text) => {
                            let _ = writeln!(out, "{text}");
                        }
                        Err(e) => {
                            return fail(
                                exitcode::SOFTWARE,
                                format!("failed to serialize result on script line {lineno}: {e}"),
                            );
                        }
                    }
                }
                let _ = out.flush();
            }
            Ok(None) => {}
            Err(e) => {
                // An unknown key name is bad script data, not a software fault.
                let code =
                    if e.kind() == std::io::ErrorKind::InvalidInput { exitcode::DATAERR } else { exitcode::SOFTWARE };
                return fail(code, format!("command on script line {lineno} failed: {e}"));
            }
        }
    }
    ExitCode::SUCCESS
}

/// `--connect`: relay our stdin to a session's command socket line by line (the same NDJSON a
/// script file holds) and print the result lines that come back.
pub fn run_connect(path: &Path, text_mode: bool) -> ExitCode {
    use crate::output::print_error;

    let stream = match vtee::net::UnixStream::connect(path) {
        Ok(s) => s,
        Err(e) => return fail(exitcode::NOHOST, format!("failed to connect to {}: {e}", path.display())),
    };
    let mut writer = match stream.try_clone() {
        Ok(w) => w,
        Err(e) => return fail(exitcode::OSERR, e.to_string()),
    };
    let Ok(reader_side) = stream.try_clone() else {
        return fail(exitcode::OSERR, "failed to clone the connection for reading");
    };
    drop(stream);

    let reader_thread = std::thread::spawn(move || {
        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        for line in std::io::BufReader::new(reader_side).lines() {
            let Ok(line) = line else { break };
            if text_mode {
                match serde_json::from_str::<vtee::CommandResult>(&line) {
                    Ok(result) => print_text(&mut out, &result),
                    Err(e) => print_error(&format!("malformed response from server: {e}")),
                }
            } else {
                let _ = writeln!(out, "{line}");
            }
            let _ = out.flush();
        }
    });

    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if writeln!(writer, "{line}").is_err() {
            break;
        }
    }
    // `try_clone` duplicates the fd, so dropping `writer` would not send EOF while the reader's
    // duplicate is open; `shutdown` acts on the socket itself.
    let _ = writer.shutdown(std::net::Shutdown::Write);
    let _ = reader_thread.join();
    ExitCode::SUCCESS
}

/// `running`, `code=N` or `signal=...`, and `timed_out=...` for a `wait_exit` reply.
fn describe_exit(running: bool, code: Option<u32>, signal: Option<&str>, timed_out: Option<bool>) -> String {
    let mut parts = vec![if running {
        "running".to_string()
    } else if let Some(signal) = signal {
        format!("exited signal={signal}")
    } else {
        format!("exited code={}", code.unwrap_or(0))
    }];
    if let Some(timed_out) = timed_out {
        parts.push(format!("timed_out={timed_out}"));
    }
    parts.join(" ")
}

/// `--text`: print only a result's screen lines, for a human skimming output.
fn print_text(out: &mut impl Write, result: &vtee::CommandResult) {
    let (label, snapshot) = match result {
        vtee::CommandResult::Snapshot { snapshot } => ("snapshot".to_string(), snapshot),
        vtee::CommandResult::WaitFor { matched, snapshot } => (format!("wait_for matched={matched}"), snapshot),
        vtee::CommandResult::Error { message } => {
            let _ = writeln!(out, "--- error: {message} ---");
            return;
        }
        vtee::CommandResult::WriteAccess { acquired } => {
            let _ = writeln!(out, "--- write_access acquired={acquired} ---");
            return;
        }
        vtee::CommandResult::ExitStatus { running, code, signal } => {
            let _ = writeln!(out, "--- exit_status {} ---", describe_exit(*running, *code, signal.as_deref(), None));
            return;
        }
        vtee::CommandResult::WaitExit { running, code, signal, timed_out } => {
            let _ = writeln!(
                out,
                "--- wait_exit {} ---",
                describe_exit(*running, *code, signal.as_deref(), Some(*timed_out))
            );
            return;
        }
    };
    let _ = writeln!(out, "--- {label} (byte_offset={}) ---", snapshot.byte_offset);
    for line in &snapshot.lines {
        let _ = writeln!(out, "{}", line.trim_end());
    }
}
