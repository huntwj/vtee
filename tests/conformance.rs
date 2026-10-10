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
