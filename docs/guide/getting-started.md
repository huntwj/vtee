# Getting started

`vtee` has four ways to be used. They differ in who is at the keyboard.

| You want to... | Use | Section |
| --- | --- | --- |
| Run a program yourself, but let tools watch it | `vtee -- <program>` | [Interactive](#interactive) |
| Let an agent drive a program with no human terminal | `vtee --headless --writable -- <program>` | [Headless](#headless-for-agents) |
| Run a fixed list of commands against a program, for a test | `vtee --script file.ndjson -- <program>` | [Script](#script-mode) |
| Look at or type into a session someone else started | `vtee attach [name]` or `vtee --connect <socket>` | [Attach and connect](#attach-and-connect) |

Build the binary first: `cargo build --release` (see the [README](../../README.md)). The examples below assume
`vtee` is on your `PATH`. Output shown was produced by running the commands; screens are trimmed.

## Interactive

```
vtee -- bash
```

Your terminal behaves as it normally would. `vtee` sets your terminal to raw mode, forwards the program's
output unchanged, forwards your keystrokes to it, and tracks the screen in the background. It also opens a
read-only socket (and a terminal socket) so other processes can see the same session. When the program exits,
`vtee` exits with the program's exit code.

The session name defaults to the program's basename (`bash` here). Use `--name` to choose another, and
`--no-listen` if you do not want any sockets created at all. See [Sessions](sessions.md).

## Headless (for agents)

```
vtee --headless --name demo --writable --env PS1='$ ' -- bash --norc
```

`--headless` means no human terminal is involved: `vtee` forwards nothing to or from its own stdin and stdout
and does not touch the terminal mode. The program runs inside the session and `vtee` keeps running until the
program exits. `--writable` allows write commands (`send`, `send_key`, `resize`) on the JSON socket; without it
the socket is read-only. `--env PS1='$ '` gives the shell a short fixed prompt; without it `bash` prints its
version in the prompt (`bash-5.2$`), which makes the screen longer and the byte offsets different from the ones
shown here.

`vtee` stays in the foreground, so background it or give it its own terminal tab:

```
vtee --headless --name demo --writable --env PS1='$ ' -- bash --norc &
```

Then use the session from anywhere (the 23 blank screen lines after the prompt are left out):

```
$ vtee ls
demo	pid=23	live

$ echo '{"cmd":"snapshot"}' | vtee --connect "${TMPDIR:-/tmp}/vtee-$(id -u)/demo.sock" --text
--- snapshot (byte_offset=2) ---
$
```

`--connect` reads commands from its stdin, one JSON object per line, and prints one result line for each command
that has a result. `--text` prints just the screen lines instead of the whole JSON. The commands are listed in the
[protocol reference](protocol.md); an agent should read [the agents guide](agents.md).

## Script mode

A script is a file with one JSON command per line. `vtee` starts the program, runs the commands in order,
prints a result for each command that has one (`snapshot`, `wait_for`, `exit_status`, `wait_exit`), kills the
program, and exits. `send`, `send_key`, `resize` and `wait_ms` print nothing.

`hello.ndjson`:

```
{"cmd":"wait_for","text":"$ ","timeout_ms":5000}
{"cmd":"send","text":"echo hello from vtee\r"}
{"cmd":"wait_for","text":"hello from vtee","timeout_ms":5000}
{"cmd":"snapshot"}
```

With `--text` each result is a header line and the screen rows (trailing spaces removed; the blank rows below the
text are left out here):

```
$ vtee --script hello.ndjson --text --env PS1='$ ' -- bash --norc
--- wait_for matched=true (byte_offset=2) ---
$

--- wait_for matched=true (byte_offset=47) ---
$ echo hello from vtee
hello from vtee
$

--- snapshot (byte_offset=47) ---
$ echo hello from vtee
hello from vtee
$
```

Be careful what you wait for: `hello from vtee` is in both the typed line and the output, so that `wait_for` could
have matched the echo before the command ran. Here the output had arrived by the time the screen was read, but you
cannot count on that. See [choosing what to wait for](agents.md#choosing-what-to-wait-for).

Without `--text` each result is one JSON line. The last line is the final snapshot; its start was:

```
{"cmd":"snapshot","snapshot":{"schema_version":1,"semantics_version":1,"byte_offset":47,"cols":80,"rows":24,"lines":["$ echo hello from vtee                                                        ","hello from vtee                                                               ", ...
```

Every line in `lines` is padded with spaces to the screen width.

### Asking how the program ended

`exit_status` and `wait_exit` report the program's exit code or signal. With this script (`exit.ndjson`):

```
{"cmd":"wait_for","text":"$ ","timeout_ms":5000}
{"cmd":"exit_status"}
{"cmd":"send","text":"exit 7\r"}
{"cmd":"wait_exit","timeout_ms":5000}
{"cmd":"exit_status"}
```

the JSON output (without the first `wait_for` result) and the text output (`--text`) are:

```
{"cmd":"exit_status","running":true}
{"cmd":"wait_exit","running":false,"code":7,"timed_out":false}
{"cmd":"exit_status","running":false,"code":7}
```

```
--- exit_status running ---
--- wait_exit exited code=7 timed_out=false ---
--- exit_status exited code=7 ---
```

A program ended by a signal reads `exited signal=Terminated`, and a `wait_exit` that ran out of time reads
`running timed_out=true`.

### Scripts do not fail on their own

The exit status of `vtee --script` is 0 when every command ran. It stays 0 when a `wait_for` timed out (the result
says `matched=false`, and in JSON `"matched":false`), and it stays 0 when the program exited with a non-zero code,
as in the example above. A script has no assertions. To make a test fail, read the output: search it for
`"matched":false` and check the `code` in the `wait_exit` reply. [`examples/script-test.sh`](../../examples/script-test.sh)
does both.

A command that cannot be parsed ends the run with status 65, and a script file that cannot be opened with 66. See
the [exit codes](cli-reference.md#exit-codes).

`--script` cannot be combined with the interactive flags (`--headless`, `--listen`, `--name` and so on).

## Attach and connect

There are two ways into a running session, and they do different jobs:

- `vtee attach [name]` gives you a real terminal view. You see the current screen immediately, then live output,
  and your typing goes to the program. This is for humans. With no name it picks the only live session, or the
  one attached to most recently.
- `vtee --connect <socket>` speaks the JSON command protocol. This is for scripts and agents.

Both work at the same time, and any number of each can be connected. Closing an attached terminal detaches
it; the session keeps running. (Ctrl-C goes to the program, not to `attach`.) Details in [Sessions](sessions.md).

## Next

- [CLI reference](cli-reference.md): every flag and exit code.
- [Protocol](protocol.md): every command and the snapshot format.
- [Agents](agents.md): the usual send, wait, read loop and its pitfalls.
- [Troubleshooting](troubleshooting.md).
