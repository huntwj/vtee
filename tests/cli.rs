//! CLI-behavior tests (`docs/decisions.md`): following what `assert_cmd`'s own testing chapter
//! recommends, observable behaviour (exit code, stdout/stderr) rather than every internal edge
//! case. Argument-parsing tests here are platform-independent (`clap` validates before anything
//! ever spawns a pty); anything that actually spawns a child stays Unix-only, same reasoning as
//! `tests/replay.rs` - native Windows ConPTY is a separate, already-tracked gap
//! (`docs/decisions.md`), not something these tests need to work around.

use assert_cmd::Command;
use predicates::prelude::*;

mod common;

/// Where `confy` would look for a config file, redirected so a developer's or runner's real
/// `attach_if_exists = true` can never change what a test asserts.
fn config_env(root: &std::path::Path) -> [(&'static str, std::path::PathBuf); 3] {
    [("XDG_CONFIG_HOME", root.join("config")), ("HOME", root.join("home")), ("APPDATA", root.join("appdata"))]
}

fn vtee() -> Command {
    static EMPTY_CONFIG: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    let root = EMPTY_CONFIG.get_or_init(|| std::env::temp_dir().join(format!("vtee-cli-cfg-{}", std::process::id())));
    let mut cmd = Command::cargo_bin("vtee").unwrap();
    for (key, value) in config_env(root) {
        cmd.env(key, value);
    }
    cmd
}

/// `attach` replays the screen as escape codes; into a pipe that is noise, so it refuses before it
/// even looks for the session. assert_cmd pipes stdout, so stdout is never a terminal here.
#[test]
fn attach_refuses_without_a_terminal() {
    vtee()
        .args(["attach", "no-such-session"])
        .assert()
        .failure()
        .code(exitcode::USAGE)
        .stderr(predicate::str::contains("attach needs a terminal"));
}

/// Leaving `attach` is not obvious (no detach key, Ctrl-C goes to the program), so `--help` says so.
#[test]
fn attach_help_explains_how_to_leave() {
    vtee()
        .args(["attach", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no detach key").and(predicate::str::contains("Ctrl-C goes to the program")));
}

/// `ls` prints name, pid and live/stale (plus `sock=`); its help once promised an age column.
#[test]
fn ls_help_describes_the_columns_it_prints() {
    vtee()
        .args(["ls", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("holder pid").and(predicate::str::contains("live or stale")))
        .stdout(predicate::str::contains(", age").not());
}

/// `--help` is for users: it must not leak project-internal vocabulary, and every flag needs a line.
#[test]
fn help_is_user_facing_and_covers_every_flag() {
    let out = vtee().arg("--help").output().unwrap();
    let help = String::from_utf8_lossy(&out.stdout);
    for internal in ["TinyFugue", "requirement 6", "CommandResult", "docs/roadmap.md", "docs/decisions.md"] {
        assert!(!help.contains(internal), "--help mentions {internal:?}");
    }
    for flag in [
        "--text",
        "--connect",
        "--log",
        "--script",
        "--listen",
        "--no-listen",
        "--writable",
        "--no-stdin",
        "--dump-on-exit",
        "--headless",
        "--name",
        "--attach-if-exists",
        "--no-attach-if-exists",
        "--cols",
        "--rows",
        "--env",
    ] {
        let at = help.find(&format!("      {flag}")).unwrap_or_else(|| panic!("--help does not list {flag}"));
        let after = &help[at..];
        let described = after.lines().nth(1).is_some_and(|l| l.trim_start().len() > 10);
        assert!(described, "{flag} has no help line");
    }
    assert!(help.contains("Fail if the name is taken"));
}

#[test]
fn requires_a_command_unless_connecting() {
    // Enforced manually now, not via clap's own `required_unless_present` (which can't
    // reference a `#[command(subcommand)]` field directly - `src/main.rs`'s `command` field
    // doc comment has the full reasoning) - the message is ours, not clap's generated one.
    vtee().assert().failure().code(exitcode::USAGE).stderr(predicate::str::contains("a command is required"));
}

#[test]
fn headless_and_no_listen_conflict() {
    vtee()
        .args(["--headless", "--no-listen", "--", "cat"])
        .assert()
        .failure()
        .code(exitcode::USAGE)
        .stderr(predicate::str::contains("--headless").and(predicate::str::contains("--no-listen")));
}

#[test]
fn listen_and_no_listen_conflict() {
    vtee()
        .args(["--listen", "--no-listen", "--", "cat"])
        .assert()
        .failure()
        .code(exitcode::USAGE)
        .stderr(predicate::str::contains("--listen").and(predicate::str::contains("--no-listen")));
}

#[test]
fn writable_rejects_an_unknown_mode() {
    vtee()
        .args(["--writable=bogus", "--", "cat"])
        .assert()
        .failure()
        .code(exitcode::USAGE)
        .stderr(predicate::str::contains("possible values"));
}

#[test]
fn cols_rejects_a_non_numeric_value() {
    vtee()
        .args(["--cols", "notanumber", "--", "cat"])
        .assert()
        .failure()
        .code(exitcode::USAGE)
        .stderr(predicate::str::contains("--cols"));
}

/// The same 1-1000 bound `resize` enforces, checked before anything is spawned.
#[test]
fn cols_and_rows_outside_the_resize_limit_are_usage_errors() {
    for (flag, value) in [("--cols", "5000"), ("--cols", "0"), ("--rows", "1001"), ("--rows", "0")] {
        vtee()
            .args([flag, value, "--no-listen", "--", "cat"])
            .assert()
            .failure()
            .code(exitcode::USAGE)
            .stderr(predicate::str::contains(flag).and(predicate::str::contains("between 1 and 1000")));
    }
    // The limits themselves are accepted: the run only fails later, on the missing script.
    vtee()
        .args(["--cols", "1000", "--rows", "1", "--script", "missing.ndjson", "--"])
        .args(exits_with(0))
        .assert()
        .failure()
        .code(exitcode::NOINPUT);
}

#[test]
fn env_rejects_a_value_without_an_equals_sign() {
    vtee()
        .args(["--env", "justastring", "--", "cat"])
        .assert()
        .failure()
        .code(exitcode::USAGE)
        .stderr(predicate::str::contains("KEY=VALUE"));
}

#[test]
fn writable_and_no_listen_conflict() {
    vtee()
        .args(["--writable", "--no-listen", "--", "cat"])
        .assert()
        .failure()
        .code(exitcode::USAGE)
        .stderr(predicate::str::contains("--writable").and(predicate::str::contains("--no-listen")));
}

/// Regression test for a real bug an adversarial review caught: `allow_hyphen_values` on the
/// trailing `command` positional used to make clap treat *any* unrecognized leading flag,
/// including its own `--version`/`-V`, as the start of the command to run rather than rejecting
/// it - so a typo'd vtee flag silently became "the program to run" and failed with a
/// confusing pty-spawn error instead of clap's own "unrecognized argument" message. Fixed by
/// dropping `allow_hyphen_values` (program args after the program name itself still pass
/// through fine via plain `trailing_var_arg` - see the Unix-only passthrough test below).
#[test]
fn unknown_flags_are_rejected_not_swallowed_as_the_command() {
    vtee()
        .arg("--this-is-not-a-real-flag")
        .assert()
        .failure()
        .code(exitcode::USAGE)
        .stderr(predicate::str::contains("unexpected argument").or(predicate::str::contains("unrecognized")));
}

#[test]
fn version_flag_is_handled_by_clap_not_swallowed() {
    vtee().arg("--version").assert().success().stdout(predicate::str::contains("vtee"));
}

/// `--script` never looks at the interactive-only flags (`run_script` has no idea they exist) -
/// rejecting the combination is more honest than silently accepting and ignoring it.
#[test]
fn script_conflicts_with_every_interactive_only_flag() {
    for flag in ["--headless", "--listen", "--no-listen", "--writable", "--no-stdin", "--dump-on-exit"] {
        vtee()
            .args(["--script", "nonexistent.ndjson", flag, "--", "cat"])
            .assert()
            .failure()
            .code(exitcode::USAGE)
            .stderr(predicate::str::contains("--script").and(predicate::str::contains(flag)));
    }
}

// ---- exit_status / wait_exit in script mode, on both platforms ---------------------------------

fn exit_script(lines: &[&str]) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "vtee-exit-{}-{}.ndjson",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    path
}

/// A program that exits with `code` straight away, and one that runs for a long time, using only
/// what both platforms have.
fn exits_with(code: u32) -> Vec<String> {
    let line = format!("exit {code}");
    if cfg!(windows) { vec!["cmd".into(), "/c".into(), line] } else { vec!["sh".into(), "-c".into(), line] }
}

fn runs_for_a_long_time() -> Vec<&'static str> {
    if cfg!(windows) { vec!["ping", "-n", "30", "127.0.0.1"] } else { vec!["sleep", "30"] }
}

fn json_lines(stdout: &[u8]) -> Vec<serde_json::Value> {
    String::from_utf8_lossy(stdout).lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

#[test]
fn wait_exit_and_exit_status_report_the_childs_exit_code() {
    let script = exit_script(&[r#"{"cmd":"wait_exit","timeout_ms":15000}"#, r#"{"cmd":"exit_status"}"#]);
    let out = vtee()
        .arg("--script")
        .arg(&script)
        .arg("--")
        .args(exits_with(7))
        .timeout(std::time::Duration::from_secs(30))
        .output()
        .unwrap();
    let _ = std::fs::remove_file(&script);
    assert!(out.status.success(), "{out:?}");
    let results = json_lines(&out.stdout);
    assert_eq!(results[0], serde_json::json!({"cmd":"wait_exit","running":false,"code":7,"timed_out":false}));
    assert_eq!(results[1], serde_json::json!({"cmd":"exit_status","running":false,"code":7}));
}

#[test]
fn wait_exit_times_out_while_the_child_is_still_running() {
    let script = exit_script(&[r#"{"cmd":"exit_status"}"#, r#"{"cmd":"wait_exit","timeout_ms":200}"#]);
    let out = vtee()
        .arg("--script")
        .arg(&script)
        .arg("--")
        .args(runs_for_a_long_time())
        .timeout(std::time::Duration::from_secs(30))
        .output()
        .unwrap();
    let _ = std::fs::remove_file(&script);
    assert!(out.status.success(), "{out:?}");
    let results = json_lines(&out.stdout);
    assert_eq!(results[0], serde_json::json!({"cmd":"exit_status","running":true}));
    assert_eq!(results[1], serde_json::json!({"cmd":"wait_exit","running":true,"timed_out":true}));
}

#[test]
fn text_mode_prints_the_exit_status_on_one_line() {
    let script = exit_script(&[r#"{"cmd":"wait_exit","timeout_ms":15000}"#]);
    vtee()
        .arg("--script")
        .arg(&script)
        .arg("--text")
        .arg("--")
        .args(exits_with(3))
        .timeout(std::time::Duration::from_secs(30))
        .assert()
        .success()
        .stdout(predicate::str::contains("--- wait_exit exited code=3 timed_out=false ---"));
    let _ = std::fs::remove_file(&script);
}

/// A program that prints `text`, then stays alive long enough to be stopped by the script run.
fn prints_then_waits(text: &str) -> Vec<String> {
    if cfg!(windows) {
        vec!["cmd".into(), "/c".into(), format!("echo {text} & ping -n 30 127.0.0.1 >nul")]
    } else {
        vec!["sh".into(), "-c".into(), format!("echo {text}; sleep 30")]
    }
}

/// `replay` shows the final screen of a `--log` recording, as text or JSON, with the size taken
/// from the recording's header.
#[test]
fn replay_shows_the_final_screen_of_a_log() {
    let log = std::env::temp_dir().join(format!("vtee-replay-{}.cast", std::process::id()));
    let script = exit_script(&[r#"{"cmd":"wait_for","text":"replay-me-xyz","timeout_ms":15000}"#]);
    let out = vtee()
        .arg("--script")
        .arg(&script)
        .args(["--cols", "70", "--rows", "12", "--log"])
        .arg(&log)
        .arg("--")
        .args(prints_then_waits("replay-me-xyz"))
        .timeout(std::time::Duration::from_secs(30))
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");

    vtee()
        .arg("replay")
        .arg(&log)
        .timeout(std::time::Duration::from_secs(10))
        .assert()
        .success()
        .stdout(predicate::str::contains("--- snapshot").and(predicate::str::contains("replay-me-xyz")));

    let out =
        vtee().arg("replay").arg(&log).arg("--json").timeout(std::time::Duration::from_secs(10)).output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let snapshot: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!((snapshot["cols"].as_u64(), snapshot["rows"].as_u64()), (Some(70), Some(12)));
    assert_eq!(snapshot["schema_version"], 1);
    assert_eq!(snapshot["semantics_version"], 1);
    assert!(snapshot["lines"].as_array().unwrap().iter().any(|l| l.as_str().unwrap().contains("replay-me-xyz")));

    let _ = std::fs::remove_file(&log);
    let _ = std::fs::remove_file(&script);
}

#[test]
fn replay_reports_a_missing_or_malformed_file() {
    vtee().args(["replay", "no-such-recording.cast"]).assert().failure().code(exitcode::NOINPUT);

    let bad = std::env::temp_dir().join(format!("vtee-replay-bad-{}.cast", std::process::id()));
    std::fs::write(&bad, "this is not an asciicast file\n").unwrap();
    vtee()
        .arg("replay")
        .arg(&bad)
        .assert()
        .failure()
        .code(exitcode::DATAERR)
        .stderr(predicate::str::contains("not an asciicast v2 recording"));
    let _ = std::fs::remove_file(&bad);
}

/// Windows PowerShell 5.1 writes a UTF-8 byte order mark at the start of a file or pipe; the first
/// command line must still parse.
#[test]
fn a_script_with_a_byte_order_mark_still_parses() {
    let script = exit_script(&["\u{feff}{\"cmd\":\"wait_exit\",\"timeout_ms\":15000}"]);
    vtee()
        .arg("--script")
        .arg(&script)
        .arg("--text")
        .arg("--")
        .args(exits_with(0))
        .timeout(std::time::Duration::from_secs(30))
        .assert()
        .success()
        .stdout(predicate::str::contains("wait_exit exited code=0"));
    let _ = std::fs::remove_file(&script);
}

#[cfg(unix)]
mod unix_behavior {
    use super::*;
    use std::os::unix::fs::DirBuilderExt;

    /// `--env` reaches the child via `portable_pty::CommandBuilder::env`, on top of the
    /// inherited environment rather than in place of it - `printenv` exits on its own once it's
    /// printed, so no hang risk the way `cat` would have.
    #[test]
    fn env_vars_reach_the_child() {
        vtee()
            .args(["--no-listen", "--env", "VTEE_TEST_VAR=hello-from-vtee", "--", "printenv", "VTEE_TEST_VAR"])
            .timeout(Duration::from_secs(5))
            .assert()
            .success()
            .stdout(predicate::str::contains("hello-from-vtee"));
    }

    /// With no terminal on the other end (`--script`: stdin and stdout are pipes), the tap is
    /// the child's terminal, so it must answer the child's queries itself - here a
    /// cursor-position report (`ESC[6n` -> `ESC[row;colR`), the one ConPTY itself sends at
    /// startup on Windows (`tests/windows_conpty.rs`); a shell or editor asks the same thing on
    /// any platform. The child echoes whatever reply it reads so the script can match on it.
    #[test]
    fn script_mode_answers_the_childs_cursor_position_query() {
        let script = std::env::temp_dir().join(format!("vtee-dsr-{}.ndjson", std::process::id()));
        std::fs::write(&script, "{\"cmd\":\"wait_for\",\"text\":\"reply=[1;1R\",\"timeout_ms\":5000}\n").unwrap();
        let result = vtee()
            .args([
                "--script",
                script.to_str().unwrap(),
                "--",
                "bash",
                "-c",
                "printf '\\033[6n'; IFS= read -rsn6 x; echo \"reply=${x:1}\"",
            ])
            .timeout(Duration::from_secs(15))
            .assert();
        let _ = std::fs::remove_file(&script);
        result.success().stdout(predicate::str::contains("\"matched\":true"));
    }

    /// A value containing `=` (e.g. a `PATH`-shaped override) must not be truncated at the
    /// first `=` on the *value* side - only the key/value split itself stops at the first one.
    #[test]
    fn env_values_containing_equals_signs_are_preserved() {
        vtee()
            .args(["--no-listen", "--env", "VTEE_TEST_VAR=a=b=c", "--", "printenv", "VTEE_TEST_VAR"])
            .timeout(Duration::from_secs(5))
            .assert()
            .success()
            .stdout(predicate::str::contains("a=b=c"));
    }

    /// Companion to `unknown_flags_are_rejected_not_swallowed_as_the_command`: dropping
    /// `allow_hyphen_values` must not have broken the actual feature `trailing_var_arg` is for -
    /// a hyphen-prefixed argument belonging to the *wrapped program*, appearing after the
    /// program name has already started the positional, still has to pass through untouched,
    /// with or without an explicit `--`. Uses `head -c 2` rather than `cat`: it exits on its own
    /// once it's read 2 bytes, so this can't hang waiting for stdin EOF the way `cat` would (the
    /// pty master stays open for the life of the session, so closing just our own stdin never
    /// delivers EOF to the child) - if `-c 2` were silently dropped, `head` would fall back to
    /// its 10-line default and hang on this input, caught by `.timeout()` rather than wedging
    /// the test suite.
    #[test]
    fn hyphen_flags_after_the_program_name_still_pass_through() {
        // Not `--headless` - that forces `forward_input` off regardless, which would make this
        // test about whether stdin forwarding happens at all, not about hyphen-flag passthrough.
        for args in [["--no-listen", "--", "head", "-c", "2"].as_slice(), ["--no-listen", "head", "-c", "2"].as_slice()]
        {
            vtee()
                .args(args)
                .timeout(Duration::from_secs(5))
                .write_stdin("hi there\n")
                .assert()
                .success()
                .stdout(predicate::str::contains("hi"));
        }
    }

    /// `--listen <path>` with a space is deliberately not the same as `--listen=<path>`
    /// (`docs/decisions.md`, following POSIX's own rule for an optional-value long option) -
    /// the path ends up read as the command to run instead, which fails to spawn. A regression
    /// test for the documented behaviour, not a bug report. Unix-only because the bare-`--listen`
    /// branch it exercises resolves `default_socket_path` first, which errors out before even
    /// reaching this on a platform where that's unsupported (native Windows).
    #[test]
    fn listen_with_a_space_is_read_as_the_command_not_a_path() {
        let tmpdir = isolated_tmpdir();
        vtee()
            .env("TMPDIR", &tmpdir)
            .args(["--listen", "/tmp/not-a-real-command", "--", "cat"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("/tmp/not-a-real-command"));
        let _ = std::fs::remove_dir_all(&tmpdir);
    }
    use std::os::unix::fs::MetadataExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    /// A child process that is killed and reaped when dropped, so a failed assertion can't leave
    /// it running.
    struct Proc(Option<std::process::Child>);

    impl std::ops::Deref for Proc {
        type Target = std::process::Child;
        fn deref(&self) -> &Self::Target {
            self.0.as_ref().expect("child already taken")
        }
    }

    impl std::ops::DerefMut for Proc {
        fn deref_mut(&mut self) -> &mut Self::Target {
            self.0.as_mut().expect("child already taken")
        }
    }

    impl Drop for Proc {
        fn drop(&mut self) {
            if let Some(child) = self.0.as_mut() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    /// `vtee` as a plain child process with `TMPDIR` and the config dirs inside `tmpdir`.
    fn vtee_process(tmpdir: &std::path::Path) -> std::process::Command {
        let mut cmd = std::process::Command::new(assert_cmd::cargo::cargo_bin("vtee"));
        cmd.env("TMPDIR", tmpdir);
        for (key, value) in config_env(tmpdir) {
            cmd.env(key, value);
        }
        cmd
    }

    /// A private `TMPDIR` per test, not the real default temp dir - `cargo test` runs these in
    /// parallel by default, and the production default-session-dir logic is keyed only by uid,
    /// so sharing the real temp dir across tests racing each other (create/remove/read at the
    /// same path from several processes at once) was flaky for reasons that had nothing to do
    /// with the behaviour under test. This isolates each test the same way a real caller would
    /// by setting `TMPDIR` themselves, rather than serializing the tests.
    fn isolated_tmpdir() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("vtee-cli-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn session_dir(tmpdir: &std::path::Path) -> PathBuf {
        let uid = unsafe { libc::getuid() };
        tmpdir.join(format!("vtee-{uid}"))
    }

    /// Polls instead of sleeping a fixed time, so tests neither flake on a loaded machine nor
    /// wait longer than they need to.
    fn wait_until(what: &str, ready: impl FnMut() -> bool) {
        wait_until_within(Duration::from_secs(10), what, ready);
    }

    fn wait_until_within(limit: Duration, what: &str, mut ready: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + limit;
        while !ready() {
            assert!(std::time::Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// A session is ready once its raw-TTY socket exists, which is the last file it binds.
    fn wait_for_session(tmpdir: &std::path::Path, name: &str) {
        let sock = session_dir(tmpdir).join(format!("{name}.tty.sock"));
        wait_until(&format!("session {name:?} to start"), || sock.exists());
    }

    /// Runs `vtee` headless (no tty needed) with `TMPDIR` pointed at an isolated directory -
    /// these tests assert on *side effects* (files created) while the process is still running,
    /// so it has to be killed rather than waited on to completion.
    fn spawn_headless(tmpdir: &std::path::Path, extra_args: &[&str]) -> Proc {
        Proc(Some(vtee_process(tmpdir).arg("--headless").args(extra_args).args(["--", "cat"]).spawn().unwrap()))
    }

    #[test]
    fn default_listen_creates_a_hardened_session_dir() {
        let tmpdir = isolated_tmpdir();
        let dir = session_dir(&tmpdir);
        let mut child = spawn_headless(&tmpdir, &[]);
        wait_for_session(&tmpdir, "cat");

        let meta = std::fs::symlink_metadata(&dir).expect("default session dir should exist");
        assert!(meta.is_dir());
        assert_eq!(meta.mode() & 0o777, 0o700, "session dir must be 0700, not {:o}", meta.mode() & 0o777);
        // A named session now has three files: the NDJSON socket, the raw-TTY socket, and the
        // pidfile `ls`/`kill`/`attach` use to find it - all named `cat.*` here (`cat` is the
        // program `spawn_headless` runs), not pid-based like before session naming existed.
        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        assert!(names.contains(&"cat.sock".to_string()), "missing cat.sock in {names:?}");
        assert!(names.contains(&"cat.tty.sock".to_string()), "missing cat.tty.sock in {names:?}");
        assert!(names.contains(&"cat.pid".to_string()), "missing cat.pid in {names:?}");
        assert_eq!(names.len(), 3, "expected exactly these three files, got {names:?}");

        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&tmpdir);
    }

    #[test]
    fn no_listen_creates_no_session_dir() {
        let tmpdir = isolated_tmpdir();
        let dir = session_dir(&tmpdir);
        let mut child = spawn_headless(&tmpdir, &["--no-listen"]);
        // Absence can't be polled for; give startup time to (wrongly) create the directory.
        std::thread::sleep(Duration::from_millis(300));

        assert!(!dir.exists(), "{} should not have been created with --no-listen", dir.display());

        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&tmpdir);
    }

    /// `--cols`/`--rows` actually reach the spawned virtual tty - checked by querying a live
    /// snapshot over `--connect`/`--writable`, the same round trip a real observer would use.
    #[test]
    fn cols_and_rows_are_reflected_in_a_live_snapshot() {
        let tmpdir = isolated_tmpdir();
        let dir = session_dir(&tmpdir);
        let mut child = spawn_headless(&tmpdir, &["--cols", "97", "--rows", "31"]);
        wait_for_session(&tmpdir, "cat");

        // Specifically the NDJSON socket (`cat.sock`), not the raw-TTY one (`cat.tty.sock`) -
        // both end in `.sock`, so matching on the extension alone would pick either.
        let sock = dir.join("cat.sock");
        assert!(sock.exists(), "expected {} to exist", sock.display());

        let output = vtee().arg("--connect").arg(&sock).write_stdin("{\"cmd\":\"snapshot\"}\n").output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("\"cols\":97"), "snapshot didn't reflect --cols: {stdout}");
        assert!(stdout.contains("\"rows\":31"), "snapshot didn't reflect --rows: {stdout}");

        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&tmpdir);
    }

    fn spawn_named(tmpdir: &std::path::Path, name: &str, extra_args: &[&str]) -> Proc {
        Proc(Some(
            vtee_process(tmpdir)
                .arg("--headless")
                .args(["--name", name])
                .args(extra_args)
                .args(["--", "cat"])
                .spawn()
                .unwrap(),
        ))
    }

    fn vtee_in(tmpdir: &std::path::Path) -> Command {
        let mut cmd = vtee();
        cmd.env("TMPDIR", tmpdir);
        for (key, value) in config_env(tmpdir) {
            cmd.env(key, value);
        }
        cmd
    }

    /// The core naming behaviour (`docs/architecture.md`, "session naming"): a second session
    /// under the same name is rejected, matching tmux's own real default (confirmed against
    /// its source, `docs/decisions.md`) - no auto-suffixing, no silent attach.
    #[test]
    fn duplicate_session_name_is_rejected() {
        let tmpdir = isolated_tmpdir();
        let mut first = spawn_named(&tmpdir, "dup-test", &[]);
        wait_for_session(&tmpdir, "dup-test");

        vtee_in(&tmpdir)
            .args(["--name", "dup-test", "--", "cat"])
            .timeout(Duration::from_secs(5))
            .assert()
            .failure()
            .code(exitcode::USAGE)
            .stderr(predicate::str::contains("duplicate session: dup-test"));

        let _ = first.kill();
        let _ = first.wait();
        let _ = std::fs::remove_dir_all(&tmpdir);
    }

    /// `--attach-if-exists` (tmux's own `-A`): the same duplicate-name situation above, but
    /// attaches instead of erroring. Verified by checking the *attaching* invocation's own
    /// stdout contains the redraw/live output a real attach produces, rather than a usage
    /// error - it would otherwise look identical to the rejection case above (same exit
    /// shape) if it silently did nothing.
    #[test]
    fn attach_if_exists_attaches_instead_of_erroring() {
        let tmpdir = isolated_tmpdir();
        let mut first = spawn_named(&tmpdir, "attach-if-exists-test", &[]);
        wait_for_session(&tmpdir, "attach-if-exists-test");

        // Attach needs a terminal, so the second invocation runs on a pty. Input sent before the
        // attach connects waits in the pty and is relayed afterwards, so one write is enough.
        let second = common::PtyProc::spawn(vtee_process(&tmpdir).args([
            "--name",
            "attach-if-exists-test",
            "--attach-if-exists",
            "--",
            "cat",
        ]));
        second.send(b"from-attacher\r");
        assert!(second.wait_for("from-attacher", Duration::from_secs(10)), "{:?}", second.text());

        let _ = first.kill();
        let _ = first.wait();
        let _ = std::fs::remove_dir_all(&tmpdir);
    }

    /// `vtee ls`/`kill` round trip: a live session shows up, `kill` ends it and removes its
    /// files, and it no longer shows up afterward.
    #[test]
    fn ls_and_kill_round_trip() {
        let tmpdir = isolated_tmpdir();
        let dir = session_dir(&tmpdir);
        let mut child = spawn_named(&tmpdir, "ls-kill-test", &[]);
        wait_for_session(&tmpdir, "ls-kill-test");

        vtee_in(&tmpdir)
            .arg("ls")
            .timeout(Duration::from_secs(5))
            .assert()
            .success()
            .stdout(predicate::str::contains("ls-kill-test").and(predicate::str::contains("live")));

        vtee_in(&tmpdir).args(["kill", "ls-kill-test"]).timeout(Duration::from_secs(5)).assert().success();

        // `kill` removes the files itself: the holder's SIGTERM handler exits without running
        // its `SessionFiles` guard.
        assert!(!dir.join("ls-kill-test.sock").exists());
        assert!(!dir.join("ls-kill-test.tty.sock").exists());
        assert!(!dir.join("ls-kill-test.pid").exists());

        vtee_in(&tmpdir)
            .arg("ls")
            .timeout(Duration::from_secs(5))
            .assert()
            .success()
            .stdout(predicate::str::contains("no sessions"));

        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&tmpdir);
    }

    /// `--no-listen`'s rationale ("no footprint in a shared /tmp at all") extends to the
    /// pidfile, not just the two sockets - confirmed by the directory not existing at all,
    /// same assertion `no_listen_creates_no_session_dir` above already makes, specifically
    /// re-checked here under `--name` (which otherwise *would* trigger naming/pidfile logic)
    /// to confirm `--no-listen` still skips it entirely even when a name is given.
    #[test]
    fn no_listen_skips_naming_entirely_even_with_a_name() {
        // Not `spawn_named` - it always passes `--headless`, which `conflicts_with "no_listen"`
        // at the clap level (unrelated to what this test checks), so this needs its own plain
        // invocation instead. Non-blocking without a real tty either way, same as every other
        // plain-interactive-mode test that doesn't attach a pty.
        let tmpdir = isolated_tmpdir();
        let dir = session_dir(&tmpdir);
        let mut child = Proc(Some(
            vtee_process(&tmpdir)
                .args(["--name", "should-not-appear", "--no-listen", "--no-stdin", "--", "cat"])
                .spawn()
                .unwrap(),
        ));
        // Absence can't be polled for; give startup time to (wrongly) create the directory.
        std::thread::sleep(Duration::from_millis(300));

        assert!(!dir.exists(), "{} should not exist with --no-listen, even with --name", dir.display());

        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&tmpdir);
    }

    /// Regression test for a real, adversarially-found bug: `--name` reached `session_paths`
    /// (`src/lib.rs`) with no validation at all, so a value containing `/` or `..` escaped the
    /// hardened per-uid directory entirely (`PathBuf::join` doesn't confine it) - defeating the
    /// whole point of that directory's own permission checks. Covers path traversal, an
    /// absolute path, and bare `.`/`..`/empty, all of which must be rejected before anything is
    /// created on disk.
    #[test]
    fn invalid_session_names_are_rejected_before_touching_disk() {
        let tmpdir = isolated_tmpdir();
        for bad_name in ["../../../tmp/evil", "/tmp/absolute-evil", "..", ".", ""] {
            vtee_in(&tmpdir)
                .args(["--name", bad_name, "--no-stdin", "--", "cat"])
                .timeout(Duration::from_secs(5))
                .assert()
                .failure()
                .stderr(predicate::str::contains("valid session name"));
        }
        // Confirms the rejection happens *before* any file is created anywhere reachable -
        // not just that vtee prints an error while still leaving something behind.
        assert!(!std::path::Path::new("/tmp/evil.sock").exists());
        assert!(!std::path::Path::new("/tmp/absolute-evil.sock").exists());
        let _ = std::fs::remove_dir_all(&tmpdir);
    }

    /// Regression test for a real, adversarially-found bug: `kill` used to trust a pidfile's
    /// raw contents unconditionally and send `SIGTERM` with no liveness check at all - a stale
    /// pidfile (left behind by a `SIGKILL`'d holder, a documented gap) pointing at a since-
    /// reused PID would signal whatever unrelated process now holds it. Verifies `kill` checks
    /// liveness first (via the same `session_is_live` `ls` already uses) rather than blindly
    /// trusting the file: a stale pidfile just gets cleaned up, signaling nothing.
    #[test]
    fn kill_does_not_blindly_trust_a_stale_pidfile() {
        let tmpdir = isolated_tmpdir();
        let dir = session_dir(&tmpdir);
        // Must actually be 0700, not just present - `default_session_dir` rejects anything
        // looser as "unsafe permissions" before `kill` even gets to the pidfile.
        std::fs::DirBuilder::new().mode(0o700).recursive(true).create(&dir).unwrap();
        // A live, unrelated process stands in for "a since-reused PID": the pidfile names it, no
        // session answers on the socket, and `kill` must leave it running.
        let sentinel = Proc(Some(std::process::Command::new("sleep").arg("60").spawn().unwrap()));
        std::fs::write(dir.join("faketest.pid"), sentinel.id().to_string()).unwrap();

        vtee_in(&tmpdir).args(["kill", "faketest"]).timeout(Duration::from_secs(5)).assert().success();

        assert!(!dir.join("faketest.pid").exists(), "the stale pidfile should be cleaned up");
        let mut sentinel = sentinel;
        std::thread::sleep(Duration::from_millis(300));
        assert!(sentinel.try_wait().unwrap().is_none(), "kill signalled a process it had no session for");
        let _ = std::fs::remove_dir_all(&tmpdir);
    }

    /// Regression test for two review findings on the NDJSON socket, which is on by default: an
    /// unbounded `resize` used to allocate a 65535x65535 grid and kill the session, and a line
    /// with no newline used to be buffered without limit.
    #[test]
    fn hostile_ndjson_input_is_rejected_and_the_session_survives() {
        use std::io::{Read, Write};
        use std::os::unix::net::UnixStream;

        let tmpdir = isolated_tmpdir();
        let dir = session_dir(&tmpdir);
        let mut child = spawn_named(&tmpdir, "hostile", &["--writable"]);
        wait_for_session(&tmpdir, "hostile");
        let sock = dir.join("hostile.sock");

        let mut s = UnixStream::connect(&sock).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(b"{\"cmd\":\"resize\",\"cols\":65535,\"rows\":65535}\n").unwrap();
        let mut buf = [0u8; 512];
        let n = s.read(&mut buf).unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).contains("exceeds"), "expected a size error");

        let mut s = UnixStream::connect(&sock).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let junk = vec![b'x'; 64 * 1024];
        for _ in 0..40 {
            if s.write_all(&junk).is_err() {
                break;
            }
        }
        let mut sink = Vec::new();
        let _ = s.read_to_end(&mut sink);

        vtee()
            .arg("--connect")
            .arg(&sock)
            .timeout(Duration::from_secs(5))
            .write_stdin("{\"cmd\":\"snapshot\"}\n")
            .assert()
            .success()
            .stdout(predicate::str::contains("\"cols\":80"));

        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&tmpdir);
    }

    /// Windows PowerShell 5.1 starts piped text with a UTF-8 byte order mark; `--connect` must not
    /// forward it as part of the first JSON line.
    #[test]
    fn connect_accepts_a_first_line_with_a_byte_order_mark() {
        let tmpdir = isolated_tmpdir();
        let dir = session_dir(&tmpdir);
        let mut child = spawn_named(&tmpdir, "bom", &[]);
        wait_for_session(&tmpdir, "bom");

        vtee()
            .arg("--connect")
            .arg(dir.join("bom.sock"))
            .timeout(Duration::from_secs(5))
            .write_stdin("\u{feff}{\"cmd\":\"snapshot\"}\n")
            .assert()
            .success()
            .stdout(predicate::str::contains("\"cols\":80"));

        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&tmpdir);
    }

    /// `--cols 0` is refused at startup, so `resize` must refuse 0 too instead of quietly raising
    /// it to 1; a refused resize changes nothing, and a valid one afterwards still works.
    #[test]
    fn resize_rejects_zero_and_leaves_the_size_alone() {
        let tmpdir = isolated_tmpdir();
        let dir = session_dir(&tmpdir);
        let mut child = spawn_named(&tmpdir, "zero", &["--writable"]);
        wait_for_session(&tmpdir, "zero");
        let sock = dir.join("zero.sock");

        let out = vtee()
            .arg("--connect")
            .arg(&sock)
            .timeout(Duration::from_secs(5))
            .write_stdin("{\"cmd\":\"resize\",\"cols\":0,\"rows\":24}\n{\"cmd\":\"resize\",\"cols\":80,\"rows\":0}\n{\"cmd\":\"snapshot\"}\n")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(stdout.matches("below the 1x1 minimum").count(), 2, "both zero sizes refused: {stdout}");
        assert!(stdout.contains("\"cols\":80") && stdout.contains("\"rows\":24"), "size unchanged: {stdout}");

        vtee()
            .arg("--connect")
            .arg(&sock)
            .timeout(Duration::from_secs(5))
            .write_stdin("{\"cmd\":\"resize\",\"cols\":100,\"rows\":30}\n{\"cmd\":\"snapshot\"}\n")
            .assert()
            .success()
            .stdout(predicate::str::contains("\"cols\":100"));

        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&tmpdir);
    }

    /// Windows PowerShell 5.1 sends the mark twice when `$OutputEncoding` already has one, and a
    /// line the server cannot parse must not look like success.
    #[test]
    fn connect_strips_a_doubled_mark_and_fails_on_unparseable_input() {
        let tmpdir = isolated_tmpdir();
        let dir = session_dir(&tmpdir);
        let mut child = spawn_named(&tmpdir, "bom2", &[]);
        wait_for_session(&tmpdir, "bom2");
        let sock = dir.join("bom2.sock");

        vtee()
            .arg("--connect")
            .arg(&sock)
            .timeout(Duration::from_secs(5))
            .write_stdin("\u{feff}\u{feff}{\"cmd\":\"snapshot\"}\n")
            .assert()
            .success()
            .stdout(predicate::str::contains("\"cols\":80"));

        vtee()
            .arg("--connect")
            .arg(&sock)
            .timeout(Duration::from_secs(5))
            .write_stdin("this is not json\n")
            .assert()
            .code(exitcode::DATAERR)
            .stdout(predicate::str::contains("bad command"));

        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&tmpdir);
    }

    /// Regression test for a real, adversarially-found bug: broadcasting to raw-TTY sinks used
    /// to do a blocking `write_all` to *every* sink under the one lock that also gates `term`,
    /// `bytes_seen`, and `snapshot()` - so one attach client that simply stops reading (a
    /// backgrounded terminal, a slow reader, a hostile one) could wedge every other sink and
    /// every snapshot indefinitely, demonstrated by the review with a client that never read and
    /// a concurrent `snapshot()` call that then never returned. Reproduces that shape directly:
    /// client A attaches and never reads, client B attaches and keeps reading, and a `snapshot`
    /// query over the unrelated NDJSON socket must still return promptly - all three go through
    /// `TapState`'s one lock, so if the fix regressed back to blocking under it, this hangs past
    /// its `.timeout()` instead of passing.
    #[test]
    fn a_stalled_tty_client_does_not_block_other_clients_or_snapshots() {
        use std::io::Read;
        use std::os::unix::net::UnixStream;
        use std::sync::Arc;

        let tmpdir = isolated_tmpdir();
        let dir = session_dir(&tmpdir);
        // `yes` floods output fast and indefinitely - exactly the kind of volume needed to fill
        // a stalled sink's bounded queue quickly rather than waiting on a slow producer.
        let mut child = Proc(Some(
            vtee_process(&tmpdir).arg("--headless").args(["--name", "holblock", "--", "yes"]).spawn().unwrap(),
        ));
        wait_for_session(&tmpdir, "holblock");

        let tty_sock = dir.join("holblock.tty.sock");
        let ndjson_sock = dir.join("holblock.sock");
        assert!(tty_sock.exists(), "expected {} to exist", tty_sock.display());

        // Client A: attaches, then never reads again - the stalled sink.
        let _client_a = UnixStream::connect(&tty_sock).unwrap();

        // Client B: attaches and keeps reading/discarding, tracking how many bytes arrive.
        let client_b = UnixStream::connect(&tty_sock).unwrap();
        let bytes_received = Arc::new(AtomicU64::new(0));
        {
            let bytes_received = bytes_received.clone();
            let mut client_b = client_b;
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match client_b.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            bytes_received.fetch_add(n as u64, Ordering::Relaxed);
                        }
                    }
                }
            });
        }

        // 4 MiB is well past the 2 MiB a client may have queued, so client A has been
        // resynchronised (its backlog replaced by a redraw) more than once by the time B has
        // seen this much - and B was never held up by it.
        wait_until_within(Duration::from_secs(30), "client B to receive 4 MiB while client A is stalled", || {
            bytes_received.load(Ordering::Relaxed) >= 4 * 1024 * 1024
        });

        // The actual regression check: a `snapshot` query on the completely separate NDJSON
        // socket must still return within the timeout. Before the fix, this call shared the
        // same lock `forward_output`'s blocking broadcast held hostage, and never returned.
        vtee()
            .arg("--connect")
            .arg(&ndjson_sock)
            .timeout(Duration::from_secs(5))
            .write_stdin("{\"cmd\":\"snapshot\"}\n")
            .assert()
            .success()
            .stdout(predicate::str::contains("\"cols\""));

        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&tmpdir);
    }

    /// The product bug behind a flaky CI run: a client the scheduler starves for a moment fell
    /// behind the producer, its queue filled, and it was dropped for good - its connection stayed
    /// open but nothing more ever arrived. A client that is merely slow must be resynchronised and
    /// keep receiving. Client C reads nothing for a while under a flood (many queue overflows),
    /// then reads; it must get a fresh stream of output, starting with a redraw.
    #[test]
    fn a_client_that_is_slow_for_a_while_keeps_receiving_afterwards() {
        use std::io::Read;
        use std::os::unix::net::UnixStream;
        use std::sync::Arc;

        let tmpdir = isolated_tmpdir();
        let dir = session_dir(&tmpdir);
        let mut child = Proc(Some(
            vtee_process(&tmpdir).arg("--headless").args(["--name", "slowtty", "--", "yes"]).spawn().unwrap(),
        ));
        wait_for_session(&tmpdir, "slowtty");

        let mut client = UnixStream::connect(dir.join("slowtty.tty.sock")).unwrap();
        // Long enough for `yes` to push far more than the 2 MiB queue limit past an unread client.
        std::thread::sleep(Duration::from_millis(1500));

        let received = Arc::new(AtomicU64::new(0));
        {
            let received = received.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                while let Ok(n) = client.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    received.fetch_add(n as u64, Ordering::Relaxed);
                }
            });
        }

        // A client that was dropped for good could still drain what was already queued and
        // buffered in the socket (about 2.3 MiB); 6 MiB can only come from a live stream.
        wait_until_within(
            Duration::from_secs(30),
            "a client that was slow to keep receiving after it caught up",
            || received.load(Ordering::Relaxed) >= 6 * 1024 * 1024,
        );

        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&tmpdir);
    }

    // ---- script mode: size, environment, raw log and replay ------------------------------------

    fn script_file(dir: &std::path::Path, lines: &[&str]) -> PathBuf {
        let path = dir.join("script.ndjson");
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        path
    }

    fn result_lines(stdout: &[u8]) -> Vec<serde_json::Value> {
        String::from_utf8_lossy(stdout).lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
    }

    /// `--cols/--rows/--env` reach the child, `--log` writes an asciicast v2 file, and replaying
    /// that file rebuilds the screen the script saw.
    #[test]
    fn script_mode_honours_size_and_env_and_writes_a_replayable_log() {
        let dir = isolated_tmpdir();
        let log = dir.join("session.cast");
        let script =
            script_file(&dir, &[r#"{"cmd":"wait_for","text":"done:xyz","timeout_ms":8000}"#, r#"{"cmd":"snapshot"}"#]);
        let out = vtee()
            .arg("--script")
            .arg(&script)
            .args(["--cols", "91", "--rows", "33", "--env", "VTEE_T=xyz", "--log"])
            .arg(&log)
            .args(["--", "sh", "-c", "echo \"size:$(stty size)\"; echo done:$VTEE_T; sleep 30"])
            .timeout(Duration::from_secs(20))
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");

        let results = result_lines(&out.stdout);
        assert_eq!(results[0]["matched"], true, "{results:?}");
        let live = &results[1]["snapshot"];
        assert_eq!((live["cols"].as_u64(), live["rows"].as_u64()), (Some(91), Some(33)));
        let text: Vec<String> =
            live["lines"].as_array().unwrap().iter().map(|l| l.as_str().unwrap().to_string()).collect();
        assert!(text.iter().any(|l| l.contains("size:33 91")), "stty saw a different size: {text:?}");
        assert!(text.iter().any(|l| l.contains("done:xyz")), "--env didn't reach the child: {text:?}");

        let raw = std::fs::read_to_string(&log).unwrap();
        let mut log_lines = raw.lines();
        let header: serde_json::Value = serde_json::from_str(log_lines.next().unwrap()).unwrap();
        assert_eq!(
            (header["version"].as_u64(), header["width"].as_u64(), header["height"].as_u64()),
            (Some(2), Some(91), Some(33))
        );
        let events: Vec<serde_json::Value> = log_lines.map(|l| serde_json::from_str(l).unwrap()).collect();
        assert!(!events.is_empty());
        assert!(events.iter().all(|e| e[1] == "o" && e[2].is_string() && e[0].is_number()), "{events:?}");

        let replayed = vtee::replay(&log, 91, 33).unwrap();
        assert_eq!(serde_json::to_value(&replayed.lines).unwrap(), live["lines"], "replay rebuilt a different screen");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_childs_exit_code_becomes_vtees_own() {
        for code in [0, 1, 7, 200] {
            vtee()
                .args(["--no-listen", "--", "sh", "-c", &format!("exit {code}")])
                .timeout(Duration::from_secs(10))
                .assert()
                .code(code);
        }
    }

    #[test]
    fn a_program_that_cannot_be_spawned_exits_with_oserr() {
        vtee()
            .args(["--no-listen", "--", "/definitely/not/a/program"])
            .timeout(Duration::from_secs(10))
            .assert()
            .failure()
            .code(exitcode::OSERR)
            .stderr(predicate::str::contains("failed to spawn"));
    }

    #[test]
    fn a_wait_for_that_times_out_reports_no_match_and_the_script_still_succeeds() {
        let dir = isolated_tmpdir();
        let script = script_file(&dir, &[r#"{"cmd":"wait_for","text":"never-appears","timeout_ms":300}"#]);
        let out = vtee()
            .arg("--script")
            .arg(&script)
            .args(["--", "sleep", "5"])
            .timeout(Duration::from_secs(20))
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        assert_eq!(result_lines(&out.stdout)[0]["matched"], false);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `wait_for` matches one screen row at a time by default, so text that crosses a soft wrap is not
    /// found; with `join_wrapped` the rows of a wrapped line are joined first. A hard line break stays a
    /// break either way.
    #[test]
    fn wait_for_can_match_across_a_soft_wrap_when_asked() {
        let dir = isolated_tmpdir();
        let script = script_file(
            &dir,
            &[
                r#"{"cmd":"wait_for","text":"delta-echo","timeout_ms":8000}"#,
                r#"{"cmd":"wait_for","text":"charlie-delta","timeout_ms":300}"#,
                r#"{"cmd":"wait_for","text":"charlie-delta","timeout_ms":8000,"join_wrapped":true}"#,
                r#"{"cmd":"wait_for","text":"echo-zulu","timeout_ms":300,"join_wrapped":true}"#,
                r#"{"cmd":"snapshot"}"#,
            ],
        );
        let out = vtee()
            .arg("--script")
            .arg(&script)
            .args([
                "--cols",
                "20",
                "--rows",
                "6",
                "--",
                "sh",
                "-c",
                "echo alpha-bravo-charlie-delta-echo; echo zulu; sleep 30",
            ])
            .timeout(Duration::from_secs(20))
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        let results = result_lines(&out.stdout);
        let matched: Vec<_> = results.iter().take(4).map(|r| r["matched"].clone()).collect();
        assert_eq!(matched, [true, false, true, false], "{results:?}");
        assert_eq!(results[4]["snapshot"]["wrapped_rows"], serde_json::json!([0]), "{results:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn send_key_and_wait_ms_drive_the_child_in_script_mode() {
        let dir = isolated_tmpdir();
        let script = script_file(
            &dir,
            &[
                r#"{"cmd":"wait_for","text":"ready","timeout_ms":8000}"#,
                r#"{"cmd":"send_key","key":"Up"}"#,
                r#"{"cmd":"wait_ms","ms":50}"#,
                r#"{"cmd":"wait_for","text":"1b 5b 41","timeout_ms":8000}"#,
            ],
        );
        let out = vtee()
            .arg("--script")
            .arg(&script)
            .args(["--", "sh", "-c", "stty raw -echo; echo ready; dd bs=1 count=3 2>/dev/null | od -An -tx1; sleep 30"])
            .timeout(Duration::from_secs(20))
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        let results = result_lines(&out.stdout);
        assert_eq!(results.len(), 2, "only the two wait_for commands print a result: {results:?}");
        assert_eq!(results[1]["matched"], true, "the Up arrow never reached the child: {results:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn send_types_text_into_the_child_in_script_mode() {
        let dir = isolated_tmpdir();
        let script = script_file(
            &dir,
            &[
                r#"{"cmd":"wait_for","text":"ready","timeout_ms":8000}"#,
                r#"{"cmd":"send","text":"hello\n"}"#,
                r#"{"cmd":"wait_for","text":"got:hello","timeout_ms":8000}"#,
            ],
        );
        let out = vtee()
            .arg("--script")
            .arg(&script)
            .args(["--", "sh", "-c", "stty -echo; echo ready; IFS= read -r l; echo \"got:$l\"; sleep 30"])
            .timeout(Duration::from_secs(20))
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        assert_eq!(result_lines(&out.stdout)[1]["matched"], true);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unknown_key_name_in_a_script_is_a_data_error() {
        let dir = isolated_tmpdir();
        let script = script_file(&dir, &[r#"{"cmd":"send_key","key":"Banana"}"#]);
        vtee()
            .arg("--script")
            .arg(&script)
            .args(["--", "sleep", "5"])
            .timeout(Duration::from_secs(20))
            .assert()
            .failure()
            .code(exitcode::DATAERR);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- the config file -----------------------------------------------------------------------

    /// confy writes the file on first load, so provoke a load (a duplicate name reaches it),
    /// find where it went, and overwrite it. Returns the live first session.
    fn session_with_config(tmpdir: &std::path::Path, name: &str, config: &str) -> Proc {
        let first = spawn_named(tmpdir, name, &[]);
        wait_for_session(tmpdir, name);
        let _ = vtee_in(tmpdir).args(["--name", name, "--", "cat"]).timeout(Duration::from_secs(10)).output();
        let mut found = Vec::new();
        let mut stack = vec![tmpdir.join("config")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "toml") {
                    found.push(path);
                }
            }
        }
        assert_eq!(found.len(), 1, "expected confy to have created one config file under {tmpdir:?}: {found:?}");
        std::fs::write(&found[0], config).unwrap();
        first
    }

    #[test]
    fn the_config_file_can_make_attach_if_exists_the_default() {
        let tmpdir = isolated_tmpdir();
        let _first = session_with_config(&tmpdir, "cfg-attach", "attach_if_exists = true\n");

        let second = common::PtyProc::spawn(vtee_process(&tmpdir).args(["--name", "cfg-attach", "--", "cat"]));
        second.send(b"via-config\r");
        assert!(second.wait_for("via-config", Duration::from_secs(10)), "{:?}", second.text());
        drop(second);
        let _ = std::fs::remove_dir_all(&tmpdir);
    }

    #[test]
    fn no_attach_if_exists_overrides_the_config_file() {
        let tmpdir = isolated_tmpdir();
        let _first = session_with_config(&tmpdir, "cfg-override", "attach_if_exists = true\n");
        vtee_in(&tmpdir)
            .args(["--name", "cfg-override", "--no-attach-if-exists", "--", "cat"])
            .timeout(Duration::from_secs(10))
            .assert()
            .failure()
            .code(exitcode::USAGE)
            .stderr(predicate::str::contains("duplicate session: cfg-override"));
        let _ = std::fs::remove_dir_all(&tmpdir);
    }

    #[test]
    fn a_malformed_config_file_warns_and_falls_back_to_rejecting_duplicates() {
        let tmpdir = isolated_tmpdir();
        let _first = session_with_config(&tmpdir, "cfg-bad", "attach_if_exists = \"not-a-bool [[[\n");
        vtee_in(&tmpdir)
            .args(["--name", "cfg-bad", "--", "cat"])
            .timeout(Duration::from_secs(10))
            .assert()
            .failure()
            .code(exitcode::USAGE)
            .stderr(predicate::str::contains("unreadable config").and(predicate::str::contains("duplicate session")));
        let _ = std::fs::remove_dir_all(&tmpdir);
    }

    // ---- the terminal is restored when vtee is signalled -------------------------------------

    fn python3_available() -> bool {
        std::process::Command::new("python3").arg("--version").output().is_ok_and(|o| o.status.success())
    }

    fn tool_required(tool: &str) -> bool {
        match std::env::var("VTEE_REQUIRE_TOOLS") {
            Ok(v) => v == "1" || v.split(',').any(|t| t.trim() == tool),
            Err(_) => false,
        }
    }

    fn assert_terminal_restored_after(signal: &str) {
        if !python3_available() {
            assert!(!tool_required("python3"), "python3 is required (VTEE_REQUIRE_TOOLS) but not installed");
            eprintln!("skipping: python3 is not installed");
            return;
        }
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/raw_mode_restore.py");
        let out = std::process::Command::new("python3")
            .arg("-I")
            .arg(script)
            .arg(assert_cmd::cargo::cargo_bin("vtee"))
            .arg(signal)
            .output()
            .unwrap();
        assert!(
            out.status.success() && String::from_utf8_lossy(&out.stdout).contains("OK"),
            "terminal not restored after SIG{signal}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Requirement 7: a user's shell is not left in raw mode when `vtee` is terminated.
    #[test]
    fn sigterm_restores_the_terminal() {
        assert_terminal_restored_after("TERM");
    }

    /// `SIGKILL` runs no destructors, so only the watchdog process can restore the terminal.
    #[test]
    fn sigkill_restores_the_terminal_through_the_watchdog() {
        assert_terminal_restored_after("KILL");
    }
}
