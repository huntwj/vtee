# CLI reference

```
vtee [OPTIONS] [--] [PROGRAM]...
vtee attach [NAME]
vtee ls
vtee kill <NAME>
vtee replay [--json] <FILE>
```

The mode is chosen by which flags you give, not by a subcommand: `--script` runs a script, `--connect` talks to an
existing session, anything else spawns `PROGRAM`. The subcommands `attach`, `ls` and `kill` manage existing
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
| `--script <PATH>` | Batch | Conflicts with `--listen`, `--no-listen`, `--writable`, `--no-stdin`, `--dump-on-exit`, `--headless`, `--name`, `--attach-if-exists` and `--no-attach-if-exists`. |
| `--connect <PATH>` | Client | Reads JSON commands from stdin. No `PROGRAM` is needed. |

## Options

| Option | Default | Meaning |
| --- | --- | --- |
| `-v`, `--verbose` | info level | Increase log verbosity (repeatable). |
| `-q`, `--quiet` | | Decrease log verbosity (repeatable). |
| `--text` | off | With `--script` or `--connect`: print only the screen text of each result instead of the full JSON. Ignored otherwise. |
| `--connect <PATH>` | | Connect to a session's JSON socket instead of spawning. |
| `--log <PATH>` | off | Record the program's raw output as an asciicast v2 file. `vtee replay <PATH>` shows its final screen. `-v` controls diagnostic logging, which is separate. |
| `--script <PATH>` | | Run a newline-delimited JSON script, then exit. |
| `--listen[=<PATH>]` | on, at the default path | Choose the JSON socket path. The `=` is required for a path: `--listen /tmp/x` is read as the program `/tmp/x`. Conflicts with `--no-listen`. |
| `--no-listen` | | Create no socket, pid file or session name, so nothing is left in the session directory (useful for a CI run driven only through stdin and stdout). Conflicts with `--listen`, `--writable` and `--headless`. |
| `--writable[=<MODE>]` | read-only | Let clients of the JSON socket write (`send`, `send_key`, `resize`), not just read. Bare `--writable` means `open`. Modes: `open` (anyone, no arbitration), `exclusive` (one writer at a time, who takes access with `acquire_write`), `exclusive-shared` (as `exclusive`, and typing at this session's own terminal takes access back). See [write policies](protocol.md#write-policies). |
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
| `-V`, `--version` | | Print the version (`vtee 0.0.0` at the time of writing). |

`--writable` also needs an equals sign for its value: `--writable=exclusive`.

`--cols` and `--rows` are checked before anything starts:

```
$ vtee --no-listen --cols 0 -- true
vtee: --cols 0 is out of range: it must be between 1 and 1000
$ echo $?
64
```

The `resize` command on the JSON socket uses the same upper limit but raises `0` to `1` instead of refusing it (see
[protocol](protocol.md#resize)).

The raw-terminal socket that `attach` uses always allows typing. `--writable` governs only the JSON socket.

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
{"schema_version":1,"byte_offset":10,"cols":20,"rows":4,"lines":["one                 ","two                 ","                    ","
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
runs to completion, `vtee` exits with the program's own exit status (low 8 bits).

| Code | Name | Seen when |
| --- | --- | --- |
| 0 | OK | Success, or the program exited 0. |
| 64 | USAGE | Bad flags, no program given (`a command is required`), `--cols` or `--rows` outside 1 to 1000, duplicate session name, `attach` with no live session, `attach` without a terminal. |
| 65 | DATAERR | `--script` line that is not a valid command, a `--connect` input line the session could not parse, a script `send_key` with an unknown key, or a `replay` file that is not an asciicast v2 recording. |
| 66 | NOINPUT | `--script` file or `replay` file could not be opened. |
| 68 | NOHOST | `kill` or `attach` for a session that does not exist (or whose name cannot be a session), `--connect` to a socket that is not there. |
| 70 | SOFTWARE | A script command failed while running, or its result could not be serialised. |
| 71 | OSERR | The program could not be spawned, the socket could not be bound, or starting a session failed because its name was rejected or its socket path would be too long. |
| 74 | IOERR | A script file could not be read. |

A write that is refused on a connection (for example `send` on a read-only socket) is not an exit code: the error
comes back as a result line and `--connect` still exits 0. Only a line the session could not parse at all (not
valid JSON, or an unknown command) makes `--connect` exit 65. See [error messages](protocol.md#errors).

## Environment

| Variable | Used for |
| --- | --- |
| `TMPDIR` (Linux) | Parent of the session directory `vtee-<uid>`. Falls back to `/tmp`. |
| `LOCALAPPDATA` (Windows) | Parent of the session directory `vtee`. |
| `XDG_CONFIG_HOME` (Linux) | Where the config file is looked for (`vtee/default-config.toml`). Falls back to `~/.config`. |
| `APPDATA` (Windows) | Config file at `%APPDATA%\vtee\config\default-config.toml`. |

## Diagnostic log

Run with a terminal and without `--headless`, `vtee` cannot print log lines to your screen without corrupting
the program's display, so it writes them to `<session dir>/<pid>.log` and prints that path once at start. In
other modes logs go to stderr. Use `-v` for more detail.
