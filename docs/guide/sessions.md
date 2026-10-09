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
vtee: couldn't check for an existing session named "../x": "../x" isn't a valid session name - no "/", no "."/"..", not empty
$ echo $?
71
```

On Windows a name is also limited in length so that the socket path stays under the 107-byte limit for Unix-domain
sockets (see [Windows](windows.md#sessions)). Other characters Windows forbids in file names are also unsuitable.

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
| `<name>.pid` | The holder's process id, used by `ls` and `kill`. |

They live in a private per-user directory:

| OS | Directory |
| --- | --- |
| Linux | `$TMPDIR/vtee-<uid>`, with `/tmp` when `TMPDIR` is unset. Created with mode `0700`. |
| Windows | `%LOCALAPPDATA%\vtee` |

On Linux the directory is created with mode `0700`, and `vtee` refuses to use an existing directory that is not
a real directory, is owned by someone else, or is readable by group or others. The directory's permissions are the
access control: a process that cannot enter the directory cannot connect.

```
$ ls -la $TMPDIR/vtee-0
drwx------ 2 root root 4096 Oct  9 19:31 .
-rw-r--r-- 1 root root    4 Oct  9 19:31 demo.pid
srwxr-xr-x 1 root root    0 Oct  9 19:31 demo.sock
srwxr-xr-x 1 root root    0 Oct  9 19:31 demo.tty.sock
```

`--no-listen` creates none of this, and `--script` never does. `--listen=<path>` moves only the JSON socket to a
path of your choosing; the other two files stay in the session directory.

Linux limits socket paths to about 108 bytes, so a very long `TMPDIR` can make session creation fail.

## Listing and ending sessions

```
$ vtee ls
demo	pid=2221	live
```

`ls` reports a session `live` only if its socket accepts a connection, not merely because the files exist. After a
holder is killed with `SIGKILL` the files stay behind and show as `stale`.

`vtee kill <name>` ends a live session and removes its files; for a stale one it just removes the files. It
checks that the socket answers before signalling the process, so a stale pidfile whose process id has been reused
cannot make `kill` signal an unrelated process.

## Attaching and detaching

`vtee attach [name]` shows the current screen and then live output, and sends your typing to the program. With
no name, it picks the only live session. When there are several it picks the one whose pidfile was written most
recently, which in practice is the most recently started.

There is no detach key. The attach client is just another connection, so you detach by ending it: close the
terminal window, or stop the `vtee attach` process from another shell. The session keeps running either way.
This is deliberate. A detach key would have to be intercepted from the program's input and could clash with what
the program needs. It also means a session never depends on an attached terminal being there.

`attach` puts your terminal in raw mode and forwards every byte, so Ctrl-C is delivered to the program (as it would
be inside tmux) and does not detach. The attach client also ends by itself when the session ends.

## Several clients at once

Any number of attach terminals and any number of JSON clients can be connected to one session. Every attached
terminal sees all output. Their input is merged in arrival order; there is no arbitration between attached
terminals. The JSON socket can arbitrate its own writers with the [exclusive write policies](protocol.md#write-policies).

Limits: 64 simultaneous connections per socket. A connection beyond that is dropped immediately.

An attached terminal that stops reading is dropped from the broadcast once its buffer fills (about 2 MiB). It does
not slow down the program or the other clients. Reattach to get a fresh view.

## Terminal queries and attach clients

Programs sometimes ask the terminal a question, such as "where is the cursor?" (`ESC[6n`). With a real terminal
attached, the terminal answers. With none attached (headless, or `--script`), `vtee` answers from its own
emulator so that the program does not wait forever. While an attach client is connected, `vtee` stays silent
and lets that terminal answer, so the program never gets two replies.
