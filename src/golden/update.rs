//! Rewriting a golden from a run (`--update`, design section 6.11): a re-record keeps what a person wrote (`ignore`
//! lines, masks, assertions, comments) and regenerates what the run decides, and says what moved.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use serde::Serialize;

use super::model::*;
use super::run::Run;
use super::write::{RecordOptions, producer_line};
use crate::snapshot::Snapshot;

/// What happened to one screen block of the golden.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenChange {
    /// The same expectations as before.
    Unchanged,
    /// The run decides something else. Each part reads like `14 cells changed (text 3, attrs 11)` or `cursor moved`.
    Changed { parts: Vec<String>, cells_changed: usize, text: usize, attrs: usize },
    /// The run has a screen the golden had no block for. It has no masks and no assertions.
    New,
    /// The golden had a block the run has no screen for. Its masks and assertions are lost.
    Dropped { masks: usize, asserts: usize },
}

/// The outcome of a re-record.
#[derive(Debug, Clone)]
pub struct Update {
    /// The golden to write.
    pub golden: Golden,
    /// The golden did not exist.
    pub created: bool,
    /// `golden` says what the file already says (compared as canonical text), so nothing needs saying.
    pub unchanged: bool,
    /// One entry per block of the old golden or of the run, whichever is longer (empty when the file was created).
    pub screens: Vec<ScreenChange>,
    /// The `schema` and `semantics` stamps of the old golden and the new one, when they differ.
    pub stamp: Option<(String, String)>,
    /// Facts the user should see that are not about a screen, for example that `--portable` was not applied.
    pub notes: Vec<String>,
}

fn count(n: usize, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {many}") }
}

fn diff_screens(old: &GoldenScreen, new: &GoldenScreen) -> ScreenChange {
    let mut parts = Vec::new();
    let (mut cells_changed, mut text, mut attrs) = (0, 0, 0);
    if (old.cols, old.rows) != (new.cols, new.rows) {
        parts.push(format!("size {}x{} -> {}x{}", old.cols, old.rows, new.cols, new.rows));
    } else {
        let none = BTreeSet::new();
        for (old_row, new_row) in old.cells.iter().zip(&new.cells) {
            for (a, b) in old_row.iter().zip(new_row) {
                let text_differs = a.ch != b.ch;
                let attrs_differ = attr_tokens(a, &none) != attr_tokens(b, &none);
                text += usize::from(text_differs);
                attrs += usize::from(attrs_differ);
                cells_changed += usize::from(text_differs || attrs_differ);
            }
        }
        if cells_changed > 0 {
            parts.push(format!("{} changed (text {text}, attrs {attrs})", count(cells_changed, "cell", "cells")));
        }
    }
    if (old.cursor_row, old.cursor_col) != (new.cursor_row, new.cursor_col) {
        parts.push("cursor moved".to_string());
    }
    if old.cursor_shape != new.cursor_shape {
        parts.push("cursor shape changed".to_string());
    }
    if old.cursor_hidden != new.cursor_hidden {
        parts.push("cursor visibility changed".to_string());
    }
    if old.modes != new.modes {
        parts.push("modes changed".to_string());
    }
    if old.alt_screen != new.alt_screen {
        parts.push("alt_screen changed".to_string());
    }
    if old.title != new.title {
        parts.push("title changed".to_string());
    }
    if old.wrapped != new.wrapped {
        parts.push("wrapped rows changed".to_string());
    }
    if old.name != new.name {
        parts.push("name changed".to_string());
    }
    if parts.is_empty() { ScreenChange::Unchanged } else { ScreenChange::Changed { parts, cells_changed, text, attrs } }
}

/// `fresh` takes the hand-written parts of `old`: what the run does not decide.
fn keep_hand_written(mut fresh: GoldenScreen, old: &GoldenScreen) -> GoldenScreen {
    fresh.ignore = old.ignore.clone();
    fresh.header_comments = old.header_comments.clone();
    fresh.x_lines = old.x_lines.clone();
    fresh.masks = old.masks.clone();
    fresh.asserts = old.asserts.clone();
    fresh.line = old.line;
    fresh
}

/// Replaces the `x-producer` line of a header, or adds it, leaving other `x-` lines and comments where they are.
fn set_producer(x_lines: &mut Vec<String>, line: Option<String>) {
    if let Some(line) = line {
        x_lines.retain(|l| !l.starts_with("x-producer"));
        x_lines.push(line);
    }
}

/// Regenerates a golden from the screens of a run (`--update` and `--update=record`). With no `existing` golden it
/// creates one, without masks or assertions. With one, each screen of the run takes the block at its position, keeps
/// that block's `ignore` lines, masks, assertions and comments, and the file keeps its own `ignore` lines, shared masks
/// and comments; blocks past the end of the run are dropped. `x-producer` changes only when something else does.
///
/// `run` must have at least one screen.
pub fn plan_record(existing: Option<&Golden>, run: &Run, options: RecordOptions) -> Update {
    let Some(existing) = existing else {
        return Update {
            golden: Golden::record(&run.screens, &run.names, options),
            created: true,
            unchanged: false,
            screens: Vec::new(),
            stamp: None,
            notes: Vec::new(),
        };
    };
    let first = &run.screens[0];
    let mut candidate = existing.clone();
    candidate.grammar = GRAMMAR_VERSION;
    candidate.schema = first.schema_version;
    candidate.semantics = Semantics::Number(first.semantics_version);
    candidate.screens = run
        .screens
        .iter()
        .enumerate()
        .map(|(i, snapshot)| {
            let fresh = GoldenScreen::from_snapshot(snapshot, run.names.get(i).cloned().flatten());
            match existing.screens.get(i) {
                Some(old) => keep_hand_written(fresh, old),
                None => fresh,
            }
        })
        .collect();

    let screens = (0..existing.screens.len().max(run.screens.len()))
        .map(|i| match (existing.screens.get(i), candidate.screens.get(i)) {
            (Some(old), Some(new)) => diff_screens(old, new),
            (Some(old), None) => {
                ScreenChange::Dropped { masks: old.mask_items().count(), asserts: old.assert_items().count() }
            }
            (None, _) => ScreenChange::New,
        })
        .collect();

    let mut notes = Vec::new();
    if options.portable {
        notes.push(
            "--portable only applies when a golden is created; the ignore lines of the existing file were kept"
                .to_string(),
        );
    }
    let stamp = (existing.schema != candidate.schema || existing.semantics != candidate.semantics).then(|| {
        (
            format!("schema {} semantics {}", existing.schema, existing.semantics),
            format!("schema {} semantics {}", candidate.schema, candidate.semantics),
        )
    });
    let unchanged = candidate.to_text() == existing.to_text();
    if !unchanged {
        set_producer(&mut candidate.x_lines, producer_line(first));
    }
    Update {
        golden: if unchanged { existing.clone() } else { candidate },
        created: false,
        unchanged,
        screens,
        stamp,
        notes,
    }
}

/// Regenerates one block of an existing golden from the only screen of a run (`--update --screen N`), `n` counting
/// from 1. The block keeps its hand-written parts; nothing else in the file changes. The caller has checked that `n`
/// is a block of `existing` and that the snapshot carries the golden's own numbers.
pub fn plan_screen(existing: &Golden, n: usize, snapshot: &Snapshot, name: Option<String>) -> Update {
    let mut candidate = existing.clone();
    let old = &existing.screens[n - 1];
    candidate.screens[n - 1] = keep_hand_written(GoldenScreen::from_snapshot(snapshot, name), old);
    let change = diff_screens(old, &candidate.screens[n - 1]);
    let unchanged = candidate.to_text() == existing.to_text();
    if !unchanged {
        set_producer(&mut candidate.x_lines, producer_line(snapshot));
    }
    let screens =
        (1..=existing.screens.len()).map(|i| if i == n { change.clone() } else { ScreenChange::Unchanged }).collect();
    Update {
        golden: if unchanged { existing.clone() } else { candidate },
        created: false,
        unchanged,
        screens,
        stamp: None,
        notes: Vec::new(),
    }
}

/// The golden with only its `semantics` stamp (and `x-producer`) replaced: `--update=restamp`.
pub fn restamped(golden: &Golden, semantics: u32, producer: Option<&Snapshot>) -> Golden {
    let mut out = golden.clone();
    out.semantics = Semantics::Number(semantics);
    if let Some(snapshot) = producer {
        set_producer(&mut out.x_lines, producer_line(snapshot));
    }
    out
}

impl Update {
    /// `CREATED`, `UPDATED` or `UNCHANGED` for the first word of the report.
    pub fn verb(&self) -> &'static str {
        if self.created {
            "CREATED"
        } else if self.unchanged {
            "UNCHANGED"
        } else {
            "UPDATED"
        }
    }

    /// The report `--update` prints (design section 6.11): a first line, then what moved, one line per fact.
    pub fn to_text(&self, name: &str) -> String {
        let k = self.golden.screens.len();
        let mut out = format!("{} {name}: {}\n", self.verb(), count(k, "screen", "screens"));
        if let Some((from, to)) = &self.stamp {
            let _ = writeln!(out, "  stamp: {from} -> {to}");
        }
        for (i, change) in self.screens.iter().enumerate() {
            let at = i + 1;
            match change {
                ScreenChange::Unchanged => {}
                ScreenChange::Changed { parts, .. } => {
                    let _ = writeln!(out, "  screen {at}: {}", parts.join(", "));
                }
                ScreenChange::New => {
                    let _ = writeln!(out, "  screen {at}: new (no masks, no asserts: add them by hand)");
                }
                ScreenChange::Dropped { masks, asserts } => {
                    let lost = match (*masks, *asserts) {
                        (0, 0) => "it had no masks and no asserts".to_string(),
                        (m, 0) => format!(
                            "it had {}, which {} lost",
                            count(m, "mask", "masks"),
                            if m == 1 { "is" } else { "are" }
                        ),
                        (0, a) => format!(
                            "it had {}, which {} lost",
                            count(a, "assert", "asserts"),
                            if a == 1 { "is" } else { "are" }
                        ),
                        (m, a) => format!(
                            "it had {} and {}, which are lost",
                            count(m, "mask", "masks"),
                            count(a, "assert", "asserts")
                        ),
                    };
                    let _ = writeln!(out, "  screen {at}: dropped ({lost})");
                }
            }
        }
        for note in &self.notes {
            let _ = writeln!(out, "  note: {note}");
        }
        out
    }

    /// The same facts as one JSON object on one line, without the final newline.
    pub fn to_json(&self, name: &str) -> String {
        #[derive(Serialize)]
        struct Screen<'a> {
            index: usize,
            change: &'static str,
            #[serde(skip_serializing_if = "Option::is_none")]
            detail: Option<&'a [String]>,
            #[serde(skip_serializing_if = "Option::is_none")]
            cells_changed: Option<usize>,
            #[serde(skip_serializing_if = "Option::is_none")]
            text: Option<usize>,
            #[serde(skip_serializing_if = "Option::is_none")]
            attrs: Option<usize>,
            #[serde(skip_serializing_if = "Option::is_none")]
            masks: Option<usize>,
            #[serde(skip_serializing_if = "Option::is_none")]
            asserts: Option<usize>,
        }
        #[derive(Serialize)]
        struct Stamp<'a> {
            from: &'a str,
            to: &'a str,
        }
        #[derive(Serialize)]
        struct Out<'a> {
            golden: &'a str,
            result: &'static str,
            count: usize,
            #[serde(skip_serializing_if = "Option::is_none")]
            stamp: Option<Stamp<'a>>,
            screens: Vec<Screen<'a>>,
            notes: &'a [String],
        }
        let screens = self
            .screens
            .iter()
            .enumerate()
            .filter_map(|(i, change)| {
                let index = i + 1;
                let base = Screen {
                    index,
                    change: "",
                    detail: None,
                    cells_changed: None,
                    text: None,
                    attrs: None,
                    masks: None,
                    asserts: None,
                };
                match change {
                    ScreenChange::Unchanged => None,
                    ScreenChange::Changed { parts, cells_changed, text, attrs } => Some(Screen {
                        change: "changed",
                        detail: Some(parts),
                        cells_changed: Some(*cells_changed),
                        text: Some(*text),
                        attrs: Some(*attrs),
                        ..base
                    }),
                    ScreenChange::New => Some(Screen { change: "new", ..base }),
                    ScreenChange::Dropped { masks, asserts } => {
                        Some(Screen { change: "dropped", masks: Some(*masks), asserts: Some(*asserts), ..base })
                    }
                }
            })
            .collect();
        let out = Out {
            golden: name,
            result: if self.created {
                "created"
            } else if self.unchanged {
                "unchanged"
            } else {
                "updated"
            },
            count: self.golden.screens.len(),
            stamp: self.stamp.as_ref().map(|(from, to)| Stamp { from, to }),
            screens,
            notes: &self.notes,
        };
        crate::ndjson::to_line(&out).expect("a summary is plain data")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::golden::Golden;

    fn snapshot(rows: &[&str], semantics: u32) -> Snapshot {
        let cols = rows.iter().map(|r| r.chars().count()).max().unwrap_or(1).max(1);
        let cells: Vec<String> = rows
            .iter()
            .map(|r| {
                let mut cells: Vec<String> = r.chars().map(|c| format!("{{\"ch\":\"{c}\"}}")).collect();
                cells.resize(cols, "{}".to_string());
                format!("[{}]", cells.join(","))
            })
            .collect();
        serde_json::from_str(&format!(
            "{{\"schema_version\":1,\"semantics_version\":{semantics},\"byte_offset\":1,\"cols\":{cols},\"rows\":{},\
             \"lines\":[],\"cells\":[{}],\"wrapped_rows\":[],\"cursor\":{{\"row\":0,\"col\":0,\"hidden\":false,\
             \"shape\":\"block\"}},\"modes\":[],\"alt_screen\":false,\"title\":null,\"scrollback\":[],\
             \"producer\":{{\"vtee\":\"9.9.9\",\"emulator\":\"emu 1\"}}}}",
            rows.len(),
            cells.join(",")
        ))
        .unwrap()
    }

    fn run(screens: &[&[&str]]) -> Run {
        Run {
            screens: screens.iter().map(|s| snapshot(s, 1)).collect(),
            names: vec![None; screens.len()],
            problems: Vec::new(),
        }
    }

    const HAND_WRITTEN: &str = "vtee-golden 1
schema 1
semantics 1
ignore title
# a note
x-producer vtee 0.1.0 alacritty_terminal 0.26.0

[masks]
# the clock
mask row=0 cols=3-4 text=/\\d\\d/

[screen 1]
size 5x1
cursor row=0 col=0 shape=block hidden=false
modes
alt_screen false
title null

[text]
0|ab 12

[masks]
mask row=0 text=ignore

[assert]
# keep me
assert row=0 text=\"ab 12\"

[screen 2]
size 5x1
cursor row=0 col=0 shape=block hidden=false
modes
alt_screen false
title null

[text]
0|zzzzz

[masks]
mask row=0 text=ignore
";

    fn parse(text: &str) -> Golden {
        Golden::parse_str(text).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn a_missing_golden_is_created_without_masks() {
        let update = plan_record(None, &run(&[&["ab 12"]]), RecordOptions::default());
        assert!(update.created);
        assert_eq!(update.verb(), "CREATED");
        assert_eq!(update.to_text("flow.golden"), "CREATED flow.golden: 1 screen\n");
        assert!(!update.golden.to_text().contains("[masks]"));
    }

    #[test]
    fn a_re_record_keeps_masks_assertions_comments_and_ignores_and_says_what_moved() {
        let existing = parse(HAND_WRITTEN);
        let update = plan_record(Some(&existing), &run(&[&["ab 13"]]), RecordOptions::default());
        let text = update.golden.to_text();
        assert!(text.contains("ignore title\n# a note\n"), "{text}");
        assert!(text.contains("# the clock\nmask row=0 cols=3-4"), "{text}");
        assert!(text.contains("# keep me\nassert row=0 text=\"ab 12\""), "{text}");
        assert!(text.contains("0|ab 13\n"), "{text}");
        assert!(text.contains("x-producer vtee 9.9.9 emu 1"), "{text}");
        assert_eq!(
            update.to_text("g.golden"),
            "UPDATED g.golden: 1 screen\n  screen 1: 1 cell changed (text 1, attrs 0)\n  screen 2: dropped (it had 1 mask, which is lost)\n"
        );
    }

    #[test]
    fn a_re_record_that_changes_nothing_leaves_the_producer_alone() {
        let existing = parse(HAND_WRITTEN);
        let update = plan_record(Some(&existing), &run(&[&["ab 12"], &["zzzzz"]]), RecordOptions::default());
        assert!(update.unchanged);
        assert_eq!(update.golden.to_text(), existing.to_text());
        assert!(update.golden.to_text().contains("x-producer vtee 0.1.0"));
        assert_eq!(update.to_text("g.golden"), "UNCHANGED g.golden: 2 screens\n");
    }

    #[test]
    fn a_new_screen_has_no_masks_and_a_changed_stamp_is_shown() {
        let existing = parse(HAND_WRITTEN);
        let mut r = run(&[&["ab 12"], &["zzzzz"], &["new"]]);
        for s in &mut r.screens {
            s.semantics_version = 2;
        }
        let update = plan_record(Some(&existing), &r, RecordOptions::default());
        let text = update.to_text("g.golden");
        assert!(
            text.starts_with("UPDATED g.golden: 3 screens\n  stamp: schema 1 semantics 1 -> schema 1 semantics 2\n"),
            "{text}"
        );
        assert!(text.contains("  screen 3: new (no masks, no asserts: add them by hand)\n"), "{text}");
        assert!(update.golden.to_text().contains("semantics 2\n"));
    }

    #[test]
    fn portable_does_not_touch_an_existing_file() {
        let existing = parse(HAND_WRITTEN);
        let update = plan_record(Some(&existing), &run(&[&["ab 12"], &["zzzzz"]]), RecordOptions { portable: true });
        assert!(update.unchanged);
        assert!(update.to_text("g.golden").contains("note: --portable only applies when a golden is created"));
    }

    #[test]
    fn one_screen_is_regenerated_alone() {
        let existing = parse(HAND_WRITTEN);
        let update = plan_screen(&existing, 2, &snapshot(&["yyyyy"], 1), None);
        let text = update.golden.to_text();
        assert!(text.contains("0|ab 12\n") && text.contains("0|yyyyy\n") && !text.contains("zzzzz"), "{text}");
        assert_eq!(
            update.to_text("g.golden"),
            "UPDATED g.golden: 2 screens\n  screen 2: 5 cells changed (text 5, attrs 0)\n"
        );
    }

    #[test]
    fn restamping_changes_the_stamp_and_the_producer_only() {
        let existing = parse(HAND_WRITTEN);
        let out = restamped(&existing, 2, Some(&snapshot(&["ab 12"], 2)));
        let was = existing.to_text();
        let now = out.to_text();
        let differing: Vec<_> = was.lines().zip(now.lines()).filter(|(a, b)| a != b).collect();
        assert_eq!(
            differing,
            [
                ("semantics 1", "semantics 2"),
                ("x-producer vtee 0.1.0 alacritty_terminal 0.26.0", "x-producer vtee 9.9.9 emu 1")
            ]
        );
        assert_eq!(was.lines().count(), now.lines().count());
    }

    #[test]
    fn the_json_summary_lists_only_what_moved() {
        let existing = parse(HAND_WRITTEN);
        let update = plan_record(Some(&existing), &run(&[&["ab 13"]]), RecordOptions::default());
        assert_eq!(
            update.to_json("g.golden"),
            "{\"golden\":\"g.golden\",\"result\":\"updated\",\"count\":1,\"screens\":[{\"index\":1,\"change\":\"changed\",\
             \"detail\":[\"1 cell changed (text 1, attrs 0)\"],\"cells_changed\":1,\"text\":1,\"attrs\":0},{\"index\":2,\
             \"change\":\"dropped\",\"masks\":1,\"asserts\":0}],\"notes\":[]}"
        );
    }
}
