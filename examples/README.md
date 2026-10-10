# Examples

Small, complete programs that drive `vtee`. Each one was run for real; the output shown is what it printed.
They assume `vtee` is on your `PATH` (see the [README](../README.md#install)) and use `bash` (Linux) or `cmd`
(Windows) as the program under test, so they run anywhere without installing anything else.

| File | Platform | What it shows |
| --- | --- | --- |
| [`client.py`](client.py) | Linux, macOS | A 30-line Python client, standard library only, that talks to the JSON socket |
| [`drive.sh`](drive.sh) | Linux, macOS | Start a session, type a command, read the screen, end the session |
| [`drive.ps1`](drive.ps1) | Windows | The same, in PowerShell, against `cmd` |
| [`arithmetic.ndjson`](arithmetic.ndjson), [`script-test.sh`](script-test.sh) | Linux, macOS | A `--script` test that passes or fails, with an exit-code check |

The JSON commands and replies are described in the [protocol reference](../docs/guide/protocol.md). For a recipe
that makes a screen repeatable, read [Testing a TUI with vtee](../docs/guide/testing-tuis.md).

## A Python client

Start a session with write access, then run the client with the socket, the text to type, and the text to wait for:

```
$ vtee --headless --name demo --writable --env 'PS1=$ ' -- bash --norc &
$ python3 examples/client.py "${TMPDIR:-/tmp}/vtee-$(id -u)/demo.sock" 'echo $((40+2))' 42
matched: True
$ echo $((40+2))
42
$
```

The client sends the text followed by Enter, waits up to five seconds, and prints the non-blank screen rows. Waiting
for `42` and not for the text it typed is deliberate: `$((40+2))` is typed and `42` is output, so the wait cannot
match the echo.

Against a session started without `--writable`, `send` is refused and the client reports it:

```
vtee error: write commands aren't allowed on this connection - start the server with --writable
```

The client does not work on Windows. Python there (3.13 was checked) does not provide `socket.AF_UNIX`, even though
the operating system supports it. Use `vtee --connect` from any language that can run a process, as
`drive.ps1` does.

End the session with `vtee kill demo`.

## Shell and PowerShell scripts

```
$ sh examples/drive.sh
[2026-10-10T05:24:45Z INFO  vtee] listening for observers (Open) at /tmp/d2run/vtee-0/drive-demo.sock
--- wait_for matched=true (byte_offset=24) ---
$ echo $((40+2))
42
$
```

The `INFO` line is `vtee`'s own log on stderr. On Windows (the working-directory path is shortened; the
rest is the real output):

```
PS> .\examples\drive.ps1
--- wait_for matched=true (byte_offset=285) ---
Microsoft Windows [Version 10.0.26300.9457]
(c) Microsoft Corporation. All rights reserved.

C:\Users\you\vtee>echo DO^NE
DONE

C:\Users\you\vtee>
```

Two Windows details are in `drive.ps1`. The marker is `DO^NE`: in `cmd`, `^` escapes the `N`, so the typed line
contains `DO^NE` and the output contains `DONE`. The POSIX trick `DO''NE` does not work there, because `cmd` prints
the quotes. And the commands go in through an ASCII file, so Windows PowerShell 5.1 does not put a byte-order mark in
front of the first one; see [PowerShell and the byte-order
mark](../docs/guide/windows.md#powershell-and-the-byte-order-mark) for when piping is safe.

## A script test that can fail

`arithmetic.ndjson` types an expression, waits for its result, tells the shell to exit with code 7, and asks for the
exit status. `script-test.sh` runs it and checks the output, because a script exits 0 even when a wait timed out:

```
$ sh examples/script-test.sh
PASS
```

Change the wait in `arithmetic.ndjson` from `"42"` to `"43"` and it fails, the way a regression would:

```
$ sh examples/script-test.sh
FAIL: a wait_for timed out
$ echo $?
1
```
