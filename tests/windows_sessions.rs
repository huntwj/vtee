//! Named sessions, `ls`/`kill`/`attach` and both sockets on native Windows (AF_UNIX via
//! `uds_windows`, `docs/decisions.md`). Each test gets its own `LOCALAPPDATA`, so sessions never
//! collide, and everything polls with a deadline. The tests share one lock: each one runs real
//! ConPTY children, and timing assertions get flaky when a dozen run at once.

#![cfg(windows)]

mod common;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};
use vtee::net::UnixStream;

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if condition() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Text with terminal control sequences removed.
fn strip_ansi(bytes: &[u8]) -> String {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != 0x1b {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        i += 1;
        match bytes.get(i) {
            Some(b'[') => {
                i += 1;
                while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                    i += 1;
                }
                i += 1;
            }
            Some(b']') => {
                i += 1;
                while i < bytes.len() && bytes[i] != 0x07 && !(bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'\\')) {
                    i += 1;
                }
                i += if bytes.get(i) == Some(&0x07) { 1 } else { 2 };
            }
            Some(_) => i += 1,
            None => {}
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn unique(stem: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!("amws-{}-{}-{stem}", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// A per-test `LOCALAPPDATA`, removed on drop.
struct Env {
    local: PathBuf,
}

impl Env {
    fn new() -> Env {
        let local = std::env::temp_dir().join(unique("env"));
        std::fs::create_dir_all(&local).unwrap();
        Env { local }
    }

    fn dir(&self) -> PathBuf {
        self.local.join("vtee")
    }

    fn sock(&self, name: &str) -> PathBuf {
        self.dir().join(format!("{name}.sock"))
    }

    fn tty_sock(&self, name: &str) -> PathBuf {
        self.dir().join(format!("{name}.tty.sock"))
    }

    fn pidfile(&self, name: &str) -> PathBuf {
        self.dir().join(format!("{name}.pid"))
    }

    fn cmd(&self) -> Command {
        let mut cmd = Command::new(assert_cmd::cargo::cargo_bin("vtee"));
        cmd.env("LOCALAPPDATA", &self.local).env("APPDATA", self.local.join("roaming"));
        cmd
    }

    fn run(&self, args: &[&str]) -> (bool, String, String) {
        let out = self.cmd().args(args).stdin(Stdio::null()).output().unwrap();
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.local);
    }
}

/// PIDs of `cmd.exe` processes whose command line contains `needle`.
fn cmd_pids(needle: &str) -> Vec<u32> {
    let script = format!(
        "Get-CimInstance Win32_Process -Filter \"Name='cmd.exe'\" | \
         Where-Object {{ $_.CommandLine -like '*{needle}*' }} | ForEach-Object {{ $_.ProcessId }}"
    );
    let out = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .expect("couldn't list processes");
    String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.trim().parse().ok()).collect()
}

/// A headless session whose child asks for one line, answers it, then blocks on a second prompt
/// so the session stays alive until the test is done with it.
struct Holder {
    child: Child,
    stem: String,
}

impl Holder {
    fn start(env: &Env, name: &str) -> Holder {
        Holder::start_with(env, name, &[], &env.sock(name))
    }

    /// A holder whose NDJSON socket is at `custom` (`--listen=<path>`) instead of the session dir.
    fn start_listening(env: &Env, name: &str, custom: &Path) -> Holder {
        Holder::start_with(env, name, &[format!("--listen={}", custom.display())], custom)
    }

    /// Ready once `sock` answers and the record exists (it is written last, after both sockets are
    /// bound).
    fn start_with(env: &Env, name: &str, flags: &[String], sock: &Path) -> Holder {
        let stem = unique("child");
        let bat = std::env::temp_dir().join(format!("{stem}.cmd"));
        std::fs::write(&bat, "@echo off\r\nset /p a=ready?\r\necho got:%a%:done\r\nset /p b=hold?\r\n").unwrap();
        let child = env
            .cmd()
            .args(["--headless", "--name", name])
            .args(flags)
            .args(["--", "cmd", "/c"])
            .arg(&bat)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pidfile = env.pidfile(name);
        assert!(
            wait_until(Duration::from_secs(20), || pidfile.exists() && UnixStream::connect(sock).is_ok()),
            "session {name:?} never started listening"
        );
        Holder { child, stem }
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        for pid in cmd_pids(&self.stem) {
            let _ = Command::new("taskkill").args(["/F", "/T", "/PID", &pid.to_string()]).output();
        }
        let _ = std::fs::remove_file(std::env::temp_dir().join(format!("{}.cmd", self.stem)));
    }
}

/// Reads from `stream` until the control-sequence-stripped text contains `needle`.
fn read_until(stream: &mut UnixStream, seen: &mut Vec<u8>, needle: &str, timeout: Duration) -> bool {
    stream.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
    let deadline = Instant::now() + timeout;
    let mut buf = [0u8; 4096];
    loop {
        if strip_ansi(seen).contains(needle) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        match stream.read(&mut buf) {
            Ok(0) => return false,
            Ok(n) => seen.extend_from_slice(&buf[..n]),
            Err(_) => {}
        }
    }
}

/// Whether the other end hangs up (EOF or a reset) within `timeout`; data and read timeouts don't count.
fn peer_closed(stream: &mut UnixStream, timeout: Duration) -> bool {
    stream.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
    let deadline = Instant::now() + timeout;
    let mut buf = [0u8; 4096];
    while Instant::now() < deadline {
        match stream.read(&mut buf) {
            Ok(0) => return true,
            Ok(_) => {}
            Err(e) if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) => {}
            Err(_) => return true,
        }
    }
    false
}

#[test]
fn a_session_creates_its_sockets_and_pidfile_and_ls_shows_it_live() {
    let _guard = serial();
    let env = Env::new();
    let holder = Holder::start(&env, "alpha");

    assert!(env.sock("alpha").exists());
    assert!(env.tty_sock("alpha").exists());
    // The record: the holder's pid, then the sockets it actually bound.
    let record = std::fs::read_to_string(env.pidfile("alpha")).unwrap();
    let lines: Vec<&str> = record.lines().collect();
    assert_eq!(lines.len(), 3, "record: {record:?}");
    assert_eq!(lines[0], holder.pid().to_string());
    assert_eq!(PathBuf::from(lines[1]), env.sock("alpha"));
    assert_eq!(PathBuf::from(lines[2]), env.tty_sock("alpha"));

    let (ok, out, err) = env.run(&["ls"]);
    assert!(ok, "ls failed: {err}");
    assert!(out.contains("alpha") && out.contains("live"), "ls output: {out}");
}

#[test]
fn a_second_session_with_the_same_name_is_rejected() {
    let _guard = serial();
    let env = Env::new();
    let _holder = Holder::start(&env, "dup");
    let (ok, _, err) = env.run(&["--headless", "--name", "dup", "--", "cmd", "/c", "exit"]);
    assert!(!ok, "duplicate name was accepted");
    assert!(err.contains("duplicate"), "stderr: {err}");
}

#[test]
fn invalid_and_overlong_names_are_rejected_before_touching_disk() {
    let _guard = serial();
    let env = Env::new();
    let long = "x".repeat(120);
    for bad in ["..\\..\\evil", "C:\\evil", "..", ".", "a/b", long.as_str()] {
        let (ok, _, err) = env.run(&["--headless", "--name", bad, "--", "cmd", "/c", "exit"]);
        assert!(!ok, "{bad:?} was accepted");
        assert!(err.contains("session name"), "{bad:?}: stderr {err}");
    }
    assert!(!env.local.join("evil.sock").exists());
}

/// `:` would turn the pidfile into an NTFS alternate data stream; device names stay special even
/// with an extension. Each must be refused up front, with the reason, and leave nothing behind.
#[test]
fn names_windows_treats_specially_are_rejected_with_a_reason() {
    let _guard = serial();
    let env = Env::new();
    for bad in ["ab:c", "a*b", "a?b", "a\"b", "a|b", "a<b", ".hidden", "trailing.", "CON", "nul", "aux.x", "COM3"] {
        let (ok, _, err) = env.run(&["--headless", "--name", bad, "--", "cmd", "/c", "exit"]);
        assert!(!ok, "{bad:?} was accepted");
        assert!(err.contains("isn't a valid session name"), "{bad:?}: stderr {err}");
    }
    let leftovers: Vec<_> = std::fs::read_dir(env.dir()).map(|d| d.flatten().collect()).unwrap_or_default();
    assert!(leftovers.is_empty(), "files were created for rejected names: {leftovers:?}");
}

/// There is no private fallback location: without a usable `%LOCALAPPDATA%` it must refuse
/// rather than put sockets in a possibly shared temp dir.
#[test]
fn a_missing_localappdata_is_an_error_not_a_fallback() {
    let _guard = serial();
    let env = Env::new();
    let out = env.cmd().env_remove("LOCALAPPDATA").arg("ls").stdin(Stdio::null()).output().unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("LOCALAPPDATA"), "stderr: {err}");
}

/// A session directory that already uses up the 107-byte socket address: the error must blame the
/// directory (not the name), and `kill` must answer like it does for any session that isn't there.
#[test]
fn a_localappdata_too_long_for_a_socket_path_is_blamed_not_the_name() {
    let _guard = serial();
    let env = Env::new();
    let long = env.local.join("x".repeat(90));
    std::fs::create_dir_all(&long).unwrap();

    let out = env.cmd().env("LOCALAPPDATA", &long).args(["kill", "a"]).stdin(Stdio::null()).output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(68), "stderr: {err}");
    assert!(err.contains("session directory") && err.contains("LOCALAPPDATA"), "stderr: {err}");
    assert!(!err.contains("session name \"a\" is too long"), "stderr: {err}");

    let out = env
        .cmd()
        .env("LOCALAPPDATA", &long)
        .args(["--headless", "--name", "a", "--", "cmd", "/c", "exit"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("session directory"));
}

#[test]
fn connect_returns_a_snapshot_of_the_live_session() {
    let _guard = serial();
    let env = Env::new();
    let _holder = Holder::start(&env, "snap");
    let sock = env.sock("snap");
    let mut child = env
        .cmd()
        .args(["--connect"])
        .arg(&sock)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(b"{\"cmd\":\"wait_for\",\"text\":\"ready?\",\"timeout_ms\":15000}\n").unwrap();
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("\"matched\":true"), "wait_for output: {text}");
}

/// Windows PowerShell 5.1 can send the UTF-8 byte order mark twice; and a line the server cannot
/// parse must make `--connect` exit non-zero (65, DATAERR) instead of looking like success.
#[test]
fn connect_strips_a_doubled_mark_and_fails_on_unparseable_input() {
    let _guard = serial();
    let env = Env::new();
    let _holder = Holder::start(&env, "bom2");
    let sock = env.sock("bom2");
    let run = |input: &[u8]| {
        let mut child = env
            .cmd()
            .args(["--connect"])
            .arg(&sock)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(input).unwrap();
        drop(stdin);
        child.wait_with_output().unwrap()
    };

    let out = run(b"\xef\xbb\xbf\xef\xbb\xbf{\"cmd\":\"wait_for\",\"text\":\"ready?\",\"timeout_ms\":15000}\n");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success() && text.contains("\"matched\":true"), "doubled mark: {text}");

    let out = run(b"this is not json\n");
    assert_eq!(out.status.code(), Some(65), "unparseable input must exit DATAERR");
    assert!(String::from_utf8_lossy(&out.stdout).contains("bad command"));
}

#[test]
fn a_raw_tty_client_gets_a_redraw_sees_live_output_and_can_type() {
    let _guard = serial();
    let env = Env::new();
    let _holder = Holder::start(&env, "tty");
    let mut client = UnixStream::connect(env.tty_sock("tty")).unwrap();
    let mut seen = Vec::new();
    assert!(
        read_until(&mut client, &mut seen, "ready?", Duration::from_secs(20)),
        "no redraw: {:?}",
        strip_ansi(&seen)
    );
    client.write_all(b"hello\r\n").unwrap();
    assert!(
        read_until(&mut client, &mut seen, "got:hello:done", Duration::from_secs(20)),
        "typing didn't reach the child: {:?}",
        strip_ansi(&seen)
    );
}

#[test]
fn two_clients_of_each_kind_see_the_same_session() {
    let _guard = serial();
    let env = Env::new();
    let _holder = Holder::start(&env, "multi");
    let mut a = UnixStream::connect(env.tty_sock("multi")).unwrap();
    let mut b = UnixStream::connect(env.tty_sock("multi")).unwrap();
    let (mut seen_a, mut seen_b) = (Vec::new(), Vec::new());
    assert!(read_until(&mut a, &mut seen_a, "ready?", Duration::from_secs(20)));
    assert!(read_until(&mut b, &mut seen_b, "ready?", Duration::from_secs(20)));

    // An NDJSON client alongside the two TTY clients.
    let mut json = UnixStream::connect(env.sock("multi")).unwrap();
    json.write_all(b"{\"cmd\":\"snapshot\"}\n").unwrap();
    let mut seen_json = Vec::new();
    assert!(read_until(&mut json, &mut seen_json, "\"cols\"", Duration::from_secs(10)));

    a.write_all(b"both\r\n").unwrap();
    assert!(read_until(&mut a, &mut seen_a, "got:both:done", Duration::from_secs(20)), "{:?}", strip_ansi(&seen_a));
    assert!(read_until(&mut b, &mut seen_b, "got:both:done", Duration::from_secs(20)), "{:?}", strip_ansi(&seen_b));
}

#[test]
fn the_attach_subcommand_relays_both_ways() {
    let _guard = serial();
    let env = Env::new();
    let _holder = Holder::start(&env, "att");
    // Attach needs a terminal, so it runs on a ConPTY.
    let attach = common::PtyProc::spawn(env.cmd().args(["attach", "att"]));
    let text = |attach: &common::PtyProc| strip_ansi(attach.text().as_bytes());
    assert!(wait_until(Duration::from_secs(20), || text(&attach).contains("ready?")), "{:?}", text(&attach));
    attach.send(b"viaattach\r\n");
    assert!(
        wait_until(Duration::from_secs(20), || text(&attach).contains("got:viaattach:done")),
        "{:?}",
        text(&attach)
    );

    // Leaving an attach session (here: killing the client) must not end the session itself.
    drop(attach);
    let (ok, out, _) = env.run(&["ls"]);
    assert!(ok && out.contains("att") && out.contains("live"), "the session should outlive its client: {out:?}");
}

/// `attach`'s reader thread sees EOF as soon as the session ends, but the main thread is blocked
/// reading attach's own stdin with no portable way to wake it up early - so when the session
/// ends, attach must exit by itself rather than sit in raw mode until the next keystroke.
#[test]
fn attach_exits_by_itself_once_the_session_ends() {
    let _guard = serial();
    let env = Env::new();
    let _holder = Holder::start(&env, "ends-under-attach");
    let mut attach = common::PtyProc::spawn(env.cmd().args(["attach", "ends-under-attach"]));
    let text = |attach: &common::PtyProc| strip_ansi(attach.text().as_bytes());
    assert!(wait_until(Duration::from_secs(20), || text(&attach).contains("ready?")), "{:?}", text(&attach));

    let (ok, _, err) = env.run(&["kill", "ends-under-attach"]);
    assert!(ok, "kill failed: {err}");

    assert!(
        wait_until(Duration::from_secs(20), || attach.has_exited()),
        "attach did not exit on its own after the session ended"
    );
}

#[test]
fn kill_removes_the_files_and_ends_the_holder_and_its_child() {
    let _guard = serial();
    let env = Env::new();
    let mut holder = Holder::start(&env, "victim");
    assert!(!cmd_pids(&holder.stem).is_empty(), "the child never started");

    let (ok, _, err) = env.run(&["kill", "victim"]);
    assert!(ok, "kill failed: {err}");

    assert!(wait_until(Duration::from_secs(10), || holder.child.try_wait().unwrap().is_some()), "holder survived kill");
    assert!(wait_until(Duration::from_secs(10), || cmd_pids(&holder.stem).is_empty()), "the child outlived kill");
    for path in [env.sock("victim"), env.tty_sock("victim"), env.pidfile("victim")] {
        assert!(!path.exists(), "{} was left behind", path.display());
    }
    let (_, out, _) = env.run(&["ls"]);
    assert!(out.contains("no sessions"), "ls output: {out}");
}

/// A force-killed holder leaves stale files behind; `ls` flags them, `kill` cleans them up
/// without signalling anything, and the name can be reused.
#[test]
fn stale_files_are_flagged_cleaned_and_the_name_is_reusable() {
    let _guard = serial();
    let env = Env::new();
    let mut holder = Holder::start(&env, "stale");
    holder.child.kill().unwrap();
    let _ = holder.child.wait();
    assert!(env.pidfile("stale").exists(), "a force-killed holder shouldn't have cleaned up");

    // Refused, not just "no longer listening": the socket file is still on disk.
    assert!(wait_until(Duration::from_secs(10), || UnixStream::connect(env.sock("stale")).is_err()));
    let (_, out, _) = env.run(&["ls"]);
    assert!(out.contains("stale") && !out.contains("live"), "ls output: {out}");

    let (ok, _, err) = env.run(&["kill", "stale"]);
    assert!(ok, "kill of a stale entry failed: {err}");
    assert!(!env.pidfile("stale").exists() && !env.sock("stale").exists());

    let _again = Holder::start(&env, "stale");
}

/// Peers see EOF/refusal once the holder dies: no socket handle leaked into the child (spawned
/// before any socket exists, so it cannot inherit one) keeps the session half-alive.
#[test]
fn killing_the_holder_closes_its_sockets_for_every_peer() {
    let _guard = serial();
    let env = Env::new();
    let mut holder = Holder::start(&env, "eof");
    let mut tty = UnixStream::connect(env.tty_sock("eof")).unwrap();
    let mut json = UnixStream::connect(env.sock("eof")).unwrap();
    let mut seen = Vec::new();
    assert!(read_until(&mut tty, &mut seen, "ready?", Duration::from_secs(20)));

    holder.child.kill().unwrap();
    let _ = holder.child.wait();

    for (label, stream) in [("tty", &mut tty), ("json", &mut json)] {
        assert!(peer_closed(stream, Duration::from_secs(10)), "{label} client never saw the session end");
    }
    assert!(
        wait_until(Duration::from_secs(10), || UnixStream::connect(env.tty_sock("eof")).is_err()),
        "something still accepts connections after the holder died"
    );
}

/// The scoping report's riskiest unknown: does a socket handle leak into a process spawned
/// afterwards (Windows inherits handles at `CreateProcess`, unlike Unix's close-on-exec)? Not
/// today: `portable-pty`'s ConPTY spawn calls `CreateProcessW` with `bInheritHandles = FALSE`, and
/// the session sockets are only bound after the child exists anyway. This pins the measurement
/// (bind, spawn a long-lived child the way `Tap::spawn` does, drop the listener: the socket must
/// stop accepting while the child lives) so a change to either of those shows up.
#[test]
fn sockets_are_not_inherited_by_children_spawned_afterwards() {
    let _guard = serial();
    let env = Env::new();
    std::fs::create_dir_all(env.dir()).unwrap();
    let path = env.dir().join("inherit.sock");
    let listener = vtee::net::UnixListener::bind(&path).unwrap();
    let accepted = std::thread::spawn({
        let listener = listener.try_clone().unwrap();
        move || listener.accept().map(|(s, _)| s)
    });
    let client = UnixStream::connect(&path).unwrap();
    let server_side = accepted.join().unwrap().unwrap();

    // Spawned the way `Tap::spawn` spawns its child: through the ConPTY backend, which does not
    // restrict inherited handles the way `std::process::Command` does.
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};
    let pair = native_pty_system().openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 }).unwrap();
    let mut cmd = CommandBuilder::new("ping");
    cmd.args(["-n", "30", "127.0.0.1"]);
    let mut sleeper = pair.slave.spawn_command(cmd).unwrap();
    let _drain = pair.master.try_clone_reader().unwrap();
    std::thread::sleep(Duration::from_millis(500));

    drop(listener);
    drop(server_side);
    let mut client = client;
    let hung_up = peer_closed(&mut client, Duration::from_secs(5));
    let refused = wait_until(Duration::from_secs(5), || UnixStream::connect(&path).is_err());
    let child_alive = sleeper.try_wait().unwrap().is_none();
    let _ = sleeper.kill();
    let _ = sleeper.wait();
    drop(pair);
    assert!(child_alive, "the probe child exited early, so this measured nothing");
    assert!(hung_up, "an accepted socket leaked into a later child: the peer never saw EOF");
    assert!(refused, "the listener leaked into a later child: it still accepts connections");
}

/// `--listen=<path>` moves the NDJSON socket out of the session directory; `ls`, `--connect` and
/// `kill` must follow the socket the holder recorded rather than the conventional one.
#[test]
fn a_listen_session_is_live_in_ls_connectable_and_killable() {
    let _guard = serial();
    let env = Env::new();
    let custom = env.local.join("custom.sock");
    let mut holder = Holder::start_listening(&env, "remote", &custom);
    assert!(!env.sock("remote").exists(), "the conventional socket shouldn't exist: --listen replaced it");

    let (ok, out, err) = env.run(&["ls"]);
    assert!(ok, "ls failed: {err}");
    assert!(out.contains("remote") && out.contains("live") && !out.contains("stale"), "ls output: {out}");
    assert!(out.contains(&format!("sock={}", custom.display())), "ls should show the real socket: {out}");

    let mut connect =
        env.cmd().arg("--connect").arg(&custom).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut stdin = connect.stdin.take().unwrap();
    stdin.write_all(b"{\"cmd\":\"wait_for\",\"text\":\"ready?\",\"timeout_ms\":15000}\n").unwrap();
    drop(stdin);
    let out = connect.wait_with_output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("\"matched\":true"), "{:?}", out.stdout);

    let (ok, _, err) = env.run(&["kill", "remote"]);
    assert!(ok, "kill failed: {err}");
    assert!(wait_until(Duration::from_secs(10), || holder.child.try_wait().unwrap().is_some()), "holder survived kill");
    assert!(wait_until(Duration::from_secs(10), || cmd_pids(&holder.stem).is_empty()), "the child outlived kill");
    for path in [custom, env.tty_sock("remote"), env.pidfile("remote")] {
        assert!(!path.exists(), "{} was left behind", path.display());
    }
}

/// A force-killed `--listen` holder is stale; `kill` cleans its custom socket and, since the
/// recorded socket is dead, signals nothing even though the record names a live process.
#[test]
fn a_killed_listen_session_is_stale_and_kill_cleans_it_without_signalling_anything() {
    let _guard = serial();
    let env = Env::new();
    let custom = env.local.join("custom.sock");
    let mut holder = Holder::start_listening(&env, "remote-stale", &custom);
    holder.child.kill().unwrap();
    let _ = holder.child.wait();
    assert!(custom.exists(), "a force-killed holder shouldn't have cleaned up");
    assert!(wait_until(Duration::from_secs(10), || UnixStream::connect(&custom).is_err()));

    let (_, out, _) = env.run(&["ls"]);
    assert!(out.contains("stale") && !out.contains("live"), "ls output: {out}");
    assert!(out.contains(&format!("sock={}", custom.display())), "ls output: {out}");

    let mut sentinel = Command::new("ping")
        .args(["-n", "120", "127.0.0.1"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let pidfile = env.pidfile("remote-stale");
    std::fs::write(
        &pidfile,
        format!("{}\n{}\n{}\n", sentinel.id(), custom.display(), env.tty_sock("remote-stale").display()),
    )
    .unwrap();
    // Old enough that only the liveness check, not the creation-time guard, protects the sentinel.
    let long_ago = std::time::SystemTime::now() - Duration::from_secs(3600);
    std::fs::OpenOptions::new().write(true).open(&pidfile).unwrap().set_modified(long_ago).unwrap();

    let (ok, _, err) = env.run(&["kill", "remote-stale"]);
    assert!(ok, "kill failed: {err}");
    std::thread::sleep(Duration::from_millis(500));
    let survived = sentinel.try_wait().unwrap().is_none();
    let _ = sentinel.kill();
    let _ = sentinel.wait();
    assert!(survived, "kill signalled a process because of a stale record");
    assert!(!custom.exists() && !pidfile.exists(), "the stale files should be cleaned up");
}

/// `--listen=<path>` naming a path that is already a live socket must be refused, not stolen from
/// the session already listening there.
#[test]
fn listen_refuses_to_take_over_a_path_that_is_already_a_live_socket() {
    let _guard = serial();
    let env = Env::new();
    let custom = env.local.join("custom.sock");
    let _first = Holder::start_listening(&env, "listen-live", &custom);

    let env2 = Env::new();
    let (ok, _, err) = env2.run(&[
        "--headless",
        "--name",
        "listen-live-2",
        &format!("--listen={}", custom.display()),
        "--",
        "cmd",
        "/c",
        "exit",
    ]);
    assert!(!ok, "a live socket was silently taken over");
    assert!(err.contains("already in use by a live session"), "stderr: {err}");

    // The first session must be untouched: still there, still answering.
    assert!(custom.exists());
    assert!(UnixStream::connect(&custom).is_ok());
}

/// `--listen=<path>` naming a path that is an ordinary file must be refused, not deleted to make
/// room for the socket.
#[test]
fn listen_refuses_to_delete_a_path_that_is_not_a_socket() {
    let _guard = serial();
    let env = Env::new();
    std::fs::create_dir_all(&env.local).unwrap();
    let custom = env.local.join("custom.sock");
    std::fs::write(&custom, "do not touch me\r\n").unwrap();

    let (ok, _, err) = env.run(&[
        "--headless",
        "--name",
        "listen-not-a-socket",
        &format!("--listen={}", custom.display()),
        "--",
        "cmd",
        "/c",
        "exit",
    ]);
    assert!(!ok, "a non-socket file was silently deleted");
    assert!(err.contains("is not a socket"), "stderr: {err}");
    assert_eq!(std::fs::read_to_string(&custom).unwrap(), "do not touch me\r\n", "the file must be left alone");
}

/// A file holding only the pid, as older versions wrote it, still resolves to the conventional
/// sockets.
#[test]
fn a_legacy_pid_only_pidfile_still_works() {
    let _guard = serial();
    let env = Env::new();
    let mut holder = Holder::start(&env, "legacy");
    std::fs::write(env.pidfile("legacy"), holder.pid().to_string()).unwrap();

    let (_, out, _) = env.run(&["ls"]);
    assert!(out.contains(&format!("legacy\tpid={}\tlive", holder.pid())), "ls output: {out}");
    assert!(!out.contains("sock="), "ls output: {out}");

    let (ok, _, err) = env.run(&["kill", "legacy"]);
    assert!(ok, "kill failed: {err}");
    assert!(wait_until(Duration::from_secs(10), || holder.child.try_wait().unwrap().is_some()), "holder survived kill");
}

/// A pidfile naming a process younger than the pidfile itself is a reused PID: `kill` must clean
/// up the session files but leave that process alone.
#[test]
fn kill_leaves_a_process_younger_than_the_pidfile_alone() {
    let _guard = serial();
    let env = Env::new();
    let _holder = Holder::start(&env, "reused");
    let mut sentinel = Command::new("ping")
        .args(["-n", "120", "127.0.0.1"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();

    let pidfile = env.pidfile("reused");
    std::fs::write(&pidfile, sentinel.id().to_string()).unwrap();
    let long_ago = std::time::SystemTime::now() - Duration::from_secs(3600);
    std::fs::OpenOptions::new().write(true).open(&pidfile).unwrap().set_modified(long_ago).unwrap();

    let (ok, _, err) = env.run(&["kill", "reused"]);
    assert!(ok, "kill failed: {err}");
    std::thread::sleep(Duration::from_millis(500));
    assert!(sentinel.try_wait().unwrap().is_none(), "kill terminated an unrelated, younger process");
    assert!(!pidfile.exists(), "the session files should still be cleaned up");

    let _ = sentinel.kill();
    let _ = sentinel.wait();
}

/// Bare `attach` follows the session attached to last: each attach bumps its record's mtime, and
/// the resolver picks the newest. Judged by which record gets touched, since both holders run the
/// same child.
#[test]
fn bare_attach_picks_the_session_attached_to_most_recently() {
    let _guard = serial();
    let env = Env::new();
    let _older = Holder::start(&env, "older");
    let _newer = Holder::start(&env, "newer");
    let mtime = |name: &str| std::fs::metadata(env.pidfile(name)).and_then(|m| m.modified()).unwrap();

    let by_name = common::PtyProc::spawn(env.cmd().args(["attach", "older"]));
    assert!(by_name.wait_for_plain("ready?", Duration::from_secs(20)), "{:?}", by_name.plain_text());
    drop(by_name);
    assert!(wait_until(Duration::from_secs(10), || mtime("older") > mtime("newer")), "attach didn't touch the record");

    let (older_before, newer_before) = (mtime("older"), mtime("newer"));
    let bare = common::PtyProc::spawn(env.cmd().arg("attach"));
    assert!(bare.wait_for_plain("ready?", Duration::from_secs(20)), "{:?}", bare.plain_text());
    assert!(
        wait_until(Duration::from_secs(10), || mtime("older") > older_before),
        "bare attach should have gone to the session attached to last"
    );
    assert_eq!(mtime("newer"), newer_before, "bare attach must not touch the other session");
}

/// A live session can be scripted from outside (`--connect` with `--script`), its screen recorded as a golden, and the
/// same screen checked again by asking for the session's screen now (`--connect` alone).
#[test]
fn a_live_session_can_be_scripted_recorded_and_checked() {
    let _guard = serial();
    let env = Env::new();
    let _holder = Holder::start(&env, "expect");
    let sock = env.sock("expect");
    let script = env.local.join("s.ndjson");
    std::fs::write(
        &script,
        "{\"cmd\":\"wait_for\",\"text\":\"ready?\",\"timeout_ms\":15000}\n{\"cmd\":\"snapshot\",\"name\":\"prompt\"}\n",
    )
    .unwrap();

    let run = |args: &[&str]| {
        env.cmd()
            .current_dir(&env.local)
            .env_remove("CI")
            .env("NO_COLOR", "1")
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    let sock = sock.to_str().unwrap();
    let out = run(&["--connect", sock, "--script", "s.ndjson", "--expect", "live.golden", "--update", "--portable"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n"), "CREATED live.golden: 1 screen\n");
    let golden = std::fs::read_to_string(env.local.join("live.golden")).unwrap();
    assert!(golden.contains("name \"prompt\"") && golden.contains("ready?"), "{golden}");

    let out = run(&["--connect", sock, "--expect", "live.golden"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stdout));
    assert_eq!(String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n"), "PASS live.golden (1 screen)\n");

    let out = run(&["--connect", sock, "--script", "s.ndjson", "--expect", "live.golden"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stdout));
}
