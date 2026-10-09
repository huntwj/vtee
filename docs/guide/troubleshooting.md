# Troubleshooting

## `vtee ls` shows a session as stale

`stale` means the session's files exist but nothing answers on its socket: the holder process died without
cleaning up (it was killed with `SIGKILL`, the machine crashed, or on Windows a console close event ended it).

```
$ vtee ls
stl	pid=5196	stale
```

Clear it with `vtee kill <name>`. That removes the leftover files and signals nothing. You can also start a new
session with the same name: a stale entry does not count as a duplicate, and its files are replaced.

## A killed holder left the program running

If the holder process is killed with `SIGKILL` (`kill -9`), it has no chance to stop the program it wraps. The
program is orphaned: it keeps running with no one reading its output and is no longer reachable through
`vtee`. This is a known gap. A normal `SIGTERM`, `vtee kill`, a closed terminal and a program that exits by
itself all clean up properly.

To find and stop an orphan, look for the program by name (`ps`, or Task Manager on Windows) and end it yourself,
then run `vtee kill <name>` to clear the stale files. On Windows, ConPTY ends the wrapped program when the
holder is killed, so this does not arise.

## My terminal is garbled after a session ended

`vtee` puts your terminal in raw mode while an interactive session or `attach` runs and restores it at exit,
including when it receives `SIGTERM` or `SIGKILL` on Linux (a small helper process does the restoring). If a
terminal is left in raw mode anyway, run `reset` (or `stty sane`) on Linux. On Windows no helper exists; the next
shell prompt usually resets the console, and if not, close the window.

## `vtee attach` printed a flood of escape codes

`attach` expects a terminal. If stdout is redirected to a file or pipe it still runs and writes the redraw as raw
bytes: one `ESC[0m` per screen cell, plus cursor movements. Run it in a real terminal.

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
| `... isn't a valid session name - no "/", no "."/"..", not empty` | `--name` must be a single plain name. |
| `... has unsafe permissions (group/other access) - refusing to use it as the default session dir` | The per-user session directory exists but is readable by others, or belongs to another user, or is not a plain directory. Fix its owner and mode (`chmod 700`) or remove it. |
| `vtee: ignoring unreadable config file: ...` | The config file is not valid TOML or has the wrong type. The setting is treated as off. Fix or delete the file (see [Sessions](sessions.md#--attach-if-exists)). |
| `session name "NAME" is too long: the socket path would be N bytes (limit 107)` | Windows only: shorten the name. |
| `error: the argument '--headless' cannot be used with '--no-listen'` | Two flags that conflict. See the table in the [CLI reference](cli-reference.md#modes-and-what-they-conflict-with). |

## `--listen /path` is treated as the program

`--listen` takes its path only with an equals sign. `--listen /tmp/x.sock -- bash` makes `/tmp/x.sock` the program
and fails to spawn it. Write `--listen=/tmp/x.sock`.

## `wait_for` never matches (or matches too early)

- It matches inside one screen line, so text that wraps does not match.
- It can match the echo of what you just typed. Wait for text that only the output contains.
- It only sees the visible screen. Text that has scrolled off is in `scrollback`.

See [the agents guide](agents.md#choosing-what-to-wait-for).

## `send` seems to do nothing

- The session was not started with `--writable`; the command returned an error line and `--connect` still exited 0.
- Use `\r`, not `\n`, for Enter.
- Under `--writable=exclusive` you must `acquire_write` first.

## Ctrl-C does nothing on Windows

If `VTEE_NEW_PROCESS_GROUP` is set in the environment, the program starts in a new process group, which
disables Ctrl-C for it. Unset it. See [Windows](windows.md#ctrl-c).

## Windows: the program hangs at startup

Older builds hung roughly half the time because ConPTY's startup cursor query went unanswered. If you see this,
check that you are running a current build. See [Windows](windows.md#terminal-queries).

## Extra diagnostics

Run with `-v` (repeat for more detail). In an interactive session the log goes to `<session dir>/<pid>.log`, and
`vtee` prints the path at startup; in other modes it goes to stderr. `--log <path>` records the program's raw
output as an asciicast file you can replay.
