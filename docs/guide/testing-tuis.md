# Testing a TUI with vtee

This page is for people who write a full-screen program and want tests that read its screen: from a script, from CI,
or from an AI agent. It covers what to pin so a run gives the same screen every time, how to wait, how to compare,
and where today's tooling stops. The command details are in the [protocol reference](protocol.md).

## The idea

`vtee` runs your program in a pseudo-terminal and keeps its screen as data. A test sends keystrokes, waits for
something to appear, and reads a snapshot. The screen is only repeatable if the inputs are, so most of this page is
about the inputs.

## Pin the inputs

A child process inherits `vtee`'s environment and working directory. `--env KEY=VALUE` adds or overrides one
variable; it never removes any. Everything below was run with `vtee --script`.

| What | How | Why |
| --- | --- | --- |
| Screen size | `--cols 100 --rows 30` | Without a terminal the default is 80 by 24. The child sees the size you set (`stty size` printed `30 100`). |
| `TERM` | `--env TERM=xterm-256color` | `TERM` is inherited. If the parent has none, the child gets `TERM=dumb`, and many programs then draw nothing useful. |
| Locale | `--env LC_ALL=C.UTF-8` | `LC_ALL` beats `LANG`. A locale that differs between machines changes banners and character widths. |
| Home and config | `--env HOME=/tmp/cleanhome` | Keeps your own dotfiles out of the test. |
| Time zone | `--env TZ=UTC` | For anything that draws a clock or a date. |
| Working directory | `cd` before you start `vtee` | The child inherits it. |
| The prompt, for a shell | `--env PS1='$ '` | A short fixed prompt keeps the screen small and easy to wait on. |

Because `--env` cannot remove variables, the only way to start the child from a known blank slate is to clear the
environment of `vtee` itself with `env -i`. `env.ndjson` asks the shell to print its environment:

```
{"cmd":"wait_for","text":"$ ","timeout_ms":5000}
{"cmd":"send","text":"env | sort | head -20\r"}
{"cmd":"wait_for","text":"_=","timeout_ms":5000}
```

The lines below are the variables the child saw, filtered with `grep` to the interesting ones:

```
$ mkdir -p /tmp/cleanhome
$ env -i PATH="$PATH" HOME=/tmp/cleanhome TERM=xterm-256color LC_ALL=C.UTF-8 TZ=UTC \
    vtee --script env.ndjson --text --cols 100 --rows 30 --env PS1='$ ' -- bash --norc
HOME=/tmp/cleanhome
LC_ALL=C.UTF-8
PS1=$
PWD=/tmp/ex
SHLVL=1
TERM=xterm-256color
TZ=UTC
```

Keep `PATH` in the list, or the program cannot be found. On Windows, set the same variables in the process that
launches `vtee.exe`; there is no `env -i`, so build the environment explicitly in your test runner.

## Wait for a condition, never for quiet

A TUI can redraw all the time (a clock, a progress bar, `top`), so "the screen has stopped changing" is not a
signal you can build on. Wait for something specific instead:

```
{"cmd":"send","text":"echo $((40+2))\r"}
{"cmd":"wait_for","text":"42","timeout_ms":5000}
```

- Wait for text that only the program's *output* contains. The typed line is echoed, so waiting for text you typed
  matches at once. `$((40+2))` is typed, `42` is output. Other shells need other markers; see
  [the agents guide](agents.md#choosing-what-to-wait-for).
- `wait_for` matches one screen line at a time, so keep the text short or widen the screen (`--cols`) so it does not
  wrap.
- `byte_offset` in a snapshot counts the output processed so far. If two snapshots have the same offset, nothing was
  drawn between them. It confirms freshness; it does not replace waiting on a condition.
- The program draws in several writes, and a snapshot is whatever has arrived. Wait for the last thing the program
  draws for that step.

## Exit status

Ask for the program's exit code with `wait_exit`, after telling it to quit:

```
{"cmd":"send","text":"exit 7\r"}
{"cmd":"wait_exit","timeout_ms":5000}
```

```
{"cmd":"wait_exit","running":false,"code":7,"timed_out":false}
```

A program killed by a signal answers with `"signal":"Terminated"` and no `code`. A program that does not quit in
time answers `"running":true,"timed_out":true`. In `--script` mode `vtee` itself still exits 0 here, so read the
reply.

## A script does not fail by itself

`--script` runs every command and exits 0 whenever they all ran. A `wait_for` that timed out is not a failure: it
prints `matched:false` with the screen as it was, and the exit status is still 0. There are no assertions in a script.
Today the test runner must look at the output. The simplest check:

```
vtee --script t.ndjson --env PS1='$ ' -- bash --norc | grep -q '"matched":false' && echo "a wait timed out"
```

Run against a script whose only command waits for text that never appears, that printed `a wait timed out`. In text
mode the same condition prints the line `--- wait_for matched=false (byte_offset=2) ---`. Fail the test when you see
either.

## Compare snapshots

For text, store the `--text` output of the final snapshot and compare with `diff`. This is a real run of the same
script with `40+2` changed to `40+3`:

```
$ diff -u expected.txt actual.txt
--- expected.txt
+++ actual.txt
@@ -1,5 +1,5 @@
 --- snapshot (byte_offset=24) ---
-$ echo $((40+2))
-42
+$ echo $((40+3))
+43
 $
```

Text mode keeps the line for every screen row, so trim trailing spaces (the `sed 's/[[:space:]]*$//'` filter) before
you store or compare. Use the JSON output when the test cares about colour, bold, reverse video, the cursor, the
title or the modes: the fields are in the [snapshot schema](protocol.md#snapshot-schema). Record the
`schema_version` next to anything you store; the [stability policy](versioning.md) says when it changes.

Volatile parts of a screen (a clock, an idle timer, a process id) have to be handled by your own comparison today.
Compare the stable rows, or match the volatile ones with a pattern, in the code that reads the snapshot.

## A live session in CI

A script is right when every step is known in advance. For a test that decides each step from what it sees, start a
session and talk to it over the socket:

```
vtee --headless --name t1 --writable --cols 100 --rows 30 -- <your-program> &
# wait for the socket (poll `vtee ls` for "live"), then drive it with --connect
vtee kill t1
```

Always end the session. The [examples](../../examples/README.md) are complete, tested programs that do exactly
this: a Python client and shell and PowerShell scripts.

## Linux and Windows in CI

- **Linux.** Any image with the program under test. The full-screen tests in this repository also need `less`, `vim`
  and `procps`; see [CI](../development/ci.md).
- **Windows.** ConPTY works on hosted runners. The terminal modes `?1`, `?1000`, `?1002` and `?1006` never appear in a
  Windows snapshot, so a golden recorded on Linux can differ there. See [Windows](windows.md).
- Keep per-test state apart: a separate `TMPDIR` on Linux or `LOCALAPPDATA` on Windows per test (the session
  directory) and a separate `XDG_CONFIG_HOME` or `APPDATA` (the config file), so tests do not see each other's
  sessions.

## Recording and replaying

`--log run.cast` writes everything the program printed as an [asciicast v2](https://docs.asciinema.org/manual/asciicast/v2/)
file. The first lines of a real one:

```
{"height":24,"timestamp":1791609754,"version":2,"width":80}
[0.001201991,"o","$ "]
[0.012654069,"o","echo hello from vtee\r\n"]
[0.012827626,"o","hello from vtee\r\n$ "]
```

A recording attached to a bug report makes a screen reproducible. `vtee replay run.cast` feeds it into a fresh
emulator and prints the final screen as text (`--json` prints the full snapshot):

```
$ vtee replay run.cast
--- snapshot (byte_offset=10) ---
one
two
```

The library function `vtee::replay` does the same from Rust, and the repository's own tests use it. See the
[CLI reference](cli-reference.md#vtee-replay-file---json).

## Planned: golden files

This does not exist yet. The plan is a text format for a stored screen, with masks for volatile cells (a clock, an
idle timer), assertions on colour and other attributes, and an `vtee golden check` command that reports the first
differing cell. It is designed but not built, and nothing on this page depends on it.
