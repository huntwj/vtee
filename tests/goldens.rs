//! The repository's own goldens, checked on every platform CI runs on (Linux and Windows). Each case is a script in
//! `tests/goldens/`, a golden recorded with `--portable`, and a program written once per platform that produces the same
//! screens. A failure here means the screens, the golden reader or the portable profile changed.
//!
//! To re-record one, on a developer machine (`--update` is refused when `CI` is set):
//!
//! ```text
//! vtee --script tests/goldens/greeting.ndjson --expect tests/goldens/greeting.golden --update --portable \
//!      --cols 30 --rows 6 -- sh -c "echo hello; echo END; sleep 60"
//! ```

use std::path::PathBuf;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_vtee");

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("goldens")
}

/// Prints `hello` and `END`, then stays alive until the run ends it.
fn greeting() -> Vec<&'static str> {
    if cfg!(windows) {
        vec!["cmd", "/c", "echo hello& echo END& ping -n 60 127.0.0.1 >nul"]
    } else {
        vec!["sh", "-c", "echo hello; echo END; sleep 60"]
    }
}

/// Prints `ready`, reads a line, prints `got:` and that line, then stays alive. Two screens: before and after the input.
fn conversation() -> Vec<&'static str> {
    if cfg!(windows) {
        vec!["cmd", "/v:on", "/c", "echo ready& set /p x=& echo got:!x!& ping -n 60 127.0.0.1 >nul"]
    } else {
        vec!["sh", "-c", "echo ready; read x; echo got:$x; sleep 60"]
    }
}

fn check(case: &str, program: &[&str]) {
    let dir = dir();
    let mut cmd = Command::new(BIN);
    cmd.current_dir(&dir)
        .env("NO_COLOR", "1")
        .args(["--script", &format!("{case}.ndjson"), "--expect", &format!("{case}.golden")])
        .args(["--cols", "30", "--rows", "6", "--"])
        .args(program);
    let out = cmd.output().unwrap();
    let report = String::from_utf8_lossy(&out.stdout).into_owned();
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(out.status.code(), Some(0), "{case}: exit status\n{report}\n{err}");
    assert!(report.starts_with(&format!("PASS {case}.golden")), "{case}: {report}");
}

#[test]
fn the_greeting_golden_passes() {
    check("greeting", &greeting());
}

#[test]
fn the_two_screen_conversation_golden_passes() {
    check("conversation", &conversation());
}

#[test]
fn the_conversation_golden_holds_two_screens() {
    let text = std::fs::read_to_string(dir().join("conversation.golden")).unwrap();
    assert!(text.contains("[screen 1]") && text.contains("[screen 2]"), "{text}");
    assert!(text.contains("name \"before\"") && text.contains("name \"after\""), "{text}");
}
