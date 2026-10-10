//! Non-interactive clients: the `--script` batch runner and the `--connect` observer.

use std::io::{BufRead, Write};
use std::path::Path;
use std::process::ExitCode;

use crate::output::{exit_code, fail, print_error};
use crate::script::{
    self, Driver, Flow, ReadError, Reply, Script, ScriptLine, Sink, SocketDriver, TapDriver, read_script, strip_bom,
};

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
/// leave a test harness hanging. A `wait_for` that times out (unless the line says `"optional":true`) makes the
/// run exit 1; the script still runs to its end.
pub fn run_script(program: &str, rest: &[String], opts: ScriptOpts) -> ExitCode {
    let script = match read_script(opts.script_path) {
        Ok(script) => script,
        Err(e) => return fail(exitcode::NOINPUT, format!("failed to open script {}: {e}", opts.script_path.display())),
    };

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

    let code = print_script(&mut TapDriver(&tap), &script, opts.text_mode);

    tap.kill();
    let _ = tap.wait();
    code
}

/// `--connect` with `--script`: the same script, run against a live session's command socket. The session is left
/// running.
pub fn run_connect_script(socket: &Path, script_path: &Path, text_mode: bool) -> ExitCode {
    let script = match read_script(script_path) {
        Ok(script) => script,
        Err(e) => return fail(exitcode::NOINPUT, format!("failed to open script {}: {e}", script_path.display())),
    };
    let mut driver = match SocketDriver::connect(socket) {
        Ok(driver) => driver,
        Err(failure) => return fail(failure.code, failure.message),
    };
    print_script(&mut driver, &script, text_mode)
}

/// Executes a script and prints each result as `--script` always has: a line of JSON, or the screen rows.
fn print_script(driver: &mut dyn Driver, script: &Script, text_mode: bool) -> ExitCode {
    let stdout = std::io::stdout();
    let mut sink = PrintSink { out: stdout.lock(), text_mode, timed_out: false };
    if let Err(failure) = script::drive(driver, &script.lines, &mut sink) {
        return fail(failure.code, failure.message);
    }
    match &script.error {
        Some(ReadError::Bad { line, message }) => {
            return fail(exitcode::DATAERR, format!("bad command on script line {line}: {message}"));
        }
        Some(ReadError::Io { line, source }) => {
            return fail(exitcode::IOERR, format!("error reading script line {line}: {source}"));
        }
        None => {}
    }
    if sink.timed_out { exit_code(1) } else { ExitCode::SUCCESS }
}

struct PrintSink<W: Write> {
    out: W,
    text_mode: bool,
    timed_out: bool,
}

impl<W: Write> Sink for PrintSink<W> {
    fn reply(&mut self, line: &ScriptLine, reply: Reply) -> Flow {
        if let Some((text, timeout_ms)) = script::timed_out_wait(line, &reply) {
            self.timed_out = true;
            // After the reply, which is printed first as it always was; the message goes to standard error.
            let message = format!(
                "script line {}: wait_for {} timed out after {timeout_ms} ms",
                line.number,
                vtee::golden::escape::encode_string(text)
            );
            self.print(&reply);
            print_error(&message);
        } else {
            self.print(&reply);
        }
        Flow::Continue
    }
}

impl<W: Write> PrintSink<W> {
    fn print(&mut self, reply: &Reply) {
        if self.text_mode {
            match serde_json::from_value::<vtee::CommandResult>(reply.value.clone()) {
                Ok(result) => print_text(&mut self.out, &result),
                Err(e) => print_error(&format!("malformed response from server: {e}")),
            }
        } else {
            let _ = writeln!(self.out, "{}", reply.line);
        }
        let _ = self.out.flush();
    }
}

/// `--connect`: relay our stdin to a session's command socket line by line (the same NDJSON a
/// script file holds) and print the result lines that come back.
pub fn run_connect(path: &Path, text_mode: bool) -> ExitCode {
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
    #[test]
    fn only_unparseable_input_counts_as_a_bad_command() {
        assert!(super::is_bad_command("bad command: expected value at line 1 column 1"));
        assert!(!super::is_bad_command("write commands aren't allowed on this connection"));
    }
}
