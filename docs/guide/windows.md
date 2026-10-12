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
| Session directory | `%LOCALAPPDATA%\vtee` (an error if `LOCALAPPDATA` is unset or not absolute - there is no temp-dir fallback, since that directory can be shared) |
| Session files | `<name>.sock`, `<name>.tty.sock`, `<name>.pid` |
| Config file | `%APPDATA%\vtee\config\default-config.toml` |

The user profile's own permissions are the main access control; `vtee` refuses a session directory that is a
junction, symbolic link or other reparse point, or is not a directory. It also checks the user behind every
connection to a socket: only your account (or SYSTEM) is let in. See [security](security.md) for what that does and
does not protect.

Unix-domain socket paths are limited to 107 bytes. The longest file is `<name>.tty.sock`, so a long profile path
plus a long name can exceed it. `vtee` reports which of the two is the problem. With a name that is too long for
the room there is (here `%LOCALAPPDATA%` is 87 characters and the name is 70):

```
> vtee --headless --name nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn -- cmd /c exit
vtee: couldn't check for an existing session named "nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn": session name "nnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnnn" is too long: its socket path would be 174 bytes (limit 107); with this session directory a name can be at most 3 bytes
```

and with a `%LOCALAPPDATA%` so long that no name fits (here 132 characters):

```
> vtee --headless --name demo -- cmd /c exit
vtee: couldn't check for an existing session named "demo": the session directory path is 139 bytes, which leaves no room for a socket: sessions need <directory>\<name>.tty.sock to fit in 107 bytes. Shorten %LOCALAPPDATA%
> vtee kill demo
vtee: no session named "demo": the session directory path is 139 bytes, which leaves no room for a socket: sessions need <directory>\<name>.tty.sock to fit in 107 bytes. Shorten %LOCALAPPDATA%
```

Starting a session exits 71 in both cases. `kill` exits 68, the same as for a session that does not exist, because
no session with that name can exist. `vtee ls` still works and prints `no sessions`. The fix is a shorter
`%LOCALAPPDATA%`, set for the `vtee` process, or a shorter name.

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

A startup crash on some machines, `STATUS_CONTROL_C_EXIT` within milliseconds of spawning (upstream wezterm issue
7025), once led to starting the child in a new process group, which also disables Ctrl-C. The crash came from the
unanswered startup query (see [Terminal queries](#terminal-queries)), not from the process group, so `vtee` no
longer does that and there is no setting for it.

## Resize

Windows has no equivalent of `SIGWINCH`, so `vtee` polls the real console size about five times a second and
resizes the program when it changes. A resize at the moment of startup can be missed until the next change.
`--cols` and `--rows` override the terminal size and are respected until the terminal changes.

## Terminal queries

At startup ConPTY asks the terminal where the cursor is and waits for an answer before showing any output. An
interactive session lets your real terminal answer (`vtee` puts your console in raw mode before starting the
program so the reply is not lost). With no terminal, in `--script` or `--headless` mode, `vtee` answers from its
emulator. Without this, the program appeared to hang about half the time.

## Terminal modes that ConPTY hides

`vtee` can only report what ConPTY passes on. A program that turns on mouse reporting (`ESC[?1000h`, `?1006h`)
or application cursor keys (`ESC[?1h`) does not show them in the snapshot's `modes` on Windows, because ConPTY
consumes those sequences and never forwards them. The cause is ConPTY, not the emulator: the same bytes from a
Linux program produce `["?1","?1000","?1006","?2004"]`, while under ConPTY on Windows 11 (build 26300) only
`?2004` appeared. What does get through includes the alternate screen (`?1049`), cursor visibility (`?25`) and
bracketed paste (`?2004`). Other private modes were not tested.

The raw output log (`--log`) shows exactly what arrives, which is how this was confirmed: a program that wrote
`?1000h`, `?1006h`, `?2004h`, `?1h` and `?25l` was recorded as `?2004h` and `?25l` only. Creating the
pseudoconsole with `PSEUDOCONSOLE_PASSTHROUGH_MODE` made no difference on this machine. Sideloading a newer
`conpty.dll`/`OpenConsole.exe` was not tried. The practical effect: do not use `modes` to detect mouse or
application-cursor state on Windows. `send_key` picks the arrow-key form from the emulator's application-cursor
state, which therefore always reads as off here; whether a program that insists on the application form still
accepts it was not tested.

## Console flags

While `vtee attach` or an interactive run owns your console, it turns on two virtual-terminal flags and puts
the original modes back when it exits, including when the console is closed or Ctrl-Break is pressed:

- `ENABLE_VIRTUAL_TERMINAL_INPUT` on input, so arrow keys, Home, End and function keys reach the program as the
  escape sequences it expects (`ESC [ A` for Up). Without it a console in raw mode delivers no bytes for those
  keys at all; the test `the_guard_enables_vt_flags_and_restores_them` measures exactly that.
- `ENABLE_VIRTUAL_TERMINAL_PROCESSING` on output, so a redraw's escape sequences are interpreted instead of
  printed. Windows Terminal and a ConPTY console already have this on. A legacy `conhost` window may not, which
  is why it is set. That case cannot be reproduced by the test harness, so it is untested here.

Nothing is changed if the handle is not a console, for example when output is redirected to a file.

## PowerShell and the byte-order mark

A UTF-8 byte-order mark (BOM) in front of a JSON command is not JSON. Windows PowerShell 5.1 writes one at the
start of piped text and of files saved as UTF-8, and writes **two** when `$OutputEncoding` is a UTF-8 encoding
that has a BOM (measured: the bytes arrive as `ef bb bf ef bb bf 7b 22 63 ...`). `vtee --connect` and
`--script` remove every leading BOM from each input line, and the session socket ignores leading BOMs on a
command line too, so both cases work without any workaround:

```
PS> '{"cmd":"exit_status"}' | vtee --connect $sock --text
--- exit_status running ---
PS> $OutputEncoding = New-Object System.Text.UTF8Encoding $true
PS> '{"cmd":"exit_status"}' | vtee --connect $sock --text
--- exit_status running ---
```

A line the session cannot parse at all (here, `not json`) is an error reply, and `--connect` now exits 65
(`DATAERR`) after printing it, so a garbled script no longer looks like success:

```
PS> 'not json' | vtee --connect $sock; $LASTEXITCODE
{"cmd":"error","message":"bad command: expected ident at line 1 column 2"}
65
```

Other error replies, such as a write refused because the session is read-only, still exit 0; check reply lines for
`"cmd":"error"`.

What was tested, all on Windows PowerShell 5.1 (5.1.26100): piping with `$OutputEncoding` set to UTF-8 without and
with a BOM, a garbled line, and a `--script` file written with `Set-Content -Encoding UTF8`. Files saved as
UTF-16 and PowerShell 7 were not tested.

## Known gaps

- **No terminal-restore watchdog.** On Linux a helper process restores the terminal mode if the holder is
  killed. On Windows nothing does; a shell normally resets the console at its next prompt.
- **`kill` and process id reuse.** `kill` checks that the session answers before it terminates the holder, and
  refuses to terminate a process that was created after the session record was last written, since that process
  cannot be the holder. A reused process id is therefore left alone; the check relies on file and process times.
- **Mouse modes are not visible in snapshots.** See the section above.
- **Windows must be 1803 or later**, for AF_UNIX sockets.
- **Antivirus** products might interfere with socket files. No problem has been seen.
- **The tests rely on tools that are not guaranteed to be installed**, such as Git for Windows' `vim` and `less`.
  See [testing](../development/testing.md).
- **Continuous integration runs on `windows-latest`,** including the pty tests, and passed on its first run. It
  covers the headless pty harness, not a human at a real console window.
