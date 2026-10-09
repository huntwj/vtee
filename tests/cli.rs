//! CLI-behavior tests (`docs/decisions.md`): following what `assert_cmd`'s own testing chapter
//! recommends, observable behaviour (exit code, stdout/stderr) rather than every internal edge
//! case. Argument-parsing tests here are platform-independent (`clap` validates before anything
//! ever spawns a pty); anything that actually spawns a child stays Unix-only, same reasoning as
//! `tests/replay.rs` - native Windows ConPTY is a separate, already-tracked gap
//! (`docs/decisions.md`), not something these tests need to work around.

use assert_cmd::Command;
use predicates::prelude::*;

fn vtee() -> Command {
    Command::cargo_bin("vtee").unwrap()
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

    /// Runs `vtee` headless (no tty needed) with `TMPDIR` pointed at an isolated directory -
    /// these tests assert on *side effects* (files created) while the process is still running,
    /// so it has to be killed rather than waited on to completion.
    fn spawn_headless(tmpdir: &std::path::Path, extra_args: &[&str]) -> std::process::Child {
        let bin = assert_cmd::cargo::cargo_bin("vtee");
        std::process::Command::new(bin)
            .env("TMPDIR", tmpdir)
            .arg("--headless")
            .args(extra_args)
            .args(["--", "cat"])
            .spawn()
            .unwrap()
    }

    #[test]
    fn default_listen_creates_a_hardened_session_dir() {
        let tmpdir = isolated_tmpdir();
        let dir = session_dir(&tmpdir);
        let mut child = spawn_headless(&tmpdir, &[]);
        std::thread::sleep(Duration::from_millis(300));

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
        std::thread::sleep(Duration::from_millis(300));

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

    fn spawn_named(tmpdir: &std::path::Path, name: &str, extra_args: &[&str]) -> std::process::Child {
        let bin = assert_cmd::cargo::cargo_bin("vtee");
        std::process::Command::new(bin)
            .env("TMPDIR", tmpdir)
            .arg("--headless")
            .args(["--name", name])
            .args(extra_args)
            .args(["--", "cat"])
            .spawn()
            .unwrap()
    }

    fn vtee_in(tmpdir: &std::path::Path) -> Command {
        let mut cmd = vtee();
        cmd.env("TMPDIR", tmpdir);
        cmd
    }

    /// The core naming behaviour (`docs/architecture.md`, "session naming"): a second session
    /// under the same name is rejected, matching tmux's own real default (confirmed against
    /// its source, `docs/decisions.md`) - no auto-suffixing, no silent attach.
    #[test]
    fn duplicate_session_name_is_rejected() {
        let tmpdir = isolated_tmpdir();
        let mut first = spawn_named(&tmpdir, "dup-test", &[]);
        std::thread::sleep(Duration::from_millis(300));

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
        std::thread::sleep(Duration::from_millis(800));

        let mut second = std::process::Command::new(assert_cmd::cargo::cargo_bin("vtee"))
            .env("TMPDIR", &tmpdir)
            .args(["--name", "attach-if-exists-test", "--attach-if-exists", "--", "cat"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(800));

        use std::io::Write as _;
        let mut stdin = second.stdin.take().unwrap();
        stdin.write_all(b"from-attacher\r").unwrap();
        stdin.flush().unwrap();
        std::thread::sleep(Duration::from_millis(800));
        drop(stdin);
        let _ = second.kill();
        let output = second.wait_with_output().unwrap();
        let out = String::from_utf8_lossy(&output.stdout);
        assert!(out.contains("from-attacher"), "expected the attached session's live echo in: {out:?}");

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
        std::thread::sleep(Duration::from_millis(300));

        vtee_in(&tmpdir)
            .arg("ls")
            .timeout(Duration::from_secs(5))
            .assert()
            .success()
            .stdout(predicate::str::contains("ls-kill-test").and(predicate::str::contains("live")));

        vtee_in(&tmpdir).args(["kill", "ls-kill-test"]).timeout(Duration::from_secs(5)).assert().success();

        // `kill` cleans up the files directly (`run_kill`'s own doc comment: `watch_sigterm`'s
        // handler calls `std::process::exit`, which would otherwise skip the holder's own
        // `SessionFiles` guard) - give the signalled process a moment to actually exit too.
        std::thread::sleep(Duration::from_millis(300));
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
        let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("vtee"))
            .env("TMPDIR", &tmpdir)
            .args(["--name", "should-not-appear", "--no-listen", "--no-stdin", "--", "cat"])
            .spawn()
            .unwrap();
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
        // `1` is guaranteed to exist (every running system has it) and guaranteed *not* to be
        // an vtee holder we spawned - the exact shape of "a stale pidfile pointing at a
        // since-reused PID" this test exists to catch, without needing a real PID-reuse race.
        std::fs::write(dir.join("faketest.pid"), "1").unwrap();

        vtee_in(&tmpdir).args(["kill", "faketest"]).timeout(Duration::from_secs(5)).assert().success();

        // The stale pidfile is cleaned up either way; the only thing this test actually proves
        // is that `kill` didn't crash or hang trying to signal PID 1 - a real signal to PID 1
        // would be either a permission error (as this test's own non-root process) or, if run
        // as root, exactly the kind of damage this fix exists to prevent. Checking the file is
        // gone confirms the normal cleanup path still ran.
        assert!(!dir.join("faketest.pid").exists());
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
        std::thread::sleep(Duration::from_millis(500));
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
        let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("vtee"))
            .env("TMPDIR", &tmpdir)
            .arg("--headless")
            .args(["--name", "holblock"])
            .args(["--", "yes"])
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(300));

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

        // Give `yes`'s output time to fill client A's queue past capacity and get it dropped,
        // while client B keeps draining its own.
        std::thread::sleep(Duration::from_millis(500));
        assert!(bytes_received.load(Ordering::Relaxed) > 0, "client B received nothing while client A was stalled");

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
}
