//! Non-interactive clients: the `--script` batch runner and the `--connect` observer.

use std::io::{BufRead, Write};
use std::path::Path;
use std::process::ExitCode;

use crate::output::{exit_code, fail};

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
        let line = strip_bom(&line);
        if line.trim().is_empty() {
            continue;
        }
        let cmd: vtee::Command = match serde_json::from_str(line) {
            Ok(c) => c,
            Err(e) => return fail(exitcode::DATAERR, format!("bad command on script line {lineno}: {e}")),
        };
        match cmd.execute(tap) {
            Ok(Some(result)) => {
                if text_mode {
                    print_text(&mut out, &result);
                } else {
                    match vtee::ndjson::to_line(&result) {
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

    // Returns whether the server rejected any input line as unparseable, so a garbled script
    // (for example a stray byte order mark) is not reported as success.
    let reader_thread = std::thread::spawn(move || {
        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        let mut saw_bad_command = false;
        for line in std::io::BufReader::new(reader_side).lines() {
            let Ok(line) = line else { break };
            let parsed = serde_json::from_str::<vtee::CommandResult>(&line);
            if let Ok(vtee::CommandResult::Error { message }) = &parsed
                && is_bad_command(message)
            {
                saw_bad_command = true;
            }
            if text_mode {
                match parsed {
                    Ok(result) => print_text(&mut out, &result),
                    Err(e) => print_error(&format!("malformed response from server: {e}")),
                }
            } else {
                let _ = writeln!(out, "{line}");
            }
            let _ = out.flush();
        }
        saw_bad_command
    });

    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if writeln!(writer, "{}", strip_bom(&line)).is_err() {
            break;
        }
    }
    // `try_clone` duplicates the fd, so dropping `writer` would not send EOF while the reader's
    // duplicate is open; `shutdown` acts on the socket itself.
    let _ = writer.shutdown(std::net::Shutdown::Write);
    if reader_thread.join().unwrap_or(false) {
        return exit_code(exitcode::DATAERR);
    }
    ExitCode::SUCCESS
}

/// The server's reply to a line that is not a valid command (`sockets.rs`, `handle_line`).
fn is_bad_command(message: &str) -> bool {
    message.starts_with("bad command:")
}

/// `replay`: feed an asciicast v2 recording (written by `--log`) into a fresh emulator and print the
/// final screen, as text or, with `json`, as the snapshot JSON.
pub fn run_replay(path: &Path, json: bool) -> ExitCode {
    let first_line = match std::fs::File::open(path) {
        Ok(f) => std::io::BufReader::new(f).lines().next(),
        Err(e) => return fail(exitcode::NOINPUT, format!("failed to open {}: {e}", path.display())),
    };
    let header: Option<serde_json::Value> = first_line.and_then(Result::ok).and_then(|l| serde_json::from_str(&l).ok());
    let dimension = |key: &str| {
        header
            .as_ref()
            .and_then(|h| h.get(key))
            .and_then(serde_json::Value::as_u64)
            .filter(|n| (1..=u64::from(vtee::MAX_TERMINAL_DIMENSION)).contains(n))
            .map(|n| n as u16)
    };
    let (Some(cols), Some(rows)) = (dimension("width"), dimension("height")) else {
        return fail(
            exitcode::DATAERR,
            format!(
                "{} is not an asciicast v2 recording: its first line has no usable width and height",
                path.display()
            ),
        );
    };
    let snapshot = match vtee::replay(path, cols, rows) {
        Ok(s) => s,
        Err(e) => return fail(exitcode::DATAERR, format!("failed to replay {}: {e}", path.display())),
    };
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    if json {
        match vtee::ndjson::to_line(&snapshot) {
            Ok(text) => {
                let _ = writeln!(out, "{text}");
            }
            Err(e) => return fail(exitcode::SOFTWARE, format!("failed to serialize the snapshot: {e}")),
        }
    } else {
        print_text(&mut out, &vtee::CommandResult::Snapshot { snapshot });
    }
    let _ = out.flush();
    ExitCode::SUCCESS
}

/// Windows PowerShell 5.1 starts piped text with a UTF-8 byte order mark, which is not JSON, and
/// sends it twice when `$OutputEncoding` already carries one; drop every leading mark.
fn strip_bom(line: &str) -> &str {
    line.trim_start_matches('\u{feff}')
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

#[cfg(test)]
mod tests {
    use super::strip_bom;

    #[test]
    fn a_leading_byte_order_mark_is_removed() {
        assert_eq!(strip_bom("\u{feff}{\"cmd\":\"snapshot\"}"), "{\"cmd\":\"snapshot\"}");
        assert_eq!(strip_bom("{\"cmd\":\"snapshot\"}"), "{\"cmd\":\"snapshot\"}");
        assert_eq!(strip_bom(""), "");
    }

    #[test]
    fn a_doubled_byte_order_mark_is_removed_too() {
        assert_eq!(strip_bom("\u{feff}\u{feff}{\"cmd\":\"snapshot\"}"), "{\"cmd\":\"snapshot\"}");
        assert_eq!(strip_bom("{\"a\":\"\u{feff}\"}"), "{\"a\":\"\u{feff}\"}");
    }

    #[test]
    fn only_unparseable_input_counts_as_a_bad_command() {
        assert!(super::is_bad_command("bad command: expected value at line 1 column 1"));
        assert!(!super::is_bad_command("write commands aren't allowed on this connection"));
    }
}
