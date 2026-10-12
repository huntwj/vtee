# Sessions

A session is one running `vtee` process (the holder) plus the program it wraps. The holder owns the pseudo-
terminal and the in-memory screen. Everything else, whether `attach`, `ls`, `kill` or a `--connect` script, finds
the holder by the session's name.

## Names

By default a session is named after the program's basename: `vtee -- /usr/bin/vim notes.txt` is session `vim`.
Choose your own with `--name`:

```
vtee --headless --name build --writable -- bash --norc
```

A name must be a single plain file-name component. `/`, `\`, `.`, `..` and the empty string are rejected, so a
name can never point outside the session directory:

```
$ vtee --name ../x -- true
vtee: couldn't check for an existing session named "../x": "../x" isn't a valid session name: no "/", no "."/"..", not empty
$ echo $?
71
```

On Windows a name is also refused if it contains a control character or one of `< > : " | ? *` (a colon would open
an NTFS alternate data stream), starts or ends with a space or a dot, or is a reserved device name (`CON`, `PRN`,
`AUX`, `NUL`, `COM1` to `COM9`, `LPT1` to `LPT9`, with or without an extension). The reason follows the same
pattern:

```
> vtee --headless --name CON -- cmd /c exit
vtee: couldn't check for an existing session named "CON": "CON" isn't a valid session name: is a reserved Windows device name
> vtee --headless --name ab:c -- cmd /c exit
vtee: couldn't check for an existing session named "ab:c": "ab:c" isn't a valid session name: contains a control character or one of < > : " | ? *
```

Both exit 71. A Windows name is also limited in length so that the socket path stays under the 107-byte limit for
Unix-domain sockets, and a very long `%LOCALAPPDATA%` can leave no room at all. Both cases name their real cause;
see [Windows](windows.md#sessions).

## Duplicate names

Two live sessions cannot share a name. This matches `tmux new-session -s`, which also refuses a duplicate:

```
$ vtee --headless --name demo -- bash --norc
vtee: duplicate session: demo
$ echo $?
64
```

A leftover from a crashed holder (a stale session) does not count as live; starting a new session with the same
name replaces its files.

### `--attach-if-exists`

Like `tmux new-session -A`: if the name is taken by a live session, attach to it instead of failing.

```
vtee --attach-if-exists --name build -- bash
```

The first call creates the session; later calls attach to it. This only makes sense for an interactive terminal.
With stdout redirected the attach just streams the redraw and output to stdout.

To make this the default, set it in the config file. `--attach-if-exists` and `--no-attach-if-exists` override the
file in either direction.

```toml
attach_if_exists = true
```

| OS | Config file |
| --- | --- |
| Linux | `$XDG_CONFIG_HOME/vtee/default-config.toml`, or `~/.config/vtee/default-config.toml` when `XDG_CONFIG_HOME` is unset |
| Windows | `%APPDATA%\vtee\config\default-config.toml` |
| macOS | Untested. The path is chosen by the `confy` library; expect `~/Library/Application Support/vtee/default-config.toml`. |

The file is only read when a name collision happens. If it cannot be parsed, `vtee` prints
`vtee: ignoring unreadable config file: ...` and behaves as if the setting were off.

The default is off, and nothing creates the file for you.

## Where the files live

Each session has three files, named after the session:

| File | Purpose |
| --- | --- |
| `<name>.sock` | The JSON command socket, used by `--connect`. |
| `<name>.tty.sock` | The raw terminal socket, used by `attach`. |
| `<name>.pid` | A record of the holder's process id and the sockets it bound, used by `ls`, `kill` and `attach`. |

They live in a private per-user directory:

| OS | Directory |
| --- | --- |
| Linux | `$TMPDIR/vtee-<uid>`, with `/tmp` when `TMPDIR` is unset. Created with mode `0700`. |
| Windows | `%LOCALAPPDATA%\vtee` |

On Linux the directory is created with mode `0700`, and `vtee` refuses to use an existing directory that is not
a real directory, is owned by someone else, or is readable by group or others. The sockets themselves are created
mode `0600`, and every connection is checked for the user on the other end. See [security](security.md) for what
that does and does not protect against.

```
$ ls -la $TMPDIR/vtee-0
drwx------ 2 root root 4096 Oct  9 19:31 .
-rw-r--r-- 1 root root   53 Oct  9 19:31 demo.pid
srw------- 1 root root    0 Oct  9 19:31 demo.sock
srw------- 1 root root    0 Oct  9 19:31 demo.tty.sock
```

`--no-listen` creates none of this, and `--script` never does. `--listen=<path>` moves only the JSON socket to a
path of your choosing; the other two files stay in the session directory. `ls`, `kill` and `attach` still find
such a session, because the `.pid` file is a three-line record: the holder's process id, then the path of the
JSON socket it bound, then the path of the terminal socket. The paths are absolute, so a relative `--listen`
path works from any directory.

```
$ cat $TMPDIR/vtee-0/custom.pid
2215
/tmp/custom.sock
/tmp/vtee-0/custom.tty.sock
```

A `.pid` file that holds only the process id, as written by earlier versions, is read as "the conventional
paths".

Linux limits socket paths to about 108 bytes, so a very long `TMPDIR` can make session creation fail.

## Listing and ending sessions

```
$ vtee ls
demo	pid=2221	live
```

`ls` reports a session `live` only if its socket accepts a connection, not merely because the files exist. On Linux
a `SIGTERM` to the holder (`kill <pid>`) ends the session and removes its files. A `SIGKILL` gives the holder no
chance to, so the files stay behind and show as `stale`, and so does ending the holder from Task Manager on
Windows. For a session started with
`--listen=<path>`, `ls` adds the socket path:

```
$ vtee ls
custom	pid=2215	live	sock=/tmp/custom.sock
demo	pid=2206	live
```

`vtee kill <name>` ends a live session and removes its files, including the sockets its record names; for a
stale one it just removes the files. It checks that the recorded socket answers before signalling the process, so
a stale pidfile whose process id has been reused cannot make `kill` signal an unrelated process. On Linux a
recorded path that is no longer a socket is left alone.

## Attaching and detaching

`vtee attach [name]` shows the current screen and then live output, and sends your typing to the program. With
no name, it picks the only live session. When there are several it picks the one attached to most recently: a
session's record file is stamped when the session starts and again each time a client attaches, and the newest
stamp wins. Until anyone has attached, that is the session started last. Attaching by name counts, so
`vtee attach work` makes `work` the one a later bare `vtee attach` goes to.

**There is no detach key, and Ctrl-C does not leave `attach`.** `attach` puts your terminal in raw mode and
forwards every byte, so Ctrl-C is delivered to the program (as it would be inside tmux). To leave, end the attach
client, which is just another connection: close the terminal window, or stop the `vtee attach` process from
another shell. The session keeps running either way, and the attach client also ends by itself when the session
ends. See [Troubleshooting](troubleshooting.md#how-do-i-leave-an-attach-session).

This is deliberate. A detach key would have to be intercepted from the program's input and could clash with what
the program needs. It also means a session never depends on an attached terminal being there.

## Several clients at once

Any number of attach terminals and any number of JSON clients can be connected to one session. Every attached
terminal sees all output. Their input is merged in arrival order; there is no arbitration between attached
terminals. The JSON socket can arbitrate its own writers with the [exclusive write policies](protocol.md#write-policies).

Limits: 64 simultaneous connections per socket. A connection beyond that is dropped immediately.

An attached terminal that falls behind (it stopped reading, or was simply slow for a while, for example on a
loaded machine) does not slow down the program or the other clients. The session keeps at most about 2 MiB of
unread output for it. When that fills, the backlog is thrown away and replaced by one redraw of the current
screen, so the terminal skips the output in between and carries on from a correct picture. It is never cut off:
a terminal that starts reading again keeps receiving. A scrolling program's lines that went by during
the gap are not shown on that terminal, though the program itself and any other client saw them. A terminal
that never reads costs the session its 2 MiB and nothing more.

## Terminal queries and attach clients

Programs sometimes ask the terminal a question, such as "where is the cursor?" (`ESC[6n`). With a real terminal
attached, the terminal answers. With none attached (headless, or `--script`), `vtee` answers from its own
emulator so that the program does not wait forever. While an attach client is connected, `vtee` stays silent
and lets that terminal answer, so the program never gets two replies.
