# JSON protocol reference

A session's `<name>.sock` speaks newline-delimited JSON (NDJSON): the client writes one command object per line and
reads result lines back. The same command vocabulary is used by `--script` files and by `vtee --connect`, which
simply relays its stdin to the socket and prints the replies.

- One command per line. Blank lines are ignored.
- The command name is the `cmd` field, in `snake_case`.
- Some commands produce a result line; the rest produce nothing when they succeed. An error produces an error line.
- Replies come back in order, one per command that has one.
- A connection stays open until you close it. A bad command gets an error line and the connection continues.

Strings are ordinary JSON. Control characters must be escaped, and JSON has no octal escape, so write ESC as
`\u001b` (not `\033`) and carriage return as `\r`.

## Commands

| `cmd` | Fields | Reply | Needs write access |
| --- | --- | --- | --- |
| `snapshot` | none | `snapshot` | no |
| `wait_for` | `text` (string), `timeout_ms` (integer) | `wait_for` | no |
| `wait_ms` | `ms` (integer) | none | no |
| `send` | `text` (string) | none | yes |
| `send_key` | `key` (string) | none | yes |
| `resize` | `cols`, `rows` (integers) | none | yes |
| `acquire_write` | none | `write_access` | only valid under an exclusive policy |
| `release_write` | none | `write_access` | only valid under an exclusive policy |

### `snapshot`

```
{"cmd":"snapshot"}
```

Reply: `{"cmd":"snapshot","snapshot":{...}}`. See [the snapshot schema](#snapshot-schema).

### `wait_for`

```
{"cmd":"wait_for","text":"via-socket","timeout_ms":3000}
```

Blocks until some line of the screen contains `text`, or until `timeout_ms` milliseconds have passed. The match is
a plain substring test against each screen line separately, so text that wraps across two lines never matches.
Only the visible screen is searched, not the scrollback. It wakes as soon as new output arrives and does not poll.

Reply: `{"cmd":"wait_for","matched":true,"snapshot":{...}}`. `matched` is `false` when it timed out; the snapshot is
the screen at that moment either way, so a timeout is not an error. The snapshot is taken at the instant of the
match, which makes it safe to use for the next decision.

### `wait_ms`

```
{"cmd":"wait_ms","ms":300}
```

Sleeps on the server for `ms` milliseconds. No reply. Prefer `wait_for`.

### `send`

```
{"cmd":"send","text":"echo hello\r"}
```

Writes `text` to the program's input exactly as given. To press Enter send `\r`, not `\n`. Use JSON escapes for
other control bytes: Tab is `\t`, Escape is `\u001b`, Backspace is `\u007f`, Ctrl-C is `\u0003`. No reply.

### `send_key`

```
{"cmd":"send_key","key":"Up"}
```

Sends a named key. No reply. An unknown name is an error. Arrow keys, Home and End use the escape form the program
currently expects (`ESC [ A` or `ESC O A`), depending on whether the program turned on application-cursor-key mode.

| Key name | Bytes sent |
| --- | --- |
| `Up`, `Down`, `Right`, `Left` | `ESC [ A/B/C/D`, or `ESC O A/B/C/D` in application-cursor mode |
| `Home`, `End` | `ESC [ H`, `ESC [ F`, or `ESC O H`, `ESC O F` in application-cursor mode |
| `PgUp`, `PgDn` | `ESC [ 5 ~`, `ESC [ 6 ~` |
| `F1` to `F4` | `ESC O P`, `ESC O Q`, `ESC O R`, `ESC O S` |
| `F5` to `F12` | `ESC [ 15 ~`, `17`, `18`, `19`, `20`, `21`, `23`, `24 ~` |
| `Ctrl-<char>` | Letters (case-insensitive) give 0x01 to 0x1a. `Ctrl-@` 0x00, `Ctrl-[` 0x1b, `Ctrl-\` 0x1c, `Ctrl-]` 0x1d, `Ctrl-^` 0x1e, `Ctrl-_` 0x1f, `Ctrl-?` 0x7f |
| `Alt-<char>` | `ESC` followed by the character |

`Ctrl-` and `Alt-` take exactly one character. There are no names for Enter, Tab, Escape or Backspace; use `send`
for those. Insert, Delete and `Shift-` combinations do not exist yet either.

### `resize`

```
{"cmd":"resize","cols":100,"rows":30}
```

Changes the terminal size seen by the program and by the emulator together. No reply. Each side must be between 1
and 1000; `0` is raised to `1`, and anything over 1000 is an error.

A resize is also refused whenever a real terminal controls the size (an interactive session with a terminal on
stdin; see [Sessions](sessions.md)), because a remote resize would fight with the terminal's own.

### `acquire_write` and `release_write`

```
{"cmd":"acquire_write"}
```

Reply: `{"cmd":"write_access","acquired":true}`. `acquired` is the state after the request. Asking while someone
else holds access is a normal outcome, not an error: you get `acquired:false`. `release_write` always answers
`acquired:false`. A holder that disconnects releases automatically. Under any policy other than the two exclusive
ones these commands are an error.

## Write policies

The policy is fixed when the session starts with `--writable`. It applies to the JSON socket only.

| Policy | How to start | What it allows |
| --- | --- | --- |
| Read-only | no `--writable` | `snapshot`, `wait_for`, `wait_ms`. `send`, `send_key` and `resize` are errors. |
| Open | `--writable` or `--writable=open` | Everything. Any connection may write at any time, with no arbitration. |
| Exclusive | `--writable=exclusive` | One connection at a time may write, after `acquire_write`. A human typing in the session's own terminal never loses the ability to type and does not revoke the holder. |
| Exclusive-shared | `--writable=exclusive-shared` | As exclusive, but each keystroke from a human at the session's own terminal revokes the current holder. The holder finds out when its next write is refused and may acquire again. |

Under the exclusive policies a write from a connection that does not hold access fails with
`write access not held - send acquire_write first`.

The raw terminal socket used by `attach` is separate and always allows typing.

## Snapshot schema

Everything about the screen at one instant, together with the number of output bytes that produced it. A snapshot
is internally consistent: the state and `byte_offset` always agree.

A real reply from a 12 by 3 screen, with the cell lists shortened:

```
{"cmd":"snapshot","snapshot":{"byte_offset":2,"cols":12,"rows":3,"lines":["$           ","            ","            "],"cells":[[{"ch":"$","fg":"Default","bg":"Default","bold":false,"dim":false,"italic":false,"underline":false,"blink":false,"reverse":false,"strike":false,"hyperlink":null}, ...]],"cursor":{"row":0,"col":2,"hidden":false,"shape":"Block"},"modes":[],"alt_screen":false,"title":null,"scrollback":[]}}
```

| Field | Type | Meaning |
| --- | --- | --- |
| `byte_offset` | integer | Total bytes of program output processed so far. Use it to tell whether the screen can have changed between two snapshots. |
| `cols`, `rows` | integer | Screen size. |
| `lines` | array of string | One string per row, padded with spaces to `cols`. Combining marks are included. Convenient when attributes do not matter. |
| `cells` | array of array of Cell | `rows` by `cols` cells, row-major. |
| `cursor` | object | `row`, `col` (zero-based), `hidden` (boolean), `shape`: `Block`, `Underline`, `Bar`, `HollowBlock` or `Hidden`. |
| `modes` | array of string | Active modes. See below. |
| `alt_screen` | boolean | Whether the alternate screen (used by full-screen programs) is active. |
| `title` | string or null | The window title set by the program, `null` if none. |
| `scrollback` | array of string | Rows that scrolled off the top, oldest first, text only. |

### Cell

| Field | Type | Meaning |
| --- | --- | --- |
| `ch` | string | The character, with any combining marks. The second half of a wide character is the empty string. |
| `fg`, `bg` | Color | See below. |
| `bold`, `dim`, `italic`, `underline`, `reverse`, `strike` | boolean | Text attributes. `underline` is true for any underline style. |
| `blink` | boolean | Always `false`; blink is not tracked. |
| `hyperlink` | string or null | The target of an OSC 8 hyperlink. |

### Color

`"Default"`, `{"Indexed":N}` for the 256-colour palette (0 to 15 are the standard and bright colours), or
`{"Rgb":[r,g,b]}` for truecolour.

```
"fg":"Default"
"fg":{"Indexed":6}
"fg":{"Rgb":[1,2,3]}
```

### Modes

`modes` lists the strings below for the modes that are on, in this order: `?1000` (mouse click reporting), `?1006`
(SGR mouse encoding), `?2004` (bracketed paste), `?1` (application cursor keys), `DECKPAM` (application keypad).
Mouse and paste modes are reported, but there is no command to send mouse or paste input.

On Windows, `?1000`, `?1006` and `?1` never appear in `modes`: ConPTY consumes those sequences and does not
forward them, so the emulator never sees them. `?2004` does appear. See
[Windows](windows.md#terminal-modes-that-conpty-hides) for the evidence.

## Errors

An error is one line: `{"cmd":"error","message":"..."}`. `vtee --connect` still exits 0 when it prints one.
Messages you will see:

| Message | Cause |
| --- | --- |
| `bad command: expected ident at line 1 column 2` | The line is not valid JSON. |
| ``bad command: unknown variant `nope`, expected one of `send`, `send_key`, ... `` | `cmd` is not one of the commands. |
| `bad command: control character (\u0000-\u001F) found while parsing a string ...` | A raw control character inside a JSON string. Escape it. |
| `unknown key "Banana"` | `send_key` with a name that does not exist. |
| `write commands aren't allowed on this connection - start the server with --writable` | `send`, `send_key` or `resize` on a read-only socket. |
| `acquire_write/release_write only mean something under the exclusive write policy` | Used without `--writable=exclusive` or `exclusive-shared`. |
| `write access not held - send acquire_write first` | A write under an exclusive policy without holding access. |
| `terminal size 5000x30 exceeds the 1000x1000 limit` | `resize` too large. |
| `resize is controlled by the real terminal on this session - a remote resize would fight with it` | `resize` on a session whose terminal owns the size. |

## Limits

| Limit | Value |
| --- | --- |
| Length of one command line | 1 MiB. A longer line closes the connection. |
| Terminal size | 1 to 1000 on each side. |
| Simultaneous connections per socket | 64. Extra connections are dropped. |

## Using the socket without `vtee --connect`

Any program that can connect to a Unix-domain socket works: open `<session dir>/<name>.sock`, write lines, read
lines. On Windows 10 and later, AF_UNIX sockets are available to Windows programs too. Close the write side when
you are finished sending (half-close) so the server sees the end of your input.
