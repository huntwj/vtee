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

## Reading a failure

`assert_cmd` prints the command, its exit code, and its stdout and stderr. For a session test, rerun it with
`-- --nocapture` and set `RUST_BACKTRACE=1`. To see what the program was showing when a test timed out, add a
`snapshot` and print its `lines`.

## Known gaps

The main untested areas are listed in the [roadmap](../roadmap.md#beta-readiness): the full snapshot attribute set,
the `redraw_sequence` round trip, `--log` through the CLI, the config-file path, terminal restore after a
signal, and write-policy enforcement for read-only sockets.
