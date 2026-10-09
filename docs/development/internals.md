# Platform internals

The parts of `vtee` that depend on the operating system, including every `unsafe` block, and why they are the
way they are. For the overall design read [architecture.md](../architecture.md) first; the dated reasoning and
measurements behind each choice are in [decisions.md](../decisions.md).

## Where `unsafe` is used

| File | What | Why it is needed |
| --- | --- | --- |
| `src/rawmode.rs` | `tcgetattr`, `tcsetattr`, and reading a `termios` back from bytes | Saving and restoring the terminal mode. The read uses `read_unaligned` because a byte array is only 1-byte aligned and `termios` needs more. |
| `src/session.rs` | `libc::getuid()` | To name and check the per-user session directory. |
| `src/session_cmds.rs` | `libc::kill` (Unix); `OpenProcess`, `TerminateProcess` (Windows) | Ending a session's holder process. |
| `src/tap.rs` | `SetConsoleCtrlHandler` (Windows) | Resetting Ctrl-C handling, and handling console close, Ctrl-Break and shutdown. |

Each block is a single system call with no pointers held across calls.

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

There is no Windows equivalent. `RawModeGuard` is a plain raw-mode toggle there. A shell normally resets the
console at its next prompt.

## Known gap: a `SIGKILL`ed holder orphans its program (Unix)

When the holder is killed with `SIGKILL`, `Drop` does not run, so the wrapped program is not stopped. It continues
as an orphan that no `vtee` command can find. Fixing it needs a second watchdog like the terminal one, for a case
that needs someone to `kill -9` the wrong process, so it is deliberately left. On Windows ConPTY ends the program
when the holder dies.

## Windows console events

`Tap::watch_sigterm` has a Windows form: a console control handler registered with `SetConsoleCtrlHandler` for
Ctrl-Break, console close and shutdown events. It kills the child and exits with code 143. Because it exits without
running destructors, it leaves the session files behind (as after `SIGKILL`); `ls` shows them stale and `kill`
clears them. Plain Ctrl-C is left to the program, not handled by `vtee`.

### Ctrl-C and process groups

A child normally gets Ctrl-C from the console. Two settings on Windows can stop that:

1. The process-wide "ignore Ctrl-C" attribute, which a child inherits from any ancestor that was started in a new
   process group. `Tap::spawn` calls `SetConsoleCtrlHandler(NULL, FALSE)` to clear it for `vtee` and its future
   children.
2. `CREATE_NEW_PROCESS_GROUP` on the child itself, which disables Ctrl-C for that group. The vendored
   `portable-pty` used to set it. It now sets it only when `VTEE_NEW_PROCESS_GROUP` is set to a value other than
   `0`.

The flag was originally added to avoid a startup crash (`STATUS_CONTROL_C_EXIT`, upstream wezterm issue 7025). The
crash did not reproduce after the startup-query fix below, so the flag is opt-in. See the 2026-10-09 entries in
[decisions.md](../decisions.md).

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

## Vendored `portable-pty`

`vendor/portable-pty/` is `portable-pty` 0.9.0 with one change, in `src/win/psuedocon.rs`: the opt-in
`CREATE_NEW_PROCESS_GROUP` described above. It is wired in through `[patch.crates-io]` in `Cargo.toml`. Its
README (`vendor/portable-pty/README-vtee-patch.md`) states the removal plan: delete the directory and the patch
entry when upstream fixes the underlying issue. The files in `vendor/` keep upstream's line endings, so the
repository's LF normalisation does not apply to them.

## Windows sessions

- **Transport.** AF_UNIX sockets (Windows 10 1803 and later) through `uds_windows`, wrapped by `src/net.rs` so the
  server and client code is shared with Unix. Named pipes were rejected because the protocol needs half-close
  (`shutdown(Write)`), which pipes lack.
- **Session directory.** `%LOCALAPPDATA%\vtee`. A reparse point or non-directory is refused. Names are capped so
  the socket path stays under the 107-byte AF_UNIX limit.
- **`kill`.** After the liveness check, `OpenProcess` and `TerminateProcess`. There is no process-id-reuse guard
  beyond the liveness check.
- **Handle inheritance.** Not an issue: the child is spawned before any socket is created, and the vendored ConPTY
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
never see a chunk twice or miss one. Each attached terminal has a bounded queue (256 chunks of up to 8 KiB) drained
by its own writer thread; the output loop only does a non-blocking `try_send` under the lock. A client that stops
reading fills its queue and is dropped from the broadcast instead of blocking everyone. After each chunk the output
loop yields, because the standard mutex is not fair and a flooding program could otherwise starve `snapshot()`.
