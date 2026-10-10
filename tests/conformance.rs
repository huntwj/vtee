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
//! Adding a case is not a bump. After adding a case, or after a deliberate change, regenerate the files with
//! `cargo test --test conformance -- --ignored bless` on Linux (the resize cases need a live pty and run on Unix
//! only, which is why blessing is Unix only). The bless step rewrites the expected files, the vocabulary and the
//! manifest that guards them, and refuses to change a frozen file unless `SEMANTICS_VERSION` was bumped first.

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

/// Runs every case this platform can run and hands vtee's canonical output to `each`.
fn for_each_case(mut each: impl FnMut(&str, Value)) {
    let dir = std::env::temp_dir().join(format!("vtee-conformance-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
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
        each(name, canonical(&snap));
    }
    let _ = fs::remove_dir_all(&dir);
}

const CHANGED: &str = concat!(
    "If the change is deliberate it is a semantics change: bump SEMANTICS_VERSION in src/snapshot.rs, ",
    "add an entry to docs/guide/snapshot-semantics.md, run `cargo test --test conformance -- --ignored bless` ",
    "on Linux and commit with the `snapshot` scope (docs/guide/versioning.md)."
);

#[test]
fn vtees_own_output_for_the_cases_has_not_changed() {
    let mut failures = Vec::new();
    for_each_case(|name, actual| {
        let path = expected_dir().join(format!("{name}.json"));
        match fs::read_to_string(&path) {
            Err(_) => failures
                .push(format!("{name}: no expected file. A new case is not a semantics change; run the bless step.")),
            Ok(text) => {
                let expected: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                if expected != actual {
                    failures.push(format!("{name}: {}", first_difference(&expected, &actual)));
                }
            }
        }
    });
    assert!(
        failures.is_empty(),
        "vtee's output changed for {} case(s). {CHANGED}\n  {}",
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
        "{} scalar(s) changed width. That changes what stored goldens mean. {CHANGED}\n  {}",
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
    let text = vocabulary::render(&vocabulary::measure());
    let frozen = read_frozen("vocabulary.txt");
    assert!(
        frozen == text,
        "the reported vocabulary changed. A new or removed name changes what a stored golden finds. {CHANGED}\n\
         --- frozen\n{frozen}--- measured\n{text}"
    );
}

fn read_frozen(name: &str) -> String {
    let path = root().join("conformance").join(name);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())).replace("\r\n", "\n")
}

// ---- the manifest that guards the frozen files -----------------------------------------------------------------

/// `tests/fixtures/conformance/MANIFEST` records the `SEMANTICS_VERSION` the frozen files were written under and a
/// hash of each one. The files cannot change unnoticed: a change makes the hashes disagree, and the only way to make
/// them agree again is the bless step, which refuses to rewrite a frozen file until `SEMANTICS_VERSION` was bumped.
/// The hash is FNV-1a 64: it only has to notice a change, and it keeps the test free of a dependency.
mod manifest {
    use super::*;
    use std::collections::BTreeMap;

    pub fn path() -> PathBuf {
        root().join("conformance/MANIFEST")
    }

    fn fnv1a64(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
    }

    /// The hash of a file's text, whatever its line endings.
    pub fn hash(text: &str) -> String {
        format!("{:016x}", fnv1a64(text.replace("\r\n", "\n").as_bytes()))
    }

    /// Every frozen file under `tests/fixtures/conformance`, by path relative to it, with the hash of its text.
    pub fn on_disk() -> BTreeMap<String, String> {
        fn walk(dir: &Path, base: &Path, out: &mut BTreeMap<String, String>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, base, out);
                    continue;
                }
                let relative = path.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
                let text = fs::read_to_string(&path).unwrap();
                if relative == "width-sample.txt" {
                    // One entry per range line, so adding a scalar on a line of its own is an addition, which is
                    // not a semantics change, while changing a recorded range or its width class is.
                    for line in text.lines().filter(|l| !l.trim().is_empty()) {
                        let (range, _) = line.split_once(' ').expect("bad width sample line");
                        out.insert(format!("{relative}#{range}"), hash(line));
                    }
                } else if relative != "MANIFEST" {
                    out.insert(relative, hash(&text));
                }
            }
        }
        let base = root().join("conformance");
        let mut out = BTreeMap::new();
        walk(&base, &base, &mut out);
        out
    }

    /// The recorded semantics version and hashes, or `None` before the first bless.
    pub fn recorded() -> Option<(u32, BTreeMap<String, String>)> {
        let text = fs::read_to_string(path()).ok()?;
        let mut version = None;
        let mut files = BTreeMap::new();
        for line in text.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
            match line.split_once(' ') {
                Some(("semantics_version", n)) => version = Some(n.trim().parse().expect("bad semantics_version")),
                Some((hash, file)) => {
                    files.insert(file.trim().to_string(), hash.to_string());
                }
                None => panic!("bad MANIFEST line {line:?}"),
            }
        }
        Some((version.expect("MANIFEST has no semantics_version line"), files))
    }

    #[cfg(unix)]
    pub fn render(version: u32, files: &BTreeMap<String, String>) -> String {
        let mut text = String::from(concat!(
            "# The frozen conformance files and the SEMANTICS_VERSION they were written under (docs/guide/versioning.md).\n",
            "# Do not edit by hand: `cargo test --test conformance -- --ignored bless` rewrites it.\n",
        ));
        text.push_str(&format!("semantics_version {version}\n"));
        for (file, hash) in files {
            text.push_str(&format!("{hash} {file}\n"));
        }
        text
    }
}

#[test]
fn the_frozen_files_match_the_manifest() {
    let (recorded_version, recorded) = manifest::recorded().expect("tests/fixtures/conformance/MANIFEST is missing");
    let actual = manifest::on_disk();
    let changed: Vec<_> =
        recorded.iter().filter(|(file, hash)| actual.get(*file).is_some_and(|h| h != *hash)).map(|(f, _)| f).collect();
    let removed: Vec<_> = recorded.keys().filter(|file| !actual.contains_key(*file)).collect();
    let added: Vec<_> = actual.keys().filter(|file| !recorded.contains_key(*file)).collect();

    assert!(
        changed.is_empty() && removed.is_empty(),
        "frozen conformance files changed (changed: {changed:?}, removed: {removed:?}). They define what a stored \
         golden means, so changing them needs a SEMANTICS_VERSION bump. {CHANGED}"
    );
    assert!(
        added.is_empty(),
        "new conformance files {added:?} are not in the MANIFEST. Adding a case or a width sample scalar is not a \
         semantics change: run `cargo test --test conformance -- --ignored bless` on Linux and commit the MANIFEST."
    );
    assert_eq!(
        recorded_version,
        vtee::SEMANTICS_VERSION,
        "SEMANTICS_VERSION is {} but the MANIFEST was written under {recorded_version}. After a bump, run the bless \
         step so the frozen files and the MANIFEST are rewritten under the new number.",
        vtee::SEMANTICS_VERSION
    );
}

#[test]
fn the_semantics_changelog_has_an_entry_for_this_version() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/guide/snapshot-semantics.md");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let heading = format!("## Semantics {}", vtee::SEMANTICS_VERSION);
    assert!(
        text.lines().any(|l| l == heading || l.starts_with(&format!("{heading} "))),
        "docs/guide/snapshot-semantics.md has no `{heading}` entry. Every SEMANTICS_VERSION bump is recorded there: \
         the number, the release, the input that changed, the old and new cells and the cases that moved."
    );
}

/// Regenerates the frozen files this test owns and the MANIFEST. Run it by hand after adding a case or after a
/// deliberate change: `cargo test --test conformance -- --ignored bless`, on Linux so the resize cases are included.
/// It refuses to change a file the MANIFEST already records unless `SEMANTICS_VERSION` differs from the recorded one.
#[cfg(unix)]
#[test]
#[ignore = "rewrites the frozen conformance files; run by hand"]
fn bless() {
    let mut new_files: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for_each_case(|name, actual| {
        new_files.insert(format!("expected/{name}.json"), format!("{}\n", serde_json::to_string(&actual).unwrap()));
    });
    new_files.insert("vocabulary.txt".to_string(), vocabulary::render(&vocabulary::measure()));

    if let Some((recorded_version, recorded)) = manifest::recorded() {
        let changed: Vec<_> = new_files
            .iter()
            .filter(|(file, text)| recorded.get(*file).is_some_and(|h| *h != manifest::hash(text)))
            .map(|(file, _)| file.as_str())
            .collect();
        assert!(
            changed.is_empty() || recorded_version != vtee::SEMANTICS_VERSION,
            "refusing to rewrite frozen files {changed:?}: SEMANTICS_VERSION is still {recorded_version}. {CHANGED}"
        );
    }

    let base = root().join("conformance");
    for (file, text) in &new_files {
        let path = base.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    let hashes = manifest::on_disk();
    fs::write(manifest::path(), manifest::render(vtee::SEMANTICS_VERSION, &hashes)).unwrap();
}
