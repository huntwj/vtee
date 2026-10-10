//! The output of a run, as the comparison sees it: the screens its snapshots produced, in order, and the problems the
//! run itself had (a `wait_for` that timed out). Source (d) of section 6.11 reads this from NDJSON text, which is what a
//! `vtee --script` run prints.

use std::fmt;

use serde_json::Value;

use super::model::Golden;
use super::version::Refusal;
use crate::snapshot::{SCHEMA_VERSION, Snapshot};

/// A `wait_for` of the script that timed out (design section 6.9, kind `wait`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunProblem {
    /// How many screens the run had produced when it happened, so `--fail-fast` knows what came before it.
    pub after_screens: usize,
    /// The line of the script (or, reading saved output, of the input) that waited.
    pub line: usize,
    /// The text it waited for, when the reply says.
    pub text: Option<String>,
    /// The time it waited, when the reply says.
    pub timeout_ms: Option<u64>,
}

/// What a run produced: the screens in order, their optional names, and the run-level problems.
#[derive(Debug, Clone, Default)]
pub struct Run {
    pub screens: Vec<Snapshot>,
    /// The `name` the script gave each `snapshot` command, paired with `screens` by position.
    pub names: Vec<Option<String>>,
    pub problems: Vec<RunProblem>,
}

/// Why the output of a run could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    /// Not valid snapshot output (exit 65). The message starts with `snapshot:`.
    Malformed(String),
    /// A snapshot carries other version numbers than the golden (exit 2).
    Refused(Refusal),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::Malformed(message) => f.write_str(message),
            RunError::Refused(refusal) => f.write_str(&refusal.reason()),
        }
    }
}

impl std::error::Error for RunError {}

fn malformed<T>(line: usize, message: impl fmt::Display) -> Result<T, RunError> {
    Err(RunError::Malformed(format!("snapshot: line {line}: {message}")))
}

/// Reads saved output: one JSON document per line, either a bare snapshot object or a reply line
/// `{"cmd":"snapshot","snapshot":{...}}`. A reply of another command is not a screen; a `wait_for` reply with
/// `"matched":false` is a [`RunProblem`] unless it carries `"optional":true`. A reply may also carry the `name` of the
/// script's `snapshot` command and, on a `wait_for`, the `line`, `text` and `timeout_ms` of the script line.
///
/// Each snapshot's version numbers are checked against `golden` as the snapshot is read, before its shape is looked
/// at: a difference is a refusal (section 6.8, step 3).
pub fn read_run(text: &str, golden: &Golden) -> Result<Run, RunError> {
    let mut run = Run::default();
    for (index, line) in text.split('\n').enumerate() {
        let number = index + 1;
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => return malformed(number, format!("not JSON: {e}")),
        };
        let Some(object) = value.as_object() else { return malformed(number, "not a JSON object") };
        let name = object.get("name").and_then(Value::as_str).map(str::to_string);
        match object.get("cmd").map(Value::as_str) {
            Some(Some("snapshot")) => {
                let Some(snapshot) = object.get("snapshot") else {
                    return malformed(number, "a snapshot reply has no `snapshot`");
                };
                run.screens.push(read_snapshot(number, snapshot, golden)?);
                run.names.push(name);
            }
            Some(Some("wait_for")) => {
                let Some(matched) = object.get("matched").and_then(Value::as_bool) else {
                    return malformed(number, "a wait_for reply has no `matched`");
                };
                let optional = object.get("optional").and_then(Value::as_bool).unwrap_or(false);
                if !matched && !optional {
                    run.problems.push(RunProblem {
                        after_screens: run.screens.len(),
                        line: object.get("line").and_then(Value::as_u64).map_or(number, |l| l as usize),
                        text: object.get("text").and_then(Value::as_str).map(str::to_string),
                        timeout_ms: object.get("timeout_ms").and_then(Value::as_u64),
                    });
                }
            }
            Some(Some(_)) => {}
            Some(None) => return malformed(number, "`cmd` is not a string"),
            None => {
                run.screens.push(read_snapshot(number, &value, golden)?);
                run.names.push(name);
            }
        }
    }
    Ok(run)
}

fn read_snapshot(line: usize, value: &Value, golden: &Golden) -> Result<Snapshot, RunError> {
    let Some(object) = value.as_object() else { return malformed(line, "a snapshot must be a JSON object") };
    let number = |key: &str| object.get(key).and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok());
    let (schema, semantics) = match (number("schema_version"), number("semantics_version")) {
        (Some(schema), Some(semantics)) => (schema, semantics),
        (Some(_), None) => {
            return malformed(
                line,
                "it has no `semantics_version`: snapshots recorded before 0.1.0 are not supported; record them again",
            );
        }
        _ => return malformed(line, "it has no `schema_version` and `semantics_version`, so it is not a snapshot"),
    };
    golden.check_snapshot(schema, semantics).map_err(RunError::Refused)?;
    if schema != SCHEMA_VERSION {
        // The golden agrees with the snapshots but this build cannot read their layout.
        return Err(RunError::Refused(Refusal::Schema { golden: schema, tool: SCHEMA_VERSION }));
    }
    let snapshot: Snapshot = match serde_json::from_value(value.clone()) {
        Ok(s) => s,
        Err(e) => return malformed(line, e),
    };
    let well_shaped = (1..=1000).contains(&snapshot.cols)
        && (1..=1000).contains(&snapshot.rows)
        && snapshot.cells.len() == snapshot.rows
        && snapshot.cells.iter().all(|row| row.len() == snapshot.cols);
    if !well_shaped {
        return malformed(line, "`cells` is not `rows` by `cols`");
    }
    Ok(snapshot)
}
