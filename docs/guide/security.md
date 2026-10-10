# Security

`vtee` follows tmux's trust model: every program running as you is trusted, and everyone else is kept out. This
page says exactly what that means, so you can decide whether it fits how you use a session.

## What a session exposes

Unless you pass `--no-listen` (or use `--script`, which never listens), every session opens two sockets:

| Socket | What a connection can do |
| --- | --- |
| `<name>.sock` (JSON) | Read the screen. Type and resize only if the session was started with `--writable`. |
| `<name>.tty.sock` (`attach`) | Read the screen live and type, always. |

Anyone who can connect can see everything on the screen, including anything a program prints that you would call
a secret. If the session runs a shell, anyone who can type into it can run commands as you.

## Who can connect

Three checks stack up, the same ones tmux makes.

1. **A private directory.** The session files live in a directory only you can enter: `$TMPDIR/vtee-<uid>` with mode
   `0700` on Linux, and `%LOCALAPPDATA%\vtee` on Windows, which is private to your profile. On Linux `vtee` refuses
   to use an existing directory that is not a real directory, is owned by someone else, or is open to group or others.
2. **A private socket.** Every socket is created with mode `0600` whatever your umask is, including the sockets
   you place yourself with `--listen=<path>`.
3. **A check of who is connecting.** On every connection `vtee` reads the user of the process on the other end and
   closes the connection unless it is you or root. On Windows the same check uses the process's user account and
   allows you and the SYSTEM account.

A different user is stopped by the first check on a shared machine, and by the second and third if the directory
or the socket was somehow reachable.

## What is trusted

**Every process that runs as you.** A browser helper, a build script, a package's install hook, a coding agent with a
shell: if it runs under your account it can find the socket directory, connect, read the screen, and attach and type.
In a shell that means it can run commands as you. tmux has exactly the same property, and it is the reason the
checks above only separate users, not programs.

Two things follow:

- **The input policies are cooperative.** `--writable=exclusive` and `exclusive-shared` stop two well-behaved
  clients typing over each other. They do not stop a program that ignores them: it can connect to the raw terminal
  socket, which is always open, or kill the session. They are safety rails, not a sandbox.
- **Run a session only where you would run the program.** If an agent or tool must not be able to type into a
  shell, do not run that shell in an `vtee` session it can reach. Use a separate user account or a container.

## Cases worth knowing

**`--listen=<path>`.** The path is yours to choose, so the directories above it are your responsibility, exactly as
with `tmux -S`. The socket itself is still `0600` and still checked on every connection, but a path in a
world-writable directory such as `/tmp` can be removed or replaced by another user. Prefer a directory only you
can enter.

**Containers and shared mounts.** The check compares user ids as the holder's own system reports them. A socket
mounted into a container is therefore refused when the connecting process is a different user from the holder's
point of view. That includes a container's root when user namespaces map it to another id. Run the client as the
same user. Only a same-user client and one different user on a single Linux system have been tested; user-namespace
setups have not.

**Sandboxed tools.** A tool that is sandboxed by file access but runs under your account may still reach the
socket directory. If the sandbox lets it enter `$TMPDIR` or `%LOCALAPPDATA%`, it has the access of any other program
running as you.

**Windows.**
- The profile directory's permissions are the first boundary. Administrators and SYSTEM can always open any
  user's files.
- The user check relies on a Windows feature that reports which process is on the other end of a socket. It worked
  in testing on Windows 11. If the feature is unavailable the profile directory is the only protection. The check
  was not tested with a second user account or with a process running elevated as the same user.

**Log files.** `--log` writes the raw output of the program, including anything it printed, to a file you name, with
your default file permissions. Treat it like the screen itself.

## What is not defended

- **Another process running as you**, as above, and **root or an administrator**, who can do anything anyway.
- **Reading the program's memory** (`ptrace`, `/proc`), which is outside what `vtee` controls.
- **Orphans after `SIGKILL`.** Killing the holder this way leaves the child running and the session files behind.
  `vtee ls` shows the session as stale and `vtee kill` clears it.
- **Escape sequences from the program.** `vtee` forwards the program's output to attached terminals unchanged, so a
  terminal sees what the program sends it. That is the same as running the program directly.
- **Resource use by a connected client.** Connections are capped (64 per socket) and command lines and output
  queues are bounded, but a client that is allowed in can still use CPU and memory.

## Leaving no footprint

`vtee --no-listen <program>` creates no directory, no sockets and no session record, and nothing can connect to it.
`--script` runs the same way.

## Reporting a problem

See [SECURITY.md](../../SECURITY.md).
