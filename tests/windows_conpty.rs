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
//! `#[ignore]`d by default (`docs/decisions.md`): measured to be genuinely flaky, not just slow -
//! 2 of 3 identical runs passed, 1 hung the full 15s, even with the `AllocConsole` mitigation in
//! place. That matches the underlying GitHub issue's own "machine-dependent" character, apparently
//! extending to run-to-run variance on one machine, not just machine-to-machine - a 1-in-3 failure
//! rate makes this unfit as a normal gate (it would fail builds for reasons unrelated to any real
//! regression). Run manually with `cargo test --test windows_conpty -- --ignored` when actually
//! investigating this area, not as part of routine verification.

#![cfg(windows)]

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
            while !stop.load(Ordering::Relaxed) {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let chunk = &buf[..n];
                        output.lock().unwrap().extend_from_slice(chunk);
                        // The minimal terminal emulation this test exists to provide: a real
                        // terminal answers a cursor-position query automatically, so stand in
                        // for one rather than leaving ConPTY's own handshake unanswered.
                        if chunk.windows(4).any(|w| w == b"\x1b[6n") {
                            let _ = writer.write_all(b"\x1b[1;1R");
                            let _ = writer.flush();
                        }
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
