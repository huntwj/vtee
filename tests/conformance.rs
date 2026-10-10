//! Conformance test (`docs/design/snapshot-and-golden-contract.md`, section 5.4): "did vtee change?".
//!
//! The differential test asks whether vtee is right by comparing it with two other emulators. This one
//! asks whether vtee's own output moved. Each of the 80 escape-sequence cases in
//! `tests/fixtures/differential/cases.json` has an expected snapshot in
//! `tests/fixtures/conformance/expected/<case>.json`: vtee's output for that input, in compact form without
//! `byte_offset` and `producer`, which are not part of what a snapshot means. They are compared as parsed
//! values, so key order and whitespace never matter.
//!
//! These files are frozen reference data. Changing one changes what a stored golden means, so it needs a
//! `SEMANTICS_VERSION` bump and an entry in `docs/guide/snapshot-semantics.md` (`docs/guide/versioning.md`).
//! Adding a case is not a bump. After adding a case, or after a deliberate change, regenerate with
//! `VTEE_BLESS_CONFORMANCE=1 cargo test --test conformance` on Linux (the resize cases need a live pty and run
//! on Unix only, so a run on Windows leaves their files alone).

use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use vtee::Snapshot;

mod common;
#[cfg(unix)]
use common::run_vtee_resized;
use common::{decode, run_vtee};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn expected_dir() -> PathBuf {
    root().join("conformance/expected")
}

fn blessing() -> bool {
    std::env::var_os("VTEE_BLESS_CONFORMANCE").is_some()
}

fn cases() -> Vec<Value> {
    let path = root().join("differential/cases.json");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str::<Value>(&text).unwrap().as_array().unwrap().clone()
}

/// A snapshot as the expected files hold it: everything except `byte_offset` and `producer`.
fn canonical(snap: &Snapshot) -> Value {
    let mut v = serde_json::to_value(snap).unwrap();
    let object = v.as_object_mut().unwrap();
    object.remove("byte_offset");
    object.remove("producer");
    v
}

/// Where two snapshots first differ, in words a person can act on.
fn first_difference(expected: &Value, actual: &Value) -> String {
    let (e, a) = (expected.as_object().unwrap(), actual.as_object().unwrap());
    for key in e.keys().chain(a.keys()).collect::<BTreeSet<_>>() {
        if e.get(key) == a.get(key) {
            continue;
        }
        if key == "cells" {
            let (er, ar) = (e[key].as_array().unwrap(), a[key].as_array().unwrap());
            for (r, (erow, arow)) in er.iter().zip(ar).enumerate() {
                for (c, (ec, ac)) in erow.as_array().unwrap().iter().zip(arow.as_array().unwrap()).enumerate() {
                    if ec != ac {
                        return format!("cells: row {r}, col {c}: expected {ec}, got {ac}");
                    }
                }
            }
        }
        return format!(
            "{key}: expected {}, got {}",
            e.get(key).unwrap_or(&Value::Null),
            a.get(key).unwrap_or(&Value::Null)
        );
    }
    "no difference".to_string()
}

#[test]
fn vtees_own_output_for_the_cases_has_not_changed() {
    let dir = std::env::temp_dir().join(format!("vtee-conformance-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    fs::create_dir_all(expected_dir()).unwrap();

    let mut failures = Vec::new();
    for case in cases() {
        let name = case["name"].as_str().unwrap();
        let cols = case["cols"].as_u64().unwrap() as u16;
        let rows = case["rows"].as_u64().unwrap() as u16;
        let input = decode(case["input"].as_str().unwrap());
        let resize = case.get("resize").map(|r| (r[0].as_u64().unwrap() as u16, r[1].as_u64().unwrap() as u16));
        let snap = match resize {
            None => run_vtee(&dir, name, cols, rows, &input),
            #[cfg(unix)]
            Some(to) => run_vtee_resized(cols, rows, &input, to),
            #[cfg(not(unix))]
            Some(_) => continue, // live resize is covered on Unix only
        };
        let actual = canonical(&snap);
        let path = expected_dir().join(format!("{name}.json"));
        if blessing() {
            fs::write(&path, format!("{}\n", serde_json::to_string(&actual).unwrap())).unwrap();
            continue;
        }
        match fs::read_to_string(&path) {
            Err(_) => failures.push(format!(
                "{name}: no expected file. A new case is not a semantics change; run with VTEE_BLESS_CONFORMANCE=1."
            )),
            Ok(text) => {
                let expected: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                if expected != actual {
                    failures.push(format!("{name}: {}", first_difference(&expected, &actual)));
                }
            }
        }
    }
    let _ = fs::remove_dir_all(&dir);
    assert!(
        failures.is_empty(),
        "vtee's output changed for {} case(s). If the change is deliberate it is a semantics change: bump \
         SEMANTICS_VERSION, add an entry to docs/guide/snapshot-semantics.md, regenerate with \
         VTEE_BLESS_CONFORMANCE=1 and commit with the `snapshot` scope.\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}

#[test]
fn every_expected_file_belongs_to_a_case_and_every_case_has_one() {
    let names: BTreeSet<String> = cases().iter().map(|c| c["name"].as_str().unwrap().to_string()).collect();
    assert_eq!(names.len(), 80, "the conformance set is the 80 escape-sequence cases");
    let files: BTreeSet<String> = fs::read_dir(expected_dir())
        .map(|entries| {
            entries
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .filter_map(|f| f.strip_suffix(".json").map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(files, names, "tests/fixtures/conformance/expected/ and cases.json must list the same cases");
}

/// The width sample: how many cells each of 1,062 scalars occupies. The 80 cases hold almost no Unicode data, so an
/// upgrade of `unicode-width`, `vte` or `alacritty_terminal` that changed a width would pass them and silently
/// change what stored goldens mean. Run-length lines: `U+0020..U+007E 1` is every scalar in the range occupying one
/// cell, `0` is zero-width (it joins the previous cell), `2` is wide.
fn width_sample() -> Vec<(char, usize)> {
    let path = root().join("conformance/width-sample.txt");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let scalar = |s: &str| char::from_u32(u32::from_str_radix(s.strip_prefix("U+").unwrap(), 16).unwrap()).unwrap();
    let mut out = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let (range, class) = line.split_once(' ').unwrap_or_else(|| panic!("bad width sample line {line:?}"));
        let class: usize = class.trim().parse().unwrap();
        let (first, last) = range.split_once("..").map_or((range, range), |(a, b)| (a, b));
        out.extend((scalar(first) as u32..=scalar(last) as u32).map(|n| (char::from_u32(n).unwrap(), class)));
    }
    out
}

/// Writes `A<scalar>œ` on one row per scalar through the real emulator and reads the column of `œ`: column 1 means
/// the scalar joined the `A` (width 0), 2 means one cell, 3 means two.
#[test]
fn the_width_sample_is_what_the_emulator_measures() {
    let sample = width_sample();
    assert_eq!(sample.len(), 1062, "the width sample holds 1,062 scalars");

    let input: String = sample.iter().map(|(ch, _)| format!("A{ch}\u{153}\r\n")).collect();
    let rows = sample.len() as u16 + 1;
    let dir = std::env::temp_dir().join(format!("vtee-width-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let snap = run_vtee(&dir, "width", 12, rows, &input);
    let _ = fs::remove_dir_all(&dir);

    let mut wrong = Vec::new();
    for (row, (ch, class)) in snap.cells.iter().zip(&sample) {
        let measured = row.iter().position(|c| c.ch.contains('\u{153}')).map(|col| col - 1);
        if measured != Some(*class) {
            wrong.push(format!("U+{:04X}: sample says {class}, the emulator gives {measured:?}", *ch as u32));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} scalar(s) changed width. That changes what stored goldens mean: bump SEMANTICS_VERSION and add an \
         entry to docs/guide/snapshot-semantics.md.\n  {}",
        wrong.len(),
        wrong.iter().take(20).cloned().collect::<Vec<_>>().join("\n  ")
    );
}

// ---- the reported vocabulary ---------------------------------------------------------------------------------

/// Everything a snapshot can report, measured from the real emulator rather than listed by hand: the field names,
/// the mode tokens, the attribute names, the values of the enumerations and the colour forms. A new name is a
/// `SEMANTICS_VERSION` bump (design section 4.1), because it changes what a stored golden finds. The frozen list is
/// `tests/fixtures/conformance/vocabulary.txt`; a name that appears or disappears fails the test.
mod vocabulary {
    use super::*;
    use std::collections::BTreeMap;

    struct Prober {
        dir: PathBuf,
    }

    impl Prober {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("vtee-vocabulary-{}", std::process::id()));
            fs::create_dir_all(&dir).unwrap();
            Prober { dir }
        }

        /// The snapshot of a 10 by 2 screen after `input`, as the JSON a client would see.
        fn json(&self, input: &str) -> Value {
            serde_json::to_value(run_vtee(&self.dir, "probe", 10, 2, input)).unwrap()
        }
    }

    impl Drop for Prober {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn keys(v: &Value) -> BTreeSet<String> {
        v.as_object().unwrap().keys().cloned().collect()
    }

    /// Private modes by number, then names alphabetically: the order `modes` is written in.
    fn mode_order(token: &str) -> (bool, u32, String) {
        match token.strip_prefix('?').and_then(|n| n.parse::<u32>().ok()) {
            Some(n) => (false, n, String::new()),
            None => (true, 0, token.to_string()),
        }
    }

    pub fn measure() -> BTreeMap<&'static str, Vec<String>> {
        let p = Prober::new();
        let base = p.json("");
        let mut cell_fields = BTreeSet::new();
        let mut colours = BTreeSet::new();
        let mut underline = BTreeSet::new();
        let mut shapes = BTreeSet::new();
        let mut modes = BTreeSet::new();
        let mut idx_range: Option<(u32, u32)> = None;

        let mut note_cells = |snap: &Value| {
            for row in snap["cells"].as_array().unwrap() {
                for cell in row.as_array().unwrap() {
                    let cell = cell.as_object().unwrap();
                    cell_fields.extend(cell.keys().cloned());
                    for key in ["fg", "bg", "underline_color"] {
                        if let Some(c) = cell.get(key).and_then(Value::as_str) {
                            let kind = match c.split_once(':') {
                                Some(("idx", n)) => {
                                    let n: u32 = n.parse().unwrap();
                                    let (lo, hi) = idx_range.unwrap_or((n, n));
                                    idx_range = Some((lo.min(n), hi.max(n)));
                                    "idx".to_string()
                                }
                                Some(("rgb", _)) => "rgb:#rrggbb".to_string(),
                                _ => c.to_string(),
                            };
                            colours.insert(kind);
                        }
                    }
                    if let Some(u) = cell.get("underline").and_then(Value::as_str) {
                        underline.insert(u.to_string());
                    }
                }
            }
        };

        // Every SGR parameter on its own, the colour forms, the underline sub-parameters and a hyperlink.
        for n in 0..=110 {
            note_cells(&p.json(&format!("\x1b[{n}mX")));
        }
        for n in 0..=255 {
            note_cells(&p.json(&format!("\x1b[38;5;{n}m\x1b[48;5;{n}m\x1b[58;5;{n}m\x1b[4mX")));
        }
        for sgr in
            ["38;2;1;2;3", "48;2;250;251;252", "58;2;9;8;7", "4:0", "4:1", "4:2", "4:3", "4:4", "4:5", "4:9", "21"]
        {
            note_cells(&p.json(&format!("\x1b[{sgr}mX")));
        }
        note_cells(&p.json("\x1b]8;;http://example.com\x07X\x1b]8;;\x07"));

        for n in 0..=9 {
            let cursor = &p.json(&format!("\x1b[{n} q"))["cursor"];
            shapes.insert(cursor["shape"].as_str().unwrap().to_string());
        }
        let mut tokens = |input: &str| {
            for m in p.json(input)["modes"].as_array().unwrap() {
                modes.insert(m.as_str().unwrap().to_string());
            }
        };
        tokens("\x1b=");
        tokens("\x1b>");
        for n in (1..=1100).chain(2000..=2100) {
            tokens(&format!("\x1b[?{n}h"));
        }

        let mut out = BTreeMap::new();
        out.insert("snapshot-fields", keys(&base).into_iter().collect());
        out.insert("cursor-fields", keys(&base["cursor"]).into_iter().collect());
        out.insert("cell-fields", cell_fields.into_iter().collect());
        let mut modes: Vec<String> = modes.into_iter().collect();
        modes.sort_by_key(|token| mode_order(token));
        out.insert("modes", modes);
        out.insert("underline-values", underline.into_iter().filter(|u| u != "none").collect());
        out.insert("cursor-shapes", shapes.into_iter().collect());
        // A default colour is never written (an absent field is the default), so it is listed unconditionally.
        colours.insert("default".to_string());
        let mut forms: Vec<String> = colours.into_iter().filter(|c| c != "idx").collect();
        let (lo, hi) = idx_range.expect("no indexed colour was reported");
        forms.push(format!("idx:{lo}..{hi}"));
        forms.sort();
        out.insert("colour-forms", forms);
        out
    }

    pub fn render(measured: &BTreeMap<&'static str, Vec<String>>) -> String {
        let mut text = String::from(
            "# The reported vocabulary at semantics 1 (design section 3.8), measured from the emulator.\n\
             # A name that appears or disappears is a SEMANTICS_VERSION bump; see tests/conformance.rs.\n",
        );
        for (kind, names) in measured {
            text.push_str(&format!("{kind}: {}\n", names.join(" ")));
        }
        text
    }
}

#[test]
fn the_reported_vocabulary_has_not_changed() {
    let measured = vocabulary::measure();
    let text = vocabulary::render(&measured);
    let path = root().join("conformance/vocabulary.txt");
    if blessing() {
        fs::write(&path, &text).unwrap();
        return;
    }
    let frozen = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())).replace("\r\n", "\n");
    assert!(
        frozen == text,
        "the reported vocabulary changed. A new or removed name changes what a stored golden finds, so it is a \
         semantics change: bump SEMANTICS_VERSION, add an entry to docs/guide/snapshot-semantics.md, regenerate with \
         VTEE_BLESS_CONFORMANCE=1 and commit with the `snapshot` scope.\n--- frozen\n{frozen}--- measured\n{text}"
    );
}
