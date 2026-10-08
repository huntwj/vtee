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
    vtee().assert().failure().code(exitcode::USAGE).stderr(predicate::str::contains("COMMAND"));
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

#[cfg(unix)]
mod unix_behavior {
    use super::*;

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
        let sockets: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "sock"))
            .collect();
        assert_eq!(sockets.len(), 1, "expected exactly one .sock file in {}", dir.display());

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

        let sock = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.path().extension().is_some_and(|ext| ext == "sock"))
            .expect("expected a socket to connect to")
            .path();

        let output = vtee().arg("--connect").arg(&sock).write_stdin("{\"cmd\":\"snapshot\"}\n").output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("\"cols\":97"), "snapshot didn't reflect --cols: {stdout}");
        assert!(stdout.contains("\"rows\":31"), "snapshot didn't reflect --rows: {stdout}");

        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&tmpdir);
    }
}
