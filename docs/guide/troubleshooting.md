# Troubleshooting

## `vtee ls` shows a session as stale

`stale` means the session's files exist but nothing answers on its socket: the holder process died without
cleaning up (it was killed with `SIGKILL`, the machine crashed, or on Windows a console close event or Task Manager
ended it). On Linux a plain `SIGTERM` to the holder is not one of these: it removes the session's files before the
holder exits.

```
$ vtee ls
stl	pid=5196	stale
```

Clear it with `vtee kill <name>`. That removes the leftover files and signals nothing. You can also start a new
session with the same name: a stale entry does not count as a duplicate, and its files are replaced.

## A killed holder left the program running

If the holder process is killed with `SIGKILL` (`kill -9`), it has no chance to stop the program it wraps. The
program is orphaned: it keeps running with no one reading its output and is no longer reachable through
`vtee`. This is a known gap. A normal `SIGTERM` (the program and the session files are both cleaned up),
`vtee kill`, a closed terminal and a program that exits by itself all clean up properly.

To find and stop an orphan, look for the program by name (`ps`, or Task Manager on Windows) and end it yourself,
then run `vtee kill <name>` to clear the stale files. On Windows, ConPTY ends the wrapped program when the
holder is killed, so this does not arise.

## My terminal is garbled after a session ended

`vtee` puts your terminal in raw mode while an interactive session or `attach` runs and restores it at exit,
including when it receives `SIGTERM` or `SIGKILL` on Linux (a small helper process does the restoring). If a
terminal is left in raw mode anyway, run `reset` (or `stty sane`) on Linux. On Windows no helper exists; the next
shell prompt usually resets the console, and if not, close the window.

## How do I leave an attach session?

There is no detach key, and Ctrl-C does not leave `attach`: it is sent to the program, like every other key. To
leave, close the terminal window, or stop the `vtee attach` process from another shell (`kill <pid>`, or Task
Manager on Windows). The session keeps running, and `vtee ls` still shows it as `live`. If the program itself
exits, the attach client ends by itself. To end the session as well, run `vtee kill <name>`.

## `vtee attach` says it needs a terminal

```
vtee: attach needs a terminal: stdin and stdout must both be terminals (use --connect for scripts)
```

`attach` replays the screen as escape codes, so it refuses with exit status 64 when stdin or stdout is a pipe or a
file. Run it in a real terminal. To read a session from a script, use `vtee --connect <socket>`, which returns
the screen as JSON or, with `--text`, as plain lines.

## A session started with `--listen=<path>` looks stale

A session started with an explicit socket path, as in `vtee --headless --name x --listen=/tmp/x.sock -- ...`,
is found by `vtee ls`, `kill` and `attach`: its `.pid` file records the socket it bound, and `ls` prints
`sock=<path>` for it. If `ls` shows such a session as `stale`, the holder really is gone.

The exception is a holder started by an earlier `vtee`, which wrote only its process id. `ls` looks for that
session's socket at the conventional path, reports it `stale`, and `kill` then removes its files without stopping
the holder. Stop it by ending the `vtee` process yourself, and use `--connect <path>` with the path you gave it
in the meantime.

## Common error messages

| Message | Meaning and fix |
| --- | --- |
| `vtee: a command is required` | You gave no program. Put one after the flags, with `--` if needed. |
| `vtee: duplicate session: NAME` | A live session already has that name. Pick another `--name`, use `--attach-if-exists`, or `vtee kill NAME`. |
| `vtee: no session named "NAME"` | `kill` for a name that has no files. Check `vtee ls`. |
| `vtee: no live sessions to attach to` | Bare `attach` with nothing running. |
| `vtee: failed to attach to "NAME": ...` | The terminal socket did not answer. The session is gone or stale. |
| `vtee: failed to connect to PATH: ...` | `--connect` to a socket that does not exist or has no listener. |
| `vtee: failed to spawn PROGRAM: ...` | The program could not be started: wrong name, not on `PATH`, not executable. |
| `vtee: couldn't check for an existing session named "NAME": "NAME" isn't a valid session name: no "/", no "."/"..", not empty` | `--name` must be a single plain name. Exits 71. On Windows the reason can instead be `contains a control character or one of < > : " \| ? *`, `starts or ends with a space or a dot` or `is a reserved Windows device name`. |
| `vtee: --cols 0 is out of range: it must be between 1 and 1000` | `--cols` or `--rows` outside 1 to 1000. Exits 64. |
| `vtee: failed to open FILE: ...` / `vtee: FILE is not an asciicast v2 recording: ...` | `vtee replay` could not open the file (exit 66) or the file is not a `--log` recording (exit 65). |
| `--- error: bad command: expected value at line 1 column 1 ---` (or `{"cmd":"error","message":"bad command: ..."}`) from `--connect` | The line was not valid JSON, and `--connect` exits 65. Leading byte-order marks are not the cause any more: they are removed. See [PowerShell and the byte-order mark](windows.md#powershell-and-the-byte-order-mark) for the tested cases. |
| `... has unsafe permissions (group/other access) - refusing to use it as the default session dir` | The per-user session directory exists but is readable by others, or belongs to another user, or is not a plain directory. Fix its owner and mode (`chmod 700`) or remove it. |
| `vtee: ignoring unreadable config file: ...` | The config file is not valid TOML or has the wrong type. The setting is treated as off. Fix or delete the file (see [Sessions](sessions.md#--attach-if-exists)). |
| `session name "NAME" is too long: its socket path would be 174 bytes (limit 107); with this session directory a name can be at most 3 bytes` | Windows only: shorten the name (or `%LOCALAPPDATA%`, which sets how much room is left). Exits 71. |
| `the session directory path is 139 bytes, which leaves no room for a socket: sessions need <directory>\<name>.tty.sock to fit in 107 bytes. Shorten %LOCALAPPDATA%` | Windows only: the profile path itself is too long for any session. Starting a session exits 71; `kill` of any name exits 68 with the same text after `no session named "NAME":`. |
| `error: the argument '--headless' cannot be used with '--no-listen'` | Two flags that conflict. See the table in the [CLI reference](cli-reference.md#modes-and-what-they-conflict-with). |

## `--listen /path` is treated as the program

`--listen` takes its path only with an equals sign. `--listen /tmp/x.sock -- bash` makes `/tmp/x.sock` the program
and fails to spawn it. Write `--listen=/tmp/x.sock`.

## `wait_for` never matches (or matches too early)

- It matches inside one screen row, so text that wraps does not match. Add `"join_wrapped":true` to join the rows of a
  soft-wrapped line first.
- It can match the echo of what you just typed. Wait for text that only the output contains.
- It only sees the visible screen. Text that has scrolled off is in `scrollback`.

See [the agents guide](agents.md#choosing-what-to-wait-for).

## A command was typed into the program instead of run by vtee

A session's two sockets are not interchangeable. `<name>.sock` parses NDJSON commands. `<name>.tty.sock`, the raw
socket `attach` uses, has no protocol at all: every byte it receives is forwarded straight to the program's input,
exactly as if a human typed it, and - unlike the JSON socket - it always accepts writes, no matter what
`--writable` policy the session was started with (`serve_tty` is always given `WritePolicy::Open`).

If a client writes a JSON command line to `<name>.tty.sock` by mistake, vtee does not reject it: it types the
literal text, including the trailing newline, into the program as keystrokes. The program then does whatever it
normally does with typed input - a shell runs it as a command, a MUD client sends it to the server over the
connection it already has open - and you see that program's ordinary response to garbage input (echoed back,
appearing inside its existing session), not a vtee error. Nothing about vtee's same-user trust check distinguishes
this from a real human typing, because from that check's point of view, that is exactly what happened.

Get the JSON socket's path from `vtee ls` (it prints `sock=<path>` for a session started with `--listen=<path>`)
and use `vtee --connect`, which always targets `<name>.sock`, or open `<name>.sock` directly if connecting without
it (see [Using the socket without `vtee --connect`](protocol.md#using-the-socket-without-vtee---connect)). Never
write a command to `<name>.tty.sock`.

## `send` seems to do nothing

- The session was not started with `--writable`; the command returned an error line and `--connect` still exited 0.
- Use `\r`, not `\n`, for Enter.
- Under `--writable=exclusive` you must `acquire_write` first.

## Windows: the program hangs at startup

Older builds hung roughly half the time because ConPTY's startup cursor query went unanswered. If you see this,
check that you are running a current build. See [Windows](windows.md#terminal-queries).

## Extra diagnostics

Run with `-v` (repeat for more detail). In an interactive session the log goes to `<session dir>/<pid>.log`, and
`vtee` prints the path at startup; in other modes it goes to stderr. `--log <path>` records the program's raw
output as an asciicast file; `vtee replay <path>` prints the screen it ends with (`--json` for the full snapshot).
