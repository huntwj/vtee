//! Checking a run against a golden and reporting the verdict, as text and as JSON (design sections 6.8 to 6.10).

use std::fmt::Write as _;

use super::compare::{
    AssertProblem, CompareOptions, Comparison, CursorField, CursorValue, MaskProblem, Mismatch, ScreenStatus, TextDiff,
    UsageError, compare,
};
use super::escape::{encode_string, visible};
use super::model::{Golden, Semantics};
use super::run::{Run, RunError, read_run};
use super::version::{Build, Invocation, Refusal, WayForward};

/// How many mismatch blocks a report shows unless asked otherwise.
pub const DEFAULT_MAX_MISMATCHES: usize = 20;

/// Options of a check beyond how screens are compared.
#[derive(Debug, Clone, Copy)]
pub struct CheckOptions {
    pub compare: CompareOptions,
    /// At most this many mismatch blocks are shown (`--max-mismatches`); the report still counts them all.
    pub max_mismatches: usize,
}

impl Default for CheckOptions {
    fn default() -> Self {
        CheckOptions { compare: CompareOptions::default(), max_mismatches: DEFAULT_MAX_MISMATCHES }
    }
}

/// Why a check could not produce a verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    /// The saved output is not valid (exit 65).
    Run(String),
    /// The options do not fit the run (exit 64).
    Usage(UsageError),
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckError::Run(message) => f.write_str(message),
            CheckError::Usage(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for CheckError {}

/// The result of a check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Fail,
    Refused,
}

/// A verdict with everything needed to print it.
#[derive(Debug, Clone)]
pub struct Report {
    /// The final component of the golden's path.
    pub name: String,
    pub verdict: Verdict,
    pub grammar: u32,
    pub schema: u32,
    pub semantics: Semantics,
    /// The numbers the run's snapshots carry (the build's own when no snapshot was read).
    pub snapshot_schema: u32,
    pub snapshot_semantics: u32,
    pub screens_expected: usize,
    pub screens_actual: usize,
    pub refusal: Option<Refusal>,
    pub way_forward: Vec<WayForward>,
    pub comparison: Option<Comparison>,
    pub max_mismatches: usize,
}

/// The name a report uses for a golden: the last component of its path, with its extension.
pub fn display_name(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

fn refused(
    golden: &Golden,
    invocation: &Invocation,
    refusal: Refusal,
    snapshot: (u32, u32),
    options: &CheckOptions,
) -> Report {
    Report {
        name: display_name(&invocation.golden),
        verdict: Verdict::Refused,
        grammar: golden.grammar,
        schema: golden.schema,
        semantics: golden.semantics,
        snapshot_schema: snapshot.0,
        snapshot_semantics: snapshot.1,
        screens_expected: golden.screens.len(),
        screens_actual: 0,
        way_forward: refusal.way_forward(invocation),
        refusal: Some(refusal),
        comparison: None,
        max_mismatches: options.max_mismatches,
    }
}

/// Checks a run you already have (for instance one that just ran a script): refuses when the golden's numbers differ
/// from this build's, otherwise compares.
pub fn check_run(
    golden: &Golden,
    invocation: &Invocation,
    run: &Run,
    options: &CheckOptions,
) -> Result<Report, UsageError> {
    let build = Build::current();
    if let Err(refusal) = golden.check_build(build) {
        return Ok(refused(golden, invocation, refusal, (build.schema, build.semantics), options));
    }
    let comparison = compare(golden, run, &options.compare)?;
    let snapshot =
        run.screens.first().map_or((build.schema, build.semantics), |s| (s.schema_version, s.semantics_version));
    Ok(finish(golden, invocation, comparison, snapshot, options))
}

/// Checks saved output (source (d) of section 6.11): refuses when the golden's numbers differ from this build's or the
/// snapshots' numbers differ from the golden's, otherwise compares. The golden's own numbers are decided first, from
/// its header, without looking at the output.
pub fn check_saved(
    golden: &Golden,
    invocation: &Invocation,
    saved: &str,
    options: &CheckOptions,
) -> Result<Report, CheckError> {
    let build = Build::current();
    if let Err(refusal) = golden.check_build(build) {
        return Ok(refused(golden, invocation, refusal, (build.schema, build.semantics), options));
    }
    let run = match read_run(saved, golden) {
        Ok(run) => run,
        Err(RunError::Malformed(message)) => return Err(CheckError::Run(message)),
        Err(RunError::Refused(refusal)) => {
            let snapshot = match &refusal {
                Refusal::SnapshotSchema { snapshot, .. } => (*snapshot, build.semantics),
                Refusal::SnapshotSemantics { snapshot, .. } => (build.schema, *snapshot),
                _ => (build.schema, build.semantics),
            };
            return Ok(refused(golden, invocation, refusal, snapshot, options));
        }
    };
    let comparison = compare(golden, &run, &options.compare).map_err(CheckError::Usage)?;
    let snapshot =
        run.screens.first().map_or((build.schema, build.semantics), |s| (s.schema_version, s.semantics_version));
    Ok(finish(golden, invocation, comparison, snapshot, options))
}

fn finish(
    golden: &Golden,
    invocation: &Invocation,
    comparison: Comparison,
    snapshot: (u32, u32),
    options: &CheckOptions,
) -> Report {
    Report {
        name: display_name(&invocation.golden),
        verdict: if comparison.passed() { Verdict::Pass } else { Verdict::Fail },
        grammar: golden.grammar,
        schema: golden.schema,
        semantics: golden.semantics,
        snapshot_schema: snapshot.0,
        snapshot_semantics: snapshot.1,
        screens_expected: comparison.screens_expected,
        screens_actual: comparison.screens_actual,
        refusal: None,
        way_forward: Vec::new(),
        comparison: Some(comparison),
        max_mismatches: options.max_mismatches,
    }
}

impl Report {
    /// The exit code of the run (section 6.10): 0 pass, 1 differs, 2 refused.
    pub fn exit_code(&self) -> i32 {
        match self.verdict {
            Verdict::Pass => 0,
            Verdict::Fail => 1,
            Verdict::Refused => 2,
        }
    }

    /// How many mismatch blocks the report shows, and how many there are.
    fn shown_and_total(&self) -> (usize, usize) {
        let total = self.comparison.as_ref().map_or(0, Comparison::total);
        (total.min(self.max_mismatches), total)
    }

    // ---- text ----------------------------------------------------------------------------------------------------

    /// The text report: one line per fact, ending in a newline.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        match (self.verdict, &self.refusal, &self.comparison) {
            (Verdict::Refused, Some(refusal), _) => {
                let _ = writeln!(out, "REFUSED {}: {}", self.name, refusal.reason());
                for step in &self.way_forward {
                    let _ = writeln!(out, "  {}", step.text);
                }
            }
            (_, _, Some(comparison)) => {
                let k = self.screens_expected;
                let screens = |n: usize| if n == 1 { "1 screen".to_string() } else { format!("{n} screens") };
                if self.verdict == Verdict::Pass {
                    let _ = writeln!(out, "PASS {} ({})", self.name, screens(k));
                    return out;
                }
                let (_, total) = self.shown_and_total();
                let failing = comparison.screens.iter().filter(|s| s.status == ScreenStatus::Fail).count();
                let _ = writeln!(
                    out,
                    "FAIL {}: {} in {} of {}",
                    self.name,
                    if total == 1 { "1 mismatch".to_string() } else { format!("{total} mismatches") },
                    failing,
                    screens(k)
                );
                let mut budget = self.max_mismatches;
                for mismatch in &comparison.run {
                    if budget == 0 {
                        break;
                    }
                    budget -= 1;
                    write_block(&mut out, mismatch);
                }
                for screen in &comparison.screens {
                    if screen.mismatches.is_empty() || budget == 0 {
                        continue;
                    }
                    match &screen.name {
                        Some(name) => {
                            let _ = writeln!(out, "screen {} {}:", screen.index, encode_string(name));
                        }
                        None => {
                            let _ = writeln!(out, "screen {}:", screen.index);
                        }
                    }
                    for mismatch in screen.mismatches.iter().take(budget) {
                        write_block(&mut out, mismatch);
                    }
                    budget = budget.saturating_sub(screen.mismatches.len());
                }
                if total > self.max_mismatches {
                    let _ = writeln!(out, "showing {} of {total}", self.max_mismatches);
                }
            }
            _ => unreachable!("a report is either refused or has a comparison"),
        }
        out
    }

    // ---- JSON ----------------------------------------------------------------------------------------------------

    /// The JSON report: one object on one line, without the final newline.
    pub fn to_json(&self) -> String {
        let (shown, total) = self.shown_and_total();
        let mut top = vec![
            ("golden", Json::str(&self.name)),
            (
                "result",
                Json::str(match self.verdict {
                    Verdict::Pass => "pass",
                    Verdict::Fail => "fail",
                    Verdict::Refused => "refused",
                }),
            ),
        ];
        if let Some(refusal) = &self.refusal {
            top.push(("reason", Json::Str(refusal.reason())));
            let commands =
                self.way_forward.iter().flat_map(|w| w.commands.iter()).map(|c| Json::Str(c.clone())).collect();
            top.push(("way_forward", Json::Arr(commands)));
        }
        top.push(("grammar", Json::Int(i64::from(self.grammar))));
        top.push(("schema", Json::Int(i64::from(self.schema))));
        top.push((
            "semantics",
            match self.semantics {
                Semantics::Number(n) => Json::Int(i64::from(n)),
                Semantics::Unknown => Json::str("unknown"),
            },
        ));
        top.push((
            "snapshot",
            Json::Obj(vec![
                ("schema", Json::Int(i64::from(self.snapshot_schema))),
                ("semantics", Json::Int(i64::from(self.snapshot_semantics))),
            ]),
        ));
        top.push(("screens_expected", Json::int(self.screens_expected)));
        top.push(("screens_actual", Json::int(self.screens_actual)));
        top.push(("total", Json::int(total)));
        top.push(("shown", Json::int(shown)));
        let mut budget = shown;
        let mut run = Vec::new();
        let mut screens = Vec::new();
        if let Some(comparison) = &self.comparison {
            for mismatch in &comparison.run {
                if budget == 0 {
                    break;
                }
                budget -= 1;
                run.push(mismatch_json(mismatch));
            }
            for screen in &comparison.screens {
                let mut entry = vec![
                    ("index", Json::int(screen.index)),
                    ("name", screen.name.as_deref().map_or(Json::Null, Json::str)),
                    (
                        "result",
                        Json::str(match screen.status {
                            ScreenStatus::Pass => "pass",
                            ScreenStatus::Fail => "fail",
                            ScreenStatus::Skipped => "skipped",
                        }),
                    ),
                ];
                if screen.status == ScreenStatus::Fail {
                    let take = budget.min(screen.mismatches.len());
                    budget -= take;
                    entry.push((
                        "mismatches",
                        Json::Arr(screen.mismatches.iter().take(take).map(mismatch_json).collect()),
                    ));
                }
                screens.push(Json::Obj(entry));
            }
        }
        top.push(("run", Json::Arr(run)));
        top.push(("screens", Json::Arr(screens)));
        let mut out = String::new();
        Json::Obj(top).write(&mut out);
        out
    }
}

// ---- text blocks -------------------------------------------------------------------------------------------------

fn list<T: ToString>(items: &[T]) -> String {
    if items.is_empty() { "(none)".to_string() } else { items.iter().map(T::to_string).collect::<Vec<_>>().join(" ") }
}

fn columns(first: usize, last: usize) -> String {
    if first == last { format!("col {first}") } else { format!("col {first}-{last}") }
}

fn quoted(text: &str) -> String {
    format!("'{}'", visible(text))
}

fn title(t: &Option<String>) -> String {
    t.as_deref().map_or("null".to_string(), encode_string)
}

fn cursor_value(value: &CursorValue) -> String {
    match value {
        CursorValue::Position { row, col } => format!("row={row} col={col}"),
        CursorValue::Shape(s) => s.clone(),
        CursorValue::Hidden(h) => h.to_string(),
    }
}

fn write_block(out: &mut String, mismatch: &Mismatch) {
    let pad = |kind: &str| format!("  {kind:<10} ");
    match mismatch {
        Mismatch::Screens { expected, actual, missing, extra, hint_screen } => {
            let _ = write!(out, "{}expected {expected}   actual {actual}", pad("screens"));
            if !missing.is_empty() {
                let _ = write!(out, "   missing {}", list(missing));
            }
            if !extra.is_empty() {
                let _ = write!(out, "   extra {}", list(extra));
            }
            if *hint_screen {
                out.push_str("   (use --screen N to compare one screen)");
            }
            out.push('\n');
        }
        Mismatch::Wait { line, text, timeout_ms } => {
            let what = match (text, timeout_ms) {
                (Some(t), Some(ms)) => format!("wait_for {} timed out after {ms} ms", encode_string(t)),
                (Some(t), None) => format!("wait_for {} timed out", encode_string(t)),
                (None, Some(ms)) => format!("wait_for timed out after {ms} ms"),
                (None, None) => "wait_for timed out".to_string(),
            };
            let _ = writeln!(out, "{}script line {line}   {what}", pad("wait"));
        }
        Mismatch::Name { expected, actual } => {
            let _ =
                writeln!(out, "{}expected {}   actual {}", pad("name"), encode_string(expected), encode_string(actual));
        }
        Mismatch::Size { expected, actual } => {
            let _ = writeln!(
                out,
                "{}expected {}x{}   actual {}x{}   (cells, masks and assertions not compared)",
                pad("size"),
                expected.0,
                expected.1,
                actual.0,
                actual.1
            );
        }
        Mismatch::Text(diff) => write_text_block(out, diff, &pad("text")),
        Mismatch::Attrs { row, col_start, col_end, expected, actual } => {
            let cells = col_end - col_start + 1;
            let _ = writeln!(
                out,
                "{}row {row} {} ({cells} cell{})   expected {}   actual {}",
                pad("attrs"),
                columns(*col_start, *col_end),
                if cells == 1 { "" } else { "s" },
                list(expected),
                list(actual)
            );
        }
        Mismatch::Cursor { field, expected, actual } => {
            let name = match field {
                CursorField::Position => "position",
                CursorField::Shape => "shape",
                CursorField::Hidden => "hidden",
            };
            let _ = writeln!(
                out,
                "{}{name:<8}   expected {}   actual {}",
                pad("cursor"),
                cursor_value(expected),
                cursor_value(actual)
            );
        }
        Mismatch::Modes { expected, actual, missing, extra } => {
            let _ = write!(out, "{}expected {}   actual {}", pad("modes"), list(expected), list(actual));
            if !missing.is_empty() {
                let _ = write!(out, "   missing {}", list(missing));
            }
            if !extra.is_empty() {
                let _ = write!(out, "   extra {}", list(extra));
            }
            out.push('\n');
        }
        Mismatch::AltScreen { expected, actual } => {
            let _ = writeln!(out, "{}expected {expected}   actual {actual}", pad("alt_screen"));
        }
        Mismatch::Title { expected, actual } => {
            let _ = writeln!(out, "{}expected {}   actual {}", pad("title"), title(expected), title(actual));
        }
        Mismatch::Wrapped { expected, actual, missing, extra } => {
            let _ = writeln!(
                out,
                "{}expected {}   actual {}   missing {}   extra {}",
                pad("wrapped"),
                list(expected),
                list(actual),
                list(missing),
                list(extra)
            );
        }
        Mismatch::Mask { line, mask, problem } => {
            let what = match problem {
                MaskProblem::Stale => "matched nothing (stale)".to_string(),
                MaskProblem::Text { row, col_start, col_end, actual } => {
                    format!("row {row} {}   actual {} does not match", columns(*col_start, *col_end), quoted(actual))
                }
            };
            let _ = writeln!(out, "{}golden line {line}   {mask}   {what}", pad("mask"));
        }
        Mismatch::Assert { line, assert, problem } => {
            let what = match problem {
                AssertProblem::NoMatch => "matched nothing".to_string(),
                AssertProblem::Text { row, col, expected, actual, failing } => {
                    format!(
                        "row {row} col {col}   expected {}   actual {}   ({failing} failing)",
                        visible(expected),
                        quoted(actual)
                    )
                }
                AssertProblem::Attr { row, col, actual, failing, .. } => {
                    format!("row {row} col {col}   actual {actual}   (first failing cell of {failing})")
                }
            };
            let _ = writeln!(out, "{}golden line {line}   {assert}   {what}", pad("assert"));
        }
    }
}

fn write_text_block(out: &mut String, diff: &TextDiff, lead: &str) {
    let _ = writeln!(
        out,
        "{lead}row {} {}   expected {}   actual {}",
        diff.row,
        columns(diff.col_start, diff.col_end),
        quoted(&diff.expected),
        quoted(&diff.actual)
    );
    let indent = " ".repeat(lead.len());
    let label = format!("{}| ", diff.row);
    let _ = writeln!(out, "{indent}{label}{}   (expected)", diff.expected_tokens);
    let _ = writeln!(out, "{indent}{label}{}   (actual)", diff.actual_tokens);
    let _ =
        writeln!(out, "{indent}{}{}", " ".repeat(label.chars().count() + diff.caret_start), "^".repeat(diff.caret_len));
}

// ---- JSON --------------------------------------------------------------------------------------------------------

/// A JSON value whose objects keep the order they were built in, so the report reads the way the design shows it.
enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(&'static str, Json)>),
}

impl Json {
    fn str(s: &str) -> Json {
        Json::Str(s.to_string())
    }

    fn int(n: usize) -> Json {
        Json::Int(i64::try_from(n).unwrap_or(i64::MAX))
    }

    fn opt_str(s: &Option<String>) -> Json {
        s.as_deref().map_or(Json::Null, Json::str)
    }

    fn ints(items: &[usize]) -> Json {
        Json::Arr(items.iter().map(|n| Json::int(*n)).collect())
    }

    fn strs(items: &[String]) -> Json {
        Json::Arr(items.iter().map(|s| Json::str(s)).collect())
    }

    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Int(n) => {
                let _ = write!(out, "{n}");
            }
            Json::Str(s) => out.push_str(&crate::ndjson::to_line(s).expect("a string always serialises")),
            Json::Arr(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Json::Obj(fields) => {
                out.push('{');
                for (i, (key, value)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    let _ = write!(out, "\"{key}\":");
                    value.write(out);
                }
                out.push('}');
            }
        }
    }
}

fn cursor_json(value: &CursorValue) -> Json {
    match value {
        CursorValue::Position { row, col } => Json::Obj(vec![("row", Json::int(*row)), ("col", Json::int(*col))]),
        CursorValue::Shape(s) => Json::str(s),
        CursorValue::Hidden(h) => Json::Bool(*h),
    }
}

fn mismatch_json(mismatch: &Mismatch) -> Json {
    let kind = |k: &str| ("kind", Json::str(k));
    Json::Obj(match mismatch {
        Mismatch::Screens { expected, actual, missing, extra, .. } => vec![
            kind("screens"),
            ("expected", Json::int(*expected)),
            ("actual", Json::int(*actual)),
            ("missing", Json::ints(missing)),
            ("extra", Json::ints(extra)),
        ],
        Mismatch::Wait { line, text, timeout_ms } => vec![
            kind("wait"),
            ("line", Json::int(*line)),
            ("text", Json::opt_str(text)),
            ("timeout_ms", timeout_ms.map_or(Json::Null, |ms| Json::Int(i64::try_from(ms).unwrap_or(i64::MAX)))),
        ],
        Mismatch::Name { expected, actual } => {
            vec![kind("name"), ("expected", Json::str(expected)), ("actual", Json::str(actual))]
        }
        Mismatch::Size { expected, actual } => {
            let size = |s: &(usize, usize)| Json::Obj(vec![("cols", Json::int(s.0)), ("rows", Json::int(s.1))]);
            vec![kind("size"), ("expected", size(expected)), ("actual", size(actual))]
        }
        Mismatch::Text(d) => vec![
            kind("text"),
            ("row", Json::int(d.row)),
            ("col_start", Json::int(d.col_start)),
            ("col_end", Json::int(d.col_end)),
            ("expected", Json::str(&d.expected)),
            ("actual", Json::str(&d.actual)),
            ("expected_row", Json::str(&d.expected_row)),
            ("actual_row", Json::str(&d.actual_row)),
        ],
        Mismatch::Attrs { row, col_start, col_end, expected, actual } => vec![
            kind("attrs"),
            ("row", Json::int(*row)),
            ("col_start", Json::int(*col_start)),
            ("col_end", Json::int(*col_end)),
            ("cells", Json::int(col_end - col_start + 1)),
            ("expected", Json::strs(expected)),
            ("actual", Json::strs(actual)),
        ],
        Mismatch::Cursor { field, expected, actual } => vec![
            kind("cursor"),
            (
                "field",
                Json::str(match field {
                    CursorField::Position => "position",
                    CursorField::Shape => "shape",
                    CursorField::Hidden => "hidden",
                }),
            ),
            ("expected", cursor_json(expected)),
            ("actual", cursor_json(actual)),
        ],
        Mismatch::Modes { expected, actual, missing, extra } => vec![
            kind("modes"),
            ("expected", Json::strs(expected)),
            ("actual", Json::strs(actual)),
            ("missing", Json::strs(missing)),
            ("extra", Json::strs(extra)),
        ],
        Mismatch::AltScreen { expected, actual } => {
            vec![kind("alt_screen"), ("expected", Json::Bool(*expected)), ("actual", Json::Bool(*actual))]
        }
        Mismatch::Title { expected, actual } => {
            vec![kind("title"), ("expected", Json::opt_str(expected)), ("actual", Json::opt_str(actual))]
        }
        Mismatch::Wrapped { expected, actual, missing, extra } => vec![
            kind("wrapped"),
            ("expected", Json::ints(expected)),
            ("actual", Json::ints(actual)),
            ("missing", Json::ints(missing)),
            ("extra", Json::ints(extra)),
        ],
        Mismatch::Mask { line, mask, problem } => {
            let mut fields = vec![kind("mask"), ("line", Json::int(*line)), ("mask", Json::str(mask))];
            match problem {
                MaskProblem::Stale => fields.push(("problem", Json::str("stale"))),
                MaskProblem::Text { row, col_start, col_end, actual } => {
                    fields.push(("problem", Json::str("text")));
                    fields.push(("row", Json::int(*row)));
                    fields.push(("col_start", Json::int(*col_start)));
                    fields.push(("col_end", Json::int(*col_end)));
                    fields.push(("actual", Json::str(actual)));
                }
            }
            fields
        }
        Mismatch::Assert { line, assert, problem } => {
            let mut fields = vec![kind("assert"), ("line", Json::int(*line)), ("assert", Json::str(assert))];
            match problem {
                AssertProblem::NoMatch => fields.push(("problem", Json::str("no_match"))),
                AssertProblem::Text { row, col, expected, actual, failing } => {
                    fields.push(("problem", Json::str("text")));
                    fields.push(("row", Json::int(*row)));
                    fields.push(("col", Json::int(*col)));
                    fields.push(("expected", Json::str(expected)));
                    fields.push(("actual", Json::str(actual)));
                    fields.push(("failing_cells", Json::int(*failing)));
                }
                AssertProblem::Attr { row, col, expected, actual, failing } => {
                    fields.push(("problem", Json::str("attr")));
                    fields.push(("row", Json::int(*row)));
                    fields.push(("col", Json::int(*col)));
                    fields.push(("expected", Json::str(expected)));
                    fields.push(("actual", Json::str(actual)));
                    fields.push(("failing_cells", Json::int(*failing)));
                }
            }
            fields
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const A2: &str = "vtee-golden 1
schema 1
semantics 1

[screen 1]
size 6x2
cursor row=0 col=0 shape=block hidden=false
modes
alt_screen false
title null

[text]
0|ab 12
1|

[attrs]
0: 3-4 bold

[masks]
mask row=0 cols=3-4 text=/[0-9]+/
";

    /// The saved output of a run whose only screen has `digits` in cells 3 and 4, bold.
    fn saved(digits: &str, schema: u32, semantics: u32) -> String {
        let d: Vec<String> = digits.chars().map(|c| format!("{{\"ch\":\"{c}\",\"bold\":true}}")).collect();
        format!(
            "{{\"cmd\":\"snapshot\",\"snapshot\":{{\"schema_version\":{schema},\"semantics_version\":{semantics},\"byte_offset\":7,\
             \"cols\":6,\"rows\":2,\"lines\":[\"ab {digits} \",\"      \"],\
             \"cells\":[[{{\"ch\":\"a\"}},{{\"ch\":\"b\"}},{{}},{},{},{{}}],[{{}},{{}},{{}},{{}},{{}},{{}}]],\
             \"wrapped_rows\":[],\"cursor\":{{\"row\":0,\"col\":0,\"hidden\":false,\"shape\":\"block\"}},\"modes\":[],\
             \"alt_screen\":false,\"title\":null,\"scrollback\":[]}}}}\n",
            d[0], d[1]
        )
    }

    fn report(saved: &str) -> Report {
        let golden = Golden::parse_str(A2).unwrap();
        let invocation = Invocation {
            golden: "dir/g.golden".to_string(),
            command: "vtee --expect dir/g.golden --snapshot run.ndjson".to_string(),
        };
        check_saved(&golden, &invocation, saved, &CheckOptions::default()).unwrap()
    }

    #[test]
    fn the_first_worked_example_passes() {
        let r = report(&saved("99", 1, 1));
        assert_eq!(r.exit_code(), 0);
        assert_eq!(r.to_text(), "PASS g.golden (1 screen)\n");
    }

    #[test]
    fn the_second_worked_example_fails_with_the_report_of_the_design() {
        let r = report(&saved("x7", 1, 1));
        assert_eq!(r.exit_code(), 1);
        assert_eq!(
            r.to_text(),
            "FAIL g.golden: 1 mismatch in 1 of 1 screen\nscreen 1:\n  mask       golden line 20   mask row=0 cols=3-4 text=/[0-9]+/   \
             row 0 col 3-4   actual 'x7' does not match\n"
        );
        assert_eq!(
            r.to_json(),
            "{\"golden\":\"g.golden\",\"result\":\"fail\",\"grammar\":1,\"schema\":1,\"semantics\":1,\
             \"snapshot\":{\"schema\":1,\"semantics\":1},\"screens_expected\":1,\"screens_actual\":1,\"total\":1,\"shown\":1,\
             \"run\":[],\"screens\":[{\"index\":1,\"name\":null,\"result\":\"fail\",\"mismatches\":[{\"kind\":\"mask\",\
             \"line\":20,\"mask\":\"mask row=0 cols=3-4 text=/[0-9]+/\",\"problem\":\"text\",\"row\":0,\"col_start\":3,\
             \"col_end\":4,\"actual\":\"x7\"}]}]}"
        );
    }

    #[test]
    fn a_different_semantics_number_is_refused_and_nothing_is_compared() {
        let golden = Golden::parse_str(&A2.replace("semantics 1", "semantics 2")).unwrap();
        let invocation = Invocation {
            golden: "g.golden".to_string(),
            command: "vtee --expect g.golden --snapshot run.ndjson".to_string(),
        };
        let r = check_saved(&golden, &invocation, &saved("x7", 1, 1), &CheckOptions::default()).unwrap();
        assert_eq!(r.exit_code(), 2);
        assert_eq!(
            r.to_text(),
            "REFUSED g.golden: semantics 2 in the golden, 1 in this vtee; nothing was compared\n  restamp (only if nothing the golden \
             checks changed):  vtee --expect g.golden --snapshot run.ndjson --update=restamp\n  or re-record with --update; what \
             changed is listed in docs/guide/snapshot-semantics.md\n"
        );
        assert!(r.to_json().contains("\"result\":\"refused\",\"reason\":\"semantics 2 in the golden"));
    }

    #[test]
    fn snapshots_that_disagree_with_the_golden_are_refused_when_read() {
        let r = report(&saved("99", 1, 7));
        assert_eq!(r.exit_code(), 2);
        assert!(r.to_text().starts_with("REFUSED g.golden: semantics 1 in the golden, 7 in the snapshots"));
    }

    #[test]
    fn snapshots_of_the_provisional_shape_are_malformed() {
        let provisional =
            "{\"schema_version\":1,\"byte_offset\":1,\"cols\":1,\"rows\":1,\"cells\":[[{\"ch\":\"a\"}]]}\n";
        let golden = Golden::parse_str(A2).unwrap();
        let invocation = Invocation { golden: "g.golden".to_string(), command: String::new() };
        let e = check_saved(&golden, &invocation, provisional, &CheckOptions::default()).unwrap_err();
        assert!(e.to_string().starts_with("snapshot: line 1: it has no `semantics_version`"), "{e}");
    }

    #[test]
    fn the_report_shows_at_most_the_asked_number_of_blocks() {
        let golden = Golden::parse_str(&A2.replace(
            "
[attrs]
0: 3-4 bold

[masks]
mask row=0 cols=3-4 text=/[0-9]+/
",
            "",
        ))
        .unwrap();
        let invocation = Invocation { golden: "g.golden".to_string(), command: String::new() };
        let options = CheckOptions { max_mismatches: 1, ..CheckOptions::default() };
        let r = check_saved(&golden, &invocation, &saved("x7", 1, 1), &options).unwrap();
        let text = r.to_text();
        assert!(text.starts_with("FAIL g.golden: 2 mismatches in 1 of 1 screen\n"), "{text}");
        assert!(text.ends_with("showing 1 of 2\n"), "{text}");
        assert!(r.to_json().contains("\"total\":2,\"shown\":1"));
    }
}
