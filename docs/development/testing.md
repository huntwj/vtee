# Testing

How the tests are organised, what each suite proves, and what you need installed to run them. The commands are in
[CONTRIBUTING.md](../../CONTRIBUTING.md#running-the-tests).

## Suites

Counts and timings are from a run on 2026-10-09 and will drift. Windows timings are from a development machine.

| Suite | Platform | Tests | Time | What it covers |
| --- | --- | --- | --- | --- |
| Unit tests in `src/` | both | about 70 | under 1 s | Named-key tables (`keys.rs`), write-policy authorization (`command.rs`), snapshot fields, colour mapping, TAB cells and the redraw round trip (`snapshot.rs`), session-record parsing |
| `tests/cli.rs` | both (some Unix-only) | 54 on Linux, 21 on Windows | about 1 s | Flag parsing and conflicts, exit codes, the session directory and its permissions, duplicate and invalid session names, `ls` and `kill`, `--attach-if-exists`, script-mode terminal queries, hostile input to the JSON socket, a stalled attach client |
| `tests/replay.rs` | both | 1 | about 9 s on Windows | Replays 15 recorded byte streams into the emulator and compares the result with the live snapshot, byte for byte |
| `tests/linux_sessions.rs` | Linux | 23 | about 6 s | Terminal-query replies with and without an attach client, many clients of both kinds, exclusive-write handoff, `less` and `vim` using the alternate screen and repainting on resize, `kill`/`SIGTERM`/`SIGKILL` lifecycle and orphan checks, attach and detach during a flood, garbage input, random escape-sequence input |
| `tests/windows_conpty.rs` | Windows | 2 | about 1 s | A real ConPTY playing the terminal for `vtee.exe`: interactive completion, and script mode answering ConPTY's startup query |
| `tests/windows_behavior.rs` | Windows | 16 | about 57 s | Size overrides and live resize, Ctrl-C, force-kill outcome, exit codes, `more`, Edit, `vim`/`less`, 20 repeated script runs, multi-line paste, output floods |
| `tests/windows_sessions.rs` | Windows | 18 | about 21 s | Sessions, `ls`, `kill`, duplicate names, both sockets, two clients, stale-file cleanup, and that the child does not inherit socket handles |
| `tests/differential.rs` | both (4 resize cases Unix-only) | 1 test over 80 cases | under 1 s | Runs each escape-sequence case through vtee's emulator and compares every cell with two independent emulators; see [Differential emulator tests](#differential-emulator-tests) |
| `tests/conformance.rs` | both (4 resize cases Unix-only) | 6, plus the ignored `bless` step on Unix | about 3 s | Whether vtee's own output moved: its snapshot for each of the 80 cases, the width sample, the reported vocabulary, the manifest that guards the files and the semantics changelog; see [Conformance tests](#conformance-tests) |
| `tests/golden_vectors/` | both | 6, plus the ignored `bless` step | under 1 s | The golden library against the frozen test vectors: 66 checks with their exact reports, 77 malformed goldens, the canonical round trips, the regex dialect and the escape set, and the manifest that guards them; see [Golden vectors](#golden-vectors) |
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

By default, if `less`, `vim` or `python3` is missing, the corresponding test prints `skipping: <tool> is not installed`
and passes. A green run in a bare image therefore does not prove the full-screen tests ran. The Windows tests that
need Edit or Git's tools look the program up and skip the same way if it is not found.

Set `VTEE_REQUIRE_TOOLS=1` and a missing tool fails the test instead. A comma-separated list (`less,vim`) requires
only those programs. CI sets it, so a missing tool cannot hide a gap. Use the Docker image from
[CONTRIBUTING.md](../../CONTRIBUTING.md#the-linux-test-image) when you need the whole Linux suite.

## Isolation

Tests must not share state or touch your real files.

- Each test that starts a session uses its own temporary directory as `TMPDIR` on Linux (or `LOCALAPPDATA` on
  Windows), so the per-user session directory is private to the test. `tests/cli.rs` builds these with
  `isolated_tmpdir()`; `tests/windows_sessions.rs` sets `LOCALAPPDATA` per test.
- The config file is read from `XDG_CONFIG_HOME` (Linux) or `APPDATA` (Windows). Tests that can reach the config
  code (any duplicate-name test) point those, and `HOME`, at a temporary directory, so a real
  `attach_if_exists = true` on your machine cannot change what a test asserts. `tests/cli.rs` does this for every
  command it runs; a new test that starts `vtee` any other way must do the same.
- Names are chosen per test so parallel tests do not collide.
- A test must stop the processes it starts, even when an assertion fails. `linux_sessions.rs` and `cli.rs` both wrap
  a started process in a guard object that kills and reaps it when dropped.

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

Of the 80 cases, 58 agree across all three emulators and 22 are adjudicated. Most of those are `vt100` or `avt`
lacking a feature (HVP, `CSI s`/`u`, IND/NEL, IRM, REP, `?7l`, charsets, colon-form colours, `4:3`/`4:0` underline
sub-parameters, tab-stop set and clear). Three cases are vtee deviations, inherited from `alacritty_terminal`
0.26.0 and not fixed in vtee (see `docs/decisions.md`):

| Case | What vtee does | What xterm does |
| --- | --- | --- |
| `erase_display_above` | `CSI 1 J` with the cursor on the second row leaves the first row | Erases from the top of the screen to the cursor |
| `origin_mode` | Resetting `?6` leaves the cursor where it is | Homes the cursor on both set and reset |
| `alt_screen_47` | Ignores `?47`, so text lands on the main screen | Switches screens |

The harness found a fourth problem that vtee did fix: `alacritty_terminal` writes a literal TAB into the cell where a
tab-stop movement started, so snapshot `lines` and `cells[].ch` contained TAB characters where a terminal shows a
blank. `src/snapshot.rs` now maps a TAB cell to a space, in the visible grid and in scrollback text, and
`redraw_sequence` never emits one (`tab_default_stops` now agrees with both oracles; `tab_set_and_clear` differs only
where `vt100` ignores tab-stop set and clear).

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

## Conformance tests

The differential tests ask whether vtee is right. `tests/conformance.rs` asks whether vtee **changed**, because a
stored golden depends on what a snapshot means, not only on how it is laid out. Everything it checks is frozen data in
`tests/fixtures/conformance/`, and changing any of it is a `SEMANTICS_VERSION` bump ([versioning](../guide/versioning.md)):

| Data | What fails when it changes |
| --- | --- |
| `expected/<case>.json`, one per differential case | vtee's snapshot for that input (without `byte_offset` and `producer`), compared as parsed values. |
| `width-sample.txt` | The width of 1,062 scalars. The test writes one row per scalar through the real emulator and reads the column of the following character; an upgrade of `unicode-width`, `vte` or `alacritty_terminal` that moves a width fails here. |
| `vocabulary.txt` | Every field, mode token, attribute, underline value, cursor shape and colour form a snapshot can report, measured from the emulator (each SGR parameter, every indexed colour, the cursor styles, 1,200 private modes, a hyperlink). A new or removed name fails. |
| `MANIFEST` | The `SEMANTICS_VERSION` the files were written under and a hash of each one (each width-sample line on its own). It is what notices a file that changed, was deleted or was added. |

Adding a case or a width-sample line is not a bump; changing or removing one is. A failure prints what to do. After
adding a case, or after a deliberate change with `SEMANTICS_VERSION` bumped and an entry added to
[snapshot semantics](../guide/snapshot-semantics.md), regenerate on Linux (Docker works), because only there do the
resize cases run:

```
cargo test --test conformance -- --ignored bless
```

`bless` rewrites the expected files, the vocabulary and the manifest. It refuses to change a file the manifest records
while `SEMANTICS_VERSION` is unchanged, so the bump cannot be skipped by regenerating. Review the diff of the fixtures
like any other: it is the change in meaning you are about to ship. Commit it with the `snapshot` scope.

## Golden vectors

`src/golden/` reads, writes, compares and reports goldens ([design](../design/snapshot-and-golden-contract.md), section 6). `tests/fixtures/golden-vectors/` holds the reference data a second implementation of the checker passes to prove it agrees: checks with their exact text and JSON reports and exit codes (`runs/`), malformed goldens (`reject/`), canonical goldens that must read and write back unchanged (`roundtrip/`), the regex dialect (`regex/vectors.tsv`) and the escape set. Its `README.md` explains the layout.

The files are generated from `tests/golden_vectors/defs.rs` and `more.rs`, and each definition states the verdict it must produce, so regenerating cannot record a wrong answer unnoticed:

```
cargo test --test golden_vectors -- --ignored bless
```

Like the conformance files they are frozen: `MANIFEST` records a hash of each and the `SEMANTICS_VERSION` they were written under, and `bless` refuses to change a recorded file while that number is unchanged. Adding a vector is not a change; changing one is a semantics bump.

## Reading a failure

`assert_cmd` prints the command, its exit code, and its stdout and stderr. For a session test, rerun it with
`-- --nocapture` and set `RUST_BACKTRACE=1`. To see what the program was showing when a test timed out, add a
`snapshot` and print its `lines`.

## Known gaps

The main untested area is listed in the [roadmap](../roadmap.md#beta-readiness): a live read-only socket rejecting
`send`, `send_key` and `resize` (the policy table in `command.rs` is unit-tested, the wiring in `sockets.rs` is
not). Snapshot fields, the `redraw_sequence` round trip, `--log` through the CLI, the config-file path and terminal
restore after a signal are covered, and the differential tests cover the per-cell attributes of the snapshot.
Windows has no test for a force-killed holder restoring the console, because nothing does.
