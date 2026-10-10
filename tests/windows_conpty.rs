//! Native Windows (ConPTY) regression tests (`docs/decisions.md`, 2026-10-08 entries).
//!
//! ConPTY, created with `PSEUDOCONSOLE_INHERIT_CURSOR` the way `portable-pty` creates it, opens
//! every session by sending a cursor-position query (`ESC[6n`) to whatever is on the other end
//! of the pty and blocks everything - including the real child's output - until a
//! `ESC[row;colR` reply arrives on its input. Two different things have to answer that:
//!
//! - **Interactive** (`vtee_runs_to_completion_under_a_real_conpty_terminal`): the human's
//!   terminal does. This test stands in for one: it opens a real pty via `portable_pty`,
//!   spawns `vtee` attached to it, and answers every `ESC[6n` it sees. Used to hang on roughly
//!   every other run:
//!   the reply reached `vtee`'s stdin, but `forward_input`'s first read had started while the
//!   console was still in cooked mode, and a cooked read swallowed it. `run_spawn` now enables
//!   raw mode *before* `Tap::spawn`; measured 0 hangs in 30+ consecutive runs after the fix.
//! - **Non-interactive** (`script_mode_answers_conptys_cursor_query_itself`): nothing is on
//!   stdin to answer, so `Tap` answers from its own emulator (`ObserverListener` queues the
//!   `Event::PtyWrite` replies `alacritty_terminal` produces, `forward_output` writes them back).
//!   Hung on every run before that.

#![cfg(windows)]

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// DSR (Device Status Report), cursor-position variant, as ConPTY sends it at startup.
const CURSOR_POSITION_QUERY: &[u8] = b"\x1b[6n";
/// A canned reply - row 1, column 1. Only that *some* well-formed reply arrives matters here.
const CURSOR_POSITION_REPLY: &[u8] = b"\x1b[1;1R";

#[test]
fn vtee_runs_to_completion_under_a_real_conpty_terminal() {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 })
        .expect("failed to open a real ConPTY pty for this test");

    let bin = assert_cmd::cargo::cargo_bin("vtee");
    let mut cmd = CommandBuilder::new(bin);
    cmd.args(["--no-listen", "--", "whoami.exe"]);

    let mut child = pair.slave.spawn_command(cmd).expect("failed to spawn vtee under the test pty");
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let output = Arc::new(Mutex::new(Vec::new()));

    let _reader_thread = {
        let stop = stop.clone();
        let output = output.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            // Scans the accumulated output (a query could straddle two reads) and answers every
            // occurrence, the way a real terminal would. Only the outer session's query is ever
            // seen here: the inner one (`vtee`'s own pty) is forwarded to this pty's conhost,
            // which answers it itself into `vtee`'s stdin - measured, not assumed.
            let mut scanned_up_to = 0usize;
            while !stop.load(Ordering::Relaxed) {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let mut out = output.lock().unwrap();
                        out.extend_from_slice(&buf[..n]);
                        let overlap = CURSOR_POSITION_QUERY.len().saturating_sub(1);
                        let mut i = scanned_up_to.saturating_sub(overlap);
                        while i + CURSOR_POSITION_QUERY.len() <= out.len() {
                            if out[i..i + CURSOR_POSITION_QUERY.len()] == *CURSOR_POSITION_QUERY {
                                let _ = writer.write_all(CURSOR_POSITION_REPLY);
                                let _ = writer.flush();
                                i += CURSOR_POSITION_QUERY.len();
                            } else {
                                i += 1;
                            }
                        }
                        scanned_up_to = out.len();
                    }
                }
            }
        })
    };

    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("try_wait failed") {
            break Some(status);
        }
        if start.elapsed() > Duration::from_secs(15) {
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    stop.store(true, Ordering::Relaxed);
    let _ = child.kill();
    // Deliberately not joined: the reader thread's `read()` is a blocking call on the pty
    // master's handle, which killing the child does not reliably unblock on Windows (measured -
    // joining here hung well past the 15s wait above with the child already gone). A short
    // sleep lets any output already in flight land in `output` before this test's only use of
    // the thread ends; the thread itself is abandoned, harmless for a file with one pty test.
    std::thread::sleep(Duration::from_millis(200));

    let out = String::from_utf8_lossy(&output.lock().unwrap()).into_owned();
    let status = status.unwrap_or_else(|| {
        panic!("vtee never exited within 15s even with the cursor-position query answered - output so far: {out:?}")
    });
    assert!(status.success(), "vtee exited non-zero ({}); output: {out:?}", status.exit_code());
    assert!(
        !out.trim().is_empty(),
        "expected whoami.exe's real output to come through the pty; got nothing but control sequences: {out:?}"
    );
}

/// `--script` has no terminal at all - stdin and stdout are pipes - so the only thing that can
/// answer ConPTY's startup `ESC[6n` is `vtee` itself. `whoami.exe` prints `DOMAIN\user`;
/// matching on the backslash avoids depending on the account name.
#[test]
fn script_mode_answers_conptys_cursor_query_itself() {
    let script = std::env::temp_dir().join(format!("vtee-conpty-script-{}.ndjson", std::process::id()));
    std::fs::write(&script, "{\"cmd\":\"wait_for\",\"text\":\"\\\\\",\"timeout_ms\":10000}\n").unwrap();

    let result = assert_cmd::Command::cargo_bin("vtee")
        .unwrap()
        .args(["--script", script.to_str().unwrap(), "--", "whoami.exe"])
        .timeout(Duration::from_secs(30))
        .output();
    let _ = std::fs::remove_file(&script);

    let output = result.expect("vtee --script did not complete");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "vtee --script failed: {}", String::from_utf8_lossy(&output.stderr));
    assert!(stdout.contains("\"matched\":true"), "whoami.exe's output never reached the emulator: {stdout}");
}
