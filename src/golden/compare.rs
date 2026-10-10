//! Comparing the screens of a run with a golden (design sections 6.5, 6.6 and 6.8).

use super::escape;
use super::model::*;
use super::run::{Run, RunProblem};
use crate::snapshot::{Cell, Snapshot};

/// What a check was asked to do beyond comparing everything.
#[derive(Debug, Clone, Copy, Default)]
pub struct CompareOptions {
    /// `--screen N`: pair the run's single screen with the golden's screen N and report only that.
    pub screen: Option<usize>,
    /// `--fail-fast`: stop at the first problem; everything after it is skipped.
    pub fail_fast: bool,
}

/// A request the options make impossible (exit 64). The message has no `vtee: ` prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageError(pub String);

impl std::fmt::Display for UsageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UsageError {}

/// The field of the cursor a `cursor` mismatch is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorField {
    Position,
    Shape,
    Hidden,
}

/// A value of a `cursor` mismatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorValue {
    Position { row: usize, col: usize },
    Shape(String),
    Hidden(bool),
}

/// Why a `mask` mismatch was reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MaskProblem {
    /// A `find=` mask whose region matched nothing.
    Stale,
    /// A `text=/RE/` mask whose span does not match.
    Text { row: usize, col_start: usize, col_end: usize, actual: String },
}

/// Why an `assert` mismatch was reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssertProblem {
    /// The region yielded no span.
    NoMatch,
    /// The text of a span differs. `failing` counts the failing spans.
    Text { row: usize, col: usize, expected: String, actual: String, failing: usize },
    /// An attribute condition failed. `failing` counts the failing cells; `col` is the first one.
    Attr { row: usize, col: usize, expected: String, actual: String, failing: usize },
}

/// One difference between the golden and the run. The report prints these in the order they were found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mismatch {
    /// The run produced another number of screens. `missing` and `extra` are 1-based.
    Screens {
        expected: usize,
        actual: usize,
        missing: Vec<usize>,
        extra: Vec<usize>,
        hint_screen: bool,
    },
    Wait {
        line: usize,
        text: Option<String>,
        timeout_ms: Option<u64>,
    },
    Name {
        expected: String,
        actual: String,
    },
    Size {
        expected: (usize, usize),
        actual: (usize, usize),
    },
    Text(Box<TextDiff>),
    Attrs {
        row: usize,
        col_start: usize,
        col_end: usize,
        expected: Vec<String>,
        actual: Vec<String>,
    },
    Cursor {
        field: CursorField,
        expected: CursorValue,
        actual: CursorValue,
    },
    Modes {
        expected: Vec<String>,
        actual: Vec<String>,
        missing: Vec<String>,
        extra: Vec<String>,
    },
    AltScreen {
        expected: bool,
        actual: bool,
    },
    Title {
        expected: Option<String>,
        actual: Option<String>,
    },
    Wrapped {
        expected: Vec<usize>,
        actual: Vec<usize>,
        missing: Vec<usize>,
        extra: Vec<usize>,
    },
    Mask {
        line: usize,
        mask: String,
        problem: MaskProblem,
    },
    Assert {
        line: usize,
        assert: String,
        problem: AssertProblem,
    },
}

/// A run of cells whose text differs (columns inclusive), with what a report needs to show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextDiff {
    pub row: usize,
    pub col_start: usize,
    pub col_end: usize,
    pub expected: String,
    pub actual: String,
    /// The whole row without trailing blanks.
    pub expected_row: String,
    pub actual_row: String,
    /// The whole row as `[text]` tokens, for display.
    pub expected_tokens: String,
    pub actual_tokens: String,
    /// Where the difference starts in `actual_tokens`, and how many characters it spans.
    pub caret_start: usize,
    pub caret_len: usize,
}

/// How a golden screen fared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenStatus {
    Pass,
    Fail,
    /// Not compared: the run produced no such screen, or `--fail-fast` stopped before it.
    Skipped,
}

/// The comparison of one golden screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenResult {
    /// 1-based, the golden's.
    pub index: usize,
    /// The golden block's `name`.
    pub name: Option<String>,
    pub status: ScreenStatus,
    pub mismatches: Vec<Mismatch>,
}

/// Everything a comparison found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comparison {
    pub screens_expected: usize,
    pub screens_actual: usize,
    /// Run-level mismatches: `screens`, `wait`, and stale shared masks.
    pub run: Vec<Mismatch>,
    pub screens: Vec<ScreenResult>,
}

impl Comparison {
    /// Mismatches in all, run-level and per screen.
    pub fn total(&self) -> usize {
        self.run.len() + self.screens.iter().map(|s| s.mismatches.len()).sum::<usize>()
    }

    /// True when nothing differs.
    pub fn passed(&self) -> bool {
        self.total() == 0
    }
}

// ---- regions -----------------------------------------------------------------------------------------------------

/// A stretch of one row; columns are inclusive.
#[derive(Debug, Clone, Copy)]
struct Span {
    row: usize,
    first: usize,
    last: usize,
    whole_row: bool,
}

fn row_text(cells: &[Cell]) -> String {
    let used = cells.iter().rposition(|c| c.ch != " ").map_or(0, |i| i + 1);
    cells[..used].iter().map(|c| c.ch.as_str()).collect()
}

fn span_text(screen: &Snapshot, span: Span) -> String {
    let cells = &screen.cells[span.row];
    if span.whole_row { row_text(cells) } else { cells[span.first..=span.last].iter().map(|c| c.ch.as_str()).collect() }
}

/// The spans of a region on the actual screen (section 6.5).
fn spans(region: &Region, screen: &Snapshot) -> Vec<Span> {
    let (first_row, last_row) = region.rows.unwrap_or((0, screen.rows - 1));
    let last_row = last_row.min(screen.rows - 1);
    let mut out = Vec::new();
    for row in first_row..=last_row {
        let (c0, c1) = region.cols.unwrap_or((0, screen.cols - 1));
        let (c0, c1) = (c0.min(screen.cols - 1), c1.min(screen.cols - 1));
        let Some(pattern) = &region.find else {
            out.push(Span { row, first: c0, last: c1, whole_row: region.cols.is_none() });
            continue;
        };
        let slice = &screen.cells[row][c0..=c1];
        let used = slice.iter().rposition(|c| c.ch != " ").map_or(0, |i| i + 1);
        let mut text = String::new();
        let mut owner: Vec<usize> = Vec::new();
        for (i, cell) in slice[..used].iter().enumerate() {
            for ch in cell.ch.chars() {
                text.push(ch);
                owner.push(i);
            }
        }
        for (start, end) in pattern.find_all(&text) {
            let first = owner[start];
            let mut last = owner[end - 1];
            while slice.get(last + 1).is_some_and(|c| c.ch.is_empty()) {
                last += 1;
            }
            out.push(Span { row, first: c0 + first, last: c0 + last, whole_row: false });
        }
    }
    out
}

// ---- the screen --------------------------------------------------------------------------------------------------

/// What comparing one screen found, and what the shared `find=` masks matched on it.
struct Screen {
    mismatches: Vec<Mismatch>,
    /// For each shared mask (in order), whether it yielded a span here. `None` when the masks were not evaluated.
    shared_hits: Option<Vec<bool>>,
}

fn compare_screen(golden: &Golden, expected: &GoldenScreen, actual: &Snapshot, actual_name: Option<&String>) -> Screen {
    let mut mismatches = Vec::new();
    let ignore = golden.ignore.union(&expected.ignore);

    if let (Some(e), Some(a)) = (&expected.name, actual_name)
        && e != a
    {
        mismatches.push(Mismatch::Name { expected: e.clone(), actual: a.clone() });
    }
    let same_size = expected.cols == actual.cols && expected.rows == actual.rows;
    if !same_size {
        mismatches
            .push(Mismatch::Size { expected: (expected.cols, expected.rows), actual: (actual.cols, actual.rows) });
    }

    // Header.
    if !ignore.cursor_position && (expected.cursor_row, expected.cursor_col) != (actual.cursor.row, actual.cursor.col) {
        mismatches.push(Mismatch::Cursor {
            field: CursorField::Position,
            expected: CursorValue::Position { row: expected.cursor_row, col: expected.cursor_col },
            actual: CursorValue::Position { row: actual.cursor.row, col: actual.cursor.col },
        });
    }
    if !ignore.cursor_shape && expected.cursor_shape != actual.cursor.shape {
        mismatches.push(Mismatch::Cursor {
            field: CursorField::Shape,
            expected: CursorValue::Shape(shape_name(expected.cursor_shape).to_string()),
            actual: CursorValue::Shape(shape_name(actual.cursor.shape).to_string()),
        });
    }
    if !ignore.cursor_hidden && expected.cursor_hidden != actual.cursor.hidden {
        mismatches.push(Mismatch::Cursor {
            field: CursorField::Hidden,
            expected: CursorValue::Hidden(expected.cursor_hidden),
            actual: CursorValue::Hidden(actual.cursor.hidden),
        });
    }
    let kept = |modes: &[String]| -> Vec<String> { sorted_modes(modes.iter().filter(|m| !ignore.modes.contains(*m))) };
    let (expected_modes, actual_modes) = (kept(&expected.modes), kept(&actual.modes));
    if expected_modes != actual_modes {
        let missing = expected_modes.iter().filter(|m| !actual_modes.contains(m)).cloned().collect();
        let extra = actual_modes.iter().filter(|m| !expected_modes.contains(m)).cloned().collect();
        mismatches.push(Mismatch::Modes { expected: expected_modes, actual: actual_modes, missing, extra });
    }
    if !ignore.alt_screen && expected.alt_screen != actual.alt_screen {
        mismatches.push(Mismatch::AltScreen { expected: expected.alt_screen, actual: actual.alt_screen });
    }
    if !ignore.title && expected.title != actual.title {
        mismatches.push(Mismatch::Title { expected: expected.title.clone(), actual: actual.title.clone() });
    }
    if !ignore.wrapped {
        let mut actual_wrapped = actual.wrapped_rows.clone();
        actual_wrapped.sort_unstable();
        actual_wrapped.dedup();
        if expected.wrapped != actual_wrapped {
            let missing = expected.wrapped.iter().filter(|r| !actual_wrapped.contains(r)).copied().collect();
            let extra = actual_wrapped.iter().filter(|r| !expected.wrapped.contains(r)).copied().collect();
            mismatches.push(Mismatch::Wrapped {
                expected: expected.wrapped.clone(),
                actual: actual_wrapped,
                missing,
                extra,
            });
        }
    }
    if !same_size {
        return Screen { mismatches, shared_hits: None };
    }

    // Masks decide which cells are compared; their own mismatches are reported after the cells.
    let (rows, cols) = (actual.rows, actual.cols);
    let mut skip_text = vec![vec![false; cols]; rows];
    let mut skip_attrs = vec![vec![false; cols]; rows];
    let mut mask_mismatches = Vec::new();
    let mut shared_hits = Vec::new();
    let shared: Vec<(&Mask, bool)> = golden.shared_mask_items().map(|m| (m, true)).collect();
    let own: Vec<(&Mask, bool)> = expected.mask_items().map(|m| (m, false)).collect();
    for (mask, is_shared) in shared.into_iter().chain(own) {
        let found = spans(&mask.region, actual);
        let stale = mask.region.find.is_some() && found.is_empty();
        if is_shared {
            shared_hits.push(mask.region.find.is_none() || !found.is_empty());
        }
        if stale && !is_shared {
            mask_mismatches.push(Mismatch::Mask {
                line: mask.line,
                mask: mask.raw.clone(),
                problem: MaskProblem::Stale,
            });
        }
        for span in found {
            let text_masked = mask.text.is_some();
            for c in span.first..=span.last {
                if text_masked {
                    skip_text[span.row][c] = true;
                }
                if mask.attrs_ignore {
                    skip_attrs[span.row][c] = true;
                }
            }
            if let Some(MaskText::Pattern(pattern)) = &mask.text {
                let text = span_text(actual, span);
                if !pattern.is_full_match(&text) {
                    mask_mismatches.push(Mismatch::Mask {
                        line: mask.line,
                        mask: mask.raw.clone(),
                        problem: MaskProblem::Text {
                            row: span.row,
                            col_start: span.first,
                            col_end: span.last,
                            actual: text,
                        },
                    });
                }
            }
        }
    }

    // Cells.
    for row in 0..rows {
        let (e_row, a_row) = (&expected.cells[row], &actual.cells[row]);
        let mut c = 0;
        while c < cols {
            if skip_text[row][c] || e_row[c].ch == a_row[c].ch {
                c += 1;
                continue;
            }
            let mut end = c;
            while end + 1 < cols && !skip_text[row][end + 1] && e_row[end + 1].ch != a_row[end + 1].ch {
                end += 1;
            }
            mismatches.push(Mismatch::Text(Box::new(text_diff(row, c, end, e_row, a_row))));
            c = end + 1;
        }
        let mut c = 0;
        while c < cols {
            let pair = |c: usize| (attr_tokens(&e_row[c], &ignore.attrs), attr_tokens(&a_row[c], &ignore.attrs));
            if skip_attrs[row][c] {
                c += 1;
                continue;
            }
            let (e, a) = pair(c);
            if e == a {
                c += 1;
                continue;
            }
            let mut end = c;
            while end + 1 < cols && !skip_attrs[row][end + 1] && pair(end + 1) == (e.clone(), a.clone()) {
                end += 1;
            }
            mismatches.push(Mismatch::Attrs { row, col_start: c, col_end: end, expected: e, actual: a });
            c = end + 1;
        }
    }
    mismatches.extend(mask_mismatches);

    for assertion in expected.assert_items() {
        if let Some(problem) = check_assert(assertion, actual) {
            mismatches.push(Mismatch::Assert { line: assertion.line, assert: assertion.raw.clone(), problem });
        }
    }
    Screen { mismatches, shared_hits: Some(shared_hits) }
}

fn text_diff(row: usize, first: usize, last: usize, expected: &[Cell], actual: &[Cell]) -> TextDiff {
    let join = |cells: &[Cell]| cells.iter().map(|c| c.ch.as_str()).collect::<String>();
    let tokens = |cells: &[Cell]| escape::encode_row(cells.iter().map(|c| c.ch.as_str()));
    let prefix = |cells: &[Cell], upto: usize| -> usize {
        let mut s = String::new();
        for cell in &cells[..upto] {
            escape::encode_cell(&cell.ch, &mut s);
        }
        s.chars().count()
    };
    let mut span = String::new();
    for cell in &actual[first..=last] {
        escape::encode_cell(&cell.ch, &mut span);
    }
    TextDiff {
        row,
        col_start: first,
        col_end: last,
        expected: join(&expected[first..=last]),
        actual: join(&actual[first..=last]),
        expected_row: row_text(expected),
        actual_row: row_text(actual),
        expected_tokens: tokens(expected),
        actual_tokens: tokens(actual),
        caret_start: prefix(actual, first),
        caret_len: span.chars().count().max(1),
    }
}

fn check_assert(assertion: &Assert, actual: &Snapshot) -> Option<AssertProblem> {
    let mut found = spans(&assertion.region, actual);
    if found.is_empty() {
        return Some(AssertProblem::NoMatch);
    }
    match &assertion.text {
        Some(AssertText::Exact(exact)) => {
            // Checked when the golden was read: a single row, no `find=`.
            let span = found[0];
            let text = span_text(actual, span);
            if &text != exact {
                return Some(AssertProblem::Text {
                    row: span.row,
                    col: span.first,
                    expected: exact.clone(),
                    actual: text,
                    failing: 1,
                });
            }
        }
        Some(AssertText::Pattern(pattern)) => {
            let failing: Vec<(Span, String)> = found
                .iter()
                .map(|s| (*s, span_text(actual, *s)))
                .filter(|(_, text)| !pattern.is_full_match(text))
                .collect();
            if let Some((span, text)) = failing.first() {
                return Some(AssertProblem::Text {
                    row: span.row,
                    col: span.first,
                    expected: format!("/{}/", pattern.source()),
                    actual: text.clone(),
                    failing: failing.len(),
                });
            }
        }
        None => {}
    }
    let mut first_failure: Option<(usize, usize, Violations)> = None;
    let mut failing = 0;
    found.sort_by_key(|s| (s.row, s.first));
    for span in found {
        for c in span.first..=span.last {
            let violations = violated(&assertion.attrs, &actual.cells[span.row][c]);
            if !violations.is_empty() {
                failing += 1;
                first_failure.get_or_insert((span.row, c, violations));
            }
        }
    }
    let (row, col, violations) = first_failure?;
    Some(AssertProblem::Attr {
        row,
        col,
        expected: violations.iter().map(|(e, _)| e.as_str()).collect::<Vec<_>>().join(" "),
        actual: violations.iter().map(|(_, a)| a.as_str()).collect::<Vec<_>>().join(" "),
        failing,
    })
}

/// The conditions a cell fails, as (expected, actual) tokens.
type Violations = Vec<(String, String)>;

fn violated(checks: &[AttrCheck], cell: &Cell) -> Violations {
    let flag = |attr: Attr| match attr {
        Attr::Bold => cell.bold,
        Attr::Dim => cell.dim,
        Attr::Italic => cell.italic,
        Attr::Reverse => cell.reverse,
        Attr::Strike => cell.strike,
        _ => cell.conceal,
    };
    let link = |cell: &Cell| match &cell.hyperlink {
        Some(uri) => format!("link={}", escape::encode_string(uri)),
        None => "!link".to_string(),
    };
    let mut out = Vec::new();
    for check in checks {
        let failure = match check {
            AttrCheck::Fg(c) => (cell.fg != *c).then(|| (format!("fg={c}"), format!("fg={}", cell.fg))),
            AttrCheck::Bg(c) => (cell.bg != *c).then(|| (format!("bg={c}"), format!("bg={}", cell.bg))),
            AttrCheck::Ulcolor(c) => (cell.underline_color != *c)
                .then(|| (format!("ulcolor={c}"), format!("ulcolor={}", cell.underline_color))),
            AttrCheck::Underline(s) => (cell.underline != *s).then(|| {
                (format!("underline={}", underline_name(*s)), format!("underline={}", underline_name(cell.underline)))
            }),
            AttrCheck::Link(uri) => (cell.hyperlink.as_ref() != Some(uri))
                .then(|| (format!("link={}", escape::encode_string(uri)), link(cell))),
            AttrCheck::NoLink => cell.hyperlink.is_some().then(|| ("!link".to_string(), link(cell))),
            AttrCheck::Flag(attr, on) => (flag(*attr) != *on).then(|| {
                let name = attr.name();
                if *on { (name.to_string(), format!("!{name}")) } else { (format!("!{name}"), name.to_string()) }
            }),
        };
        out.extend(failure);
    }
    out
}

// ---- the run -----------------------------------------------------------------------------------------------------

/// Compares the screens of a run with a golden, in the order of section 6.8: pair by position, compare each pair, then
/// judge the shared masks over the whole run. The golden's versions must already have been checked.
pub fn compare(golden: &Golden, run: &Run, options: &CompareOptions) -> Result<Comparison, UsageError> {
    let pairs: Vec<(usize, usize)> = match options.screen {
        Some(n) => {
            if run.screens.len() != 1 {
                return Err(UsageError(format!(
                    "--screen {n} selects one screen, but the run produced {}",
                    run.screens.len()
                )));
            }
            if n == 0 || n > golden.screens.len() {
                return Err(UsageError(format!(
                    "--screen {n} selects one screen, but the golden has {} screens",
                    golden.screens.len()
                )));
            }
            vec![(n - 1, 0)]
        }
        None => (0..golden.screens.len().min(run.screens.len())).map(|i| (i, i)).collect(),
    };

    let mut run_level = Vec::new();
    if options.screen.is_none() && golden.screens.len() != run.screens.len() {
        run_level.push(Mismatch::Screens {
            expected: golden.screens.len(),
            actual: run.screens.len(),
            missing: (run.screens.len() + 1..=golden.screens.len()).collect(),
            extra: (golden.screens.len() + 1..=run.screens.len()).collect(),
            hint_screen: run.screens.len() == 1 && golden.screens.len() > 1,
        });
    }

    let mut results: Vec<ScreenResult> = Vec::new();
    let mut hits: Vec<bool> = vec![false; golden.shared_mask_items().count()];
    let mut evaluated = false;
    let selected: Vec<usize> = match options.screen {
        Some(n) => vec![n - 1],
        None => (0..golden.screens.len()).collect(),
    };
    for g in selected {
        let screen = &golden.screens[g];
        let pair = pairs.iter().find(|(gi, _)| *gi == g);
        let Some(&(_, a)) = pair else {
            results.push(ScreenResult {
                index: g + 1,
                name: screen.name.clone(),
                status: ScreenStatus::Skipped,
                mismatches: Vec::new(),
            });
            continue;
        };
        let outcome = compare_screen(golden, screen, &run.screens[a], run.names.get(a).and_then(Option::as_ref));
        if let Some(screen_hits) = &outcome.shared_hits {
            evaluated = true;
            for (hit, here) in hits.iter_mut().zip(screen_hits) {
                *hit |= *here;
            }
        }
        let status = if outcome.mismatches.is_empty() { ScreenStatus::Pass } else { ScreenStatus::Fail };
        results.push(ScreenResult { index: g + 1, name: screen.name.clone(), status, mismatches: outcome.mismatches });
    }

    for problem in &run.problems {
        run_level.push(wait_mismatch(problem));
    }
    if evaluated {
        for (mask, hit) in golden.shared_mask_items().zip(&hits) {
            if mask.region.find.is_some() && !hit {
                run_level.push(Mismatch::Mask { line: mask.line, mask: mask.raw.clone(), problem: MaskProblem::Stale });
            }
        }
    }

    let mut comparison = Comparison {
        screens_expected: golden.screens.len(),
        screens_actual: run.screens.len(),
        run: run_level,
        screens: results,
    };
    if options.fail_fast {
        stop_at_first_problem(&mut comparison, run, options);
    }
    Ok(comparison)
}

fn wait_mismatch(problem: &RunProblem) -> Mismatch {
    Mismatch::Wait { line: problem.line, text: problem.text.clone(), timeout_ms: problem.timeout_ms }
}

/// `--fail-fast`: keep only the problem that happened first and mark what came after it as skipped. Problems are
/// ordered as the run met them: screen *i* is compared when it is produced, a `wait_for` falls after the screens that
/// came before it, a wrong number of screens is known at the end, and the shared masks are judged last.
fn stop_at_first_problem(comparison: &mut Comparison, run: &Run, options: &CompareOptions) {
    // (position, phase, which)
    enum Which {
        Screen(usize),
        Run(usize),
    }
    let mut first: Option<((usize, u8), Which)> = None;
    let mut consider = |key: (usize, u8), which: Which| {
        if first.as_ref().is_none_or(|(k, _)| key < *k) {
            first = Some((key, which));
        }
    };
    for (i, result) in comparison.screens.iter().enumerate() {
        if !result.mismatches.is_empty() {
            let position = if options.screen.is_some() { 1 } else { result.index };
            consider((position, 0), Which::Screen(i));
        }
    }
    for (i, mismatch) in comparison.run.iter().enumerate() {
        let key = match mismatch {
            Mismatch::Wait { .. } => {
                let after =
                    run.problems.iter().filter(|p| wait_mismatch(p) == *mismatch).map(|p| p.after_screens).next();
                (after.unwrap_or(usize::MAX - 2), 1)
            }
            Mismatch::Screens { .. } => (usize::MAX - 1, 0),
            _ => (usize::MAX, 0),
        };
        consider(key, Which::Run(i));
    }
    let Some((key, which)) = first else { return };
    let kept_screen = match which {
        Which::Screen(i) => {
            comparison.run.clear();
            comparison.screens[i].mismatches.truncate(1);
            Some(i)
        }
        Which::Run(i) => {
            let mismatch = comparison.run.remove(i);
            comparison.run = vec![mismatch];
            None
        }
    };
    for (i, result) in comparison.screens.iter_mut().enumerate() {
        let position = if options.screen.is_some() { 1 } else { result.index };
        let before = match kept_screen {
            Some(kept) => i <= kept,
            None => position <= key.0,
        };
        if !before {
            result.mismatches.clear();
            result.status = ScreenStatus::Skipped;
        } else if Some(i) != kept_screen {
            result.mismatches.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::golden::LoadError;
    use crate::snapshot::{Cursor, CursorShape};

    /// A snapshot of text lines (plain ASCII, at most `cols` each), cursor at the origin.
    fn screen(cols: usize, text: &[&str]) -> Snapshot {
        let cells: Vec<Vec<Cell>> = text
            .iter()
            .map(|line| {
                let mut row: Vec<Cell> = line.chars().map(|c| Cell { ch: c.to_string(), ..Cell::default() }).collect();
                row.resize_with(cols, Cell::default);
                row
            })
            .collect();
        Snapshot {
            schema_version: 1,
            semantics_version: 1,
            byte_offset: 0,
            cols,
            rows: text.len(),
            lines: text.iter().map(|l| format!("{l:<cols$}")).collect(),
            cells,
            wrapped_rows: vec![],
            cursor: Cursor { row: 0, col: 0, hidden: false, shape: CursorShape::Block },
            modes: vec![],
            alt_screen: false,
            title: None,
            scrollback: vec![],
            producer: Default::default(),
        }
    }

    /// The golden of `text`, with `extra` (sections written by hand) appended.
    fn golden(extra: &str, text: &[&str], cols: usize) -> Golden {
        let recorded = Golden::record(&[screen(cols, text)], &[], Default::default()).to_text();
        let source = format!("{recorded}{extra}");
        match Golden::parse_str(&source) {
            Ok(g) => g,
            Err(LoadError::Malformed(e)) => panic!("{e}\n{source}"),
            Err(other) => panic!("{other}"),
        }
    }

    fn run_of(screens: Vec<Snapshot>) -> Run {
        Run { names: vec![None; screens.len()], screens, problems: vec![] }
    }

    fn check(g: &Golden, screens: Vec<Snapshot>) -> Comparison {
        compare(g, &run_of(screens), &CompareOptions::default()).unwrap()
    }

    #[test]
    fn identical_screens_pass() {
        let g = golden("", &["hello", "world"], 8);
        let c = check(&g, vec![screen(8, &["hello", "world"])]);
        assert!(c.passed(), "{c:?}");
        assert_eq!(c.screens[0].status, ScreenStatus::Pass);
    }

    #[test]
    fn a_changed_character_is_one_text_block_per_run_of_cells() {
        let g = golden("", &["hello", "world"], 8);
        let c = check(&g, vec![screen(8, &["hellO", "wxrld"])]);
        let Mismatch::Text(first) = &c.screens[0].mismatches[0] else { panic!("{c:?}") };
        assert_eq!((first.row, first.col_start, first.col_end), (0, 4, 4));
        assert_eq!((first.expected.as_str(), first.actual.as_str()), ("o", "O"));
        assert_eq!(c.total(), 2);
        let c = check(&g, vec![screen(8, &["HELLO", "world"])]);
        assert_eq!(c.total(), 1, "adjacent differing cells are one block");
    }

    #[test]
    fn a_masked_figure_may_change_but_not_its_shape() {
        let g = golden("\n[masks]\nmask find=/[0-9]+/ text=/[0-9]+/\n", &["load 123 ok"], 12);
        assert!(check(&g, vec![screen(12, &["load 907 ok"])]).passed());
        let c = check(&g, vec![screen(12, &["load ??? ok"])]);
        assert!(matches!(c.screens[0].mismatches[0], Mismatch::Text(_)), "{c:?}");
        assert!(
            c.screens[0].mismatches.iter().any(|m| matches!(m, Mismatch::Mask { problem: MaskProblem::Stale, .. }))
        );
    }

    #[test]
    fn a_shared_find_mask_is_stale_only_when_it_matches_nowhere() {
        let source = "vtee-golden 1\nschema 1\nsemantics 1\n\n[masks]\nmask find=/[0-9]+/ text=ignore\n";
        let one = golden("", &["a1"], 3);
        let two = golden("", &["a"], 3);
        let mut g = Golden::parse_str(&format!(
            "{source}\n[screen 1]\n{}",
            one.to_text().split("[screen 1]\n").nth(1).unwrap()
        ))
        .unwrap();
        let second = two.screens[0].clone();
        g.screens.push(second);
        let digits_in_first = check(&g, vec![screen(3, &["a2"]), screen(3, &["a"])]);
        assert!(digits_in_first.passed(), "{digits_in_first:?}");
        let nowhere = check(&g, vec![screen(3, &["a"]), screen(3, &["a"])]);
        assert!(matches!(nowhere.run.as_slice(), [Mismatch::Mask { problem: MaskProblem::Stale, .. }]), "{nowhere:?}");
    }

    #[test]
    fn a_different_number_of_screens_is_a_run_level_mismatch() {
        let g = golden("", &["a"], 3);
        let c = check(&g, vec![screen(3, &["a"]), screen(3, &["a"])]);
        assert!(
            matches!(c.run.as_slice(), [Mismatch::Screens { missing, extra, .. }] if missing.is_empty() && extra == &[2])
        );
        assert_eq!(c.screens[0].status, ScreenStatus::Pass);
    }

    #[test]
    fn the_screen_option_must_fit_the_run_and_the_golden() {
        let g = golden("", &["a"], 3);
        let one = run_of(vec![screen(3, &["a"])]);
        let two = run_of(vec![screen(3, &["a"]), screen(3, &["a"])]);
        let select = |n| CompareOptions { screen: Some(n), fail_fast: false };
        assert!(compare(&g, &two, &select(1)).unwrap_err().0.contains("the run produced 2"));
        assert!(compare(&g, &one, &select(4)).unwrap_err().0.contains("the golden has 1 screens"));
        assert!(compare(&g, &one, &select(1)).unwrap().passed());
    }

    #[test]
    fn a_size_mismatch_stops_the_cell_comparison() {
        let g = golden("", &["hello"], 8);
        let c = check(&g, vec![screen(9, &["jello"])]);
        assert!(matches!(c.screens[0].mismatches.as_slice(), [Mismatch::Size { .. }]), "{c:?}");
    }

    #[test]
    fn assertions_check_the_text_and_every_cell() {
        let g = golden(
            "\n[assert]\nassert row=0 text=\"hello\" !bold\nassert row=0 cols=0-1 bold\nassert find=/zzz/ bold\n",
            &["hello"],
            8,
        );
        let mut actual = screen(8, &["hello"]);
        actual.cells[0][0].bold = true;
        let c = check(&g, vec![actual]);
        let problems: Vec<_> = c.screens[0]
            .mismatches
            .iter()
            .filter_map(|m| if let Mismatch::Assert { problem, .. } = m { Some(problem.clone()) } else { None })
            .collect();
        assert_eq!(problems.len(), 3, "{c:?}");
        assert!(matches!(&problems[0], AssertProblem::Attr { row: 0, col: 0, expected, actual, failing: 1 }
            if expected == "!bold" && actual == "bold"));
        assert!(matches!(&problems[1], AssertProblem::Attr { row: 0, col: 1, failing: 1, .. }));
        assert_eq!(problems[2], AssertProblem::NoMatch);
    }

    #[test]
    fn fail_fast_keeps_only_the_first_problem() {
        let g = golden("", &["a", "b"], 3);
        let mut g = g;
        let second = golden("", &["a"], 3).screens.remove(0);
        g.screens = vec![g.screens[0].clone(), second.clone(), second];
        let run = run_of(vec![screen(3, &["a", "b"]), screen(3, &["x"]), screen(3, &["y"])]);
        let c = compare(&g, &run, &CompareOptions { screen: None, fail_fast: true }).unwrap();
        assert_eq!(c.total(), 1);
        assert_eq!(
            c.screens.iter().map(|s| s.status).collect::<Vec<_>>(),
            [ScreenStatus::Pass, ScreenStatus::Fail, ScreenStatus::Skipped]
        );
    }
}
