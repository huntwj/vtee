//! End-to-end session tests: DSR answering with and without an attach client, several clients
//! of both protocols at once, real full-screen programs, holder lifecycle, and stress/fuzz.
//! Each test runs its own holder in a private `TMPDIR`; everything polls with a deadline.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

mod common;

const BIN: &str = env!("CARGO_BIN_EXE_vtee");

fn wait_until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn unique(prefix: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{prefix}-{}-{n}", std::process::id()))
}

fn connect_retry(path: &Path) -> UnixStream {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match UnixStream::connect(path) {
            Ok(s) => return s,
            Err(e) => {
                assert!(Instant::now() < deadline, "couldn't connect to {}: {e}", path.display());
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

fn spawn_holder(tmp: &Path, name: &str, flags: &[&str], program: &[&str]) -> Child {
    Command::new(BIN)
        .env("TMPDIR", tmp)
        .arg("--headless")
        .args(["--name", name])
        .args(flags)
        .arg("--")
        .args(program)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .unwrap()
}

fn vtee_in(tmp: &Path) -> Command {
    let mut cmd = Command::new(BIN);
    cmd.env("TMPDIR", tmp);
    cmd
}

fn terminate(child: &mut Child) {
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// A running holder in its own `TMPDIR`; torn down on drop.
struct Session {
    tmp: PathBuf,
    name: String,
    child: Child,
    owns_tmp: bool,
}

impl Session {
    fn start(name: &str, flags: &[&str], program: &[&str]) -> Session {
        let tmp = unique("vtee-linux");
        std::fs::create_dir_all(&tmp).unwrap();
        let child = spawn_holder(&tmp, name, flags, program);
        let s = Session { tmp, name: name.to_string(), child, owns_tmp: true };
        s.wait_ready();
        s
    }

    /// A second holder in an existing `TMPDIR` (the first one's directory is not removed by it).
    fn start_in(tmp: &Path, name: &str, flags: &[&str], program: &[&str]) -> Session {
        let child = spawn_holder(tmp, name, flags, program);
        let s = Session { tmp: tmp.to_path_buf(), name: name.to_string(), child, owns_tmp: false };
        s.wait_ready();
        s
    }

    /// A holder whose NDJSON socket is at `custom` (`--listen=<path>`) instead of in the session
    /// directory. Ready once the record is written (it is written last, after both sockets are
    /// bound) and the custom socket answers.
    fn start_listening(name: &str, custom: &Path, program: &[&str]) -> Session {
        let tmp = unique("vtee-linux");
        std::fs::create_dir_all(&tmp).unwrap();
        let child = spawn_holder(&tmp, name, &[&format!("--listen={}", custom.display())], program);
        let s = Session { tmp, name: name.to_string(), child, owns_tmp: true };
        let pidfile = s.pidfile();
        wait_until("the session record and custom socket", || pidfile.exists() && UnixStream::connect(custom).is_ok());
        s
    }

    fn dir(&self) -> PathBuf {
        let uid = unsafe { libc::getuid() };
        self.tmp.join(format!("vtee-{uid}"))
    }

    fn pidfile(&self) -> PathBuf {
        self.dir().join(format!("{}.pid", self.name))
    }

    fn sock(&self) -> PathBuf {
        self.dir().join(format!("{}.sock", self.name))
    }

    fn tty_sock(&self) -> PathBuf {
        self.dir().join(format!("{}.tty.sock", self.name))
    }

    /// Ready once the NDJSON socket answers and the TTY socket file exists. The TTY socket is
    /// deliberately not connected to: that would register (then drop) a sink.
    fn wait_ready(&self) {
        let sock = self.sock();
        let tty = self.tty_sock();
        wait_until("session sockets", || tty.exists() && sock.exists() && UnixStream::connect(&sock).is_ok());
    }

    fn nd(&self) -> Nd {
        Nd::connect(&self.sock())
    }

    fn tty(&self) -> Tty {
        Tty::connect(&self.tty_sock())
    }

    fn files_exist(&self) -> bool {
        ["sock", "tty.sock", "pid"].iter().any(|ext| self.dir().join(format!("{}.{ext}", self.name)).exists())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        terminate(&mut self.child);
        if self.owns_tmp {
            let _ = std::fs::remove_dir_all(&self.tmp);
        }
    }
}

/// An NDJSON client.
struct Nd {
    w: UnixStream,
    r: BufReader<UnixStream>,
}

impl Nd {
    fn connect(path: &Path) -> Nd {
        let s = connect_retry(path);
        s.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
        Nd { r: BufReader::new(s.try_clone().unwrap()), w: s }
    }

    fn send(&mut self, v: &Value) {
        writeln!(self.w, "{v}").unwrap();
    }

    fn recv(&mut self) -> Value {
        let mut line = String::new();
        self.r.read_line(&mut line).expect("no reply from the session");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("bad reply {line:?}: {e}"))
    }

    fn request(&mut self, v: Value) -> Value {
        self.send(&v);
        self.recv()
    }

    fn snapshot(&mut self) -> Value {
        self.request(json!({"cmd": "snapshot"}))["snapshot"].clone()
    }

    fn wait_for(&mut self, text: &str, timeout_ms: u64) -> bool {
        self.request(json!({"cmd": "wait_for", "text": text, "timeout_ms": timeout_ms}))["matched"]
            .as_bool()
            .expect("wait_for reply")
    }

    /// `send`/`resize` get no reply on success; a snapshot request is the barrier.
    fn send_text(&mut self, text: &str) {
        self.send(&json!({"cmd": "send", "text": text}));
        self.snapshot();
    }
}

fn screen(snapshot: &Value) -> String {
    snapshot["lines"].as_array().unwrap().iter().map(|l| l.as_str().unwrap()).collect::<Vec<_>>().join("\n")
}

/// A raw-TTY client collecting everything the session sends it.
struct Tty {
    w: UnixStream,
    buf: Arc<Mutex<Vec<u8>>>,
}

impl Tty {
    fn connect(path: &Path) -> Tty {
        let s = connect_retry(path);
        let mut reader = s.try_clone().unwrap();
        let buf = Arc::new(Mutex::new(Vec::new()));
        let sink = buf.clone();
        std::thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            while let Ok(n) = reader.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                sink.lock().unwrap().extend_from_slice(&chunk[..n]);
            }
        });
        let tty = Tty { w: s, buf };
        // The redraw is queued at registration, so seeing it means this client is a live sink.
        tty.wait_raw(b"\x1b[2J");
        tty
    }

    fn write(&mut self, bytes: &[u8]) {
        self.w.write_all(bytes).unwrap();
    }

    fn raw(&self) -> Vec<u8> {
        self.buf.lock().unwrap().clone()
    }

    fn text(&self) -> String {
        strip_ansi(&self.raw())
    }

    fn wait_raw(&self, needle: &[u8]) {
        wait_until(&format!("{:?} in the raw TTY stream", String::from_utf8_lossy(needle)), || {
            self.raw().windows(needle.len()).any(|w| w == needle)
        });
    }

    fn wait_text(&self, needle: &str) {
        wait_until(&format!("{needle:?} on a TTY client"), || self.text().contains(needle));
    }
}

fn strip_ansi(raw: &[u8]) -> String {
    let mut out = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] != 0x1b {
            out.push(raw[i]);
            i += 1;
            continue;
        }
        i += 1;
        match raw.get(i) {
            Some(b'[') => {
                i += 1;
                while i < raw.len() && !(0x40..=0x7e).contains(&raw[i]) {
                    i += 1;
                }
                i += 1;
            }
            Some(b']') => {
                i += 1;
                while i < raw.len() && raw[i] != 0x07 && raw[i] != 0x1b {
                    i += 1;
                }
                i += if raw.get(i) == Some(&0x1b) { 2 } else { 1 };
            }
            Some(b'(') | Some(b')') => i += 2,
            Some(_) => i += 1,
            None => {}
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn procs_running(argv0: &str, arg: &str) -> Vec<u32> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else { return found };
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else { continue };
        let Ok(cmdline) = std::fs::read(entry.path().join("cmdline")) else { continue };
        let args: Vec<&[u8]> = cmdline.split(|b| *b == 0).collect();
        let first = args.first().map(|a| String::from_utf8_lossy(a).into_owned()).unwrap_or_default();
        if first.rsplit('/').next() == Some(argv0) && args.contains(&arg.as_bytes()) {
            found.push(pid);
        }
    }
    found
}

fn on_path(binary: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(binary).exists()))
}

/// Tiny deterministic PRNG so fuzz failures reproduce from the seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}

// ---------------------------------------------------------------------------------------------
// 1. Who answers the child's cursor-position query

const DSR_REPORT: &str =
    "printf 'got=%d esc=%d body=<%s>\\n' \"${#x}\" \"$(printf %s \"$x\" | tr -cd '\\033' | wc -c)\" \"${x:1}\"";

fn dsr_child(wait_for_go: bool) -> String {
    let go = if wait_for_go { "IFS= read -rsn1 go; " } else { "" };
    format!("stty -echo -icanon; {go}printf '\\033[6n'; sleep 1; IFS= read -rs -t 1 -n 100 x; {DSR_REPORT}; sleep 120")
}

fn parse_dsr(screen: &str) -> (usize, usize, String) {
    let start = screen.rfind("got=").unwrap_or_else(|| panic!("no got= line in {screen:?}"));
    let line = screen[start..].lines().next().unwrap().trim_end();
    let got = line["got=".len()..line.find(" esc=").unwrap()].parse().unwrap();
    let esc = line[line.find(" esc=").unwrap() + 5..line.find(" body=<").unwrap()].parse().unwrap();
    let body = line[line.find("body=<").unwrap() + 6..line.rfind('>').unwrap()].to_string();
    (got, esc, body)
}

#[test]
fn without_an_attach_client_the_tap_answers_the_query_exactly_once() {
    let session = Session::start("dsr-solo", &[], &["bash", "-c", &dsr_child(false)]);
    let mut nd = session.nd();
    assert!(nd.wait_for("body=<", 15_000), "child never reported what it read");
    let (got, esc, body) = parse_dsr(&screen(&nd.snapshot()));
    assert_eq!(esc, 1, "expected exactly one reply (one ESC), got {esc}");
    assert_eq!(got, body.len() + 1);
    assert!(body.starts_with('[') && body.ends_with('R'), "not a cursor position report: {body:?}");
}

#[test]
fn with_an_attach_client_only_the_clients_reply_reaches_the_child() {
    let session = Session::start("dsr-attached", &[], &["bash", "-c", &dsr_child(true)]);
    let mut tty = session.tty();
    tty.write(b"g");
    tty.wait_raw(b"\x1b[6n");
    tty.write(b"\x1b[7;9R");
    tty.wait_text("body=<");
    let (got, esc, body) = parse_dsr(&tty.text());
    assert_eq!(esc, 1, "the tap must not add its own reply while a client is attached");
    assert_eq!(body, "[7;9R", "child should see only the client's reply");
    assert_eq!(got, 6);
}

// ---------------------------------------------------------------------------------------------
// 2. Several clients at once

const ECHO_CHILD: &str = "stty -echo; echo ready; while IFS= read -r l; do echo \"out:$l\"; done";

#[test]
fn many_clients_of_both_kinds_see_the_same_output_and_survive_each_others_disconnects() {
    let session = Session::start("multi", &["--writable"], &["bash", "-c", ECHO_CHILD]);
    let mut ttys: Vec<Tty> = (0..3).map(|_| session.tty()).collect();
    let mut nds: Vec<Nd> = (0..2).map(|_| session.nd()).collect();
    assert!(nds[0].wait_for("ready", 15_000));

    nds[0].send_text("hello\r");
    for tty in &ttys {
        tty.wait_text("out:hello");
    }
    for nd in &mut nds {
        assert!(nd.wait_for("out:hello", 15_000));
    }

    // Attaching mid-session: the redraw shows what's already on screen, then live output follows.
    let late = session.tty();
    late.wait_text("out:hello");
    nds[1].send_text("world\r");
    late.wait_text("out:world");
    for tty in &ttys {
        tty.wait_text("out:world");
    }

    // One TTY client and one NDJSON client vanish abruptly (the latter mid-wait).
    drop(ttys.remove(1));
    nds[1].send(&json!({"cmd": "wait_for", "text": "never-appears", "timeout_ms": 60_000}));
    drop(nds.remove(1));

    // Typing through a TTY client works and everyone else still gets the output.
    ttys[0].write(b"again\r");
    for tty in ttys.iter().chain(std::iter::once(&late)) {
        tty.wait_text("out:again");
    }
    assert!(nds[0].wait_for("out:again", 15_000));
}

#[test]
fn exclusive_write_hands_off_between_clients_and_releases_on_disconnect() {
    let session = Session::start("excl", &["--writable=exclusive"], &["bash", "-c", ECHO_CHILD]);
    let mut a = session.nd();
    let mut b = session.nd();
    assert!(a.wait_for("ready", 15_000));

    let denied = a.request(json!({"cmd": "send", "text": "x"}));
    assert_eq!(denied["cmd"], "error");
    assert!(denied["message"].as_str().unwrap().contains("not held"), "{denied}");

    assert_eq!(a.request(json!({"cmd": "acquire_write"}))["acquired"], true);
    // Re-acquiring while already the holder reports the lock's actual state: still held.
    assert_eq!(a.request(json!({"cmd": "acquire_write"}))["acquired"], true);
    assert_eq!(b.request(json!({"cmd": "acquire_write"}))["acquired"], false);
    assert_eq!(b.request(json!({"cmd": "send", "text": "x"}))["cmd"], "error");

    a.send_text("one\r");
    assert!(a.wait_for("out:one", 15_000));

    assert_eq!(a.request(json!({"cmd": "release_write"}))["acquired"], false);
    assert_eq!(b.request(json!({"cmd": "acquire_write"}))["acquired"], true);
    b.send_text("two\r");
    assert!(b.wait_for("out:two", 15_000));

    // B disconnecting releases the lock without an explicit release.
    drop(b);
    wait_until("the lock to be released on disconnect", || {
        a.request(json!({"cmd": "acquire_write"}))["acquired"] == true
    });
    a.send_text("three\r");
    assert!(a.wait_for("out:three", 15_000));
}

// ---------------------------------------------------------------------------------------------
// 3. Real full-screen programs

fn scratch_file(contents: &str) -> PathBuf {
    let path = unique("vtee-fixture");
    std::fs::write(&path, contents).unwrap();
    path
}

/// `VTEE_REQUIRE_TOOLS=1` (or a comma-separated list such as `less,vim`) turns a missing program
/// from a printed skip into a failure, so CI can't go green by running nothing.
fn tool_required(tool: &str) -> bool {
    match std::env::var("VTEE_REQUIRE_TOOLS") {
        Ok(v) => v == "1" || v.split(',').any(|t| t.trim() == tool),
        Err(_) => false,
    }
}

fn numbered_lines() -> String {
    (1..=200).map(|i| format!("line{i}\n")).collect()
}

fn fullscreen_roundtrip(binary: &str, command: &str, quit_keys: &str) {
    if !on_path(binary) {
        assert!(!tool_required(binary), "{binary} is required (VTEE_REQUIRE_TOOLS) but not installed");
        eprintln!("skipping: {binary} is not installed");
        return;
    }
    let file = scratch_file(&numbered_lines());
    let script = format!("{command} {}; echo finished-marker; sleep 120", file.display());
    let session = Session::start(
        &format!("prog-{binary}"),
        &["--writable", "--env", "TERM=xterm-256color", "--cols", "80", "--rows", "24"],
        &["bash", "-c", &script],
    );
    let mut nd = session.nd();
    assert!(nd.wait_for("line1", 15_000), "{binary} never drew the file");
    let snap = nd.snapshot();
    assert_eq!(snap["alt_screen"], true, "{binary} should be on the alternate screen");
    assert!(!screen(&snap).contains("line40"), "24 rows can't show line40:\n{}", screen(&snap));

    nd.send(&json!({"cmd": "resize", "cols": 100, "rows": 30}));
    wait_until("the program to repaint at the new size", || {
        let snap = nd.snapshot();
        snap["cols"] == 100 && snap["rows"] == 30 && screen(&snap).contains("line29")
    });

    nd.send_text(quit_keys);
    assert!(nd.wait_for("finished-marker", 15_000), "{binary} didn't exit");
    assert_eq!(nd.snapshot()["alt_screen"], false, "alt screen should end with {binary}");
    let _ = std::fs::remove_file(file);
}

#[test]
fn less_uses_the_alt_screen_repaints_on_resize_and_leaves_cleanly() {
    fullscreen_roundtrip("less", "less", "q");
}

#[test]
fn vim_uses_the_alt_screen_repaints_on_resize_and_leaves_cleanly() {
    let binary = if on_path("vim.tiny") { "vim.tiny" } else { "vim" };
    fullscreen_roundtrip(binary, &format!("{binary} -u NONE -N"), ":q\r");
}

// ---------------------------------------------------------------------------------------------
// 4. Lifecycle

#[test]
fn kill_removes_the_files_and_ends_both_holder_and_child() {
    let mut session = Session::start("lifecycle", &[], &["sleep", "31337"]);
    assert_eq!(procs_running("sleep", "31337").len(), 1);

    let status = vtee_in(&session.tmp).args(["kill", "lifecycle"]).status().unwrap();
    assert!(status.success());

    wait_until("holder to exit", || matches!(session.child.try_wait(), Ok(Some(_))));
    wait_until("child to end (no orphan)", || procs_running("sleep", "31337").is_empty());
    assert!(!session.files_exist(), "session files should be gone");
}

#[test]
fn a_sigkilled_holder_leaves_a_stale_entry_that_ls_flags_and_kill_cleans() {
    let mut session = Session::start("stale", &[], &["cat"]);
    session.child.kill().unwrap();
    session.child.wait().unwrap();
    assert!(session.files_exist(), "SIGKILL can't clean up, files should remain");

    let ls = vtee_in(&session.tmp).arg("ls").output().unwrap();
    let out = String::from_utf8_lossy(&ls.stdout);
    assert!(out.contains("stale") && out.contains("stale\t"), "ls output: {out:?}");

    assert!(vtee_in(&session.tmp).args(["kill", "stale"]).status().unwrap().success());
    assert!(!session.files_exist());
    let ls = vtee_in(&session.tmp).arg("ls").output().unwrap();
    assert!(String::from_utf8_lossy(&ls.stdout).contains("no sessions"));
}

#[test]
fn sigterm_to_the_holder_ends_the_child_and_the_name_can_be_reused() {
    let mut first = Session::start("reuse", &[], &["sleep", "31338"]);
    assert_eq!(procs_running("sleep", "31338").len(), 1);

    unsafe {
        libc::kill(first.child.id() as i32, libc::SIGTERM);
    }
    wait_until("holder to exit", || matches!(first.child.try_wait(), Ok(Some(_))));
    wait_until("child to end", || procs_running("sleep", "31338").is_empty());

    // An external SIGTERM must leave nothing behind for `ls` to show as stale.
    for file in [first.pidfile(), first.sock(), first.tty_sock()] {
        assert!(!file.exists(), "{} was left behind after SIGTERM", file.display());
    }

    let second = Session::start_in(&first.tmp, "reuse", &[], &["cat"]);
    let mut nd = second.nd();
    nd.snapshot();
}

/// `SIGHUP` is what the kernel sends a foreground process when its controlling terminal closes;
/// unlike `SIGTERM`, there is no dedicated application-level convention for it, so it must be
/// handled the same way or a closed terminal leaves the same mess a `SIGKILL` would.
#[test]
fn sighup_to_the_holder_ends_the_child_and_leaves_nothing_behind() {
    let mut session = Session::start("hup", &[], &["sleep", "31342"]);
    assert_eq!(procs_running("sleep", "31342").len(), 1);

    unsafe {
        libc::kill(session.child.id() as i32, libc::SIGHUP);
    }
    wait_until("holder to exit", || matches!(session.child.try_wait(), Ok(Some(_))));
    wait_until("child to end", || procs_running("sleep", "31342").is_empty());

    for file in [session.pidfile(), session.sock(), session.tty_sock()] {
        assert!(!file.exists(), "{} was left behind after SIGHUP", file.display());
    }
}

// ---------------------------------------------------------------------------------------------
// 4b. Sessions started with an explicit `--listen=<path>`

/// A long-lived process standing in for "whatever a stale pidfile's PID belongs to now".
struct Sentinel(Child);

impl Sentinel {
    fn start(seconds: &str) -> Sentinel {
        Sentinel(Command::new("sleep").arg(seconds).stdin(Stdio::null()).spawn().unwrap())
    }

    fn alive(&mut self) -> bool {
        matches!(self.0.try_wait(), Ok(None))
    }
}

impl Drop for Sentinel {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn a_listen_session_is_live_in_ls_connectable_and_attachable_by_name() {
    let custom = unique("vtee-custom").with_extension("sock");
    let session = Session::start_listening("custom", &custom, &["cat"]);
    assert!(!session.sock().exists(), "the conventional socket shouldn't exist: --listen replaced it");

    let ls = vtee_in(&session.tmp).arg("ls").output().unwrap();
    let out = String::from_utf8_lossy(&ls.stdout);
    assert!(out.contains("custom") && out.contains("live") && !out.contains("stale"), "ls output: {out:?}");
    assert!(out.contains(&format!("sock={}", custom.display())), "ls should show the real socket: {out:?}");

    Nd::connect(&custom).snapshot();

    // Attach by name and bare: both must find the raw-TTY socket through the record. Attach needs
    // a terminal, so each runs on a pty and must receive the redraw.
    for args in [&["attach", "custom"][..], &["attach"][..]] {
        let attach = common::PtyProc::spawn(vtee_in(&session.tmp).args(args));
        assert!(attach.wait_for("\x1b[2J", Duration::from_secs(10)), "{args:?}: no redraw: {:?}", attach.text());
    }
    let _ = std::fs::remove_file(&custom);
}

#[test]
fn a_second_holder_with_the_same_name_is_rejected_even_when_the_first_used_listen() {
    let custom = unique("vtee-custom").with_extension("sock");
    let session = Session::start_listening("dupe", &custom, &["cat"]);
    let out = vtee_in(&session.tmp)
        .args(["--headless", "--name", "dupe", "--", "cat"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("duplicate session"), "{:?}", out.stderr);
    let _ = std::fs::remove_file(&custom);
}

/// `--listen=<path>` naming a path that is already a live socket - a different, still-running
/// session, or the same path handed to two holders at once - must be refused, not silently
/// stolen out from under the session already listening there.
#[test]
fn listen_refuses_to_take_over_a_path_that_is_already_a_live_socket() {
    let custom = unique("vtee-custom").with_extension("sock");
    let first = Session::start_listening("listen-live", &custom, &["cat"]);

    let tmp2 = unique("vtee-linux");
    std::fs::create_dir_all(&tmp2).unwrap();
    let out = vtee_in(&tmp2)
        .args(["--headless", "--name", "listen-live-2", &format!("--listen={}", custom.display()), "--", "cat"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("already in use by a live session"), "{:?}", out.stderr);

    // The first session must be untouched: still there, still answering.
    assert!(custom.exists());
    Nd::connect(&custom).snapshot();
    drop(first);
    let _ = std::fs::remove_file(&custom);
}

/// `--listen=<path>` naming a path that is an ordinary file - a typo, or a path that matters to
/// someone - must be refused, not deleted to make room for the socket.
#[test]
fn listen_refuses_to_delete_a_path_that_is_not_a_socket() {
    let custom = unique("vtee-custom").with_extension("sock");
    std::fs::write(&custom, "do not touch me\n").unwrap();

    let tmp = unique("vtee-linux");
    std::fs::create_dir_all(&tmp).unwrap();
    let out = vtee_in(&tmp)
        .args(["--headless", "--name", "listen-not-a-socket", &format!("--listen={}", custom.display()), "--", "cat"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("is not a socket"), "{:?}", out.stderr);
    assert_eq!(std::fs::read_to_string(&custom).unwrap(), "do not touch me\n", "the file must be left alone");
    let _ = std::fs::remove_file(&custom);
}

#[test]
fn kill_stops_a_listen_session_and_removes_its_custom_socket() {
    let custom = unique("vtee-custom").with_extension("sock");
    let mut session = Session::start_listening("custom-kill", &custom, &["sleep", "31339"]);
    assert_eq!(procs_running("sleep", "31339").len(), 1);

    let status = vtee_in(&session.tmp).args(["kill", "custom-kill"]).status().unwrap();
    assert!(status.success());

    wait_until("holder to exit", || matches!(session.child.try_wait(), Ok(Some(_))));
    wait_until("child to end (no orphan)", || procs_running("sleep", "31339").is_empty());
    assert!(!custom.exists(), "the custom socket should be removed");
    assert!(!session.files_exist(), "the session files should be gone");
}

#[test]
fn a_killed_listen_session_is_stale_and_kill_cleans_it_without_signalling_anything() {
    let custom = unique("vtee-custom").with_extension("sock");
    let mut session = Session::start_listening("custom-stale", &custom, &["cat"]);
    session.child.kill().unwrap();
    session.child.wait().unwrap();
    assert!(custom.exists(), "SIGKILL can't clean up, the custom socket should remain");

    let ls = vtee_in(&session.tmp).arg("ls").output().unwrap();
    let out = String::from_utf8_lossy(&ls.stdout);
    assert!(out.contains("stale") && !out.contains("live"), "ls output: {out:?}");
    assert!(out.contains(&format!("sock={}", custom.display())), "ls output: {out:?}");

    // The record now names a live, unrelated process, as a reused PID would. The recorded socket is
    // dead, so `kill` must not signal it.
    let mut sentinel = Sentinel::start("31340");
    let tty = session.tty_sock();
    std::fs::write(session.pidfile(), format!("{}\n{}\n{}\n", sentinel.0.id(), custom.display(), tty.display()))
        .unwrap();

    assert!(vtee_in(&session.tmp).args(["kill", "custom-stale"]).status().unwrap().success());
    assert!(sentinel.alive(), "kill signalled a process because of a stale record");
    assert!(!custom.exists(), "the stale custom socket should be cleaned up");
    assert!(!session.pidfile().exists());
}

#[test]
fn kill_leaves_a_recorded_path_alone_when_it_is_no_longer_a_socket() {
    let custom = unique("vtee-custom").with_extension("sock");
    let mut session = Session::start_listening("custom-reused", &custom, &["cat"]);
    session.child.kill().unwrap();
    session.child.wait().unwrap();

    // Someone has since put an ordinary file where the dead session's socket used to be.
    std::fs::remove_file(&custom).unwrap();
    std::fs::write(&custom, "not a socket").unwrap();

    assert!(vtee_in(&session.tmp).args(["kill", "custom-reused"]).status().unwrap().success());
    assert_eq!(std::fs::read_to_string(&custom).unwrap(), "not a socket");
    assert!(!session.pidfile().exists());
    let _ = std::fs::remove_file(&custom);
}

#[test]
fn a_legacy_pid_only_pidfile_still_works() {
    let mut session = Session::start("legacy", &[], &["sleep", "31341"]);
    std::fs::write(session.pidfile(), session.child.id().to_string()).unwrap();

    let ls = vtee_in(&session.tmp).arg("ls").output().unwrap();
    let out = String::from_utf8_lossy(&ls.stdout);
    assert!(out.contains(&format!("legacy\tpid={}\tlive", session.child.id())), "ls output: {out:?}");
    assert!(!out.contains("sock="), "a default session has nothing to point out: {out:?}");

    assert!(vtee_in(&session.tmp).args(["kill", "legacy"]).status().unwrap().success());
    wait_until("holder to exit", || matches!(session.child.try_wait(), Ok(Some(_))));
    wait_until("child to end", || procs_running("sleep", "31341").is_empty());
    assert!(!session.files_exist());
}

#[test]
fn a_default_session_records_its_real_paths_in_the_pidfile() {
    let session = Session::start("recorded", &[], &["cat"]);
    let text = std::fs::read_to_string(session.pidfile()).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines,
        [
            session.child.id().to_string(),
            session.sock().display().to_string(),
            session.tty_sock().display().to_string()
        ]
    );
}

// ---------------------------------------------------------------------------------------------
// 5. Stress and fuzz

#[test]
fn attach_detach_churn_during_a_flood_keeps_the_session_responsive() {
    let mut session = Session::start("flood", &[], &["yes"]);
    let mut nd = session.nd();
    let mut last_offset = 0;
    for i in 0..50 {
        let tty = session.tty();
        if i % 2 == 0 {
            // Read a little, then vanish with data still in flight.
            wait_until("flood bytes", || tty.raw().len() > 2000);
        }
        drop(tty);
        let offset = nd.snapshot()["byte_offset"].as_u64().unwrap();
        assert!(offset >= last_offset);
        last_offset = offset;
    }
    assert!(session.child.try_wait().unwrap().is_none(), "holder died during churn");
    assert!(nd.snapshot()["byte_offset"].as_u64().unwrap() > 0);
}

#[test]
fn garbage_and_malformed_json_never_take_the_session_down() {
    let session = Session::start("fuzz-json", &["--writable"], &["cat"]);
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let nasty: Vec<Vec<u8>> = vec![
        b"{".to_vec(),
        b"{\"cmd\":".to_vec(),
        b"{\"cmd\":\"nonsense\"}".to_vec(),
        b"null".to_vec(),
        b"[]".to_vec(),
        b"\"\\u0000\"".to_vec(),
        b"{\"cmd\":\"send\"}".to_vec(),
        b"{\"cmd\":\"resize\",\"cols\":-1,\"rows\":0}".to_vec(),
        b"{\"cmd\":\"resize\",\"cols\":0,\"rows\":0}".to_vec(),
        b"{\"cmd\":\"resize\",\"cols\":1,\"rows\":1}".to_vec(),
        b"{\"cmd\":\"resize\",\"cols\":2,\"rows\":2}".to_vec(),
        b"{\"cmd\":\"resize\",\"cols\":300,\"rows\":100}".to_vec(),
        b"{\"cmd\":\"send_key\",\"key\":\"Ctrl-\"}".to_vec(),
        b"{\"cmd\":\"send_key\",\"key\":\"\\u00e9\\u00e9\"}".to_vec(),
        b"{\"cmd\":\"send\",\"text\":\"\\ud800\"}".to_vec(),
        b"{\"cmd\":\"wait_for\",\"text\":\"\",\"timeout_ms\":1}".to_vec(),
        b"[".repeat(10_000),
        b"{\"a\":".repeat(5_000),
        vec![0xff, 0xfe, 0xfd, b'\n'],
    ];
    for round in 0..30 {
        let mut stream = connect_retry(&session.sock());
        stream.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
        let mut payload = Vec::new();
        for _ in 0..8 {
            if rng.below(2) == 0 {
                let n = rng.below(2048) as usize;
                payload.extend(rng.bytes(n));
            } else {
                payload.extend(&nasty[rng.below(nasty.len() as u64) as usize]);
            }
            payload.push(b'\n');
        }
        let _ = stream.write_all(&payload);
        let _ = stream.write_all(b"{\"cmd\":\"snapshot\"}\n");
        // Same connection must still work: skip error lines until the snapshot reply arrives.
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut got_snapshot = false;
        while Instant::now() < deadline && !got_snapshot {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => got_snapshot = line.contains("\"cmd\":\"snapshot\""),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(_) => break,
            }
        }
        // Every payload line is far under the line cap, so the connection must survive the
        // garbage and still answer; a second connection must work too.
        assert!(got_snapshot, "round {round}: the abused connection stopped answering");
        let mut fresh = session.nd();
        let snap = fresh.snapshot();
        assert!(snap["cols"].as_u64().unwrap() >= 1, "round {round}: bad snapshot {snap}");
    }
    let mut nd = session.nd();
    nd.send(&json!({"cmd": "resize", "cols": 80, "rows": 24}));
    assert_eq!(nd.snapshot()["cols"], 80);
}

fn escape_soup(seed: u64, len: usize) -> Vec<u8> {
    let mut rng = Rng(seed);
    let fixed: [&[u8]; 16] = [
        b"\x1b[?1049h",
        b"\x1b[?1049l",
        b"\x1b[r",
        b"\x1b[1;1r",
        b"\x1b[999999999;999999999H",
        b"\x1b[2147483647C",
        b"\x1b[999999999@",
        b"\x1b[999999999L",
        b"\x1b#8",
        b"\x1b[8;99999;99999t",
        "\u{4f60}\u{597d}\u{1f600}e\u{301}".as_bytes(),
        b"\x1b[6n\x1b[c\x1b[>c\x1b[?2026$p",
        b"\x1b]0;title\x07",
        b"\x1b]8;;http://example.com\x1b\\link\x1b]8;;\x1b\\",
        b"\x1bP1$r\x1b\\",
        b"\x1b[38;2;999;999;999m\x1b[48;5;999m\x1b[999m",
    ];
    let mut out = Vec::with_capacity(len + 64);
    while out.len() < len {
        match rng.below(6) {
            0 => out.extend(fixed[rng.below(fixed.len() as u64) as usize]),
            1 => {
                out.extend(b"\x1b[");
                for _ in 0..rng.below(5) {
                    out.extend(rng.below(100_000).to_string().as_bytes());
                    out.push(if rng.below(2) == 0 { b';' } else { b':' });
                }
                out.push(0x40 + rng.below(0x3f) as u8);
            }
            2 => {
                out.extend(b"\x1b]");
                let n = rng.below(40) as usize;
                out.extend(rng.bytes(n));
                if rng.below(2) == 0 {
                    out.push(0x07);
                }
            }
            3 => {
                out.push(0x1b);
                out.push(rng.next() as u8);
            }
            4 => {
                let n = rng.below(64) as usize;
                out.extend(rng.bytes(n));
            }
            _ => out.extend(b"plain text "),
        }
    }
    out
}

#[test]
fn random_escape_sequence_soup_never_takes_the_session_down() {
    for seed in [1u64, 0xDEAD_BEEF, 0x1234_5678_9ABC_DEF0] {
        let soup = escape_soup(seed, 30_000);
        let file = unique("vtee-soup");
        std::fs::write(&file, &soup).unwrap();
        let script = format!("cat {}; printf '\\033c\\033[?1049lsoup-done\\n'; sleep 120", file.display());
        let mut session = Session::start(&format!("soup-{seed:x}"), &["--writable"], &["bash", "-c", &script]);
        let mut nd = session.nd();
        assert!(nd.wait_for("soup-done", 20_000), "seed {seed:#x}: session never got past the soup");
        let snap = nd.snapshot();
        assert!(snap["byte_offset"].as_u64().unwrap() >= soup.len() as u64, "seed {seed:#x}");
        assert!(session.child.try_wait().unwrap().is_none(), "seed {seed:#x}: holder died");

        // Still fully functional afterwards.
        nd.send(&json!({"cmd": "resize", "cols": 90, "rows": 30}));
        let snap = nd.snapshot();
        assert_eq!((snap["cols"].as_u64(), snap["rows"].as_u64()), (Some(90), Some(30)), "seed {seed:#x}");
        let tty = session.tty();
        drop(tty);
        let _ = std::fs::remove_file(file);
    }
}

fn mtime(path: &Path) -> std::time::SystemTime {
    std::fs::metadata(path).and_then(|m| m.modified()).unwrap()
}

/// Bare `attach` follows the session attached to last, not the one started last.
#[test]
fn bare_attach_picks_the_session_attached_to_most_recently() {
    let older = Session::start("older", &[], &["sh", "-c", "echo MARK-OLDER; exec cat"]);
    let newer = Session::start_in(&older.tmp, "newer", &[], &["sh", "-c", "echo MARK-NEWER; exec cat"]);
    let bare = |expect: &str, other: &str| {
        let attach = common::PtyProc::spawn(vtee_in(&older.tmp).arg("attach"));
        assert!(attach.wait_for_plain(expect, Duration::from_secs(10)), "wanted {expect}: {:?}", attach.plain_text());
        assert!(!attach.plain_text().contains(other), "got {other} too: {:?}", attach.plain_text());
    };

    // Nothing has been attached to yet, so the one started last wins.
    bare("MARK-NEWER", "MARK-OLDER");

    // Attaching to the older session by name makes it the most recent.
    let by_name = common::PtyProc::spawn(vtee_in(&older.tmp).args(["attach", "older"]));
    assert!(by_name.wait_for_plain("MARK-OLDER", Duration::from_secs(10)), "{:?}", by_name.plain_text());
    drop(by_name);
    wait_until("the older session's record to be touched", || mtime(&older.pidfile()) > mtime(&newer.pidfile()));
    bare("MARK-OLDER", "MARK-NEWER");
}

/// `attach`'s reader thread sees EOF as soon as the session ends, but the main thread is blocked
/// reading attach's own stdin with no portable way to wake it up early - so when the session
/// ends, attach must exit by itself rather than sit in raw mode until the next keystroke.
#[test]
fn attach_exits_by_itself_once_the_session_ends() {
    let session = Session::start("ends-under-attach", &[], &["cat"]);
    let mut attach = common::PtyProc::spawn(vtee_in(&session.tmp).arg("attach"));
    assert!(attach.wait_for("\x1b[2J", Duration::from_secs(10)), "no redraw: {:?}", attach.text());

    assert!(vtee_in(&session.tmp).args(["kill", "ends-under-attach"]).status().unwrap().success());

    wait_until("attach to exit by itself after the session ended", || attach.has_exited());
}

// ---- exit_status / wait_exit over the socket ----------------------------------------------------

/// Both commands are reads, so a default (read-only) session answers them. The holder itself sits
/// in `Tap::wait` the whole time, which used to keep the child locked for anyone else.
#[test]
fn exit_status_and_wait_exit_follow_the_child_over_the_socket() {
    let s = Session::start("exitcode", &[], &["sh", "-c", "sleep 2; exit 7"]);
    let mut nd = s.nd();

    let started = Instant::now();
    assert_eq!(nd.request(json!({"cmd": "exit_status"})), json!({"cmd": "exit_status", "running": true}));
    assert!(started.elapsed() < Duration::from_secs(1), "exit_status waited on the holder's own wait()");

    assert_eq!(
        nd.request(json!({"cmd": "wait_exit", "timeout_ms": 200})),
        json!({"cmd": "wait_exit", "running": true, "timed_out": true})
    );
    assert_eq!(
        nd.request(json!({"cmd": "wait_exit", "timeout_ms": 15000})),
        json!({"cmd": "wait_exit", "running": false, "code": 7, "timed_out": false})
    );
    // The holder lingers for a connected client, so the answer is still available and stable.
    assert_eq!(nd.request(json!({"cmd": "exit_status"})), json!({"cmd": "exit_status", "running": false, "code": 7}));
}

#[test]
fn a_child_ended_by_a_signal_reports_the_signal_and_no_code() {
    let s = Session::start("exitsignal", &[], &["sh", "-c", "sleep 1; kill -TERM $$"]);
    let mut nd = s.nd();
    let reply = nd.request(json!({"cmd": "wait_exit", "timeout_ms": 15000}));
    assert_eq!(reply["cmd"], "wait_exit", "{reply}");
    assert_eq!(reply["running"], false, "{reply}");
    assert_eq!(reply["timed_out"], false, "{reply}");
    assert!(reply["signal"].as_str().is_some_and(|s| !s.is_empty()), "no signal in {reply}");
    assert!(reply.get("code").is_none(), "a signalled child has no exit code: {reply}");
}

/// A holder with no clients still exits the moment the child does: the linger only applies while
/// an NDJSON client is connected.
#[test]
fn a_session_with_no_clients_ends_as_soon_as_the_child_exits() {
    let mut s = Session::start("exitfast", &[], &["sh", "-c", "sleep 1; exit 0"]);
    let started = Instant::now();
    wait_until("the holder to end", || s.child.try_wait().ok().flatten().is_some());
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
}

// ---- --connect with --script ---------------------------------------------------------------------

fn script_file(lines: &[&str]) -> PathBuf {
    let path = unique("vtee-connect-script");
    let mut text = lines.join("\n");
    text.push('\n');
    std::fs::write(&path, text).unwrap();
    path
}

fn reply_lines(stdout: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(stdout).lines().map(|l| serde_json::from_str(l).expect("a JSON reply line")).collect()
}

/// The script goes to a live session instead of a new program: typing reaches it, replies come back in order, and the
/// session is still there afterwards.
#[test]
fn a_script_can_be_run_against_a_live_session() {
    let s = Session::start("scripted", &["--writable"], &["sh", "-c", "echo ready; read x; echo got:$x; sleep 60"]);
    let script = script_file(&[
        r#"{"cmd":"wait_for","text":"ready","timeout_ms":8000}"#,
        r#"{"cmd":"send","text":"hi\r"}"#,
        r#"{"cmd":"wait_for","text":"got:hi","timeout_ms":8000}"#,
        r#"{"cmd":"snapshot"}"#,
    ]);
    let out = vtee_in(&s.tmp).arg("--connect").arg(s.sock()).arg("--script").arg(&script).output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let replies = reply_lines(&out.stdout);
    assert_eq!(replies.len(), 3, "{replies:?}");
    assert_eq!(replies[0]["matched"], true);
    assert_eq!(replies[1]["matched"], true);
    assert!(screen(&replies[2]["snapshot"]).contains("got:hi"));
    assert_eq!(s.nd().request(json!({"cmd": "exit_status"}))["running"], true, "the session was left running");
    let _ = std::fs::remove_file(script);
}

/// A read-only session refuses `send`; the script run stops there with the refusal, not silently.
#[test]
fn a_script_run_against_a_read_only_session_stops_at_a_refused_write() {
    let s = Session::start("scripted-ro", &[], &["sh", "-c", "echo ready; sleep 60"]);
    let script = script_file(&[r#"{"cmd":"send","text":"x"}"#, r#"{"cmd":"snapshot"}"#]);
    let out = vtee_in(&s.tmp).arg("--connect").arg(s.sock()).arg("--script").arg(&script).output().unwrap();
    assert_eq!(out.status.code(), Some(exitcode::NOPERM), "{out:?}");
    assert!(out.stdout.is_empty(), "the snapshot after the refused send must not have run: {out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("the session refused script line 1") && stderr.contains("--writable"), "{stderr}");
    let _ = std::fs::remove_file(script);
}

#[test]
fn a_timed_out_wait_fails_a_script_run_against_a_live_session_too() {
    let s = Session::start("scripted-wait", &[], &["sh", "-c", "echo ready; sleep 60"]);
    let script = script_file(&[r#"{"cmd":"wait_for","text":"never","timeout_ms":300}"#]);
    let out = vtee_in(&s.tmp).arg("--connect").arg(s.sock()).arg("--script").arg(&script).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert_eq!(reply_lines(&out.stdout)[0]["matched"], false);
    assert!(String::from_utf8_lossy(&out.stderr).contains(r#"script line 1: wait_for "never" timed out after 300 ms"#));
    let _ = std::fs::remove_file(script);
}

#[test]
fn a_script_cannot_reach_a_session_that_is_not_there() {
    let tmp = unique("vtee-connect-none");
    std::fs::create_dir_all(&tmp).unwrap();
    let script = script_file(&[r#"{"cmd":"snapshot"}"#]);
    let out = vtee_in(&tmp).arg("--connect").arg(tmp.join("none.sock")).arg("--script").arg(&script).output().unwrap();
    assert_eq!(out.status.code(), Some(exitcode::UNAVAILABLE), "{out:?}");
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_file(script);
}

// ---- --expect against a live session ----------------------------------------------------------------

fn expect_in(s: &Session, args: &[&str]) -> std::process::Output {
    let mut cmd = vtee_in(&s.tmp);
    cmd.current_dir(&s.tmp).env_remove("CI").env("NO_COLOR", "1").args(args);
    cmd.output().unwrap()
}

/// `--connect SOCK --expect GOLDEN` takes the screen of the session now: it can be recorded and checked, and the
/// session keeps running.
#[test]
fn the_screen_of_a_live_session_can_be_recorded_and_checked() {
    let s = Session::start("expect-now", &[], &["sh", "-c", "echo hello; echo END; sleep 60"]);
    let mut nd = s.nd();
    wait_until("the output", || screen(&nd.snapshot()).contains("END"));
    let sock = s.sock();
    let sock = sock.to_str().unwrap();

    let out = expect_in(&s, &["--connect", sock, "--expect", "now.golden", "--update", "--portable"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "CREATED now.golden: 1 screen\n");
    let text = std::fs::read_to_string(s.tmp.join("now.golden")).unwrap();
    assert!(text.contains("\n 0|hello\n 1|END\n"), "{text}");

    let out = expect_in(&s, &["--connect", sock, "--expect", "now.golden"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stdout));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "PASS now.golden (1 screen)\n");

    std::fs::write(s.tmp.join("now.golden"), text.replace(" 0|hello", " 0|howdy")).unwrap();
    let out = expect_in(&s, &["--connect", sock, "--expect", "now.golden"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("text       row 0"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(nd.request(json!({"cmd": "exit_status"}))["running"], true, "the session was left running");
}

/// `--connect SOCK --script FILE --expect GOLDEN`: the script runs against the live session and its screens are
/// judged; a wait that times out is a problem of the run, as it is with a program of its own.
#[test]
fn a_script_run_against_a_live_session_is_judged_against_a_golden() {
    let s =
        Session::start("expect-script", &["--writable"], &["sh", "-c", "echo ready; read x; echo got:$x; sleep 60"]);
    let sock = s.sock();
    let sock = sock.to_str().unwrap();
    let script = script_file(&[
        r#"{"cmd":"wait_for","text":"ready","timeout_ms":8000}"#,
        r#"{"cmd":"send","text":"hi\r"}"#,
        r#"{"cmd":"wait_for","text":"got:hi","timeout_ms":8000}"#,
        r#"{"cmd":"snapshot","name":"typed"}"#,
    ]);
    let script = script.to_str().unwrap();

    let out =
        expect_in(&s, &["--connect", sock, "--script", script, "--expect", "typed.golden", "--update", "--portable"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let text = std::fs::read_to_string(s.tmp.join("typed.golden")).unwrap();
    assert!(text.contains("name \"typed\"") && text.contains("got:hi"), "{text}");

    let out = expect_in(&s, &["--connect", sock, "--script", script, "--expect", "typed.golden"]);
    // The session has now been typed into twice, so its screen shows a second line: the golden notices.
    assert_eq!(out.status.code(), Some(1), "{}", String::from_utf8_lossy(&out.stdout));

    let lenient = script_file(&[r#"{"cmd":"wait_for","text":"never","timeout_ms":300}"#, r#"{"cmd":"snapshot"}"#]);
    let out = expect_in(
        &s,
        &["--connect", sock, "--script", lenient.to_str().unwrap(), "--expect", "typed.golden", "--screen", "1"],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stdout)
            .contains("wait       script line 1   wait_for \"never\" timed out after 300 ms")
    );
    let _ = std::fs::remove_file(script);
    let _ = std::fs::remove_file(lenient);
}

#[test]
fn a_golden_cannot_be_checked_against_a_session_that_is_not_there() {
    let tmp = unique("vtee-expect-none");
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::write(
        tmp.join("g.golden"),
        "vtee-golden 1\nschema 1\nsemantics 1\n\n[screen 1]\nsize 3x1\ncursor row=0 col=0 shape=block hidden=false\nmodes\nalt_screen false\ntitle null\n\n[text]\n0|\n",
    )
    .unwrap();
    let out = vtee_in(&tmp)
        .current_dir(&tmp)
        .env_remove("CI")
        .args(["--connect", "none.sock", "--expect", "g.golden"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(exitcode::UNAVAILABLE), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).starts_with("vtee: failed to connect to none.sock"));
    let _ = std::fs::remove_dir_all(&tmp);
}
