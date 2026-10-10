# Architecture

How `vtee` is built. The first part is a short tour for someone new to the code; the rest is the detailed
reference, written stage by stage as each piece was built (the dated headings), kept as history and corrected when
files moved. See `requirements.md` for what this is trying to satisfy, `roadmap.md` for what's still missing, and
`development/internals.md` for the OS-specific details.

## How the pieces fit

**One process per session.** The process that owns a session (the *holder*) spawns the wrapped program in a
pseudo-terminal and runs for as long as the program does. Everything else is a client of the holder.

**Threads inside the holder** (`src/tap.rs`, `src/sockets.rs`):

- an *output thread* reads the program's output. For each chunk it forwards the raw bytes to our own stdout (only
  in an interactive session), appends them to the optional raw log, feeds them to the `alacritty_terminal`
  emulator, and broadcasts them to attached terminals;
- an *input thread* (interactive only) copies our stdin to the program;
- one accept loop per socket, and a thread per connection;
- a *writer thread* per attached terminal, fed by a byte-bounded queue (`SinkQueue`, `src/tap.rs`), so a slow terminal cannot block the output thread; a terminal that falls behind gets one redraw in place of its backlog;
- a resize watcher (`SIGWINCH` on Unix, console-size polling on Windows).

**One lock for the screen.** The emulator, the byte counter and the list of attached terminals sit behind a single
mutex (`TapState`). A snapshot takes it, so a snapshot's screen and its `byte_offset` always agree; an attaching
terminal takes it to get a redraw of the current screen and start receiving later output as one atomic step.

**Two sockets, two protocols**, both in the per-user session directory (`src/session.rs`) and named after the
session:

- `<name>.sock`, newline-delimited JSON commands (`src/command.rs`): snapshot, wait, send, resize and the
  write-handoff commands. Meant for programs. Write access is governed by the `WritePolicy` chosen at start.
- `<name>.tty.sock`, raw bytes both ways (`vtee attach`). Meant for humans. A new client first receives a redraw
  of the current screen (`redraw_sequence`, `src/snapshot.rs`), then the live byte stream. Its typing goes to the
  program.

Any number of clients of either kind can be connected. A third file, `<name>.pid`, is a small record of the holder's
pid and the sockets it actually bound; `ls`, `kill` and `attach` read it to find the holder.

**Terminal queries.** Programs sometimes ask the terminal a question (`ESC[6n`, "where is the cursor?"). With a
real terminal attached it answers. With none, the emulator's own answer is written back so the program does not
wait forever; while an attach client is connected the emulator stays silent so the program never gets two replies.

**Platform differences** are confined to a few modules: sockets (`src/net.rs`, AF_UNIX on both platforms), session
directory rules (`src/session.rs`), raw mode and its restore watchdog (`src/rawmode.rs`), and console-event
handling (`src/tap.rs`). See `development/internals.md`.

## Source layout (2026-10-08)

`src/lib.rs` and `src/main.rs` were split on 2026-10-08. Where things live now (the dated sections below were
written against the old single files, and have been corrected where a path mattered):

| File | Contents |
| --- | --- |
| `tap.rs` | `Tap`, `SpawnOptions`, shared state, output/input forwarding threads |
| `serve.rs` | `ServeOptions`, `ExclusiveLock` (including `permits`), the public `serve`/`serve_tty` entry points |
| `net.rs` | `UnixListener`/`UnixStream`: `std`'s on Unix, `uds_windows` (AF_UNIX) on Windows |
| `sockets.rs` | Accept loops, connection caps, the NDJSON and raw-TTY connection handlers (both platforms) |
| `session.rs` | Session directory, name validation, `session_paths`/`session_is_live`/`session_names` |
| `rawmode.rs` | `RawModeGuard` and the terminal-restore watchdog |
| `command.rs`, `snapshot.rs`, `keys.rs`, `log.rs` | Protocol types and `authorize`/`Denied`, snapshot + redraw, named keys, asciicast log |
| `main.rs` | CLI parsing and the spawn path |
| `session_cmds.rs`, `client.rs`, `output.rs` | `attach`/`ls`/`kill`; `--connect`/`--script`; error and exit-code helpers |

The sections from here on are the detailed reference and the record of how each stage was built. Headings with a
date describe the design as of that date; `decisions.md` has the measurements and alternatives behind them.

## Shape

```
          stdin                                     pty master              pty slave
human  ------------> forward_input thread  ------------------------------->  child
       <------------ forward_output thread <-------------------------------
         stdout        |        |
                        |        \--> RawLog (if --log)
                        \-----------> TapState { term: alacritty_terminal::Term, bytes_seen }
                                       (behind one Mutex, shared with Tap::snapshot()/resize())
```

Two threads per `Tap`, both spawned from `Tap::spawn` (`src/tap.rs`):

- **`forward_input`**: blocking reads from our own `stdin`, writes straight to the pty master's writer. No
  processing, no buffering beyond the OS's. Detached (not joined) - it's expected to still be blocked in a read
  when the process exits, and that's fine, since it's reading a strictly local fd.
- **`forward_output`**: blocking reads from the pty master's reader. Each chunk is, in order: written to our own
  `stdout` (requirement 7 - this happens *before* anything else touches it, so the emulator parsing can never
  delay or alter what the human sees), appended to the raw log if one is open, then fed into the emulator while
  holding `TapState`'s lock, incrementing `bytes_seen` before releasing it. That lock ordering - write raw bytes
  out, *then* touch shared state - is what the "the emulator only watches, never alters what's forwarded"
  guarantee actually rests on in the code (not just a design intention).

A third thread (`watch_resize`) blocks on `SIGWINCH` via `signal_hook` (Unix) or polls the console size every 200 ms
(Windows has no `SIGWINCH`), and on each change re-reads the
real terminal's size and pushes it to both the pty (`MasterPty::resize`) and the emulator (`Term::resize`) so
they can never disagree about size.

## Why `term` and `bytes_seen` share one lock

`TapState` (private to `src/tap.rs`) is `{ term: Term<ObserverListener>, bytes_seen: u64, tty_sinks }` behind a single
`Mutex`. This is deliberate, not just convenient: requirement 4 wants a snapshot "as of byte N" to never disagree
with itself. If `bytes_seen` were a separate `AtomicU64` (an earlier version of this code did exactly that),
there'd be a window - however small - between the emulator being updated and the counter catching up, where a
concurrent `snapshot()` call could read new state paired with a stale offset. One lock, acquired once per chunk
in `forward_output`, makes that pairing atomic by construction instead of "usually fine in practice."

## The emulator boundary: `ObserverListener`

`alacritty_terminal::Term` takes an `EventListener` type parameter and calls back into it synchronously, from
inside the same `advance()` call that's already holding `TapState`'s lock (`src/tap.rs`, `ObserverListener`).
Title updates (`Event::Title`/`Event::ResetTitle`) are the only event this project currently cares about, and
they're stored in their *own*, separate `Arc<Mutex<Option<String>>>` rather than as a field of `TapState` - on
purpose, because the callback fires while `TapState`'s lock is already held on that same thread, and `std::sync::
Mutex` isn't reentrant. Locking order is always "big lock (`TapState`) first, small lock (title) second," in both
`forward_output`'s path (big held, small taken inside the callback) and `Tap::snapshot()`'s path (big taken, then
small) - never the other order, which is what avoids a deadlock here.

## Reading state out: `src/snapshot.rs`

`snapshot::build()` takes a `&Term<ObserverListener>` plus the byte offset and title (both already known to the
caller, so this function stays a pure read) and produces a `Snapshot`: per-cell `Cell` (text - including any
combining marks `alacritty_terminal` stores separately via `Cell::zerowidth()`, merged back into one `String` so
callers never need to know about that split - color, every SGR attribute except blink, hyperlink), `Cursor`
(position, visibility, shape), active DEC-private modes as their conventional string form (`"?1000"`, `"DECKPAM"`,
...; same vocabulary the probes and `decisions.md` already use), whether the alternate screen is active, the
title, and scrollback (plain text only, from `grid.history_size()` and negative `Line` indices - no per-cell
detail there yet). `Tap::snapshot()` is the only place that calls it live; `log::replay()` calls the same
function after replaying a log into a fresh `Term`, which is exactly what makes `tests/replay.rs`'s "live and
replayed snapshots match" assertion meaningful - both paths produce a `Snapshot` the same way.

## The raw log: `src/log.rs`

`RawLog` is asciicast v2 (the format `ht` and `avt` already use - `probe-results.md`): one header line, then one
`[seconds_since_start, "o", text]` per chunk `forward_output` sees, written and flushed immediately (not batched -
simplicity over throughput, revisit if it matters). Chunks are converted with `String::from_utf8_lossy`, so
non-UTF8 bytes in the *log* get replaced - this never touches the bytes actually forwarded to stdout or the
child, only the log's own copy, and it's the same limitation `avt`'s `feed_str`-only API has (`decisions.md`).

`replay()` reads that file back, skips the header, feeds each `"o"` event's text through a fresh
`Processor`/`Term` pair (ignoring the timestamps entirely - that's the point: a replay is "these bytes, in
order," not "these bytes, with this timing"), and returns a `Snapshot` built the same way the live path does.

## What a caller sees: `Tap`'s public surface

`Tap::spawn(program, args, SpawnOptions) -> Tap`, `Tap::snapshot() -> Snapshot`, `Tap::resize(cols, rows)`,
`Tap::wait() -> ExitStatus`, `Tap::watch_resize()` (`SIGWINCH` on Unix, console-size polling on Windows), `Tap::bytes_seen() -> u64`, `Tap::send_bytes`/`send_text`/`send_key`,
`Tap::wait_for_text`, `Tap::app_cursor_mode`. `SpawnOptions::forward_input`/`forward_output` are the independent
switches between this project's modes (`roadmap.md`, "Full duplex" stage 3b mode (d)): both `true` forwards our
own stdin to the child and the child's raw output to our own stdout (requirement 7, the human-at-a-real-terminal
case); both `false` does neither - the caller drives everything through `Command`s instead (the batch-CLI case
below); independently, `forward_output` alone lets a human watch without being able to type. `SpawnOptions::env`
(`&[(String, String)]`, `decisions.md`) sets/overrides environment variables on the child via
`portable_pty::CommandBuilder::env`, layered on top of `vtee`'s own inherited environment rather than
replacing it - an empty slice (the default) changes nothing. `RawModeGuard` stays separate from `Tap` on
purpose either way - it manages *our own* controlling terminal's mode, not the child's pty, and nothing in
non-interactive mode has a real terminal of its own to manage.

A `Tap` can be used in-process as a library, or out of process: the batch CLI below runs one script to
completion, and the observation, full-duplex and named-session sections further down add live sockets so other
processes can query and drive a running session.

## The command core: `src/command.rs`, `src/keys.rs`

`Command` is the one vocabulary this project uses for "do something to a running tap and maybe get a `Snapshot`
back" - `Send { text }`, `SendKey { key }`, `Resize { cols, rows }`, `WaitMs { ms }`,
`WaitFor { text, timeout_ms }`, `Snapshot`, and the write-handoff pair `AcquireWrite`/`ReleaseWrite`. `Command::execute(&self, tap: &Tap)` runs one against a `Tap` and
returns `Option<CommandResult>` - `None` for everything except `Snapshot` and `WaitFor`, which produce a tagged
`CommandResult::Snapshot { snapshot }` / `CommandResult::WaitFor { matched, snapshot }`. This exists as its own
module, independent of how the commands arrive, specifically so a later live protocol can drive the identical
enum from a socket instead of a script file (`roadmap.md`) without a second design.

`SendKey` resolves through `src/keys.rs`: a name (`Up`, `Home`, `F5`, `Ctrl-c`, `Alt-x`, ...) to the bytes to
send, requested by TinyFugue because they have no such table of their own. Arrow keys and Home/End pick between
CSI (`ESC [`) and SS3 (`ESC O`) forms based on `Tap::app_cursor_mode()` - a direct read of
`TermMode::APP_CURSOR` off the live `Term` - matching xterm's own behaviour, which is exactly why TF asked for
this to be mode-aware: it reads that distinction via termcap/terminfo.

Input injection needed `Tap`'s pty-master writer to stop being owned exclusively by the `forward_input` thread:
it's now `Arc<Mutex<Box<dyn Write + Send>>>`, shared between that thread (when `interactive`) and
`Tap::send_bytes`. Each write is one call under the lock, so a script's `Send`/`SendKey` and (in interactive
mode) a human's own typing can't interleave *within* a single write - see `roadmap.md` for the still-open
*policy* question of which should take priority when both are happening at once, which this locking doesn't
answer by itself.

`Tap::wait_for_text` is `WaitFor`'s scoped-down wait-for-predicate (`roadmap.md`): "does the screen's text
contain this substring," not an arbitrary predicate over a full `Snapshot` - that's what TinyFugue's own answer
said was actually needed right now. Implemented with a `Condvar` paired with `TapState`'s `Mutex`
(`forward_output` calls `notify_all()` after every chunk it applies), so it wakes exactly when there's new state
to check instead of polling.

## The batch CLI: `vtee --script <path> -- <program> [args...]`

What TinyFugue actually asked for first (`roadmap.md`): spawn `program` non-interactively
(`SpawnOptions { forward_input: false, forward_output: false, .. }` - no stdin forwarding, no raw passthrough to our own stdout, so stdout stays
clean for structured output), read `<path>` as newline-delimited JSON, run each line as a `Command` in order, and
write one `CommandResult` JSON line to stdout for every `Snapshot`/`WaitFor` command. `--log <path>` still works
the same way in this mode, for anyone who also wants the raw byte log alongside the script's structured output.
`--text` (added 2026-10-07, requested by TinyFugue while developing scripts by hand) swaps that JSON for just
`snapshot.lines`, prefixed with which command produced it and the byte offset - quicker to read, not meant to be
parsed by anything; a harness wants the JSON form.

**Cleanup is deterministic, not hopeful.** `run_script` always calls `Tap::kill()` once the script finishes -
whether it ran to completion or stopped partway on an error - rather than assuming the program being tested will
exit on its own once the script stops feeding it input. Caught by review, not by a test that happened to exercise
it: an earlier version did nothing at the end of a script, which is fine for a program that quits itself, but
would otherwise orphan it and - for anything reading from a pty with nothing left to feed it, e.g. `cat` with no
file argument - hang the whole CLI forever on the implicit `wait()` that used to be the only way out. `main`
returns `std::process::ExitCode` rather than calling `std::process::exit` anywhere once a `Tap` might be alive -
a normal function return runs every local's destructor first, where `std::process::exit` skips destructors
entirely, so calling it while a `Tap` is still alive would bypass this no matter how careful `run_script` itself
is. `Tap` also kills its child in its own `Drop` impl as a second, independent safety net, not the primary
mechanism - it covers any future caller (library or CLI) that drops a `Tap` through a path this file didn't
anticipate. Exit codes themselves follow sysexits.h (the `exitcode` crate) rather than ad hoc `0`/`1`/`2` -
`run_spawn` specifically propagates the wrapped program's *own* exit code on failure rather than
collapsing every non-zero outcome to a fixed value, the way `env`/`time` do, since a caller scripting around
`vtee` (CI, TF's harness) is more likely to care which code the wrapped program actually exited with than
whether `vtee` itself has an opinion about it.

## Observation: `vtee --listen <path>` and `vtee --connect <path>` (2026-10-07)

"Connect and watch during active use" (`roadmap.md`'s crawl/walk/run plan, stage 2) - a separate process reading
live state from an already-running interactive session, without touching it. Reuses the `Command`/`CommandResult`
core exactly as designed for this (`roadmap.md`'s "shared command core"): `Tap::serve` binds a Unix domain socket
(an AF_UNIX socket on Windows too - see "Named sessions on Windows") and, per connection, reads the same newline-delimited
`Command` JSON the batch CLI reads from a script file, executes it, and writes back a `CommandResult` line -
except every command is checked first against `ServeOptions` via `Command::authorize` (below), which can turn
it into a `CommandResult::Error` instead of ever reaching `execute`.

Needed one refactor first: `Tap::serve` takes `Arc<Tap>`, because the accept loop and each connection's handler
have to outlive the call and run concurrently with whatever `run_spawn` does next - an ordinary borrow
couldn't. That meant every field of `Tap` had to actually be `Sync`, not just usable one thread at a time -
`child: Box<dyn Child + Send + Sync>` and `output_done: Receiver<()>` both had to move behind a `Mutex` (`Receiver`
is `Send` but not `Sync` - measured via the compiler refusing `Arc<Tap>: Send` with a clear trail back to that
field, not guessed at), and `Tap::wait`/`Tap::kill` went from `&mut self` to `&self` as a result - a straightforward
mechanical change once the fields were ready for it.

`vtee --connect <path>` is the client: connects, relays its own stdin to the socket line by line (the exact
same `Command` format, typed or piped in live instead of from a file), prints whatever comes back (`--text` for
the same human-readable rendering the batch CLI offers). One real bug found by testing two commands over one
connection, not by reasoning about it: the first version's `drop(writer)` after relaying stdin didn't actually
signal EOF to the server, because `UnixStream::try_clone` duplicates the underlying fd, and a stream socket only
sees EOF when *every* fd referencing it closes (or `shutdown` is called explicitly) - the original `stream` handle
and the reader thread's clone were both still open, so the connection just hung. Fixed with an explicit
`writer.shutdown(Shutdown::Write)`, which acts on the socket itself regardless of how many other fds reference it.

## Full duplex, stage 3a: `vtee --listen <path> --writable` (2026-10-07)

Observation's read-only gate is now a real authorization check, `Command::authorize(policy, resize_owned_by_terminal)`
(`src/command.rs`), called once per command in `serve_connection` before `execute` is ever reached:
`Snapshot`/`WaitFor`/`WaitMs` always pass; `Send`/`SendKey`/`Resize` pass only under `WritePolicy::Open`, which a
connection only gets when the server was started with `--writable` (`ServeOptions`, default
`WritePolicy::ReadOnly` - observation's original behaviour, unchanged unless asked for). `Resize` has one more
rule on top, checked independently of the write policy: rejected outright whenever
`resize_owned_by_terminal` is true, which `run_spawn` sets from the same `is_terminal()` check
`RawModeGuard`/`watch_resize` already act on - a remote resize has no business fighting with a real human's
actual terminal size.

`WritePolicy` is deliberately an enum with room to grow, not a bool, per the user's explicit call
(`roadmap.md`, "I think those should all be options"): stage 3b's remaining policies (exclusive handoff,
mutually-exclusive-with-a-human) are additions to this enum later, and - noticed while writing this up, not
planned in advance - stage 3b's "free-for-all" option turns out to just be `WritePolicy::Open` reused as-is;
nothing about that mode is specific to there being no competing human, so it needed no new code at all.

The mechanism underneath needed no changes: `Tap::send_bytes` (built for the batch CLI) already serializes any
number of concurrent callers through one lock, so a `--connect` client and the interactive stdin-forwarding
thread writing "at the same time" was already safe at the byte level before this stage - stage 3a was entirely
about deciding *whether* to let a second caller reach that existing path, not making the path itself safe.

Verified end to end: a `send` rejected on a read-only listener with a clear error, accepted and visible on
screen on a `--writable` one, and a remote `resize` actually changing a live session's reported size (80x24 to
100x40) when nothing else claimed ownership of it.

## Surviving signals: `SIGTERM` and `SIGKILL` (2026-10-07)

`SIGTERM` is catchable, so `Tap::watch_sigterm` just needs a handler: a `signal_hook` thread that, on receipt,
disables raw mode (a safe no-op if it was never enabled) and kills the child via `self.child.clone_killer()`
rather than through `&mut Tap` - this runs on its own thread, and whatever thread holds `&mut Tap` may be
blocked inside `Tap::wait()` when the signal arrives, so it can't be reached that way. Called from both
`run_spawn` (`main.rs`) and `run_script` (`client.rs`) unconditionally - harmless in script mode, which never enables
raw mode in the first place.

`SIGKILL` cannot be caught, full stop - no handler, no exception, a Unix guarantee. The only way to survive it
is a second, independent process. `RawModeGuard` spawns one (Unix only; Windows has no termios and no watchdog yet) using a
`UnixStream::pair()` dead-man's-switch: one end becomes the watchdog's stdin, the other is held by the guard for
its lifetime. When the main process dies for *any* reason - clean exit, panic, or `SIGKILL` - the OS closes all
its file descriptors with no cooperation needed from the dying process, the watchdog's blocking read on its end
returns EOF, and it restores the terminal and exits. On a clean drop this fires too, just not because of a
crash - harmless.

Getting this right took three rounds of measuring against a real pty (`python3`'s `pty.fork()`, inspecting
`termios` from the master side), not reasoning about it in the abstract - each of the three bugs below looked
plausible-and-done until it was actually tested:

1. **First attempt just called `crossterm::terminal::disable_raw_mode()` from the watchdog.** It silently did
   nothing. `crossterm`'s raw-mode functions remember the pre-raw termios in a process-local static that only
   `enable_raw_mode` populates - the watchdog never called that, in that process, so its copy of the static is
   empty and `disable_raw_mode` no-ops rather than erroring, which is what made this look like it worked (`Ok`
   return) right up until the measurement showed the terminal was still raw. Fixed by capturing the real termios
   with `libc::tcgetattr` in `RawModeGuard::new` *before* enabling raw mode, and sending those bytes to the
   watchdog as the first thing written to the socketpair - it reads them before blocking on EOF, and restores
   that exact snapshot itself with `libc::tcsetattr`, independent of anything crossterm remembers.
2. **With that fixed, the watchdog still never got far enough to run it.** A session leader dying sends
   `SIGHUP` to the terminal's foreground process group, and the watchdog - unless moved - is in that same group;
   its default `SIGHUP` disposition (terminate) was killing it before it ever saw the pipe's EOF. Fixed by
   registering `SIGHUP` with `signal_hook` at the very start of `run_tty_guard` - even though nothing ever reads
   from the resulting `Signals`, registering it at all replaces the default disposition, the same mechanism
   `watch_resize`/`watch_sigterm` use for their own signals.
3. **With *that* fixed too, restoring still failed**: opening `/dev/tty` after detecting EOF returned `ENXIO`.
   POSIX disassociates the controlling terminal from a session once its leader exits, even for processes still
   alive and still part of that session - the watchdog survives the first two bugs' fixes, but by the time it
   tries to open the terminal, that association is already gone. Fixed by opening `/dev/tty` immediately at
   watchdog startup, while the session leader is still alive, and holding that fd for later rather than
   reopening it after the fact.

Verified, not assumed: 5 consecutive real-pty runs of both `SIGTERM` and `SIGKILL` restoring cooked mode
(`ICANON`) every time, plus a check that a normal clean exit leaves no orphaned watchdog process behind.

## Full duplex, stage 3b: exclusive handoff (`--writable=exclusive`/`exclusive-shared`, 2026-10-07)

`WritePolicy::Exclusive { human_priority: HumanPriority }` adds the "take the pen" mode scoped in `roadmap.md`:
only one connection may `Send`/`SendKey`/`Resize` at a time, via a new `ExclusiveLock` (`src/serve.rs`) - a
`Mutex<Option<u64>>` holding the id of whichever connection currently holds write access, or `None`. `Tap` owns
one `Arc<ExclusiveLock>`, created once per `Tap` regardless of whether `serve` is ever called with `Exclusive` at
all (inert otherwise - nothing ever checks it under `ReadOnly`/`Open`). `Tap::serve`'s accept loop hands each
connection a fresh id from `next_connection_id` (an `AtomicU64`), so `ExclusiveLock` only ever has to compare
small integers, not track connections or sockets itself.

`Command::AcquireWrite`/`ReleaseWrite` (new `Command` variants, `src/command.rs`) are handled entirely inside
`serve_connection`, not `Command::execute` - unlike every other command, the right behaviour here depends on
*which connection* is asking, something `execute`'s `&Tap` signature has no way to express. `authorize` only
checks that these two are structurally reachable under `WritePolicy::Exclusive` at all (rejected under
`ReadOnly`/`Open` - they mean nothing there); `serve_connection` then does the actual arbitration:
`AcquireWrite` calls `ExclusiveLock::try_acquire(id)` and replies with `CommandResult::WriteAccess { acquired }`
either way (a losing acquire is a well-formed reply, not an `Error` - asking and being told no is a normal
outcome, not a malformed request); `Send`/`SendKey`/`Resize` additionally check `is_held_by(id)` before
`execute`, replying `Error` if the caller doesn't currently hold the lock. `release_if_held_by(id)` runs
unconditionally when a connection's loop ends, on *every* exit path (clean `ReleaseWrite`, disconnect, or an I/O
error) - a holder that vanishes without saying so doesn't leave the lock stuck.

`HumanPriority` is the sub-setting scoped in `roadmap.md` ("I think that's just one more setting/option, right?"):
whether the interactive stdin-forwarding thread's own writes are exempt from this lock (`AlwaysWins`) or
participate in it by revoking whoever currently holds it (`Participant`). Implemented as a second piece of shared
state, `human_revokes_lock: Arc<AtomicBool>`, set once by `Tap::serve` (`true` only for
`Exclusive { human_priority: Participant }`) and read by `forward_input` after every chunk it forwards - human
keystrokes are *never* delayed or blocked either way, only `Participant` additionally calls
`ExclusiveLock::revoke()` once the bytes are already on their way to the child. This reacts to a real event (a
keystroke arrived) rather than inferring intent from idle time, the same reasoning that excluded option (c) in
`roadmap.md`. A remote holder finds out it lost the lock the next time it tries to write (`Error`, not a push
notification - there is no mechanism for the server to tell an idle connection anything unprompted), and can
`AcquireWrite` again once the human isn't actively holding it.

Verified end to end against a real pty (`python3`'s `pty.fork()`, same technique as the signal-handling tests
above, not named pipes - see below): a remote client sends `acquire_write` and gets `{"acquired":true}`; a
simulated human keystroke is written directly to the pty master; the same remote client's next `send` gets
`{"cmd":"error","message":"write access not held - send acquire_write first"}` without ever sending
`release_write`, confirming the keystroke itself revoked the lock rather than the remote client merely losing a
race to be re-granted it; a subsequent `acquire_write` from the same client succeeds again. A FIFO-based test
harness was tried first and abandoned after two real bugs in the *script*, not `vtee`: opening the FIFO's write
end after the server process was already blocked inside its own `< fifo` redirection (so the server never
reached the point of listening on its socket), and, after fixing that, insufficient settle time between
unblocking the FIFO and asserting on the result. Switching to a real pty removed the whole class of ordering
hazard at once, the same win `pty.fork()` already gave the `SIGTERM`/`SIGKILL` tests.

## Socket security: the default `--listen` path (2026-10-08)

`default_session_dir()` (`src/session.rs`, which replaced the earlier `default_socket_path()`) is tmux's own answer to "who can connect" (`docs/decisions.md`
has the full comparison): not socket-file permissions, but a private per-UID directory
(`$TMPDIR/vtee-<uid>/`, mode `0700`, passed straight to `mkdir` rather than `chmod`'d afterwards) that nothing
else can even traverse into. If that directory already exists, it's verified rather than trusted - rejected
outright if it isn't a plain directory (`symlink_metadata`, so a symlink is caught the same way a wrong-type
file would be), isn't owned by the current UID, or has any group/other permission bit set at all. `Cli`'s
`listen` field (`src/main.rs`) routes a bare `--listen`, or no `--listen` at all, through this; an explicit
`--listen=<path>` is untouched and stays the caller's own responsibility. The socket inside is named
`<pid>.sock`; `Tap::serve`'s pre-bind `remove_file` (above) already handles a stale file left by a killed,
pid-reused session. `default_log_path` (below) shares the same directory via a common `default_session_dir`
helper, so a session's socket and its diagnostic log always live side by side.

This narrows who can *reach* the socket at all - it adds nothing on top for a client that does connect, which
still gets exactly what `WritePolicy` on that connection allows, with no per-client identity beyond the
sequential id `ExclusiveLock` uses to arbitrate the write lock.

## The CLI: `clap`, three primary modes, and where diagnostics go (2026-10-08)

`src/main.rs`'s `Cli` struct (a `clap::Parser` derive) replaced a hand-rolled flag parser
(`take_path_flag`/`take_switch`/etc.) that had grown past what was comfortable to maintain by hand - seven
different ad-hoc parsing functions, one of which (`run_spawn`'s own flags) had already pushed that
function's signature past `clippy::too_many_arguments` (fixed separately by bundling them into
`InteractiveOpts`). `docs/decisions.md` has the full crate-by-crate reasoning; this is the shape it produced.

Three primary modes, selected by which flags are present rather than a `clap` subcommand, so every existing
invocation keeps working as more flags are added: `--script <path>` (batch), `--connect <path>` (observation
client, nothing spawned), and plain `[--] <command> [args...]` (interactive - itself split into "a human's
there" and `--headless`, "nobody is"). `command` is `#[arg(trailing_var_arg = true, allow_hyphen_values =
true, required_unless_present = "connect")]` - everything after the last recognized flag (or after an explicit
`--`) is the program and its own arguments, the same shape `cargo run --` already uses.

`--listen`/`--writable` are both `clap`'s "optional value" shape (`num_args(0..=1)`, `require_equals(true)`,
`default_missing_value(...)`) - `--flag` alone means one thing, `--flag=value` means another, matching
`grep --color[=WHEN]` and POSIX's own rule that an optional-argument long option *requires* the `=` form.
`Cli::try_parse()` (not `Parser::parse()`) is used specifically so a bad argument's exit code can be remapped
from `clap`'s own convention (`2`) to this project's `exitcode`/sysexits one (`exitcode::USAGE`, 64) via
`clap::Error::exit_code()` - `--help`/`--version` are errors in `clap`'s own model too but exit 0 and print to
stdout, which `Error::print()` already routes correctly on its own.

**Where diagnostic output goes** is its own small piece of logic, because during an interactive session with a
real terminal, `vtee` only ever shows the *child's* screen - there is no stderr of our own that's safe to
write to without interleaving with it. `mux_shows_child_live` (`main()`) is true exactly when something else
currently owns the real terminal (`!headless && stdin is a tty`); when true, `log`/`env_logger` is pointed at
a file from `default_log_path` (colour forced off via `write_style(Never)`, so escape codes never land in a
log file) instead of stderr, and a one-time `print_info` notice says where before raw mode is ever entered.
Fatal/abort messages (`print_error`) always go straight to stderr regardless, coloured via `owo-colors` when
the stream supports it (auto-detected, so piped output and the log file both stay plain text) - printing
through the file-backed logger here would hide the one thing a human absolutely needs to see right as the
process exits. Tracing this through surfaced one real pre-existing bug: the `--listen` setup-failure message
was being printed via plain `eprintln!` *after* `RawModeGuard` had already put the terminal in raw mode, with
the child potentially already drawing - fixed by dropping the guard (restoring cooked mode) before printing.

## Named sessions and `vtee attach` (2026-10-08)

Prompted by a direct question: could a human, in a separate shell, connect to a session an AI started and
*see what it sees*, live - the way `tmux attach` works, with several clients (of either kind) attachable to
one session at once? The answer required a real architectural change, not a new flag: today's interactive mode
fused "the process that owns the pty and the child" with "whatever is currently watching it," tying
`forward_input`/`forward_output` directly to whichever process happened to call `Tap::spawn`. There was no way
for a *different*, later process to attach to a live session at all - `--connect` only ever reached the
structured NDJSON protocol, never a live, continuously-updating raw view.

**Sessions are named now**, replacing the old pid-based socket naming entirely. `vtee::session_paths(name)`
(`src/session.rs`) resolves three paths inside the same hardened per-uid directory `default_session_dir` already
provides: the NDJSON socket (`<name>.sock`, unchanged protocol), a new raw-TTY socket (`<name>.tty.sock`), and
a pidfile (`<name>.pid`) `vtee ls`/`kill`/`attach` use to find and signal the holder process. The default
name is the program's own basename (`vtee tf` → session `tf`); `--name <NAME>` overrides it.
`vtee::session_is_live(name)` checks liveness by actually trying to connect to the NDJSON socket, not by
trusting the pidfile or the socket file's mere existence - a stale file left by a killed session must not look
"alive," the same reasoning `Tap::serve`'s own pre-bind `remove_file` already relies on.

**The pidfile is a record, so `--listen=<path>` sessions are found too.** `--listen=<path>` binds the NDJSON
socket somewhere other than `<name>.sock`, and the readers used to look only at the conventional path, so such a
session showed as `stale` and `kill` left its holder running. The holder now writes `<name>.pid` as three lines
(`SessionRecord`, `src/session.rs`): its pid, the NDJSON socket path, the raw-TTY socket path, both made absolute
because readers run from other directories. Line 1 stays a bare pid so older readers still work, and a legacy
pid-only file, or an empty line, falls back to the conventional paths. The record is written last, after both
sockets are bound, so a record's presence means the sockets exist. `session_is_live`, `ls`, `kill` and `attach`
all go through `session_record(name)`.

**Duplicate names are rejected, matching tmux's own real default** (confirmed against tmux's actual source,
not assumed): `tmux new-session -s NAME` on an existing name errors `duplicate session: NAME` - no
auto-suffixing, no silent attach. `vtee <program>` does the same. The one exception, also tmux's own
(`-A`): `--attach-if-exists` attaches instead of erroring, overridable per-invocation either direction
(`--no-attach-if-exists` forces the error even if a standing default says otherwise) and settable as that
standing default via a new config file - `VteeConfig { attach_if_exists: bool }` (`src/main.rs`), loaded with
`confy::load("vtee", None)`, TOML, at `confy`'s own default XDG path. The first persistent config this
project has had; declined earlier this session specifically because nothing needed one yet - this is that
need.

**The raw-TTY socket is purely additive, deliberately not a replacement for `forward_input`/`forward_output`.**
The obvious-looking design - route the *originating* invocation's own display through a self-connection to its
own new TTY socket, "instead of" the direct stdio path - was tried and reverted during implementation: the raw
byte protocol has no framing for an `acquire_write`/`release_write` handshake, so `WritePolicy::Exclusive`
can't mean anything there, which would have silently retired the already-built-and-tested
`HumanPriority`/exclusive-handoff machinery (stage 3b) the moment a human attached via TTY instead of owning
the original stdio. Instead: the originating invocation's own `forward_input`/`forward_output` are completely
unchanged, and `Tap::serve_tty` (new, mirroring `Tap::serve`'s shape) binds a *second*, independent listener
that only `vtee attach`/other remote clients use - always `WritePolicy::Open` (no `Exclusive` concept for
this protocol; a remote TTY attacher can always type, exactly as the local invocation's own typing already
could, independent of this policy entirely).

**Multicast output, redraw-on-attach, and why they're one atomic operation.** Every attached TTY client
receives the live byte stream via a new `Tap` field, `TapState::tty_sinks` - deliberately living inside the
*same* `Mutex` as `term`/`bytes_seen`, not a separate lock: `forward_output` already serializes "apply this
chunk to `term`" and must also serialize "broadcast this chunk to every sink" relative to a *new* client's own
"snapshot current state, then start receiving future chunks" (`Tap::attach_tty_sink`) - two separate locks
would leave a real window where a chunk lands in neither the new client's redraw nor its live feed (or lands in
both). `vtee::snapshot::redraw_sequence(&Snapshot) -> Vec<u8>` (new) reconstructs the current screen as raw
ANSI before the live feed starts, so attaching mid-session shows the current picture immediately, the way
`tmux attach` does - checked directly against `alacritty_terminal` 0.26.0's own source first: no existing API
does this (it only ever consumes ANSI, never re-emits it), so this is new code, not a missed library feature.
Scope, deliberately: visual state only (cells + cursor); restoring input-affecting modes (app-cursor-keys,
mouse reporting) for a freshly-attached client's own keystrokes is a smaller, known follow-on.

**`vtee ls`/`kill`** read/signal via the pidfile record. `kill` signals only after the recorded NDJSON socket
answers, then cleans up the pidfile and the recorded sockets itself rather than relying on the holder's own
cleanup - `watch_sigterm`'s handler calls `std::process::exit` directly on `SIGTERM`, which skips every destructor
including the holder's own cleanup guard (`SessionFiles`, `src/main.rs`), so without this the files would be left
behind despite the process actually stopping. On Unix a recorded path is removed only if it is still a socket: a
stale record's path may since have been reused for something else. `ls` prints `sock=<path>` for a session whose
NDJSON socket is not the conventional one.

**Detach has no keybinding**, unlike tmux (which needs a prefix key, Ctrl-b `d`, because its server requires
an explicit detach signal). Here, the holder's lifetime depends on the child process, not on any one client
connection, so "detaching" is just the attach client process ending - Ctrl-C, closing the window - with no
escape sequence stealing keystrokes from the child. This keeps requirement 7's full-transparency promise
intact for this feature rather than trading it away for tmux-literalism.

**Verified end to end** (not just unit-level): a human (simulated via a real pty) spawns a named session;
a *separate* process attaches and sees the live screen immediately (redraw) followed by further live updates;
typing *in the attached client* reaches the child and is visible back on the original spawner's own terminal -
true bidirectional multi-client behaviour, not just one-way observation. Also verified: duplicate-name
rejection, `--attach-if-exists` attaching instead, and the `ls`/`kill` round trip. `tests/cli.rs`'s
`unix_behavior` module has the automated versions of the CLI-visible pieces (naming, duplicates, `ls`/`kill`);
the deeper protocol mechanics (multicast, redraw ordering, bidirectional typing) were verified via a scripted
real-pty harness during implementation, the same technique `tests/windows_conpty.rs` and the `Participant`/
free-for-all verifications earlier this session already used.

**Known gap, accepted rather than solved**: if the holder process is itself `SIGKILL`'d, no destructor runs
(including `SessionFiles`), so its socket/pidfile are left behind as stale leftovers and its child reparents to
init, unreachable via `vtee` from then on. Fixing this would need a second watchdog process, the same trick
`RawModeGuard` already uses for terminal restoration - real, but for an edge case that requires deliberately
`kill -9`ing the holder specifically, not its child. Revisit only if it actually bites; `session_is_live`
already degrades gracefully around a stale leftover (a subsequent same-named spawn succeeds, treating it as
removable, the same way `Tap::serve` already treats a stale socket file).

## Known rough edges (tracked, not silently ignored)

- `Tap::wait()` doesn't block forever waiting for the pty master's reader to see EOF, because it isn't guaranteed
  to happen promptly on every platform (measured: it doesn't on Windows ConPTY) - it waits up to 500ms via a
  channel and moves on. On Linux this is a non-issue in practice (EOF arrives essentially immediately); the
  timeout exists for robustness against platforms/situations where it doesn't, not because Linux needs it.
- Only the active screen buffer is exposed by `Snapshot`, not both - deferred, see `roadmap.md`/`decisions.md`.
- `Tap::spawn` takes environment variables through `SpawnOptions::env` (`--env`), including `TERM`; there is no separate `TERM` setting.

## Named sessions on Windows (2026-10-09)

Sessions, `attach`/`ls`/`kill` and both sockets work on native Windows. The pieces that differ from Unix:

- **Transport:** AF_UNIX sockets (Windows 10 1803+) through `uds_windows`, re-exported by `src/net.rs` so the
  servers and clients are written once. Named pipes were rejected (`decisions.md`).
- **Session directory:** `%LOCALAPPDATA%\vtee` (temp dir if unset), private to the user by the profile's ACL; a
  reparse point or non-directory there is refused. Session names are capped so `<name>.tty.sock` stays under the
  107-byte AF_UNIX path limit.
- **`kill`:** after the same liveness check as Unix, `OpenProcess` + `TerminateProcess` on the holder; ConPTY reaps
  the wrapped program. Files are removed by `kill` itself. There is no PID-reuse guard beyond the liveness check.
- **Termination events:** `watch_sigterm` registers a console control handler for close/break/shutdown that kills
  the child and exits 143. Session files are left for `ls`/`kill`, as after a `SIGKILL`.
- **No terminal-restore watchdog:** nothing restores console modes if the holder is force-killed (shells reset
  them at the next prompt). `RawModeGuard` is a plain raw-mode toggle on Windows.
- **Handle inheritance:** not an issue. The child is spawned before any socket exists, and `portable-pty`'s ConPTY
  spawn calls `CreateProcessW` with `bInheritHandles = FALSE` (measured, `tests/windows_sessions.rs`).

