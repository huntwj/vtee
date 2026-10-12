//! "Tests that need no TinyFugue" (HANDOFF.md step 5): run `cat` on the probe sequence files,
//! and check that replaying the raw log reproduces the same state as the live tap did
//! (HANDOFF.md step 4) - the thing that makes a byte-log-based golden test trustworthy at all.

use std::path::PathBuf;

const COLS: u16 = 40;
const ROWS: u16 = 12;
const TESTS: &[&str] =
    &["T1", "T2", "T3", "T4", "T5", "T6", "T7a", "T7b", "T8", "T9", "T10", "T11", "T12", "T13", "T14"];

fn cat_path() -> &'static str {
    if cfg!(windows) { "C:/Program Files/Git/usr/bin/cat.exe" } else { "/bin/cat" }
}

#[test]
fn replay_matches_live_for_every_probe() {
    let dir = std::env::temp_dir().join(format!("vtee-replay-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    for name in TESTS {
        let bin = PathBuf::from("docs/probe").join(format!("{name}.bin"));
        assert!(bin.exists(), "missing probe file {}", bin.display());
        let log_path = dir.join(format!("{name}.cast"));

        let opts = vtee::SpawnOptions {
            cols: COLS,
            rows: ROWS,
            log_path: Some(&log_path),
            forward_input: false,
            forward_output: false,
            env: &[],
        };
        let tap = vtee::Tap::spawn(cat_path(), &[bin.to_string_lossy().into_owned()], opts)
            .unwrap_or_else(|e| panic!("{name}: spawn failed: {e}"));

        let status = tap.wait().unwrap_or_else(|e| panic!("{name}: wait failed: {e}"));
        assert!(status.success(), "{name}: cat exited with {status:?}");

        let live = tap.snapshot();
        let replayed = vtee::replay(&log_path, COLS, ROWS).unwrap_or_else(|e| panic!("{name}: replay failed: {e}"));

        assert_eq!(live.lines, replayed.lines, "{name}: replayed text differs from live");
        assert_eq!(live.cursor.row, replayed.cursor.row, "{name}: cursor row differs");
        assert_eq!(live.cursor.col, replayed.cursor.col, "{name}: cursor col differs");
        assert_eq!(live.alt_screen, replayed.alt_screen, "{name}: alt_screen differs");
        assert_eq!(live.byte_offset, replayed.byte_offset, "{name}: byte_offset differs");
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// `Tap::resize` already refuses a size outside 1 to `MAX_TERMINAL_DIMENSION`; a library caller
/// of `Tap::spawn` bypasses the CLI's own `--cols`/`--rows` validation entirely, so `spawn` must
/// enforce the same bound itself rather than silently clamping (it previously only did this for
/// 0, via `.max(1)`, and never checked the upper bound at all). Unlike `resize`, `replay` is
/// deliberately not bounded this way - see its own doc comment.
#[test]
fn spawn_enforces_the_same_terminal_size_bound_as_resize() {
    for (cols, rows) in
        [(0, 24), (80, 0), (vtee::MAX_TERMINAL_DIMENSION + 1, 24), (80, vtee::MAX_TERMINAL_DIMENSION + 1)]
    {
        let opts =
            vtee::SpawnOptions { cols, rows, log_path: None, forward_input: false, forward_output: false, env: &[] };
        match vtee::Tap::spawn(cat_path(), &[], opts) {
            Ok(_) => panic!("{cols}x{rows} should be refused"),
            Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput, "{cols}x{rows}: {e}"),
        }
    }
}
