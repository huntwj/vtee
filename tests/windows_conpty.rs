//! Automates the verification a human did manually from a real terminal (`docs/decisions.md`,
//! 2026-10-08): does `vtee` actually work when run under a *real* ConPTY-backed terminal, not
//! just when its own stdin/stdout are redirected pipes (which have no way to answer ConPTY's own
//! cursor-position query, `ESC[6n`, and so hang forever - a real terminal answers that
//! automatically, which is what this test stands in for). Acts as a minimal terminal: opens a
//! real pty via our own (patched) `portable_pty`, spawns `vtee` attached to it, and answers
//! any `ESC[6n` it sees so the session can actually complete, the same way a real terminal would.
//!
//! Windows-only: this is specifically a regression test for the `CREATE_NEW_PROCESS_GROUP` patch
//! (`vendor/portable-pty/README-vtee-patch.md`) - Linux never had the bug it guards against.
//!
//! `#[ignore]`d by default (`docs/decisions.md`): measured to be genuinely flaky, not just slow,
//! and the flakiness rate itself bounces around across batches of identical runs (2/3, then 1/5,
//! then 5/6 failed) with no code change between them - the signature of a lower-level race, not
//! an application-level bug. Two concrete, plausible causes were investigated and ruled out with
//! actual evidence, not assumed away:
//! - Whether the query could arrive split across two `read()`s, so checking only one chunk in
//!   isolation could miss it - fixed anyway (checking the accumulated buffer is strictly more
//!   correct), but diagnostics showed the query is always seen intact as the very first 4 bytes,
//!   pass or fail.
//! - Whether this test's own nested ConPTY sessions (the outer one hosting `vtee.exe`, the
//!   inner one `vtee.exe` creates internally for `whoami.exe`) each emit their own independent
//!   query, with a single-shot reply swallowing the second - fixed anyway (replies to every
//!   occurrence found, not just the first), but diagnostics showed only one query ever appears
//!   at all, in every run, pass or fail.
//!
//! Both fixes are kept as real robustness improvements (correct behavior regardless), but neither
//! explains the actual hang - whatever's underneath is lower-level than this test can observe.
//! A 1-in-3-to-1-in-2-ish failure rate makes this unfit as a normal gate (it would fail builds for
//! reasons unrelated to any real regression). Run manually with
//! `cargo test --test windows_conpty -- --ignored` when actually investigating this area, not as
//! part of routine verification.

#![cfg(windows)]

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// DSR (Device Status Report), cursor-position variant: ConPTY issues this toward whatever's on
/// the other end of its pty at session startup, expecting `CURSOR_POSITION_REPLY` back via
/// stdin - a real terminal answers it automatically, which is what this test stands in for.
const CURSOR_POSITION_QUERY: &[u8] = b"\x1b[6n";
/// A canned reply to `CURSOR_POSITION_QUERY` - row 1, column 1. The actual position doesn't
/// matter for this test; only that *some* well-formed reply unblocks whatever's waiting on one.
const CURSOR_POSITION_REPLY: &[u8] = b"\x1b[1;1R";

#[test]
#[ignore]
fn vtee_runs_to_completion_under_a_real_conpty_terminal() {
    // Measured, not a precaution added on spec (docs/decisions.md): this test's own `openpty()`
    // call below hung when *this test process* had no console of its own (reproduced via a
    // PowerShell `Start-Process`-wrapped `cargo test` - passed every time run directly or via a
    // Bash-tool-invoked `cargo test`). `AllocConsole` is a no-op failure (ignored) if one
    // already exists. `Tap::spawn` carries the same call for the same reason in production.
    unsafe {
        winapi::um::consoleapi::AllocConsole();
    }
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
            // How far the query-scan has gotten, and how many replies have been sent - *not* a
            // single-shot flag. Measured, not assumed, that this test nests two separate ConPTY
            // sessions (the outer one hosting `vtee.exe` itself, created by this test's own
            // `openpty()`; the inner one `vtee.exe` creates internally via `Tap::spawn` to
            // host `whoami.exe`) - if each session's own startup handshake emits its own
            // cursor-position query independently, and `vtee`'s `forward_output` relays the
            // inner one through transparently (which it does - it has no logic to answer
            // escape sequences itself, same as it wouldn't for a real human's terminal), a
            // single-shot "answered" flag would silently swallow the second one, leaving it
            // unanswered forever - diagnostics showed the first query always gets a successful
            // reply, pass or fail, which pointed here rather than at the reply itself.
            let mut scanned_up_to = 0usize;
            let mut replies_sent = 0u32;
            while !stop.load(Ordering::Relaxed) {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let mut out = output.lock().unwrap();
                        out.extend_from_slice(&buf[..n]);
                        // Rescan from a little before where the last scan ended, in case a
                        // previous read ended mid-pattern - the minimal terminal emulation this
                        // test exists to provide: a real terminal answers every cursor-position
                        // query automatically, so stand in for one rather than leaving any of
                        // ConPTY's own handshakes unanswered.
                        let overlap = CURSOR_POSITION_QUERY.len().saturating_sub(1);
                        let mut i = scanned_up_to.saturating_sub(overlap);
                        while i + CURSOR_POSITION_QUERY.len() <= out.len() {
                            if out[i..i + CURSOR_POSITION_QUERY.len()] == *CURSOR_POSITION_QUERY {
                                replies_sent += 1;
                                eprintln!("[test] cursor-position query #{replies_sent} at offset {i}, replying");
                                let write_result = writer.write_all(CURSOR_POSITION_REPLY);
                                let flush_result = writer.flush();
                                eprintln!(
                                    "[test] reply #{replies_sent} write: {write_result:?}, flush: {flush_result:?}"
                                );
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
    // the thread ends; the thread itself is abandoned, harmless for a file with one test.
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
