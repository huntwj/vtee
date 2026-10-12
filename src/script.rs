//! Script files: the lines of one, the two ways to execute them (against a program started here, or against a live
//! session's socket), and the rules for a run (a `wait_for` that times out fails it unless the line says it may).

use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;

use serde_json::Value;

/// One command of a script, with the fields the runner (not the program or the session) reads.
#[derive(Debug, Clone)]
pub struct ScriptLine {
    /// 1-based line of the script file.
    pub number: usize,
    /// The text of the line without a byte order mark, which is what a session's socket is sent.
    pub raw: String,
    pub command: vtee::Command,
    /// The `name` of a `snapshot` command: a label the runner attaches to the screen.
    pub name: Option<String>,
    /// `"optional":true` on a `wait_for`: it may time out without failing the run.
    pub optional: bool,
}

/// Why a script could not be read to its end.
#[derive(Debug)]
pub enum ReadError {
    Io {
        line: usize,
        source: io::Error,
    },
    /// Not a command (invalid JSON, an unknown `cmd`, a field of the wrong type).
    Bad {
        line: usize,
        message: String,
    },
}

/// The lines of a script up to the first one that is not a command, and that problem if there is one. A plain
/// `--script` run executes the lines before the problem and then stops with it; a run with `--expect` refuses to start.
#[derive(Debug)]
pub struct Script {
    pub lines: Vec<ScriptLine>,
    pub error: Option<ReadError>,
}

/// Windows PowerShell 5.1 starts piped text with a UTF-8 byte order mark, which is not JSON, and sends it twice when
/// `$OutputEncoding` already carries one; drop every leading mark.
pub fn strip_bom(line: &str) -> &str {
    line.trim_start_matches('\u{feff}')
}

/// Reads a script file. Only failing to open it is an `Err`; a bad line ends the `Script` early.
pub fn read_script(path: &Path) -> io::Result<Script> {
    let file = std::fs::File::open(path)?;
    let mut lines = Vec::new();
    for (index, line) in BufReader::new(file).lines().enumerate() {
        let number = index + 1;
        let line = match line {
            Ok(l) => l,
            Err(source) => return Ok(Script { lines, error: Some(ReadError::Io { line: number, source }) }),
        };
        let text = strip_bom(&line);
        if text.trim().is_empty() {
            continue;
        }
        match parse_line(number, text) {
            Ok(parsed) => lines.push(parsed),
            Err(message) => return Ok(Script { lines, error: Some(ReadError::Bad { line: number, message }) }),
        }
    }
    Ok(Script { lines, error: None })
}

fn parse_line(number: usize, text: &str) -> Result<ScriptLine, String> {
    let command: vtee::Command = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let mut name = None;
    let mut optional = false;
    match &command {
        vtee::Command::Snapshot => match value.get("name") {
            None | Some(Value::Null) => {}
            Some(Value::String(s)) => name = Some(s.clone()),
            Some(_) => return Err("`name` must be a string".to_string()),
        },
        vtee::Command::WaitFor { .. } => match value.get("optional") {
            None | Some(Value::Null) => {}
            Some(Value::Bool(b)) => optional = *b,
            Some(_) => return Err("`optional` must be true or false".to_string()),
        },
        _ => {}
    }
    Ok(ScriptLine { number, raw: text.to_string(), command, name, optional })
}

/// A run that cannot go on: the exit code and the message, which the caller prints.
#[derive(Debug)]
pub struct Failure {
    pub code: exitcode::ExitCode,
    pub message: String,
}

/// What a command returned: the line a session prints for it, and that line parsed.
#[derive(Debug)]
pub struct Reply {
    pub line: String,
    pub value: Value,
}

impl Reply {
    fn cmd(&self) -> &str {
        self.value.get("cmd").and_then(Value::as_str).unwrap_or("")
    }
}

/// Executes the commands of a script somewhere.
pub trait Driver {
    /// Runs one command. `None` when the command has nothing to report (`send`, `wait_ms`, ...).
    fn execute(&mut self, line: &ScriptLine) -> Result<Option<Reply>, Failure>;
}

/// A program started by this process.
pub struct TapDriver<'a>(pub &'a vtee::Tap);

impl Driver for TapDriver<'_> {
    fn execute(&mut self, line: &ScriptLine) -> Result<Option<Reply>, Failure> {
        // `Command::execute`'s handling for these two is a no-op `Ok(None)` by design - over a
        // socket, the connection handler intercepts them first, because only it knows which
        // connection is asking (`src/command.rs`). A freshly-spawned program has no connections
        // to arbitrate between at all, so silently doing nothing here would misleadingly look
        // like success to a script author, rather than telling them the command means nothing
        // in this context.
        if matches!(line.command, vtee::Command::AcquireWrite | vtee::Command::ReleaseWrite) {
            return Err(Failure {
                code: exitcode::DATAERR,
                message: format!(
                    "script line {}: acquire_write/release_write only mean something against a live session \
                     (--connect), not a program this run spawned itself",
                    line.number
                ),
            });
        }
        let result = match line.command.execute(self.0) {
            Ok(Some(result)) => result,
            Ok(None) => return Ok(None),
            Err(e) => {
                // An unknown key name is bad script data, not a software fault.
                let code = if e.kind() == io::ErrorKind::InvalidInput { exitcode::DATAERR } else { exitcode::SOFTWARE };
                return Err(Failure { code, message: format!("command on script line {} failed: {e}", line.number) });
            }
        };
        let text = vtee::ndjson::to_line(&result).map_err(|e| Failure {
            code: exitcode::SOFTWARE,
            message: format!("failed to serialize result on script line {}: {e}", line.number),
        })?;
        let value = serde_json::from_str(&text).map_err(|e| Failure {
            code: exitcode::SOFTWARE,
            message: format!("failed to read back the result of script line {}: {e}", line.number),
        })?;
        Ok(Some(Reply { line: text, value }))
    }
}

/// A live session, over its command socket.
pub struct SocketDriver {
    writer: vtee::net::UnixStream,
    reader: BufReader<vtee::net::UnixStream>,
}

impl SocketDriver {
    pub fn connect(path: &Path) -> Result<SocketDriver, Failure> {
        let unreachable = |message: String| Failure { code: exitcode::UNAVAILABLE, message };
        let stream = vtee::net::UnixStream::connect(path)
            .map_err(|e| unreachable(format!("failed to connect to {}: {e}", path.display())))?;
        let reader = stream.try_clone().map_err(|e| unreachable(format!("failed to clone the connection: {e}")))?;
        Ok(SocketDriver { writer: stream, reader: BufReader::new(reader) })
    }

    fn send(&mut self, text: &str) -> Result<(), Failure> {
        writeln!(self.writer, "{text}").and_then(|()| self.writer.flush()).map_err(|e| Failure {
            code: exitcode::UNAVAILABLE,
            message: format!("lost the connection to the session: {e}"),
        })
    }

    fn read_reply(&mut self) -> Result<Reply, Failure> {
        let lost = |why: String| Failure {
            code: exitcode::UNAVAILABLE,
            message: format!("lost the connection to the session: {why}"),
        };
        let mut text = String::new();
        match self.reader.read_line(&mut text) {
            Ok(0) => Err(lost("the session closed it".to_string())),
            Err(e) => Err(lost(e.to_string())),
            Ok(_) => {
                let text = text.trim_end_matches(['\n', '\r']).to_string();
                let value = serde_json::from_str(&text).map_err(|e| Failure {
                    code: exitcode::DATAERR,
                    message: format!("malformed response from the session: {e}"),
                })?;
                Ok(Reply { line: text, value })
            }
        }
    }
}

/// True for the commands whose reply a session always sends.
fn always_replies(command: &vtee::Command) -> bool {
    use vtee::Command::*;
    matches!(command, Snapshot | WaitFor { .. } | ExitStatus | WaitExit { .. } | AcquireWrite | ReleaseWrite)
}

impl Driver for SocketDriver {
    fn execute(&mut self, line: &ScriptLine) -> Result<Option<Reply>, Failure> {
        self.send(&line.raw)?;
        let reply = if always_replies(&line.command) {
            Some(self.read_reply()?)
        } else {
            // `send`, `send_key`, `resize` and `wait_ms` are answered only when they are refused. An `exit_status`
            // after them is always answered, in order, so it marks where their answer would have been.
            self.send(r#"{"cmd":"exit_status"}"#)?;
            let mut refusal = None;
            loop {
                let reply = self.read_reply()?;
                if reply.cmd() == "exit_status" {
                    break;
                }
                refusal.get_or_insert(reply);
            }
            refusal
        };
        match reply {
            Some(reply) if reply.cmd() == "error" => {
                let message = reply.value.get("message").and_then(Value::as_str).unwrap_or("unknown error").to_string();
                let code = if is_bad_input(&message) { exitcode::DATAERR } else { exitcode::NOPERM };
                Err(Failure { code, message: format!("the session refused script line {}: {message}", line.number) })
            }
            other => Ok(other),
        }
    }
}

/// Whether an error message describes a malformed request (the line itself, or a value it
/// carried) rather than an actual permission/policy refusal - the exact, closed set of messages
/// `Command::execute`/`handle_line` can send that aren't one of `Denied`'s three variants
/// (`src/command.rs`). Everything else (`Denied`'s own messages: read-only, the wrong write
/// policy for acquire_write/release_write, a resize the real terminal owns) really is a refusal,
/// not bad data, and keeps the previous default of exitcode::NOPERM.
fn is_bad_input(message: &str) -> bool {
    message.starts_with("bad command:")
        || message.starts_with("unknown key ")
        || (message.starts_with("terminal size ") && (message.contains("exceeds the") || message.contains("below the")))
}

/// What a `wait_for` reply says went wrong, if a `wait_for` that may not time out did.
pub fn timed_out_wait<'a>(line: &'a ScriptLine, reply: &Reply) -> Option<(&'a str, u64)> {
    let vtee::Command::WaitFor { text, timeout_ms, .. } = &line.command else { return None };
    (reply.cmd() == "wait_for" && reply.value.get("matched") == Some(&Value::Bool(false)) && !line.optional)
        .then_some((text.as_str(), *timeout_ms))
}

/// The line a saved run keeps for a reply (design section 6.11, source (d)): the reply as the session printed it, plus
/// what the script line said that the reply does not: a `snapshot`'s `name`, and a `wait_for`'s line number, text,
/// timeout and `optional`. Saved this way, a run gives the same verdict offline as it did live.
pub fn saved_line(line: &ScriptLine, reply: &Reply) -> String {
    let mut value = reply.value.clone();
    if let Some(object) = value.as_object_mut() {
        match &line.command {
            vtee::Command::Snapshot => {
                if let Some(name) = &line.name {
                    object.insert("name".to_string(), Value::String(name.clone()));
                }
            }
            vtee::Command::WaitFor { text, timeout_ms, .. } => {
                object.insert("line".to_string(), Value::from(line.number));
                object.insert("text".to_string(), Value::String(text.clone()));
                object.insert("timeout_ms".to_string(), Value::from(*timeout_ms));
                if line.optional {
                    object.insert("optional".to_string(), Value::Bool(true));
                }
            }
            _ => {}
        }
    }
    vtee::ndjson::to_line(&value).expect("a JSON value serializes")
}

/// Whether to go on after a reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Stop,
}

/// Receives the replies of a run, in order.
pub trait Sink {
    fn reply(&mut self, line: &ScriptLine, reply: Reply) -> Flow;
}

/// Executes every line of the script through `driver`, giving each reply to `sink`, until the end or until the sink
/// says to stop. A command that fails ends the run with its [`Failure`].
pub fn drive(driver: &mut dyn Driver, lines: &[ScriptLine], sink: &mut dyn Sink) -> Result<(), Failure> {
    for line in lines {
        if let Some(reply) = driver.execute(line)?
            && sink.reply(line, reply) == Flow::Stop
        {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> Script {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "vtee-script-test-{}-{}.ndjson",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, text).unwrap();
        let script = read_script(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        script
    }

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
    fn a_script_reads_names_and_optional_waits() {
        let script = read(
            "{\"cmd\":\"snapshot\",\"name\":\"first\"}\n\n\u{feff}{\"cmd\":\"wait_for\",\"text\":\"x\",\"timeout_ms\":5,\"optional\":true}\n{\"cmd\":\"snapshot\"}\n",
        );
        assert!(script.error.is_none());
        let seen: Vec<_> = script.lines.iter().map(|l| (l.number, l.name.clone(), l.optional)).collect();
        assert_eq!(seen, [(1, Some("first".to_string()), false), (3, None, true), (4, None, false)]);
        assert!(!script.lines[1].raw.starts_with('\u{feff}'));
    }

    #[test]
    fn reading_stops_at_the_first_bad_line_and_keeps_the_ones_before_it() {
        let script = read("{\"cmd\":\"snapshot\"}\nnot json\n{\"cmd\":\"snapshot\"}\n");
        assert_eq!(script.lines.len(), 1);
        match script.error {
            Some(ReadError::Bad { line: 2, message }) => assert_eq!(message, "expected ident at line 1 column 2"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_name_or_optional_of_the_wrong_type_is_a_bad_line() {
        for text in [
            "{\"cmd\":\"snapshot\",\"name\":3}\n",
            "{\"cmd\":\"wait_for\",\"text\":\"x\",\"timeout_ms\":1,\"optional\":\"yes\"}\n",
        ] {
            assert!(matches!(read(text).error, Some(ReadError::Bad { line: 1, .. })), "{text}");
        }
    }

    #[test]
    fn a_saved_line_carries_what_the_script_line_said() {
        let wait = &read("{\"cmd\":\"wait_for\",\"text\":\"ok\",\"timeout_ms\":50,\"optional\":true}\n").lines[0];
        let reply = Reply { line: String::new(), value: serde_json::json!({"cmd": "wait_for", "matched": false}) };
        assert_eq!(
            saved_line(wait, &reply),
            r#"{"cmd":"wait_for","line":1,"matched":false,"optional":true,"text":"ok","timeout_ms":50}"#
        );
        assert_eq!(timed_out_wait(wait, &reply), None, "an optional wait may time out");
        let strict = &read("{\"cmd\":\"wait_for\",\"text\":\"ok\",\"timeout_ms\":50}\n").lines[0];
        assert_eq!(timed_out_wait(strict, &reply), Some(("ok", 50)));
    }
}
