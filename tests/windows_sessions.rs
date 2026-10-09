//! Named sessions, `ls`/`kill`/`attach` and both sockets on native Windows (AF_UNIX via
//! `uds_windows`, `docs/decisions.md`). Each test gets its own `LOCALAPPDATA`, so sessions never
//! collide, and everything polls with a deadline. The tests share one lock: each one runs real
//! ConPTY children, and timing assertions get flaky when a dozen run at once.

#![cfg(windows)]

use std::io::{Read, Write};
use std::path::PathBuf;
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
        cmd.env("LOCALAPPDATA", &self.local);
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
        let stem = unique("child");
        let bat = std::env::temp_dir().join(format!("{stem}.cmd"));
        std::fs::write(&bat, "@echo off\r\nset /p a=ready?\r\necho got:%a%:done\r\nset /p b=hold?\r\n").unwrap();
        let child = env
            .cmd()
            .args(["--headless", "--name", name, "--", "cmd", "/c"])
            .arg(&bat)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let sock = env.sock(name);
        assert!(
            wait_until(Duration::from_secs(20), || UnixStream::connect(&sock).is_ok()),
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
    assert_eq!(std::fs::read_to_string(env.pidfile("alpha")).unwrap().trim(), holder.pid().to_string());

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
    let mut attach = env
        .cmd()
        .args(["attach", "att"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = attach.stdout.take().unwrap();
    let seen = std::sync::Arc::new(Mutex::new(Vec::<u8>::new()));
    let reader = {
        let seen = seen.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = stdout.read(&mut buf) {
                if n == 0 {
                    break;
                }
                seen.lock().unwrap().extend_from_slice(&buf[..n]);
            }
        })
    };
    let text = |seen: &Mutex<Vec<u8>>| strip_ansi(&seen.lock().unwrap());
    assert!(wait_until(Duration::from_secs(20), || text(&seen).contains("ready?")), "{:?}", text(&seen));
    let mut stdin = attach.stdin.take().unwrap();
    stdin.write_all(b"viaattach\r\n").unwrap();
    stdin.flush().unwrap();
    assert!(wait_until(Duration::from_secs(20), || text(&seen).contains("got:viaattach:done")), "{:?}", text(&seen));
    drop(stdin);
    assert!(
        wait_until(Duration::from_secs(10), || attach.try_wait().unwrap().is_some()),
        "attach didn't exit on stdin EOF"
    );
    let _ = reader.join();
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
/// today: the vendored ConPTY spawn calls `CreateProcessW` with `bInheritHandles = FALSE`, and
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
