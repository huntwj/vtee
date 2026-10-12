# Platform internals

The parts of `vtee` that depend on the operating system, including every `unsafe` block, and why they are the
way they are. For the overall design read [architecture.md](../architecture.md) first; the dated reasoning and
measurements behind each choice are in [decisions.md](../decisions.md).

## Where `unsafe` is used

| File | What | Why it is needed |
| --- | --- | --- |
| `src/rawmode.rs` | `tcgetattr`, `tcsetattr`, and reading a `termios` back from bytes (Unix); `GetConsoleMode`, `SetConsoleMode` (Windows) | Saving and restoring the terminal/console mode. The Unix read uses `read_unaligned` because a byte array is only 1-byte aligned and `termios` needs more. |
| `src/session.rs` | `libc::getuid()` | To name and check the per-user session directory. |
| `src/session_cmds.rs` | `libc::kill` (Unix); `OpenProcess`, `TerminateProcess`, `GetProcessTimes` (Windows) | Ending a session's holder process, and checking it's not a reused PID first. |
| `src/tap.rs` | `SetConsoleCtrlHandler` (Windows) | Resetting Ctrl-C handling, and handling console close, Ctrl-Break and shutdown. |
| `src/net.rs` | `libc::umask` (Unix); `libc::geteuid`, `libc::getpeereid` (Unix); `WSAIoctl` with `SIO_AF_UNIX_GETPEERPID`, `OpenProcess`, `GetCurrentProcess`, `OpenProcessToken`, `GetTokenInformation`, `GetLengthSid`, `CloseHandle` (Windows) | Binding a socket no one else can open, and checking the uid/SID of whoever connected. |

Most blocks are a single system call with no pointers held across calls. The exception is
`net.rs::windows_peer::token_user_sid`, which reads a `TOKEN_USER` into a byte buffer and then holds a raw pointer
into that same buffer (the SID `GetTokenInformation` placed inside it) across `GetLengthSid` and the final
`slice::from_raw_parts` - safe only because the buffer outlives both calls, but worth knowing about before moving
code around it.

## Terminal restore (Unix)

While an interactive session or `attach` runs, the terminal is in raw mode. If the process exits normally or
panics, `RawModeGuard` restores it when dropped. A `SIGKILL` runs no code at all, so a second process does it.

- On entering raw mode, `vtee` records the original `termios` and starts itself again as `vtee
  --internal-tty-guard` (a hidden flag handled before argument parsing, so it never shows in `--help`). The helper's
  stdin is one end of a socket pair; the main process keeps the other end.
- The main process sends the saved `termios` over the socket, then the helper blocks reading it.
- When the main process dies for any reason, the operating system closes its end of the socket. The helper sees
  end-of-file, restores the terminal and exits.
- `crossterm::terminal::disable_raw_mode` cannot be used in the helper because it relies on state held in the
  parent process, which is why the `termios` is captured and sent explicitly.
- The helper opens `/dev/tty` before it waits: reopening it after the session leader has gone fails with `ENXIO`.
- It also registers a `SIGHUP` handler (it only needs to stay registered) so the kernel's hangup of the process
  group after the leader exits does not kill it before it can restore the terminal.

`SIGTERM` is handled in the ordinary way (`Tap::watch_sigterm`, through `signal-hook`): the handler restores the
terminal, kills the child, and exits. This does not need the helper.

There is no Windows equivalent. `RawModeGuard` there also turns on `ENABLE_VIRTUAL_TERMINAL_INPUT`/`_PROCESSING`
on top of raw mode, restoring the original console modes on drop; nothing restores them if the holder is
force-killed. A shell normally resets the console at its next prompt.

## Known gap: a `SIGKILL`ed holder orphans its program (Unix)

When the holder is killed with `SIGKILL`, `Drop` does not run, so the wrapped program is not stopped. It continues
as an orphan that no `vtee` command can find. Fixing it needs a second watchdog like the terminal one, for a case
that needs someone to `kill -9` the wrong process, so it is deliberately left. On Windows ConPTY ends the program
when the holder dies.

## Windows console events

`Tap::watch_sigterm_removing` has a Windows form: a console control handler registered with `SetConsoleCtrlHandler`
for Ctrl-Break, console close and shutdown events. It removes the session's files itself first - `exit`-ing
without running destructors means `SessionFiles`'s own cleanup never gets the chance - then kills the child and
exits with code 143, the same shape as the Unix `SIGTERM`/`SIGHUP` handler. Plain Ctrl-C is left to the program,
not handled by `vtee`.

### Ctrl-C and process groups

A child normally gets Ctrl-C from the console. On Windows the process-wide "ignore Ctrl-C" attribute can stop
that: a child inherits it from any ancestor that was started in a new process group. `Tap::spawn` calls
`SetConsoleCtrlHandler(NULL, FALSE)` to clear it for `vtee` and its future children. `portable-pty` does not
create the child in a new process group.

An earlier vendored copy of `portable-pty` did, and the startup crash it was meant to avoid
(`STATUS_CONTROL_C_EXIT`, upstream wezterm issue 7025) turned out to come from the unanswered startup query below.
See the 2026-10-09 and 2026-10-10 entries in [decisions.md](../decisions.md).

## ConPTY startup query

`portable-pty` creates every pseudo-console with `PSEUDOCONSOLE_INHERIT_CURSOR`. ConPTY then writes a cursor
position request (`ESC[6n`) at session start and holds back *all* output, including the real program's, until a
reply (`ESC[row;colR`) arrives on its input. Two situations left that reply unsent:

- **Interactive.** Your real console answers, but `vtee` started the stdin-forwarding thread before it put the
  console in raw mode. A Windows console read that begins in cooked (line-input) mode stays cooked, and the escape
  character clears the line buffer, so the reply vanished. Raw mode is now enabled before the spawn.
- **`--script` and `--headless`.** Nobody was there to answer. `alacritty_terminal` already produces the right
  answer for each query it understands as an `Event::PtyWrite`; `ObserverListener` used to ignore it. When no human
  terminal can answer, it queues those replies and `forward_output` writes them back to the program after each
  chunk is parsed, outside the state lock. While an attach client is connected the replies are not sent, because
  that terminal sees the same bytes and answers itself; a second reply would arrive as stray keystrokes.

The rule for "no human terminal": the session was not started with both input and output forwarding on, or stdin or
stdout is not a terminal. This also helps on Linux, for programs that ask the terminal something at startup.

## Windows sessions

- **Transport.** AF_UNIX sockets (Windows 10 1803 and later) through `uds_windows`, wrapped by `src/net.rs` so the
  server and client code is shared with Unix. Named pipes were rejected because the protocol needs half-close
  (`shutdown(Write)`), which pipes lack.
- **Session directory.** `%LOCALAPPDATA%\vtee`. A reparse point or non-directory is refused. Names are capped so
  the socket path stays under the 107-byte AF_UNIX limit.
- **`kill`.** After the liveness check, `OpenProcess` and `TerminateProcess`. A process-id-reuse guard compares the
  target's creation time (`GetProcessTimes`) against the pidfile's mtime and leaves alone anything created after
  the pidfile was last written.
- **Handle inheritance.** Not an issue: the child is spawned before any socket is created, and `portable-pty`'s ConPTY
  spawn passes `bInheritHandles = FALSE`. A test pins this (`tests/windows_sessions.rs`).
- **Resize.** No `SIGWINCH` on Windows, so `Tap::watch_resize` polls the console size every 200 ms.

## Session directory (Unix)

`$TMPDIR/vtee-<uid>` is created with mode `0700` (set in the `mkdir` call itself so there is no window with a
looser mode). If it already exists it is verified, not trusted: it must be a real directory (a symlink is rejected
because the check does not follow links), owned by the current user, with no group or other permission bits. If
two processes race to create it, the loser re-verifies rather than failing. This is the same access model tmux
uses: the directory, not the socket file's mode, is the access control. A session name is a single path component,
validated in the one function that turns a name into paths, so every caller is covered.

## Broadcast, redraw and slow clients

All shared state (the emulator, the byte count, and the list of attached terminals) sits behind one lock, so
"snapshot the screen and start receiving future output" is one atomic step for a newly attached client; it can
never see a chunk twice or miss one. Each attached terminal has its own `SinkQueue`, drained by its own writer
thread, bounded in bytes rather than in chunk count (`SINK_QUEUE_BYTES`, 2 MiB). A client that stops reading fills
its queue; instead of being dropped from the broadcast, its entire backlog is replaced with one redraw of the
current screen, so it resynchronises instead of going stale or blocking everyone. After each chunk the output loop
yields, because the standard mutex is not fair and a flooding program could otherwise starve `snapshot()`.
