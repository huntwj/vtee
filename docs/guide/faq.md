# FAQ and glossary

## Glossary

| Term | Meaning |
| --- | --- |
| **session** | A program running under `vtee`, with its screen kept in memory, reachable by a name. `vtee ls` lists them. |
| **holder** | The `vtee` process that owns the pseudo-terminal and the program, and serves the sockets. A session lasts as long as its holder; the holder exits shortly after the program does. |
| **tap** | The library type (`Tap`) behind a holder: the pty, the terminal emulator and the threads that connect them. The name comes from the way a tap copies a stream without changing it. See [architecture](../architecture.md). |
| **client** | Anything connected to a session. There are two kinds: a *terminal client* (`vtee attach`, which sees and types raw bytes like a terminal) and a *command client* (a program speaking the [JSON protocol](protocol.md), for example through `vtee --connect`). Any number of each can be connected at once. |
| **attach** | Connecting a terminal to a running session so you see its screen live and type into it. Closing the terminal detaches; the session keeps running. |
| **snapshot** | The whole screen at one instant as data: text, cell attributes, cursor, modes, title. Stamped with `byte_offset`. |
| **`byte_offset`** | How many bytes of program output had been processed when a snapshot was taken. Equal offsets mean nothing was drawn in between. |
| **headless** | A session with no human terminal at all (`--headless`). Nothing is forwarded to or from the holder's own stdin and stdout. |
| **script** | A file of JSON commands run in order against a fresh program (`--script`). |
| **stale** | A session record whose holder is gone but whose files were left behind, for example after a `SIGKILL`. `vtee ls` says `stale`; `vtee kill <name>` cleans it up. See [troubleshooting](troubleshooting.md). |
| **pen** | Informal name for exclusive write access: under an exclusive write policy only the connection that holds it (after `acquire_write`) may type. See [write policies](protocol.md#write-policies). |
| **golden** | A stored expected screen that a test compares against. Made and checked with `vtee --script S --expect G -- APP`; see [testing TUIs](testing-tuis.md#golden-files). |
| **ConPTY** | Windows' pseudo-console, which `vtee` uses on Windows. See [Windows](windows.md). |

## Questions

**Is this a replacement for tmux?** No. There are no panes, windows or status bar. It borrows tmux's idea of a
session that outlives its viewers and that you can attach to, and adds a structured view of the screen for programs.

**How do I leave an attached session?** Close the terminal window, or end the `vtee attach` process from another
shell. There is deliberately no detach key: Ctrl-C and every other key go to the program. The session keeps running.

**Why `\r` and not `\n` for Enter?** Terminals send carriage return when you press Enter. A shell usually accepts
`\n` too, but a full-screen program often does not. Send `\r`. See [agents](agents.md#pitfalls).

**Can a wait on the screen go wrong?** Yes. Waiting for text you typed matches the echo of your own input at once.
Wait for something only the output contains. See [agents](agents.md#choosing-what-to-wait-for).

**Who else can see or type into my session?** The sockets live in a per-user directory that other users cannot enter
(`${TMPDIR:-/tmp}/vtee-<uid>` on Linux, `%LOCALAPPDATA%\vtee` on Windows). Any program running as your own user
can connect, read the screen, and type through the terminal socket that `attach` uses. The JSON socket is read-only
unless the session was started with `--writable`. Details are in [Sessions](sessions.md).

**`vtee ls` says `stale`. What now?** The holder is gone but left its files. `vtee kill <name>` removes them.
It does not signal anything when the socket does not answer.

**What happens to the program if `vtee` is killed with `SIGKILL`?** It is orphaned, because a killed process runs
no cleanup. Use `vtee kill` to end a session.

**Does it run on macOS?** It is Unix, so it may, but it is not tested.

**Do I need Docker to run the tests?** Only to run the Linux suite from Windows. See
[CONTRIBUTING](../../CONTRIBUTING.md#the-linux-test-image).

**Where do I report a screen that looks wrong?** Open an issue with the command you ran and a recording made with
`--log run.cast` (check it first with `vtee replay run.cast`); see
[CONTRIBUTING](../../CONTRIBUTING.md#reporting-problems).
