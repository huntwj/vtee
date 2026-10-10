//! The golden test vectors (design appendix A): golden, saved run, options, and the exact verdict, for a second
//! implementation of the checker to prove it agrees. The files under `tests/fixtures/golden-vectors/` are the frozen
//! data; this test runs every one of them through the reference implementation.
//!
//! `cargo test --test golden_vectors -- --ignored bless` regenerates the files from `golden_vectors/defs.rs` and
//! `golden_vectors/more.rs` (each definition also states the verdict it must produce, so a regeneration cannot quietly
//! write down a wrong answer). Changing a frozen vector is a `SEMANTICS_VERSION` change (`docs/guide/versioning.md`).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use vtee::golden::{
    CheckError, CheckOptions, Golden, Invocation, LoadError, check_saved, escape, pattern::Pattern, refuse_unread,
};

mod defs;
mod more;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/golden-vectors")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// What running one vector produces.
enum Outcome {
    Report { exit: i32, text: String, json: String },
    Error { exit: i32, message: String },
}

fn parse_args(args: &[String]) -> CheckOptions {
    let mut options = CheckOptions::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--screen" => {
                options.compare.screen = Some(args[i + 1].parse().expect("--screen N"));
                i += 2;
            }
            "--fail-fast" => {
                options.compare.fail_fast = true;
                i += 1;
            }
            "--max-mismatches" => {
                options.max_mismatches = args[i + 1].parse().expect("--max-mismatches N");
                i += 2;
            }
            other => panic!("the vector harness does not know the option {other}"),
        }
    }
    options
}

fn report_outcome(report: vtee::golden::Report) -> Outcome {
    Outcome::Report { exit: report.exit_code(), text: report.to_text(), json: report.to_json() }
}

/// What the runner does with a vector: read the golden, read the saved run, compare.
fn run_vector(golden: &[u8], saved: &str, args: &[String]) -> Outcome {
    let options = parse_args(args);
    let command = format!(
        "vtee --expect golden.golden --snapshot run.ndjson{}",
        args.iter().map(|a| format!(" {a}")).collect::<String>()
    );
    let invocation = Invocation { golden: "golden.golden".to_string(), command, tail: String::new() };
    let golden = match Golden::parse(golden) {
        Ok(g) => g,
        Err(LoadError::Malformed(e)) => return Outcome::Error { exit: 65, message: format!("golden.golden: {e}") },
        Err(LoadError::Refused(refusal)) => return report_outcome(refuse_unread(&invocation, refusal, &options)),
    };
    match check_saved(&golden, &invocation, saved, &options) {
        Ok(report) => report_outcome(report),
        Err(CheckError::Run(message)) => Outcome::Error { exit: 65, message },
        Err(CheckError::Usage(usage)) => Outcome::Error { exit: 64, message: format!("vtee: {usage}") },
    }
}

fn lines(path: &Path) -> Vec<String> {
    read(path).lines().map(str::to_string).collect()
}

// ---- verifying the files -----------------------------------------------------------------------------------------

fn dirs(parent: &str) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(root().join(parent))
        .unwrap_or_else(|e| panic!("tests/fixtures/golden-vectors/{parent}: {e}"))
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    found.sort();
    found
}

#[test]
fn every_run_vector_gives_its_recorded_verdict() {
    let vectors = dirs("runs");
    assert!(vectors.len() >= 60, "only {} run vectors", vectors.len());
    for dir in vectors {
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        let golden = fs::read(dir.join("golden.golden")).unwrap();
        let saved = fs::read_to_string(dir.join("run.ndjson")).unwrap();
        let args = lines(&dir.join("args"));
        let exit: i32 = read(&dir.join("exit")).trim().parse().unwrap();
        match run_vector(&golden, &saved, &args) {
            Outcome::Report { exit: got, text, json } => {
                assert_eq!(got, exit, "{name}: exit code\n{text}");
                assert_eq!(text, read(&dir.join("expected.txt")), "{name}: text report");
                assert_eq!(format!("{json}\n"), read(&dir.join("expected.json")), "{name}: JSON report");
                assert!(!dir.join("error").exists(), "{name}: has an error file but produced a report");
            }
            Outcome::Error { exit: got, message } => {
                assert_eq!(got, exit, "{name}: exit code\n{message}");
                assert_eq!(message, read(&dir.join("error")).trim_end_matches('\n'), "{name}: error message");
            }
        }
    }
}

/// The command line of a run vector: `--expect golden.golden --snapshot run.ndjson` and the vector's own options, run
/// in the vector's directory so the golden's name in the report is the vector's.
fn vtee_on(dir: &Path, extra: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_vtee"))
        .current_dir(dir)
        .env_remove("CI")
        .env_remove("FORCE_COLOR")
        .env("NO_COLOR", "1")
        .args(["--expect", "golden.golden", "--snapshot", "run.ndjson"])
        .args(extra)
        .output()
        .unwrap()
}

/// The same vectors, through the command: the options, the report on standard output, the message on standard error
/// and the exit code are what a second implementation of the runner has to match.
#[test]
fn every_run_vector_gives_its_verdict_through_the_command() {
    for dir in dirs("runs") {
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        let args = lines(&dir.join("args"));
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let exit: i32 = read(&dir.join("exit")).trim().parse().unwrap();

        let text = vtee_on(&dir, &args);
        assert_eq!(
            text.status.code(),
            Some(exit),
            "{name}: exit code
{}",
            String::from_utf8_lossy(&text.stderr)
        );
        if dir.join("expected.txt").exists() {
            assert_eq!(String::from_utf8_lossy(&text.stdout), read(&dir.join("expected.txt")), "{name}: text report");
            assert!(text.stderr.is_empty(), "{name}: standard error is not empty");
            let mut json_args = args.clone();
            json_args.extend(["--format", "json"]);
            let json = vtee_on(&dir, &json_args);
            assert_eq!(json.status.code(), Some(exit), "{name}: exit code with --format json");
            // A refusal suggests the command that was typed, which here includes `--format json`.
            let json_out = String::from_utf8_lossy(&json.stdout).replace(" --format json", "");
            assert_eq!(json_out, read(&dir.join("expected.json")), "{name}: JSON report");
        } else {
            let want = read(&dir.join("error"));
            assert_eq!(String::from_utf8_lossy(&text.stderr).trim_end(), want.trim_end(), "{name}: message");
            assert!(text.stdout.is_empty(), "{name}: standard output is not empty");
        }
    }
}

#[test]
fn every_reject_vector_is_malformed() {
    let mut count = 0;
    for entry in fs::read_dir(root().join("reject")).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let bytes = fs::read(&path).unwrap();
        match Golden::parse(&bytes) {
            Err(LoadError::Malformed(e)) => assert!(!e.message.is_empty(), "{name}"),
            other => panic!("{name} should be malformed, got {other:?}"),
        }
        count += 1;
    }
    assert!(count >= 70, "only {count} reject vectors");
}

#[test]
fn every_canonical_golden_reads_and_writes_back_unchanged() {
    for entry in fs::read_dir(root().join("roundtrip")).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let golden = Golden::parse(&fs::read(&path).unwrap()).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(golden.to_text(), read(&path), "{name}");
    }
}

/// `\u{H}` is the only escape in a vector's subject.
fn subject(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if let Some(after) = rest.strip_prefix("\\u{") {
            let end = after.find('}').expect("unterminated \\u{ in a regex vector");
            out.push(char::from_u32(u32::from_str_radix(&after[..end], 16).unwrap()).unwrap());
            rest = &after[end + 1..];
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

fn span(range: (usize, usize)) -> String {
    format!("[{},{})", range.0, range.1)
}

#[test]
fn the_regex_dialect_gives_the_recorded_results() {
    let table = read(&root().join("regex/vectors.tsv"));
    let mut count = 0;
    for (n, row) in table.lines().enumerate().filter(|(_, l)| !l.starts_with('#') && !l.is_empty()) {
        let cols: Vec<&str> = row.split('\t').collect();
        assert_eq!(cols.len(), 4, "regex/vectors.tsv line {}: expected 4 tab-separated columns", n + 1);
        let (pattern, subject_text, operation, expected) = (cols[0], subject(cols[1]), cols[2], cols[3]);
        let here = format!("regex/vectors.tsv line {}: {row:?}", n + 1);
        let got = match operation {
            "reject" => Pattern::new(pattern).map_or("reject".to_string(), |_| "accepted".to_string()),
            "line" => {
                let golden = more::BASE.replacen("mask row=0 cols=3-4 text=/[0-9]+/", pattern, 1);
                match Golden::parse_str(&golden) {
                    Err(LoadError::Malformed(_)) => "reject".to_string(),
                    _ => "accepted".to_string(),
                }
            }
            "search" => Pattern::new(pattern)
                .unwrap_or_else(|e| panic!("{here}: {e}"))
                .search(&subject_text)
                .map_or("no match".to_string(), |m| format!("match {}", span(m))),
            "full" => {
                let p = Pattern::new(pattern).unwrap_or_else(|e| panic!("{here}: {e}"));
                if p.is_full_match(&subject_text) { "match" } else { "no match" }.to_string()
            }
            "find" => {
                let found = Pattern::new(pattern).unwrap_or_else(|e| panic!("{here}: {e}")).find_all(&subject_text);
                if found.is_empty() {
                    "none".to_string()
                } else {
                    found.into_iter().map(span).collect::<Vec<_>>().join(" ")
                }
            }
            other => panic!("{here}: unknown operation {other}"),
        };
        assert_eq!(got, expected, "{here}");
        count += 1;
    }
    assert!(count >= 46, "only {count} regex vectors");
}

#[test]
fn the_escape_set_file_is_the_frozen_data() {
    let text = read(&root().join("escapes/escape-set.txt"));
    let ranges = text.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).count();
    assert_eq!(ranges, 876);
    // A few scalars on each side of a boundary.
    for (c, escaped) in
        [('\u{1f}', true), (' ', false), ('~', false), ('\u{7f}', true), ('\u{ad}', true), ('\u{2800}', true)]
    {
        assert_eq!(escape::needs_escape(c), escaped, "U+{:X}", c as u32);
    }
}

// ---- writing the files -------------------------------------------------------------------------------------------

// ---- the manifest that guards the frozen files ------------------------------------------------------------------

/// `tests/fixtures/golden-vectors/MANIFEST` records the `SEMANTICS_VERSION` the vectors were written under and a hash
/// of each file. A vector cannot change unnoticed: the hashes would disagree, and the only way to make them agree again
/// is the bless step, which refuses to change a recorded file until `SEMANTICS_VERSION` was bumped. Adding a vector is
/// not a semantics change. The hash is FNV-1a 64 over the bytes (the files are byte-exact data, line endings included).
mod manifest {
    use super::*;

    pub fn path() -> PathBuf {
        root().join("MANIFEST")
    }

    pub fn hash(bytes: &[u8]) -> String {
        let h = bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3));
        format!("{h:016x}")
    }

    pub fn on_disk() -> BTreeMap<String, String> {
        fn walk(dir: &Path, base: &Path, out: &mut BTreeMap<String, String>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, base, out);
                } else if path.file_name().is_some_and(|n| n != "MANIFEST" && n != "README.md") {
                    let relative = path.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
                    out.insert(relative, hash(&fs::read(&path).unwrap()));
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(&root(), &root(), &mut out);
        out
    }

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

    pub fn render(version: u32, files: &BTreeMap<String, String>) -> String {
        let mut text = String::from(concat!(
            "# The frozen golden vectors and the SEMANTICS_VERSION they were written under (docs/guide/versioning.md).\n",
            "# Do not edit by hand: `cargo test --test golden_vectors -- --ignored bless` rewrites it.\n",
        ));
        text.push_str(&format!("semantics_version {version}\n"));
        for (file, hash) in files {
            text.push_str(&format!("{hash} {file}\n"));
        }
        text
    }
}

#[test]
fn the_frozen_vectors_match_the_manifest() {
    let (recorded_version, recorded) = manifest::recorded().expect("tests/fixtures/golden-vectors/MANIFEST is missing");
    let actual = manifest::on_disk();
    let changed: Vec<_> =
        recorded.iter().filter(|(file, hash)| actual.get(*file).is_some_and(|h| h != *hash)).map(|(f, _)| f).collect();
    let removed: Vec<_> = recorded.keys().filter(|file| !actual.contains_key(*file)).collect();
    let added: Vec<_> = actual.keys().filter(|file| !recorded.contains_key(*file)).collect();
    assert!(
        changed.is_empty() && removed.is_empty(),
        "frozen golden vectors changed (changed: {changed:?}, removed: {removed:?}). They define what a stored golden \
         means, so changing them needs a SEMANTICS_VERSION bump (docs/guide/versioning.md)."
    );
    assert!(
        added.is_empty(),
        "new vectors {added:?} are not in the MANIFEST. Adding a vector is not a semantics change: run \
         `cargo test --test golden_vectors -- --ignored bless` and commit the MANIFEST."
    );
    assert_eq!(
        recorded_version,
        vtee::SEMANTICS_VERSION,
        "SEMANTICS_VERSION is {} but the MANIFEST was written under {recorded_version}. After a bump, run the bless \
         step so the vectors and the MANIFEST are rewritten under the new number.",
        vtee::SEMANTICS_VERSION
    );
}

#[test]
#[ignore = "rewrites the frozen golden vectors; run by hand"]
fn bless() {
    use defs::{Expect, REGEX_VECTORS, run_vectors};

    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut write = |path: String, bytes: &[u8]| {
        files.insert(path, bytes.to_vec());
    };

    for vector in run_vectors() {
        let dir = format!("runs/{}", vector.dir);
        let args: Vec<String> = vector.args.iter().map(|a| a.to_string()).collect();
        let outcome = run_vector(&vector.golden, &vector.run, &args);
        write(format!("{dir}/golden.golden"), &vector.golden);
        write(format!("{dir}/run.ndjson"), vector.run.as_bytes());
        write(format!("{dir}/args"), args.iter().map(|a| format!("{a}\n")).collect::<String>().as_bytes());
        match (outcome, vector.expect) {
            (Outcome::Report { exit, text, json }, Expect::Report { exit: want, contains }) => {
                assert_eq!(exit, want, "{}: wrong exit code\n{text}", vector.dir);
                for needle in contains {
                    assert!(text.contains(needle), "{}: report lacks {needle:?}\n{text}", vector.dir);
                }
                write(format!("{dir}/exit"), format!("{exit}\n").as_bytes());
                write(format!("{dir}/expected.txt"), text.as_bytes());
                write(format!("{dir}/expected.json"), format!("{json}\n").as_bytes());
            }
            (Outcome::Error { exit, message }, Expect::Error { exit: want, contains }) => {
                assert_eq!(exit, want, "{}: wrong exit code\n{message}", vector.dir);
                assert!(message.contains(contains), "{}: message lacks {contains:?}\n{message}", vector.dir);
                write(format!("{dir}/exit"), format!("{exit}\n").as_bytes());
                write(format!("{dir}/error"), format!("{message}\n").as_bytes());
            }
            (Outcome::Report { text, .. }, Expect::Error { .. }) => {
                panic!("{}: expected an error, got\n{text}", vector.dir)
            }
            (Outcome::Error { message, .. }, Expect::Report { .. }) => {
                panic!("{}: expected a report, got\n{message}", vector.dir)
            }
        }
    }

    for (name, bytes) in more::rejects() {
        assert!(matches!(Golden::parse(&bytes), Err(LoadError::Malformed(_))), "reject/{name} must be malformed");
        write(format!("reject/{name}.golden"), &bytes);
    }
    for (name, text) in more::roundtrips() {
        let golden = Golden::parse_str(&text).unwrap_or_else(|e| panic!("roundtrip/{name}: {e}"));
        assert_eq!(golden.to_text(), text, "roundtrip/{name} is not canonical");
        write(format!("roundtrip/{name}.golden"), text.as_bytes());
    }
    write("regex/vectors.tsv".to_string(), REGEX_VECTORS.as_bytes());
    // The escape set is data, not generated: it stays as it is.
    files.insert("escapes/escape-set.txt".to_string(), fs::read(root().join("escapes/escape-set.txt")).unwrap());

    if let Some((recorded_version, recorded)) = manifest::recorded() {
        let changed: Vec<&String> = files
            .iter()
            .filter(|(file, bytes)| recorded.get(*file).is_some_and(|h| *h != manifest::hash(bytes)))
            .map(|(file, _)| file)
            .collect();
        let removed: Vec<&String> = recorded.keys().filter(|file| !files.contains_key(*file)).collect();
        assert!(
            (changed.is_empty() && removed.is_empty()) || recorded_version != vtee::SEMANTICS_VERSION,
            "refusing to change frozen vectors {changed:?} / remove {removed:?}: SEMANTICS_VERSION is still \
             {recorded_version}. Changing a frozen vector is a semantics change (docs/guide/versioning.md)."
        );
    }
    let base = root();
    for sub in ["runs", "reject", "roundtrip", "regex"] {
        let _ = fs::remove_dir_all(base.join(sub));
    }
    for (file, bytes) in &files {
        let path = base.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    let hashes: BTreeMap<String, String> = files.iter().map(|(f, b)| (f.clone(), manifest::hash(b))).collect();
    fs::write(manifest::path(), manifest::render(vtee::SEMANTICS_VERSION, &hashes)).unwrap();
}
