# CLI reference

```
vtee [OPTIONS] [--] [PROGRAM]...
vtee --script FILE --expect GOLDEN [--update[=MODE]] [OPTIONS] -- PROGRAM...
vtee attach [NAME]
vtee ls
vtee kill <NAME>
vtee replay [--json] <FILE>
```

The mode is chosen by which flags you give, not by a subcommand: `--script` runs a script, `--connect` talks to an
existing session, `--expect` checks a run against a golden file, anything else spawns `PROGRAM`. The subcommands `attach`, `ls` and `kill` manage existing
sessions, and `replay` shows a recording. `vtee help <subcommand>` and `vtee <subcommand> --help` print the
help for one. The option descriptions below follow `vtee --help`; run that for the exact wording on your build.

Everything after the last recognised flag is the program and its arguments. Put `--` before the program if its
name or first argument could be mistaken for an `vtee` flag. Flags that come after the program name are passed
to the program, not to `vtee`:

```
$ vtee --no-listen -- printenv HOME
```

## Modes and what they conflict with

| Flag | Mode | Notes |
| --- | --- | --- |
| (none) | Interactive | Your terminal is forwarded to and from the program; sockets are opened. |
| `--headless` | Agent-only | No human stdin or stdout. Conflicts with `--no-listen`. |
| `--script <PATH>` | Batch | Conflicts with `--listen`, `--no-listen`, `--writable`, `--no-stdin`, `--dump-on-exit`, `--headless`, `--name`, `--attach-if-exists` and `--no-attach-if-exists`. Needs a `PROGRAM`, or `--connect`. |
| `--connect <PATH>` | Client | Reads JSON commands from stdin. No `PROGRAM` is needed, and one is an error. With `--script` it runs that script against the session instead. |
| `--expect <GOLDEN>` | Screen test | Compares the screens a run produces with a golden file. See [Testing with goldens](#testing-with-goldens---expect). |

## Options

| Option | Default | Meaning |
| --- | --- | --- |
| `-v`, `--verbose` | info level | Increase log verbosity (repeatable). |
| `-q`, `--quiet` | | Decrease log verbosity (repeatable). |
| `--text` | off | With `--script` or `--connect`: print only the screen text of each result instead of the full JSON. Ignored otherwise. |
| `--connect <PATH>` | | Connect to a session's JSON socket instead of spawning. Reads commands from standard input, or with `--script` runs the script against the session and leaves it running. |
| `--log <PATH>` | off | Record the program's raw output as an asciicast v2 file. `vtee replay <PATH>` shows its final screen. `-v` controls diagnostic logging, which is separate. |
| `--script <PATH>` | | Run a newline-delimited JSON script, then exit. A `wait_for` that times out makes the run exit 1 unless its line says `"optional":true`; the script still runs to its end. |
| `--expect <GOLDEN>` | | Compare the screens the run produces with GOLDEN, or with `--update` write GOLDEN from them. The terminal defaults to 80 by 24. |
| `--update[=<MODE>]` | | With `--expect`: rewrite the golden from the run. `record` (the bare form), or `restamp`. The `=` is required for a mode. Refused when `CI` is set, unless `VTEE_ALLOW_UPDATE=1`. |
| `--screen <N>` | | With `--expect`: compare (or, with `--update`, rewrite) one screen, counted from 1, of a golden with several. |
| `--format <FORMAT>` | `text` | With `--expect`: print the report as `text` or `json`. |
| `--portable` | | With `--expect --update`, creating a golden: write the `ignore` lines that make it pass on Linux and Windows. |
| `--fail-fast` | | With `--expect`: stop at the first difference or timed-out wait; report the rest as skipped. |
| `--max-mismatches <N>` | 20 | With `--expect`: show at most N mismatches (the total is still counted). |
| `--snapshot <FILE>` | | With `--expect`: check saved output instead of running anything. `-` reads standard input. |
| `--listen[=<PATH>]` | on, at the default path | Choose the JSON socket path. The `=` is required for a path: `--listen /tmp/x` is read as the program `/tmp/x`. Conflicts with `--no-listen`. |
| `--no-listen` | | Create no socket, pid file or session name, so nothing is left in the session directory (useful for a CI run driven only through stdin and stdout). Conflicts with `--listen`, `--writable` and `--headless`. |
| `--writable[=<MODE>]` | read-only | Let clients of the JSON socket write (`send`, `send_key`, `resize`), not just read. Bare `--writable` means `open`. Modes: `open` (anyone, no arbitration), `exclusive` (one writer at a time, who takes access with `acquire_write`), `exclusive-shared` (as `exclusive`, and typing at this session's own terminal, or through `attach`, takes access back). See [write policies](protocol.md#write-policies). |
| `--no-stdin` | off | Show the program's output in this terminal but do not send this terminal's keystrokes to it. Socket clients and `attach` can still type. |
| `--dump-on-exit` | off | When the program exits, print the final screen to stderr. |
| `--headless` | off | Run without using this terminal at all: no input or output forwarding, so the session is driven through its sockets and `vtee attach`. See [Getting started](getting-started.md#headless-for-agents). |
| `--name <NAME>` | program basename | Session name, so `attach`, `ls` and `kill` can find it. A live session's name cannot be reused unless `--attach-if-exists` is set. See [Sessions](sessions.md). |
| `--attach-if-exists` | off, or the config file | If a live session already has this name, attach to it instead of failing (like tmux's `new-session -A`). Overrides the config file's `attach_if_exists`. |
| `--no-attach-if-exists` | | Fail if the name is taken, even when the config file sets `attach_if_exists = true`. |
| `--cols <N>` | terminal width, else 80 | Terminal width in columns, 1 to 1000. Applies to `--script` too. A value outside the range is refused at startup with exit status 64. |
| `--rows <N>` | terminal height, else 24 | Terminal height in rows, 1 to 1000. Applies to `--script` too. Same range check as `--cols`. |
| `--env <KEY=VALUE>` | | Set an environment variable for the program (repeatable). Added to the inherited environment, which is not cleared. Applies to `--script` too. |
| `-h`, `--help` | | Print help. |
| `-V`, `--version` | | Print the version (for example `vtee 0.1.0`). |

`--writable` also needs an equals sign for its value: `--writable=exclusive`.

`--cols` and `--rows` are checked before anything starts:

```
$ vtee --no-listen --cols 0 -- true
vtee: --cols 0 is out of range: it must be between 1 and 1000
$ echo $?
64
```

The `resize` command on the JSON socket applies the same 1 to 1000 range and refuses a value outside it with an
error reply, rather than clamping it (see [protocol](protocol.md#resize)).

The raw-terminal socket that `attach` uses always allows typing. `--writable` governs only the JSON socket.

## Testing with goldens (`--expect`)

A golden is a text file holding the expected output of a run: the screens its `snapshot` commands produce, in order,
with the volatile parts masked. A test is three things, a program, a script and a golden:

```
vtee --script flow.ndjson --expect flow.golden --update -- tf     # record: CREATED flow.golden: 3 screens
vtee --script flow.ndjson --expect flow.golden -- tf              # check:  PASS flow.golden (3 screens)
```

Where the screens come from:

| Form | Screens | Afterwards |
| --- | --- | --- |
| `--script FILE --expect G -- PROGRAM` | one per `snapshot` in the script | the program is stopped |
| `--connect SOCK --script FILE --expect G` | the same, run against a live session | the session keeps running |
| `--connect SOCK --expect G` | one: the session's screen now | the session keeps running |
| `--snapshot FILE --expect G` (or `-`) | one per document in the saved output of an earlier `--script` run | none: nothing is run |

The size defaults to 80 by 24 here, not to the terminal's, so a result does not depend on where it was run. The run
reads no standard input (except `--snapshot -`) and writes nothing to the terminal. The whole script is read, and
the golden's version numbers checked, before the program is started: a bad script or a golden this build will not
read costs no run.

A script's `snapshot` can carry a `name`, which the golden records and a later run checks when both sides have one.
A `wait_for` that times out is a problem of the run; if it may time out, say so on its line with `"optional":true`.

The report is on standard output, `PASS`, `FAIL` or `REFUSED` on its first line; `--format json` prints one JSON
object instead. The golden file format is described in [golden-format.md](golden-format.md); the report is specified in
section 6.9 of the [snapshot and golden contract](../design/snapshot-and-golden-contract.md).

**Exit codes of a run with `--expect`:**

| Code | Meaning |
| --- | --- |
| 0 | Pass, or an update that succeeded (including one that changed nothing). |
| 1 | The output differs from the golden, or a `wait_for` timed out, or an update refused to write. |
| 2 | Refused: the golden was recorded under another `schema` or `semantics` number (or a newer grammar), so nothing was compared. The report says what to run next. This is deliberately not a sysexits code, so a script can tell "differs" from "cannot compare". |
| 64 | Usage error: a combination of options that makes no sense (the message says which), a bad `--update` mode, a `--screen` that does not fit. |
| 65 | The golden, the saved output or the script is malformed. The message names the file and the line (`script line N: ...`, `snapshot: line N: ...`). |
| 66 | The golden (when not recording), the script or the saved output cannot be opened. |
| 69 | The session cannot be reached (`--connect`). |
| 71 | The program could not be started. |
| 74 | Another I/O error, including a golden that cannot be written. |

**Updating.** `--update` (the same as `--update=record`) creates the golden, or regenerates the screens of an existing
one and keeps what a person wrote in it: `ignore` lines, shared and per-screen `[masks]`, `[assert]` sections and
comments. It writes only if the run succeeded; a timed-out wait writes nothing and exits 1. It always prints what
changed:

```
UPDATED flow.golden: 3 screens
  screen 2: 14 cells changed (text 3, attrs 11), cursor moved
  screen 3: new (no masks, no asserts: add them by hand)
```

`--update=restamp` rewrites only the `semantics` number (and the advisory `x-producer` line), and only if a run
under this build still matches everything the golden checks: the safe answer when a `vtee` upgrade changed nothing
your golden cares about. Otherwise it writes nothing, prints the differences and exits 1. `--update=migrate` is not
available in this release (nothing exists to migrate from yet).

Because a pipeline that records its own expectations would pass forever, **every `--update` is refused with exit 64
when the `CI` environment variable is set and non-empty**, unless `VTEE_ALLOW_UPDATE=1` is also set. A check is never
guarded.

**Combinations that are refused (exit 64).** Each message says what to do; the main ones: `--expect` needs a source
(a program with `--script`, `--connect`, or `--snapshot`); a program needs `--script`; `--script` needs a program or
`--connect`; `--connect` cannot be combined with a program; `--snapshot` cannot be combined with `--connect`,
`--script`, a program, `--cols`, `--rows`, `--env` or `--log`; `--update`, `--screen`, `--format`, `--portable`,
`--fail-fast`, `--max-mismatches` and `--snapshot` all need `--expect`; `--text` has no effect with `--expect`;
`--portable` goes with `--update` (record); `--screen` goes with checking and `--update` (record).

## Subcommands

### `vtee attach [NAME]`

Attach your terminal to a live session. You see the current screen first, then live output; your keystrokes go to
the program. With no `NAME` it uses the only live session, or the one attached to most recently if there are several (the
one started last, if none has been attached to yet).
**There is no detach key, and Ctrl-C does not leave `attach`.** Every key, Ctrl-C included, goes to the program.
To leave, close the terminal window or stop the `vtee attach` process from another shell (`kill` it, or end it
in Task Manager on Windows). That ends the attach client and leaves the session running. The same text is in
`vtee attach --help`.

`attach` needs a terminal on both stdin and stdout, because the screen arrives as escape codes. With either one
redirected it refuses before looking for the session and exits 64. To read a session from a script, use
`--connect` instead.

```
$ vtee attach demo | cat
vtee: attach needs a terminal: stdin and stdout must both be terminals (use --connect for scripts)
$ echo $?
64
```

```
$ vtee attach nosuch
vtee: failed to attach to "nosuch": No connection could be made because the target machine actively refused it. (os error 10061)
$ echo $?
68
```

The error text after the colon comes from the operating system, so it differs between Linux and Windows.

### `vtee ls`

One line per session: `name`, `pid=<holder pid>`, and `live` or `stale`. A session started with `--listen=<path>`
has a fourth field, `sock=<path>`, naming the JSON socket it actually bound. Prints `no sessions` when there are
none.

```
$ vtee ls
demo	pid=2221	live
```

A `stale` entry is a leftover from a holder that was killed without cleaning up. `vtee kill <name>` removes it.

### `vtee kill <NAME>`

If the session is live, ends its holder process (which ends the wrapped program) and removes the session's
sockets (the ones recorded in its pidfile, including a `--listen=<path>` socket) and the pidfile. If the session
is stale, only the leftover files are removed and nothing is signalled. It exits 0 in both cases.

```
$ vtee kill nosuch
vtee: no session named "nosuch"
$ echo $?
68
```

A name that cannot be a session here also exits 68, with the reason after the colon (on Windows, a session
directory so long that no socket fits; see [Windows](windows.md#sessions)).

### `vtee replay <FILE> [--json]`

Replays a recording made with `--log` into a fresh emulator and prints the final screen. The terminal size comes
from the recording's header. By default the output is the same text form `--text` gives; with `--json` it is the
full snapshot, as the JSON socket would return it.

```
$ vtee --script s.ndjson --text --log run.cast --cols 20 --rows 4 -- sh -c 'printf "one\ntwo\n"; sleep 2'
--- wait_for matched=true (byte_offset=10) ---
one
two


$ vtee replay run.cast
--- snapshot (byte_offset=10) ---
one
two


$ vtee replay run.cast --json | cut -c1-150
{"schema_version":1,"semantics_version":1,"byte_offset":10,"cols":20,"rows":4,"lines":["one                 ","two                 ","                    ","
```

```
$ vtee replay nosuch.cast
vtee: failed to open nosuch.cast: No such file or directory (os error 2)
$ echo $?
66
$ vtee replay bad.cast
vtee: bad.cast is not an asciicast v2 recording: its first line has no usable width and height
$ echo $?
65
```

A file that cannot be opened exits 66 (NOINPUT); a file that is not an asciicast v2 recording exits 65 (DATAERR).

## Exit codes

`vtee` uses the BSD `sysexits.h` numbers for its own failures. In interactive and headless mode, when the program
runs to completion, `vtee` exits with the program's own exit status (low 8 bits). A program ended by a signal is
the one exception: `vtee` itself always exits 1 for that, not the shell's `128 + signal` convention - the
underlying library only exposes the signal as a name (`architecture.md`), not a number to compute that from. Use
the JSON socket's `exit_status`/`wait_exit` (`protocol.md#exit_status`) to learn the signal by name.

| Code | Name | Seen when |
| --- | --- | --- |
| 0 | OK | Success, or the program exited 0. |
| 1 | | A `--script` run in which a `wait_for` timed out (unless its line says `"optional":true`), or a run with `--expect` whose output differs. |
| 2 | | A run with `--expect` refused to compare, because the golden was recorded under other version numbers. |
| 64 | USAGE | Bad flags or a combination of flags that makes no sense, no program given (`a command is required`), `--cols` or `--rows` outside 1 to 1000, duplicate session name, `attach` with no live session, `attach` without a terminal, `--update` while `CI` is set. |
| 65 | DATAERR | `--script` line that is not a valid command, a `--connect` input line the session could not parse, a script `send_key` with an unknown key, a `replay` file that is not an asciicast v2 recording, or a golden or saved output that is malformed. |
| 66 | NOINPUT | `--script` file, `--snapshot` file, `--expect` golden (when not recording) or `replay` file could not be opened. |
| 68 | NOHOST | `kill` or `attach` for a session that does not exist (or whose name cannot be a session), `--connect` to a socket that is not there. |
| 69 | UNAVAILABLE | With `--expect`, or `--connect` with `--script`: the session cannot be reached or ended the connection. |
| 70 | SOFTWARE | A script command failed while running, or its result could not be serialised. |
| 77 | NOPERM | `--connect` with `--script`: the session refused a command of the script (for example `send` on a read-only socket). |
| 71 | OSERR | The program could not be spawned, the socket could not be bound, or starting a session failed because its name was rejected or its socket path would be too long. |
| 74 | IOERR | A script file could not be read, or a golden could not be written. |

A write that is refused on a connection (for example `send` on a read-only socket) is not an exit code for
`--connect` reading standard input: the error comes back as a result line and `--connect` still exits 0. Only a line
the session could not parse at all (not valid JSON, or an unknown command) makes it exit 65. A script run with
`--connect --script` is different: it stops at the refused command and exits 77. See [error messages](protocol.md#errors).

## Environment

| Variable | Used for |
| --- | --- |
| `TMPDIR` (Linux) | Parent of the session directory `vtee-<uid>`. Falls back to `/tmp`. |
| `LOCALAPPDATA` (Windows) | Parent of the session directory `vtee`. |
| `XDG_CONFIG_HOME` (Linux) | Where the config file is looked for (`vtee/default-config.toml`). Falls back to `~/.config`. |
| `APPDATA` (Windows) | Config file at `%APPDATA%\vtee\config\default-config.toml`. |
| `CI` | When set and non-empty, `--update` is refused (exit 64). |
| `VTEE_ALLOW_UPDATE` | `1` lets `--update` run although `CI` is set. |

## Diagnostic log

Run with a terminal and without `--headless`, `vtee` cannot print log lines to your screen without corrupting
the program's display, so it writes them to `<session dir>/<pid>.log` and prints that path once at start. In
other modes logs go to stderr. Use `-v` for more detail.
