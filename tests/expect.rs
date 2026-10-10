//! `--expect` and `--update` end to end, on both platforms: recording a golden from a script run, checking against it,
//! refusing, restamping, and the usage errors. The programs are a shell on Unix and `cmd` on Windows, printing a
//! fixed line; goldens are recorded with `--portable`, which ignores the platform differences (window title, modes).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

const BIN: &str = env!("CARGO_BIN_EXE_vtee");

fn tmp(label: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "vtee-expect-{label}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A program that prints `text`, then `END` (what the scripts wait for, so a screen is taken only once the output is
/// complete), and then stays alive until the script run ends it.
fn prints(text: &str) -> Vec<String> {
    if cfg!(windows) {
        ["cmd", "/c", &format!("echo {text}& echo END& ping -n 60 127.0.0.1 >nul")].map(String::from).to_vec()
    } else {
        ["sh", "-c", &format!("echo {text}; echo END; sleep 60")].map(String::from).to_vec()
    }
}

fn vtee(dir: &Path, args: &[&str]) -> Output {
    Command::new(BIN)
        .current_dir(dir)
        .env_remove("CI")
        .env_remove("VTEE_ALLOW_UPDATE")
        .env_remove("FORCE_COLOR")
        .env("NO_COLOR", "1")
        .args(args)
        .output()
        .unwrap()
}

fn vtee_env(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    Command::new(BIN)
        .current_dir(dir)
        .env_remove("CI")
        .env_remove("VTEE_ALLOW_UPDATE")
        .env("NO_COLOR", "1")
        .envs(env.iter().copied())
        .args(args)
        .output()
        .unwrap()
}

fn run_with(dir: &Path, options: &[&str], program: &[String]) -> Output {
    let mut args: Vec<&str> = options.to_vec();
    args.push("--");
    args.extend(program.iter().map(String::as_str));
    vtee(dir, &args)
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).replace("\r\n", "\n")
}

fn code(out: &Output) -> i32 {
    out.status.code().unwrap_or(-1)
}

const WAIT_HELLO_AND_SNAPSHOT: &str =
    "{\"cmd\":\"wait_for\",\"text\":\"END\",\"timeout_ms\":15000}\n{\"cmd\":\"snapshot\",\"name\":\"greeting\"}\n";

fn script(dir: &Path, name: &str, text: &str) -> String {
    fs::write(dir.join(name), text).unwrap();
    name.to_string()
}

/// Records `hello` into `flow.golden` with `--portable` and returns the directory.
fn recorded() -> PathBuf {
    let dir = tmp("rec");
    script(&dir, "flow.ndjson", WAIT_HELLO_AND_SNAPSHOT);
    let out = run_with(
        &dir,
        &[
            "--script",
            "flow.ndjson",
            "--expect",
            "flow.golden",
            "--update",
            "--portable",
            "--cols",
            "30",
            "--rows",
            "4",
        ],
        &prints("hello"),
    );
    assert_eq!(code(&out), 0, "{}\n{}", stdout(&out), stderr(&out));
    assert_eq!(stdout(&out), "CREATED flow.golden: 1 screen\n");
    dir
}

fn golden(dir: &Path) -> String {
    fs::read_to_string(dir.join("flow.golden")).unwrap().replace("\r\n", "\n")
}

#[test]
fn a_recorded_golden_passes_and_says_what_it_expects() {
    let dir = recorded();
    let text = golden(&dir);
    assert!(text.starts_with("vtee-golden 1\nschema 1\nsemantics 1\nignore modes "), "{text}");
    assert!(text.contains("ignore title\n"), "the portable profile is written: {text}");
    assert!(text.contains("[screen 1]\nsize 30x4\n"), "{text}");
    assert!(text.contains("name \"greeting\"\n"), "{text}");
    assert!(text.contains("\n0|hello\n"), "{text}");
    assert!(!text.contains("[masks]") && !text.contains("[assert]"), "a recording has no masks or assertions");

    let out = run_with(
        &dir,
        &["--script", "flow.ndjson", "--expect", "flow.golden", "--cols", "30", "--rows", "4"],
        &prints("hello"),
    );
    assert_eq!(code(&out), 0, "{}\n{}", stdout(&out), stderr(&out));
    assert_eq!(stdout(&out), "PASS flow.golden (1 screen)\n");
    assert_eq!(stderr(&out), "");
}

#[test]
fn the_terminal_size_defaults_to_80_by_24_whatever_runs_the_test() {
    let dir = tmp("size");
    script(&dir, "flow.ndjson", WAIT_HELLO_AND_SNAPSHOT);
    let out = run_with(&dir, &["--script", "flow.ndjson", "--expect", "flow.golden", "--update"], &prints("hello"));
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(golden(&dir).contains("size 80x24\n"));
}

#[test]
fn different_output_fails_with_a_report_and_exit_1() {
    let dir = recorded();
    let args = ["--script", "flow.ndjson", "--expect", "flow.golden", "--cols", "30", "--rows", "4"];
    let out = run_with(&dir, &args, &prints("hellp"));
    assert_eq!(code(&out), 1, "{}\n{}", stdout(&out), stderr(&out));
    let report = stdout(&out);
    assert!(report.starts_with("FAIL flow.golden: 1 mismatch in 1 of 1 screen\nscreen 1 \"greeting\":\n"), "{report}");
    assert!(report.contains("text       row 0 col 4   expected 'o'   actual 'p'"), "{report}");

    let mut json_args = args.to_vec();
    json_args.extend(["--format", "json"]);
    let out = run_with(&dir, &json_args, &prints("hellp"));
    assert_eq!(code(&out), 1);
    let json: serde_json::Value = serde_json::from_str(stdout(&out).trim()).expect("one JSON object");
    assert_eq!(json["result"], "fail");
    assert_eq!(json["screens"][0]["mismatches"][0]["kind"], "text");
}

#[test]
fn a_re_record_keeps_masks_assertions_and_comments_and_says_what_moved() {
    let dir = recorded();
    let mut text = golden(&dir);
    text = text.replacen("\n[screen 1]", "\n[masks]\n# the greeting varies\nmask row=1 text=ignore\n\n[screen 1]", 1);
    text.push_str("\n[assert]\n# keep this note\nassert row=0 text=\"hello\"\n");
    fs::write(dir.join("flow.golden"), &text).unwrap();

    let out = run_with(
        &dir,
        &["--script", "flow.ndjson", "--expect", "flow.golden", "--update", "--cols", "30", "--rows", "4"],
        &prints("howdy"),
    );
    assert_eq!(code(&out), 0, "{}\n{}", stdout(&out), stderr(&out));
    let summary = stdout(&out);
    assert!(
        summary.starts_with("UPDATED flow.golden: 1 screen\n  screen 1: 4 cells changed (text 4, attrs 0)"),
        "{summary}"
    );
    let after = golden(&dir);
    assert!(after.contains("# the greeting varies\nmask row=1 text=ignore\n"), "{after}");
    assert!(after.contains("# keep this note\nassert row=0 text=\"hello\"\n"), "{after}");
    assert!(after.contains("\n0|howdy\n"), "{after}");
    assert!(after.contains("ignore title\n"), "the ignore lines are kept: {after}");

    // The same run again changes nothing, and does not touch the file.
    let before = fs::read(dir.join("flow.golden")).unwrap();
    let out = run_with(
        &dir,
        &["--script", "flow.ndjson", "--expect", "flow.golden", "--update", "--cols", "30", "--rows", "4"],
        &prints("howdy"),
    );
    assert_eq!(stdout(&out), "UNCHANGED flow.golden: 1 screen\n");
    assert_eq!(fs::read(dir.join("flow.golden")).unwrap(), before);
}

#[test]
fn a_timed_out_wait_fails_the_run_and_update_writes_nothing() {
    let dir = recorded();
    let before = fs::read(dir.join("flow.golden")).unwrap();
    script(
        &dir,
        "stuck.ndjson",
        "{\"cmd\":\"wait_for\",\"text\":\"never-printed\",\"timeout_ms\":300}\n{\"cmd\":\"snapshot\"}\n",
    );

    let out = run_with(
        &dir,
        &["--script", "stuck.ndjson", "--expect", "flow.golden", "--cols", "30", "--rows", "4"],
        &prints("hello"),
    );
    assert_eq!(code(&out), 1, "{}", stdout(&out));
    assert!(
        stdout(&out).contains("wait       script line 1   wait_for \"never-printed\" timed out after 300 ms"),
        "{}",
        stdout(&out)
    );

    let out = run_with(
        &dir,
        &["--script", "stuck.ndjson", "--expect", "flow.golden", "--update", "--cols", "30", "--rows", "4"],
        &prints("hello"),
    );
    assert_eq!(code(&out), 1);
    assert!(stdout(&out).starts_with("NOT WRITTEN flow.golden: a wait_for timed out"), "{}", stdout(&out));
    assert_eq!(fs::read(dir.join("flow.golden")).unwrap(), before, "the golden was left alone");

    // The same wait, allowed to time out, is not a failure.
    script(
        &dir,
        "lenient.ndjson",
        "{\"cmd\":\"wait_for\",\"text\":\"never-printed\",\"timeout_ms\":300,\"optional\":true}\n{\"cmd\":\"wait_for\",\"text\":\"END\",\"timeout_ms\":15000}\n{\"cmd\":\"snapshot\",\"name\":\"greeting\"}\n",
    );
    let out = run_with(
        &dir,
        &["--script", "lenient.ndjson", "--expect", "flow.golden", "--cols", "30", "--rows", "4"],
        &prints("hello"),
    );
    assert_eq!(code(&out), 0, "{}\n{}", stdout(&out), stderr(&out));
}

#[test]
fn update_is_refused_in_ci_unless_it_is_allowed() {
    let dir = recorded();
    let args = [
        "--script",
        "flow.ndjson",
        "--expect",
        "flow.golden",
        "--update",
        "--cols",
        "30",
        "--rows",
        "4",
        "--",
        "never-started",
    ];
    let out = vtee_env(&dir, &args, &[("CI", "true")]);
    assert_eq!(code(&out), 64, "{}", stderr(&out));
    assert!(stderr(&out).starts_with("vtee: --update is refused when CI is set"), "{}", stderr(&out));

    // A refused update never costs a run: the program above does not exist, and that was not even tried.
    let allowed = vtee_env(&dir, &args, &[("CI", "true"), ("VTEE_ALLOW_UPDATE", "1")]);
    assert_eq!(code(&allowed), 71, "{}", stderr(&allowed));
    assert!(stderr(&allowed).contains("failed to spawn never-started"));

    // Checking is never guarded.
    let check = vtee_env(
        &dir,
        &["--script", "flow.ndjson", "--expect", "flow.golden", "--cols", "30", "--rows", "4", "--", "never-started"],
        &[("CI", "true")],
    );
    assert_eq!(code(&check), 71);
}

#[test]
fn a_golden_of_another_semantics_is_refused_and_restamping_changes_only_that_line() {
    let dir = recorded();
    let text = golden(&dir).replace("semantics 1\n", "semantics 7\n");
    fs::write(dir.join("flow.golden"), &text).unwrap();

    let args = ["--script", "flow.ndjson", "--expect", "flow.golden", "--cols", "30", "--rows", "4"];
    let out = run_with(&dir, &args, &prints("hello"));
    assert_eq!(code(&out), 2, "{}", stdout(&out));
    let report = stdout(&out);
    assert!(
        report.starts_with("REFUSED flow.golden: semantics 7 in the golden, 1 in this vtee; nothing was compared\n"),
        "{report}"
    );
    let suggestion = report.lines().find(|l| l.contains("--update=restamp")).expect("a restamp command is suggested");
    let at_options = suggestion.find("--update=restamp").unwrap();
    let at_program = suggestion.find(" -- ").expect("the program follows");
    assert!(at_options < at_program, "the option must come before the program: {suggestion}");

    let mut restamp = args.to_vec();
    restamp.push("--update=restamp");
    let out = run_with(&dir, &restamp, &prints("hello"));
    assert_eq!(code(&out), 0, "{}\n{}", stdout(&out), stderr(&out));
    assert_eq!(stdout(&out), "RESTAMPED flow.golden: semantics 7 -> 1\n");
    let after = golden(&dir);
    let changed: Vec<_> = text.lines().zip(after.lines()).filter(|(a, b)| a != b).collect();
    assert_eq!(changed.len(), 1, "only the semantics line differs: {changed:?}");
    assert_eq!(after.lines().count(), text.lines().count());

    let out = run_with(&dir, &args, &prints("hello"));
    assert_eq!(code(&out), 0);

    // Already current: nothing to do, nothing run.
    let out = run_with(&dir, &restamp, &prints("hello"));
    assert_eq!(stdout(&out), "UNCHANGED flow.golden: semantics 1 is already the number of this vtee\n");
}

#[test]
fn restamping_refuses_when_something_the_golden_checks_would_change() {
    let dir = recorded();
    let text = golden(&dir).replace("semantics 1\n", "semantics 7\n");
    fs::write(dir.join("flow.golden"), &text).unwrap();
    let out = run_with(
        &dir,
        &["--script", "flow.ndjson", "--expect", "flow.golden", "--update=restamp", "--cols", "30", "--rows", "4"],
        &prints("hellp"),
    );
    assert_eq!(code(&out), 1);
    let report = stdout(&out);
    assert!(
        report.starts_with("NOT RESTAMPED flow.golden: 1 difference that would change what the golden checks"),
        "{report}"
    );
    assert!(report.contains("FAIL flow.golden: 1 mismatch"), "{report}");
    assert_eq!(golden(&dir), text, "nothing was written");
}

#[test]
fn goldens_of_other_layouts_are_refused_with_the_way_forward() {
    let dir = recorded();
    let text = golden(&dir);
    let args = ["--script", "flow.ndjson", "--expect", "flow.golden", "--cols", "30", "--rows", "4"];

    fs::write(dir.join("flow.golden"), text.replace("schema 1\n", "schema 0\n")).unwrap();
    let out = run_with(&dir, &args, &prints("hello"));
    assert_eq!(code(&out), 2);
    assert!(stdout(&out).contains("migrate the file:  vtee --expect flow.golden --update=migrate"), "{}", stdout(&out));

    fs::write(dir.join("flow.golden"), text.replace("vtee-golden 1\n", "vtee-golden 2\n")).unwrap();
    let out = run_with(&dir, &args, &prints("hello"));
    assert_eq!(code(&out), 2);
    assert!(
        stdout(&out).starts_with("REFUSED flow.golden: grammar 2 in the golden, 1 in this vtee"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn files_that_are_missing_or_malformed_have_their_own_exit_codes() {
    let dir = tmp("files");
    script(&dir, "flow.ndjson", WAIT_HELLO_AND_SNAPSHOT);
    let never = vec!["never-started".to_string()];

    let out = run_with(&dir, &["--script", "flow.ndjson", "--expect", "none.golden"], &never);
    assert_eq!(code(&out), 66);
    assert_eq!(stderr(&out).trim_end(), "vtee: none.golden: no such file (give --update to record it)");

    fs::write(dir.join("bad.golden"), "vtee-golden 1\nschema 1\n").unwrap();
    let out = run_with(&dir, &["--script", "flow.ndjson", "--expect", "bad.golden"], &never);
    assert_eq!(code(&out), 65, "{}", stderr(&out));
    assert!(stderr(&out).starts_with("bad.golden: "), "{}", stderr(&out));

    let out = run_with(&dir, &["--script", "missing.ndjson", "--expect", "bad.golden"], &never);
    assert_eq!(code(&out), 65, "the golden is read first");

    fs::write(dir.join("g.golden"), recorded_golden_text()).unwrap();
    let out = run_with(&dir, &["--script", "missing.ndjson", "--expect", "g.golden"], &never);
    assert_eq!(code(&out), 66);
    assert!(stderr(&out).contains("failed to open script missing.ndjson"), "{}", stderr(&out));

    // The whole script is read before the program is started: line 2 is bad, and `never-started` was never tried.
    script(&dir, "bad.ndjson", "{\"cmd\":\"snapshot\"}\nnot json\n");
    let out = run_with(&dir, &["--script", "bad.ndjson", "--expect", "g.golden"], &never);
    assert_eq!(code(&out), 65, "{}", stderr(&out));
    assert_eq!(stderr(&out).trim_end(), "script line 2: expected ident at line 1 column 2");

    let out = run_with(&dir, &["--script", "flow.ndjson", "--expect", "g.golden"], &never);
    assert_eq!(code(&out), 71, "{}", stderr(&out));
}

fn recorded_golden_text() -> String {
    "vtee-golden 1\nschema 1\nsemantics 1\n\n[screen 1]\nsize 3x1\ncursor row=0 col=0 shape=block hidden=false\nmodes\nalt_screen false\ntitle null\n\n[text]\n0|\n".to_string()
}

#[test]
fn a_draft_golden_is_replaced_by_an_update_and_a_malformed_one_is_not() {
    let dir = tmp("draft");
    script(&dir, "flow.ndjson", WAIT_HELLO_AND_SNAPSHOT);
    fs::write(dir.join("old.golden"), "vtee-golden 0\nwhatever\n").unwrap();
    let out = run_with(
        &dir,
        &["--script", "flow.ndjson", "--expect", "old.golden", "--update", "--portable"],
        &prints("hello"),
    );
    assert_eq!(code(&out), 0, "{}\n{}", stdout(&out), stderr(&out));
    assert!(
        stdout(&out).starts_with("CREATED old.golden: 1 screen\n  note: replaced a vtee-golden 0 draft"),
        "{}",
        stdout(&out)
    );
    assert!(fs::read_to_string(dir.join("old.golden")).unwrap().starts_with("vtee-golden 1\n"));

    fs::write(dir.join("broken.golden"), "vtee-golden 1\nschema 1\n").unwrap();
    let out = run_with(&dir, &["--script", "flow.ndjson", "--expect", "broken.golden", "--update"], &prints("hello"));
    assert_eq!(code(&out), 65);
    assert!(stderr(&out).contains("cannot be updated while it is malformed"), "{}", stderr(&out));
}

#[test]
fn one_screen_of_a_composite_can_be_compared_and_re_recorded() {
    let dir = tmp("screens");
    let two = "{\"cmd\":\"wait_for\",\"text\":\"END\",\"timeout_ms\":15000}\n{\"cmd\":\"snapshot\",\"name\":\"first\"}\n{\"cmd\":\"wait_ms\",\"ms\":50}\n{\"cmd\":\"snapshot\",\"name\":\"second\"}\n";
    script(&dir, "two.ndjson", two);
    let args = ["--script", "two.ndjson", "--expect", "two.golden", "--cols", "20", "--rows", "3"];
    let mut record = args.to_vec();
    record.extend(["--update", "--portable"]);
    let out = run_with(&dir, &record, &prints("one"));
    assert_eq!(stdout(&out), "CREATED two.golden: 2 screens\n", "{}", stderr(&out));

    // One screen of the golden against a script that makes only one.
    script(
        &dir,
        "one.ndjson",
        "{\"cmd\":\"wait_for\",\"text\":\"END\",\"timeout_ms\":15000}\n{\"cmd\":\"snapshot\"}\n",
    );
    let out = run_with(
        &dir,
        &["--script", "one.ndjson", "--expect", "two.golden", "--screen", "2", "--cols", "20", "--rows", "3"],
        &prints("uno"),
    );
    assert_eq!(code(&out), 1, "{}", stdout(&out));
    assert!(stdout(&out).contains("screen 2 \"second\":"), "{}", stdout(&out));
    assert!(!stdout(&out).contains("screen 1"), "{}", stdout(&out));

    let out = run_with(
        &dir,
        &[
            "--script",
            "one.ndjson",
            "--expect",
            "two.golden",
            "--screen",
            "2",
            "--update",
            "--cols",
            "20",
            "--rows",
            "3",
        ],
        &prints("uno"),
    );
    assert_eq!(code(&out), 0, "{}\n{}", stdout(&out), stderr(&out));
    assert_eq!(
        stdout(&out),
        "UPDATED two.golden: 2 screens\n  screen 2: 2 cells changed (text 2, attrs 0), name changed\n"
    );
    let text = fs::read_to_string(dir.join("two.golden")).unwrap().replace("\r\n", "\n");
    assert!(text.contains("0|one\n") && text.contains("0|uno\n"), "{text}");

    // A script that makes two screens cannot pick one.
    let out = run_with(
        &dir,
        &["--script", "two.ndjson", "--expect", "two.golden", "--screen", "1", "--cols", "20", "--rows", "3"],
        &prints("one"),
    );
    assert_eq!(code(&out), 64);
    assert_eq!(stderr(&out).trim_end(), "vtee: --screen 1 selects one screen, but the run produced 2");

    // And only an existing golden has screens to pick from.
    let out = run_with(
        &dir,
        &["--script", "one.ndjson", "--expect", "new.golden", "--screen", "1", "--update"],
        &prints("uno"),
    );
    assert_eq!(code(&out), 64);
    assert_eq!(
        stderr(&out).trim_end(),
        "vtee: --screen 1 updates one screen of an existing golden; new.golden does not exist"
    );
}

#[test]
fn saved_output_checks_the_same_offline_as_it_did_live() {
    let dir = recorded();
    // What a plain `--script` run prints is what `--snapshot` reads.
    let live = run_with(&dir, &["--script", "flow.ndjson", "--cols", "30", "--rows", "4"], &prints("hello"));
    assert_eq!(code(&live), 0);
    fs::write(dir.join("saved.ndjson"), &live.stdout).unwrap();
    let out = vtee(&dir, &["--expect", "flow.golden", "--snapshot", "saved.ndjson"]);
    assert_eq!(code(&out), 0, "{}\n{}", stdout(&out), stderr(&out));
    assert_eq!(stdout(&out), "PASS flow.golden (1 screen)\n");

    // Standard input is `-`.
    let mut child = Command::new(BIN)
        .current_dir(&dir)
        .env_remove("CI")
        .args(["--expect", "flow.golden", "--snapshot", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(&mut child.stdin.take().unwrap(), &live.stdout).unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(code(&out), 0);
}

#[test]
fn the_illegal_combinations_exit_64_with_their_messages() {
    let dir = tmp("usage");
    let cases: &[(&[&str], &str)] = &[
        (
            &["--expect", "g.golden"],
            "vtee: --expect needs a source of screens: a program after --, --connect SOCK or --snapshot FILE",
        ),
        (
            &["--expect", "g.golden", "--", "sh"],
            "vtee: a program needs --script FILE to say what to type; to look at it by hand use --connect",
        ),
        (&["--script", "s.ndjson"], "vtee: --script needs a program to run or --connect SOCK"),
        (
            &["--connect", "x.sock", "--", "sh"],
            "vtee: --connect talks to a running session; it cannot be combined with a program",
        ),
        (
            &["--expect", "g.golden", "--snapshot", "s", "--script", "s2"],
            "vtee: --snapshot reads finished output; it cannot be combined with",
        ),
        (&["--update"], "vtee: --update needs --expect GOLDEN"),
        (&["--screen", "2"], "vtee: --screen needs --expect GOLDEN"),
        (&["--format", "json"], "vtee: --format needs --expect GOLDEN"),
        (&["--portable"], "vtee: --portable needs --expect GOLDEN"),
        (&["--fail-fast"], "vtee: --fail-fast needs --expect GOLDEN"),
        (&["--max-mismatches", "3"], "vtee: --max-mismatches needs --expect GOLDEN"),
        (
            &["--expect", "g.golden", "--snapshot", "s", "--text"],
            "vtee: --text has no effect with --expect, which prints a report",
        ),
        (
            &["--expect", "g.golden", "--snapshot", "s", "--portable"],
            "vtee: --portable applies when a golden is created; use it with --update",
        ),
        (
            &["--expect", "g.golden", "--snapshot", "s", "--update=restamp", "--portable"],
            "vtee: --portable applies when a golden is created; use it with --update",
        ),
        (
            &["--expect", "g.golden", "--snapshot", "s", "--update=restamp", "--screen", "1"],
            "vtee: --screen applies to checking and to --update (record)",
        ),
        (
            &["--expect", "g.golden", "--snapshot", "s", "--update=frob"],
            "vtee: unknown --update mode 'frob'; expected record, restamp or migrate",
        ),
        (&["--expect", "g.golden", "--update=migrate"], "vtee: --update=migrate is not available in this release"),
        (
            &["--expect", "g.golden", "--update=migrate", "--screen", "1"],
            "vtee: --update=migrate rewrites the file in place and runs nothing; give only --expect GOLDEN",
        ),
        (
            &["--expect", "g.golden", "--snapshot", "s", "--format", "yaml"],
            "error: invalid value 'yaml' for '--format <FORMAT>'",
        ),
    ];
    for (args, message) in cases {
        let out = vtee(&dir, args);
        assert_eq!(code(&out), 64, "{args:?}: {}", stderr(&out));
        assert!(stderr(&out).starts_with(message), "{args:?}\nwanted: {message}\ngot:    {}", stderr(&out));
        assert!(out.stdout.is_empty(), "{args:?}: nothing on standard output");
    }
}

#[test]
fn fail_fast_stops_at_the_first_problem_and_skips_the_rest() {
    let dir = tmp("fast");
    let snap = "{\"cmd\":\"snapshot\"}
";
    let wait_end = "{\"cmd\":\"wait_for\",\"text\":\"END\",\"timeout_ms\":15000}
";
    script(&dir, "flow.ndjson", &format!("{wait_end}{snap}{snap}{snap}"));
    let record = run_with(
        &dir,
        &["--script", "flow.ndjson", "--expect", "g.golden", "--update", "--portable", "--cols", "20", "--rows", "3"],
        &prints("hello"),
    );
    assert_eq!(code(&record), 0, "{}", stderr(&record));

    // The first screen matches; then a wait times out, and the screens after it are not looked at.
    let stuck = format!(
        "{wait_end}{snap}{{\"cmd\":\"wait_for\",\"text\":\"never\",\"timeout_ms\":300}}
{snap}{snap}"
    );
    script(&dir, "stuck.ndjson", &stuck);
    let args = [
        "--script",
        "stuck.ndjson",
        "--expect",
        "g.golden",
        "--fail-fast",
        "--cols",
        "20",
        "--rows",
        "3",
        "--format",
        "json",
    ];
    let out = run_with(&dir, &args, &prints("hello"));
    assert_eq!(code(&out), 1, "{}", stdout(&out));
    let json: serde_json::Value = serde_json::from_str(stdout(&out).trim()).unwrap();
    assert_eq!(json["run"].as_array().unwrap().len(), 1, "{json}");
    assert_eq!(json["run"][0]["kind"], "wait", "{json}");
    let results: Vec<_> =
        json["screens"].as_array().unwrap().iter().map(|s| s["result"].as_str().unwrap().to_string()).collect();
    assert_eq!(results, ["pass", "skipped", "skipped"], "{json}");

    // Without --fail-fast every problem is told.
    let all: Vec<&str> = args.iter().copied().filter(|a| *a != "--fail-fast").collect();
    let out = run_with(&dir, &all, &prints("hello"));
    let json: serde_json::Value = serde_json::from_str(stdout(&out).trim()).unwrap();
    assert_eq!(json["run"].as_array().unwrap().len(), 1, "{json}");
    assert_eq!(json["screens_actual"], 3);
}
