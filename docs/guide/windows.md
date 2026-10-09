# Windows

`vtee` runs natively on Windows 10 version 1803 or later, using the ConPTY pseudo-console. Sessions, `attach`,
`ls`, `kill`, the JSON socket and the terminal socket all work. Linux is the primary platform; this page lists
the differences and the gaps.

Build and run it like on Linux (`cargo build --release`, then `vtee.exe`). Examples in the other guides use
Unix shells; substitute a Windows program, for instance `powershell -NoLogo -NoProfile` or `cmd /k`.

## What works

- Spawning a program and getting its output, interactively, headless, and in `--script` mode.
- The program's exit code is returned as `vtee`'s own.
- Full-screen programs (`more`, the Edit editor, Git for Windows' `vim` and `less` were tested).
- Pasting several lines and programs that print a lot of output.
- Resizing the terminal you run `vtee` in resizes the program inside.
- Ctrl-C in an interactive session reaches the program.
- Named sessions, `attach`, `ls`, `kill`, and several clients at once.
- Colours, the window title and the cursor are reported in snapshots.

## Sessions

Sockets are Unix-domain sockets (AF_UNIX), which Windows 10 1803 and later provide, through the `uds_windows`
library. Named pipes were not used because the protocol relies on half-closing a connection.

| Item | Location |
| --- | --- |
| Session directory | `%LOCALAPPDATA%\vtee` (the temp directory if `LOCALAPPDATA` is unset) |
| Session files | `<name>.sock`, `<name>.tty.sock`, `<name>.pid` |
| Config file | `%APPDATA%\vtee\config\default-config.toml` |

The user profile's own permissions are the access control; `vtee` refuses a session directory that is a
junction, symbolic link or other reparse point, or is not a directory.

Unix-domain socket paths are limited to 107 bytes. The longest file is `<name>.tty.sock`, so a long profile path
plus a long name can exceed it; `vtee` then reports an error that names the length.

```
$ vtee ls
winrw	pid=26260	live
```

`vtee kill <name>` checks that the session answers on its socket, then ends the holder with
`TerminateProcess`. ConPTY then ends the wrapped program, and `kill` removes the files. A holder killed some other
way leaves a `stale` entry, as on Linux (`ls` shows it, `kill` clears it). The program did not outlive the holder in
tests.

A console close, Ctrl-Break or shutdown event ends the program and the holder with exit code 143. It leaves the
session files in place, like a `SIGKILL` would; `vtee ls` then shows the session as `stale` and `vtee kill`
clears it.

## Ctrl-C

Ctrl-C in an interactive session is delivered to the program. Two things have to be right for that on Windows.
`vtee` resets the process-wide setting that makes a child ignore Ctrl-C (a child inherits it from any ancestor
started in a new process group), and it does not start the program in a new process group.

A startup crash on some machines, `STATUS_CONTROL_C_EXIT` within milliseconds of spawning
(upstream wezterm issue 7025), was originally avoided by starting the child in a new process group. That
workaround is now off by default because it also disables Ctrl-C for the program, and the crash did not reproduce
without it. If you do see that crash, set the variable to turn the workaround back on:

```
set VTEE_NEW_PROCESS_GROUP=1
vtee.exe -- <program>
```

With it set, Ctrl-C no longer reaches the program in that session. Any value except `0` turns the workaround on
(an empty value counts); leaving the variable unset, or setting it to `0`, leaves it off.

## Resize

Windows has no equivalent of `SIGWINCH`, so `vtee` polls the real console size about five times a second and
resizes the program when it changes. A resize at the moment of startup can be missed until the next change.
`--cols` and `--rows` override the terminal size and are respected until the terminal changes.

## Terminal queries

At startup ConPTY asks the terminal where the cursor is and waits for an answer before showing any output. An
interactive session lets your real terminal answer (`vtee` puts your console in raw mode before starting the
program so the reply is not lost). With no terminal, in `--script` or `--headless` mode, `vtee` answers from its
emulator. Without this, the program appeared to hang about half the time.

## Known gaps

- **No terminal-restore watchdog.** On Linux a helper process restores the terminal mode if the holder is
  killed. On Windows nothing does; a shell normally resets the console at its next prompt.
- **`attach` does not set the console's virtual-terminal flags.** Escape sequences and special keys may not
  behave correctly through `vtee attach` in every console host. This is untested.
- **`kill` and process id reuse.** `kill` checks that the session answers before it terminates the holder, but
  between that check and the termination the holder could exit and its id be reused. The window is small; no
  further guard exists yet.
- **Mouse modes.** A program that turned on mouse reporting did not show `?1000` in `modes` in a test; ConPTY
  seems to handle it itself.
- **Windows must be 1803 or later**, for AF_UNIX sockets.
- **Antivirus** products might interfere with socket files. No problem has been seen.
- **The tests rely on tools that are not guaranteed to be installed**, such as Git for Windows' `vim` and `less`.
  See [testing](../development/testing.md).
- **Windows is not exercised in continuous integration yet.** The workflow has not run on real runners.
