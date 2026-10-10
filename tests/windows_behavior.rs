//! Native Windows behaviour tests beyond the startup handshake (`tests/windows_conpty.rs`):
//! resize, termination, TUIs, determinism, paste and flood. Two harnesses:
//!
//! - `Terminal`: opens a real ConPTY via the patched `portable_pty`, runs `vtee` inside it and
//!   plays the human's terminal (answers `ESC[6n`, types, resizes). Used for interactive-mode
//!   behaviour.
//! - `run_script`: `vtee --script`, stdin/stdout are pipes. Used where the emulator's own view
//!   (a snapshot) is the thing under test.
//!
//! Everything polls with a deadline; the tests share one lock because ConPTY sessions are heavy
//! enough that running a dozen at once makes timing-based assertions flaky for reasons unrelated
//! to the product.

#![cfg(windows)]

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde_json::Value;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

const CURSOR_POSITION_QUERY: &[u8] = b"\x1b[6n";
const CURSOR_POSITION_REPLY: &[u8] = b"\x1b[1;1R";

/// Text with terminal control sequences removed, so assertions can match what a person would read.
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

fn vtee_bin() -> PathBuf {
    assert_cmd::cargo::cargo_bin("vtee")
}

fn unique_path(stem: &str, ext: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("vtee-wb-{}-{n}-{stem}.{ext}", std::process::id()))
}

/// A scratch `.cmd` file; quoting nested `cmd /c "..."` through two argument parsers is a trap,
/// a batch file isn't.
fn batch_file(stem: &str, body: &str) -> PathBuf {
    let path = unique_path(stem, "cmd");
    std::fs::write(&path, format!("@echo off\r\n{}\r\n", body.replace('\n', "\r\n"))).unwrap();
    path
}

struct Terminal {
    master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Box<dyn Child + Send + Sync>,
    raw: Arc<Mutex<Vec<u8>>>,
}

impl Terminal {
    /// Runs `vtee <args>` as the only thing in a fresh ConPTY of the given size.
    fn spawn(cols: u16, rows: u16, args: &[&str]) -> Terminal {
        let pair = native_pty_system()
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .expect("failed to open a ConPTY for the test terminal");
        let mut cmd = CommandBuilder::new(vtee_bin());
        cmd.args(args);
        let child = pair.slave.spawn_command(cmd).expect("failed to spawn vtee under the test pty");
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().unwrap();
        let writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(pair.master.take_writer().unwrap()));
        let raw = Arc::new(Mutex::new(Vec::new()));
        {
            let writer = writer.clone();
            let raw = raw.clone();
            // Abandoned at the end of the test: a blocking read on the master isn't reliably
            // unblocked by killing the child (`tests/windows_conpty.rs`).
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                let mut scanned = 0usize;
                while let Ok(n) = reader.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    let mut out = raw.lock().unwrap();
                    out.extend_from_slice(&buf[..n]);
                    let mut i = scanned.saturating_sub(CURSOR_POSITION_QUERY.len() - 1);
                    while i + CURSOR_POSITION_QUERY.len() <= out.len() {
                        if out[i..i + CURSOR_POSITION_QUERY.len()] == *CURSOR_POSITION_QUERY {
                            let mut w = writer.lock().unwrap();
                            let _ = w.write_all(CURSOR_POSITION_REPLY);
                            let _ = w.flush();
                            i += CURSOR_POSITION_QUERY.len();
                        } else {
                            i += 1;
                        }
                    }
                    scanned = out.len();
                }
            });
        }
        Terminal { master: pair.master, writer, child, raw }
    }

    fn text(&self) -> String {
        strip_ansi(&self.raw.lock().unwrap())
    }

    fn wait_for(&self, needle: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if self.text().contains(needle) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn send(&self, bytes: &[u8]) {
        let mut w = self.writer.lock().unwrap();
        w.write_all(bytes).unwrap();
        w.flush().unwrap();
    }

    fn resize(&self, cols: u16, rows: u16) {
        self.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).unwrap();
    }

    fn pid(&self) -> u32 {
        self.child.process_id().expect("vtee has no pid")
    }

    /// The exit code once `vtee` is gone, or `None` if it was still running at the deadline.
    fn wait_exit(&mut self, timeout: Duration) -> Option<u32> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait().expect("try_wait failed") {
                return Some(status.exit_code());
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

/// PIDs of processes named `image` whose command line contains `needle`.
fn pids_of(image: &str, needle: &str) -> Vec<u32> {
    let script = format!(
        "Get-CimInstance Win32_Process -Filter \"Name='{image}'\" | \
         Where-Object {{ $_.CommandLine -like '*{needle}*' }} | ForEach-Object {{ $_.ProcessId }}"
    );
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .expect("couldn't list processes");
    String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.trim().parse().ok()).collect()
}

fn kill_pids(pids: &[u32]) {
    for pid in pids {
        let _ = std::process::Command::new("taskkill").args(["/F", "/T", "/PID", &pid.to_string()]).output();
    }
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
        std::thread::sleep(Duration::from_millis(100));
    }
}

struct ScriptRun {
    results: Vec<Value>,
    stderr: String,
    success: bool,
}

impl ScriptRun {
    /// Text rows of the last snapshot-bearing result.
    fn last_lines(&self) -> Vec<String> {
        let snap = self.results.iter().rev().find_map(|r| r.get("snapshot")).expect("no snapshot in script output");
        snap["lines"].as_array().unwrap().iter().map(|l| l.as_str().unwrap().trim_end().to_string()).collect()
    }

    fn matched(&self, index: usize) -> bool {
        self.results[index]["matched"].as_bool().unwrap_or(false)
    }
}

/// `vtee --script <lines> <flags> -- <program...>`.
fn run_script(lines: &[&str], flags: &[&str], program: &[&str]) -> ScriptRun {
    let script = unique_path("script", "ndjson");
    std::fs::write(&script, lines.join("\n") + "\n").unwrap();
    let output = assert_cmd::Command::cargo_bin("vtee")
        .unwrap()
        .arg("--script")
        .arg(&script)
        .args(flags)
        .arg("--")
        .args(program)
        .timeout(Duration::from_secs(45))
        .output()
        .expect("vtee --script did not complete");
    let _ = std::fs::remove_file(&script);
    let stdout = String::from_utf8_lossy(&output.stdout);
    ScriptRun {
        results: stdout.lines().filter_map(|l| serde_json::from_str(l).ok()).collect(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        success: output.status.success(),
    }
}

fn wait_for_line(text: &str, timeout_ms: u64) -> String {
    format!("{{\"cmd\":\"wait_for\",\"text\":{},\"timeout_ms\":{timeout_ms}}}", Value::String(text.into()))
}

const SNAPSHOT: &str = "{\"cmd\":\"snapshot\"}";

fn s(path: &Path) -> &str {
    path.to_str().unwrap()
}

// ---- (1) resize ------------------------------------------------------------------------------

/// `--cols`/`--rows` reach the child's console, not just the emulator: `mode con` asks the
/// console itself.
#[test]
fn cols_and_rows_override_reaches_the_childs_console() {
    let _guard = serial();
    let run = run_script(
        &[&wait_for_line("Columns:", 15000), SNAPSHOT],
        &["--cols", "97", "--rows", "31"],
        &["cmd", "/c", "mode", "con"],
    );
    assert!(run.success, "script failed: {}", run.stderr);
    assert!(run.matched(0), "mode con never printed: {:?}", run.results);
    let snap = &run.results[1]["snapshot"];
    assert_eq!((snap["cols"].as_u64(), snap["rows"].as_u64()), (Some(97), Some(31)));
    let text = run.last_lines().join("\n");
    assert!(text.contains("Columns:") && text.contains("97"), "child didn't see 97 columns:\n{text}");
    assert!(text.contains("31"), "child didn't see 31 lines:\n{text}");
}

/// Resizing the *outer* terminal while `vtee` runs interactively reaches the child: Unix uses
/// `SIGWINCH`, Windows has no such signal so `watch_resize` (`src/tap.rs`) polls the console size.
#[test]
fn resizing_the_outer_terminal_reaches_the_child() {
    let _guard = serial();
    let bat = batch_file("resize", "mode con\nping -n 8 127.0.0.1 >nul\nmode con");
    let term = Terminal::spawn(80, 24, &["--no-listen", "--", "cmd", "/c", s(&bat)]);
    assert!(term.wait_for("Columns:", Duration::from_secs(15)), "first mode con never printed:\n{}", term.text());
    term.resize(120, 40);
    assert!(
        wait_until(Duration::from_secs(20), || term.text().contains("120")),
        "child never reported 120 columns after the outer resize:\n{}",
        term.text()
    );
    let _ = std::fs::remove_file(bat);
}

// ---- (2) signals and termination --------------------------------------------------------------

/// A Ctrl-C typed into the terminal interrupts the child, as under any other terminal. The
/// child used to inherit an "ignore CTRL+C" attribute from whatever launched `vtee` (this test
/// runner's own ancestry has it); the CLI now restores the default at startup
/// (`docs/decisions.md`, 2026-10-09 and 2026-10-10).
#[test]
fn ctrl_c_ends_a_long_running_child_and_leaves_no_orphan() {
    let _guard = serial();
    let mut term = Terminal::spawn(100, 30, &["--no-listen", "--", "ping", "-n", "31", "127.0.0.1"]);
    assert!(term.wait_for("Pinging", Duration::from_secs(15)), "ping never started:\n{}", term.text());
    assert!(!pids_of("PING.EXE", "-n 31 127.0.0.1").is_empty(), "expected ping to be running");

    term.send(b"\x03");
    let code = term.wait_exit(Duration::from_secs(10));
    let still_running = pids_of("PING.EXE", "-n 31 127.0.0.1");
    kill_pids(&still_running);
    assert!(code.is_some(), "vtee didn't exit within 10s of Ctrl-C; output:\n{}", term.text());
    assert!(still_running.is_empty(), "Ctrl-C left ping.exe running: {still_running:?}");
}

/// Documented outcome of force-killing the holder (the Windows analogue of the `SIGKILL` gap in
/// `docs/decisions.md`): `taskkill /F` on `vtee.exe` skips every destructor, and what happens
/// to the child is up to ConPTY. This test asserts what was measured so a change either way is
/// noticed.
#[test]
fn force_killing_vtee_documents_what_happens_to_the_child() {
    let _guard = serial();
    let mut term = Terminal::spawn(100, 30, &["--no-listen", "--", "ping", "-n", "37", "127.0.0.1"]);
    assert!(term.wait_for("Pinging", Duration::from_secs(15)), "ping never started:\n{}", term.text());
    let before = pids_of("PING.EXE", "-n 37 127.0.0.1");
    assert!(!before.is_empty(), "expected ping to be running");

    let status = std::process::Command::new("taskkill").args(["/F", "/PID", &term.pid().to_string()]).output().unwrap();
    assert!(status.status.success(), "taskkill failed: {}", String::from_utf8_lossy(&status.stderr));
    assert!(term.wait_exit(Duration::from_secs(10)).is_some(), "vtee still alive after taskkill /F");

    let orphan_gone = wait_until(Duration::from_secs(10), || pids_of("PING.EXE", "-n 37 127.0.0.1").is_empty());
    let leftover = pids_of("PING.EXE", "-n 37 127.0.0.1");
    kill_pids(&leftover);
    eprintln!("taskkill /F on vtee: child ping.exe orphaned = {}", !orphan_gone);
    assert!(
        orphan_gone,
        "measured outcome changed: ping.exe {leftover:?} survives a force-killed vtee (ConPTY did not reap it)"
    );
}

/// The child's exit code is `vtee`'s exit code, the way `env`/`time` behave.
#[test]
fn child_exit_codes_are_propagated() {
    let _guard = serial();
    for code in [0u32, 1, 3, 200] {
        let bat = batch_file("exit", &format!("exit /b {code}"));
        let mut term = Terminal::spawn(80, 24, &["--no-listen", "--", "cmd", "/c", s(&bat)]);
        let got = term.wait_exit(Duration::from_secs(20));
        let _ = std::fs::remove_file(bat);
        assert_eq!(got, Some(code), "wrong exit code for exit /b {code}; output:\n{}", term.text());
    }
}

/// `--env` reaches the child on top of the inherited environment (requirement 6), including a
/// value that itself contains `=`.
#[test]
fn env_vars_reach_the_child() {
    let _guard = serial();
    let bat = batch_file("env", "echo first:%VTEE_T%:\necho second:%VTEE_U%:\necho path-kept:%SystemRoot%:");
    let run = run_script(
        &[&wait_for_line("path-kept:C:", 15000), SNAPSHOT],
        &["--env", "VTEE_T=xyz", "--env", "VTEE_U=a=b=c"],
        &["cmd", "/c", s(&bat)],
    );
    let _ = std::fs::remove_file(&bat);
    assert!(run.success, "script failed: {}", run.stderr);
    assert!(run.matched(0), "the batch file never finished: {:?}", run.last_lines());
    let lines = run.last_lines();
    assert!(lines.iter().any(|l| l.contains("first:xyz:")), "--env missing: {lines:?}");
    assert!(lines.iter().any(|l| l.contains("second:a=b=c:")), "a value containing '=' was cut: {lines:?}");
}

/// A program that does not exist fails with the OS-error exit code and says so, rather than
/// hanging or exiting 0.
#[test]
fn a_program_that_cannot_be_spawned_exits_with_oserr() {
    let _guard = serial();
    let output = assert_cmd::Command::cargo_bin("vtee")
        .unwrap()
        .args(["--no-listen", "--", "definitely-not-a-program-vtee.exe"])
        .timeout(Duration::from_secs(30))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(71), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("failed to spawn"), "{output:?}");
}

// ---- (3) full-screen-ish programs --------------------------------------------------------------

/// Absolute path of `exe` as `where.exe` resolves it, if installed. Tests pass the absolute
/// path because the PATH a spawned child sees can differ from the one this test process was
/// started with (Git Bash launches native programs with a trimmed PATH).
fn find_exe(exe: &str) -> Option<PathBuf> {
    let out = std::process::Command::new("where.exe").arg(exe).output().ok()?;
    let first = String::from_utf8_lossy(&out.stdout).lines().next()?.trim().to_string();
    (out.status.success() && !first.is_empty()).then(|| PathBuf::from(first))
}

/// `VTEE_REQUIRE_TOOLS=1` (or a comma-separated list such as `less,vim`) turns a missing program
/// from a printed skip into a failure, so CI can't go green by running nothing.
fn tool_required(tool: &str) -> bool {
    match std::env::var("VTEE_REQUIRE_TOOLS") {
        Ok(v) => v == "1" || v.split(',').any(|t| t.trim() == tool),
        Err(_) => false,
    }
}

fn sample_text_file() -> PathBuf {
    let path = unique_path("sample", "txt");
    let body: String = (1..=60).map(|i| format!("sample-line-{i:02}\r\n")).collect();
    std::fs::write(&path, body).unwrap();
    path
}

#[test]
fn more_pages_a_file_into_the_snapshot() {
    let _guard = serial();
    let file = sample_text_file();
    let run = run_script(
        &[&wait_for_line("sample-line-05", 15000), SNAPSHOT],
        &["--cols", "100", "--rows", "20"],
        &["more.com", s(&file)],
    );
    let _ = std::fs::remove_file(&file);
    assert!(run.success, "script failed: {}", run.stderr);
    assert!(run.matched(0), "more never showed the file: {:?}", run.last_lines());
    let lines = run.last_lines();
    assert!(lines.iter().any(|l| l.contains("sample-line-01")), "first line missing: {lines:?}");
    assert!(!lines.iter().any(|l| l.contains("sample-line-60")), "more didn't paginate: {lines:?}");
}

#[test]
fn ms_edit_draws_its_menu_bar_on_the_alternate_screen() {
    let Some(edit) = find_exe("edit.exe") else {
        assert!(!tool_required("edit"), "edit.exe is required (VTEE_REQUIRE_TOOLS) but not installed");
        eprintln!("skipped: edit.exe not installed");
        return;
    };
    let _guard = serial();
    let run = run_script(&[&wait_for_line("File", 20000), SNAPSHOT], &["--cols", "100", "--rows", "30"], &[s(&edit)]);
    assert!(run.success, "script failed: {}", run.stderr);
    assert!(run.matched(0), "edit never drew a menu bar: {:?}", run.last_lines());
    let lines = run.last_lines().join("\n");
    assert!(lines.contains("Edit") && lines.contains("View"), "menu bar incomplete:\n{lines}");
}

#[test]
fn git_for_windows_vim_and_less_render_and_use_the_alternate_screen() {
    let (Some(vim), Some(less)) = (find_exe("vim.exe"), find_exe("less.exe")) else {
        assert!(
            !tool_required("vim") && !tool_required("less"),
            "vim.exe and less.exe are required (VTEE_REQUIRE_TOOLS) but not found on PATH"
        );
        eprintln!("skipped: Git for Windows vim/less not found");
        return;
    };
    let _guard = serial();
    let file = sample_text_file();
    for program in [s(&vim), s(&less)] {
        let run = run_script(
            &[&wait_for_line("sample-line-03", 20000), SNAPSHOT],
            &["--cols", "100", "--rows", "30", "--env", "TERM=xterm-256color"],
            &[program, s(&file)],
        );
        assert!(run.success, "{program} script failed: {}", run.stderr);
        assert!(run.matched(0), "{program} never showed the file: {:?}", run.last_lines());
        assert_eq!(
            run.results[1]["snapshot"]["alt_screen"].as_bool(),
            Some(true),
            "{program} should be on the alternate screen"
        );
    }
    let _ = std::fs::remove_file(&file);
}

// ---- (4) script determinism ----------------------------------------------------------------------

/// The same small command, 20 times in a row, must give the same screen every time.
#[test]
fn script_mode_is_deterministic_across_twenty_runs() {
    let _guard = serial();
    let dir = unique_path("det", "d");
    std::fs::create_dir_all(&dir).unwrap();
    for name in ["alpha.txt", "beta.txt", "gamma.txt"] {
        std::fs::write(dir.join(name), "x").unwrap();
    }
    let commands: Vec<(Vec<&str>, &str)> =
        vec![(vec!["cmd", "/c", "echo", "hello-vtee"], "hello-vtee"), (vec!["whoami.exe"], "\\")];
    for (program, marker) in &commands {
        let mut first: Option<Vec<String>> = None;
        for run_no in 1..=20 {
            let run = run_script(&[&wait_for_line(marker, 15000), SNAPSHOT], &[], program);
            assert!(run.success && run.matched(0), "{program:?} run {run_no}: {} {:?}", run.stderr, run.results);
            let lines: Vec<String> = run.last_lines().into_iter().filter(|l| !l.is_empty()).collect();
            match &first {
                None => first = Some(lines),
                Some(expected) => assert_eq!(&lines, expected, "{program:?} run {run_no} differs from run 1"),
            }
        }
    }
    let listing = run_script(
        &[&wait_for_line("gamma.txt", 15000), SNAPSHOT],
        &[],
        &["cmd", "/c", "dir", "/b", dir.to_str().unwrap()],
    );
    let lines: Vec<String> = listing.last_lines().into_iter().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines, ["alpha.txt", "beta.txt", "gamma.txt"], "dir listing wrong");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Under ConPTY the emulator only sees what ConPTY forwards. Alternate screen and cursor hiding get
/// through. Mouse reporting (`?1000`, `?1006`) and application cursor keys (`?1`) do not: ConPTY
/// consumes them (`docs/guide/windows.md`), so they are deliberately not asserted here either way.
#[test]
fn modes_that_conpty_forwards_reach_the_emulator() {
    let _guard = serial();
    let command = "[Console]::Out.Write([string][char]27 + '[?1049h' + [string][char]27 + '[?25l' + 'MODE-MARK'); \
                   Start-Sleep -Seconds 3";
    let run = run_script(
        &[&wait_for_line("MODE-MARK", 20000), SNAPSHOT],
        &[],
        &["powershell", "-NoProfile", "-Command", command],
    );
    assert!(run.success && run.matched(0), "{} {:?}", run.stderr, run.results);
    let snap = run.results.iter().rev().find_map(|r| r.get("snapshot")).expect("no snapshot");
    assert_eq!(snap["alt_screen"], true, "alternate screen should reach the emulator: {snap}");
    assert_eq!(snap["cursor"]["hidden"], true, "a hidden cursor should reach the emulator");
}

// ---- (5) paste and flood -------------------------------------------------------------------------

/// Two lines pasted in one write arrive intact and in order.
#[test]
fn pasted_multiline_input_reaches_the_child() {
    let _guard = serial();
    let bat = batch_file("paste", "set /p a=first?\nset /p b=second?\necho got:%a%:%b%:done");
    let mut term = Terminal::spawn(80, 24, &["--no-listen", "--", "cmd", "/c", s(&bat)]);
    assert!(term.wait_for("first?", Duration::from_secs(15)), "prompt never appeared:\n{}", term.text());
    term.send(b"one\r\ntwo\r\n");
    assert!(term.wait_for("got:one:two:done", Duration::from_secs(15)), "paste mangled:\n{}", term.text());
    assert_eq!(term.wait_exit(Duration::from_secs(15)), Some(0));
    let _ = std::fs::remove_file(bat);
}

/// 30,000 lines of output must neither hang `vtee` nor lose the child's exit status.
#[test]
fn a_flood_of_output_keeps_the_exit_status_interactive() {
    let _guard = serial();
    let bat =
        batch_file("flood", "for /l %%i in (1,1,30000) do @echo flood-line-%%i-padding-padding-padding\nexit /b 7");
    let mut term = Terminal::spawn(100, 30, &["--no-listen", "--", "cmd", "/c", s(&bat)]);
    let code = term.wait_exit(Duration::from_secs(90));
    let _ = std::fs::remove_file(bat);
    assert_eq!(code, Some(7), "flood hung or lost the exit status; tail of output:\n{}", {
        let t = term.text();
        t[t.len().saturating_sub(300)..].to_string()
    });
    assert!(term.wait_for("flood-line-30000", Duration::from_secs(10)), "tail of the flood never arrived");
}

/// The emulator keeps up with a flood too: the last line is on screen when the script sees it.
#[test]
fn a_flood_of_output_reaches_the_emulator_in_script_mode() {
    let _guard = serial();
    let bat = batch_file("flood-script", "for /l %%i in (1,1,30000) do @echo flood-line-%%i-padding-padding-padding");
    let run = run_script(&[&wait_for_line("flood-line-30000", 80000), SNAPSHOT], &[], &["cmd", "/c", s(&bat)]);
    let _ = std::fs::remove_file(bat);
    assert!(run.success, "script failed: {}", run.stderr);
    assert!(run.matched(0), "never saw the last flood line; screen tail: {:?}", run.last_lines());
}
