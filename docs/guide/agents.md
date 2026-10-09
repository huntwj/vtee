# Driving a session as an AI agent

This is a practical guide for an agent (or a test harness) that controls a program through `vtee`. The command
details are in the [protocol reference](protocol.md).

## Setup

Start the session with a stable name, headless, with write access:

```
vtee --headless --name work --writable --env PS1='$ ' -- bash --norc &
```

- `--headless`: nothing in your own stdin or stdout is forwarded, so it will not interfere with your tool's
  output.
- `--writable`: required for `send`, `send_key` and `resize`.
- `--env`: make the program's behaviour predictable. A short fixed prompt makes `wait_for` reliable.
- `--cols` and `--rows`: pick a size. The default is 80 by 24 without a terminal.

Then find the socket. It is `<name>.sock` in the session directory (`${TMPDIR:-/tmp}/vtee-<uid>` on Linux,
`%LOCALAPPDATA%\vtee` on Windows). `vtee ls` shows whether the session is live.

```
SOCK="${TMPDIR:-/tmp}/vtee-$(id -u)/work.sock"
```

`--listen=<path>` lets you pick the socket path yourself, but a session started that way is not seen by `vtee
ls` and cannot be ended with `vtee kill` (see [Troubleshooting](troubleshooting.md#a-session-started-with---listenpath-looks-stale)).
Prefer the default path.

Each call to `vtee --connect "$SOCK"` is its own connection. Send several commands at once on one connection by
writing several lines:

```
printf '%s\n' '{"cmd":"send","text":"ls\r"}' '{"cmd":"wait_for","text":"$ ","timeout_ms":3000}' | vtee --connect "$SOCK"
```

## The loop: send, wait, read

Every interaction follows the same three steps.

1. `send` (or `send_key`) the input. These return nothing.
2. `wait_for` text that proves the program has reacted. The reply carries a snapshot taken at the moment of the
   match.
3. Read the snapshot from that reply. Do not send a separate `snapshot` command if the `wait_for` already gave you
   one.

```
{"cmd":"send","text":"echo hello from vtee\r"}
{"cmd":"wait_for","text":"hello from vtee","timeout_ms":5000}
```

Sleeping is the wrong way to wait. `wait_for` wakes the instant the text appears, and when the timeout passes it
tells you so (`"matched":false`) rather than failing, with the screen as it was at that moment. A timeout is
information: look at the snapshot to see what the program is actually showing.

## Choosing what to wait for

`wait_for` is a substring match against each screen line. Three consequences matter:

- **The program's own echo can match your text.** If you wait for text that is part of the command you just typed,
  the match fires immediately on the echo of your input, before the command has run. Wait for something only the
  output contains, such as a marker built from pieces:

  ```
  {"cmd":"send","text":"echo DO''NE\r"}
  {"cmd":"wait_for","text":"DONE","timeout_ms":5000}
  ```

  The typed line contains `DO''NE`, the output contains `DONE`.
- **Text that wraps across two lines does not match.** Keep waited-for text short, or widen the screen with
  `--cols`.
- **Padding.** Every line in a snapshot is padded with spaces to the screen width. Trim before comparing, or match
  with a substring.

A fixed prompt is the usual end-of-command marker. If the prompt is `$ `, waiting for `$ ` works for the first
command, but the old prompt is still on the screen from before, so a command that finishes later could match the
earlier one. Use an output marker for anything where that matters.

## Reading the screen

Use `lines` for text and `cells` when you need colour or attributes (for example, to find the highlighted menu
entry by its `reverse` flag). `cursor` tells you where the program is waiting for input.

`byte_offset` counts all output processed so far. It is how you tell whether anything has happened since an earlier
snapshot: equal offsets mean no new output. To see that a busy program has gone quiet, take snapshots a short
`wait_ms` apart until `byte_offset` stops changing. Prefer a `wait_for` on a marker when you can; quiet is a weak
signal for a program that updates a clock.

`title` holds the window title if the program set one, a cheap way to read a program's own status line.

## Full-screen programs

For editors, pagers and other TUIs:

- `alt_screen` is `true` while the program is using the alternate screen. Use it to know when the program has
  started and when it has quit.
- Navigation keys are `send_key` (`Up`, `PgDn`, `Home`, `F1` and so on). Letters and Enter go through `send`.
  Escape is `{"cmd":"send","text":"\u001b"}`.
- If the screen looks stale or garbled, `resize` to a slightly different size and back. Most programs repaint on a
  size change.
- Many programs query the terminal ("where is the cursor?") when they start. `vtee` answers those from its own
  emulator when no terminal is attached, so they do not hang.

## Scripts versus a live connection

`--script` runs a fixed list of commands, prints the results, kills the program and exits. It is the right tool for
a regression test where every step is known in advance: one process, one exit status. It cannot branch on what it
sees.

An agent that decides each step from the previous screen needs a live session and `--connect`, with one or more
commands per call as in the loop above.

## Text mode and JSON mode

By default each result is one line of JSON. With `--text`, `snapshot` and `wait_for` results print only the screen
lines (trailing spaces removed, with a header giving the byte offset), which is much shorter to put into a model's
context:

```
$ echo '{"cmd":"snapshot"}' | vtee --connect "$SOCK" --text
--- snapshot (byte_offset=2) ---
$
```

Switch to JSON when you need cell attributes, the cursor, or the title.

## Pitfalls

- **Sending a newline instead of carriage return.** Terminals use `\r` for Enter. `\n` often works in a shell and
  fails in a TUI.
- **Writing `\033` in JSON.** JSON has no octal escapes. Use `\u001b`. A raw control character inside a JSON string
  is also an error. See [Errors](protocol.md#errors).
- **Reading right after sending.** The program has not necessarily reacted yet. Always `wait_for`.
- **Forgetting `--writable`.** `send` fails with `write commands aren't allowed on this connection`, and `--connect`
  still exits 0. Check result lines for `"cmd":"error"`.
- **Two writers.** If a human is also attached, both of you can type into the same program. Use
  `--writable=exclusive` and `acquire_write` if you need to be the only writer.
- **Leaving sessions running.** End the session when done: `vtee kill work`. A session ends by itself when the
  program exits.
- **Errors look different in text mode.** They print as `--- error: <message> ---`, and acquire/release replies as
  `--- write_access acquired=true ---`. Check for them.

## Cleaning up

```
vtee kill work
vtee ls
no sessions
```
