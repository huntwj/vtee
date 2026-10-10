# Testing

How the tests are organised, what each suite proves, and what you need installed to run them. The commands are in
[CONTRIBUTING.md](../../CONTRIBUTING.md#running-the-tests).

## Suites

Counts and timings are from a run on 2026-10-09 and will drift. Windows timings are from a development machine.

| Suite | Platform | Tests | Time | What it covers |
| --- | --- | --- | --- | --- |
| Unit tests in `src/` | both | 6 | under 1 s | Named-key tables (`keys.rs`), colour mapping (`snapshot.rs`) |
| `tests/cli.rs` | both (some Unix-only) | 26 on Linux, 10 on Windows | about 2 s | Flag parsing and conflicts, exit codes, the session directory and its permissions, duplicate and invalid session names, `ls` and `kill`, `--attach-if-exists`, script-mode terminal queries, hostile input to the JSON socket, a stalled attach client |
| `tests/replay.rs` | both | 1 | about 9 s on Windows | Replays 15 recorded byte streams into the emulator and compares the result with the live snapshot, byte for byte |
| `tests/linux_sessions.rs` | Linux | 12 | about 5 s | Terminal-query replies with and without an attach client, many clients of both kinds, exclusive-write handoff, `less` and `vim` using the alternate screen and repainting on resize, `kill`/`SIGTERM`/`SIGKILL` lifecycle and orphan checks, attach and detach during a flood, garbage input, random escape-sequence input |
| `tests/windows_conpty.rs` | Windows | 2 | about 1 s | A real ConPTY playing the terminal for `vtee.exe`: interactive completion, and script mode answering ConPTY's startup query |
| `tests/windows_behavior.rs` | Windows | 13 | about 60 s | Size overrides and live resize, Ctrl-C, force-kill outcome, exit codes, `more`, Edit, `vim`/`less`, 20 repeated script runs, multi-line paste, output floods |
| `tests/windows_sessions.rs` | Windows | 11 | about 14 s | Sessions, `ls`, `kill`, duplicate names, both sockets, two clients, stale-file cleanup, and that the child does not inherit socket handles |
| `tests/differential.rs` | both (4 resize cases Unix-only) | 1 test over 80 cases | under 1 s | Runs each escape-sequence case through vtee's emulator and compares every cell with two independent emulators; see [Differential emulator tests](#differential-emulator-tests) |
| `tests/windows_console.rs` | Windows | 2 | under 1 s | The console VT flags `RawModeGuard` sets and restores, and that an Up-arrow reaches the program as `ESC [ A` |

The CLI tests run the real binary through `assert_cmd` and check what a user sees (exit code, output, files), not
internals. The Linux and Windows session suites do the same against live sessions.

## Platform matrix

| Behaviour | Linux | Windows |
| --- | --- | --- |
| Spawn, script, snapshot | tested | tested |
| Sessions, `attach`, `ls`, `kill` | tested | tested |
| JSON socket, write policies | tested | tested for the basics; exclusive handoff only on Linux |
| Raw terminal socket | tested, including a stalled reader | tested |
| Resize | tested through `vim` and `less` | tested |
| Terminal restore after `SIGKILL` | not tested directly | no watchdog exists |
| Ctrl-C | not tested | tested |

## Tests that need external programs

Some tests drive real programs:

| Program | Used by | Install |
| --- | --- | --- |
| `bash`, `cat`, `printenv`, `sh` | most Linux tests | present in any Linux base image |
| `less`, `vim` | `linux_sessions.rs` | `apt-get install less vim-tiny` |
| `ps`/`/proc` | process cleanup checks | `procps` |
| `ping`, `more`, `whoami`, `cmd`, PowerShell | Windows tests | part of Windows |
| Edit, Git for Windows' `vim` and `less` | `windows_behavior.rs` | optional |

Today, if `less` or `vim` is missing, the corresponding test prints `skipping: <tool> is not installed` and passes.
A green run in a bare image therefore does not prove the full-screen tests ran. Use the Docker image from
[CONTRIBUTING.md](../../CONTRIBUTING.md#the-linux-test-image) when you need the whole suite. The Windows tests that
need Edit or Git's tools look the program up and skip the same way if it is not found.

Planned: an environment variable, `VTEE_REQUIRE_TOOLS`, that turns those skips into failures, set in CI so a
missing tool cannot hide a gap. If it is not yet in the tree you are reading, the skipping behaviour above still
applies.

## Isolation

Tests must not share state or touch your real files.

- Each test that starts a session uses its own temporary directory as `TMPDIR` on Linux (or `LOCALAPPDATA` on
  Windows), so the per-user session directory is private to the test. `tests/cli.rs` builds these with
  `isolated_tmpdir()`; `tests/windows_sessions.rs` sets `LOCALAPPDATA` per test.
- The config file is read from `XDG_CONFIG_HOME` (Linux) or `APPDATA` (Windows). Tests that can reach the config
  code (any duplicate-name test) should point those at a temporary directory too. Not every existing test does yet;
  a real config on your machine can change their outcome.
- Names are chosen per test so parallel tests do not collide.
- A test must stop the processes it starts, even when an assertion fails. `linux_sessions.rs` uses a guard object
  for this. The older helpers in `cli.rs` kill at the end of the test, so a failed assertion can leak a process.

## Waiting

Tests wait for conditions by polling with a timeout, not by sleeping: for a socket to appear, for text on the
screen (`wait_for`), or for a file to disappear. A few tests keep a short fixed delay because they assert that
something does not happen, or they wait on a relay that cannot be observed from outside; each has a comment.

## Hunting flakiness

The pty tests are the ones that can be timing-sensitive. To repeat them:

```
for i in $(seq 1 20); do cargo test --test linux_sessions || break; done
```

On Windows, repeat the three `windows_*` test files the same way. Restrict CPUs in Docker (`--cpus=2`) to expose
timing assumptions that a fast machine hides. If a test fails only under load, the usual cause is a wait that is
too short or a fixed sleep standing in for a condition.

The Windows ConPTY tests were once flaky (about half of runs hung) because ConPTY's startup cursor query went
unanswered. That is fixed; see [internals](internals.md#conpty-startup-query). If they hang again, suspect that
first.

## Differential emulator tests

`tests/differential.rs` answers "is vtee's virtual terminal right?" by comparison. The corpus is
`tests/fixtures/differential/cases.json`: 80 small named cases (SGR attributes and colours, cursor movement,
erase, scroll regions, insert/delete, wide and combining characters, tabs, alternate screen, line-drawing
charsets, wrapping, origin mode, live resize, and a few vtee-only checks for title, hyperlinks and cursor
shape). Inputs use `\e` for ESC and `\\` for a backslash; CR, LF and BEL are ordinary JSON escapes.

Each case runs through vtee's emulator (`vtee::replay`, the same `Term` the live tap uses; the resize
cases go through a live `Tap`, on Unix only) and is compared cell by cell, on text, colours, six attributes
(bold, dim, italic, underline, reverse, strike) and the cursor, with the committed output of two independent
emulators, `avt` and `vt100` (`oracle_avt.json`, `oracle_vt100.json`). An attribute an oracle cannot report
(`vt100` has no strike-through) is skipped for that oracle.

- **A case passes** when vtee equals both oracles.
- **A disagreement is only accepted** if `adjudications.json` records it, with a verdict and the exact list of
  differences. The test checks that list, so a fix or a regression changes it and fails the test: a recorded bug
  that gets fixed makes the test fail until its entry is removed.
- **Reading a failure:** the message names the case, the two emulators, and the first differing row and column with
  both sets of attributes.

Of the 80 cases, 57 agree across all three emulators and 23 are adjudicated. Most of those are `vt100` or `avt`
lacking a feature (HVP, `CSI s`/`u`, IND/NEL, IRM, REP, `?7l`, charsets, colon-form colours, `4:3`/`4:0` underline
sub-parameters). Five are vtee deviations, inherited from `alacritty_terminal` 0.26.0 and not fixed in vtee:

| Case | What vtee does | What xterm does |
| --- | --- | --- |
| `erase_display_above` | `CSI 1 J` with the cursor on the second row leaves the first row | Erases from the top of the screen to the cursor |
| `origin_mode` | Resetting `?6` leaves the cursor where it is | Homes the cursor on both set and reset |
| `alt_screen_47` | Ignores `?47`, so text lands on the main screen | Switches screens |
| `tab_default_stops`, `tab_set_and_clear` | A tab writes a literal TAB into the cell it starts in, so snapshot `lines` contain TAB | Leaves the cell blank (positions are correct) |

`resize_shrink_cols` is a difference of design: vtee re-wraps long lines when the width shrinks, as alacritty, VTE
and kitty do, where xterm truncates.

### Adding or changing a case

1. Edit `cases.json`.
2. Regenerate the oracles: `sh tests/differential/regenerate.sh` (uses Docker, or a local toolchain with network
   access; the generator in `tests/differential/oracle/` is a standalone crate, not part of the vtee build).
3. Triage: `VTEE_DIFF_REPORT=/tmp/report.json cargo test --test differential` writes every disagreement to the
   file instead of failing. Decide for each which emulator is right, by xterm's documented behaviour, not by
   majority.
4. Add an entry to `adjudications.json` with `verdict`, `reason` and the `problems` list from the report. A case
   where all three agree needs no entry.

## Reading a failure

`assert_cmd` prints the command, its exit code, and its stdout and stderr. For a session test, rerun it with
`-- --nocapture` and set `RUST_BACKTRACE=1`. To see what the program was showing when a test timed out, add a
`snapshot` and print its `lines`.

## Known gaps

The main untested areas are listed in the [roadmap](../roadmap.md#beta-readiness): the `redraw_sequence` round
trip, `--log` through the CLI, the config-file path, terminal restore after a signal, and write-policy enforcement
for read-only sockets. The differential tests now cover the per-cell attributes of the snapshot.
