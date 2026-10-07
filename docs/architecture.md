# Architecture

How `vtee` is actually built, as of 2026-10-06 (v0). Written because this didn't exist anywhere as a single
document before now - it was scattered across source comments and `HANDOFF.md`'s design notes. See
`requirements.md` for what this is trying to satisfy and `roadmap.md` for what's still missing.

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

Two threads per `Tap`, both spawned from `Tap::spawn` (`src/lib.rs`):

- **`forward_input`**: blocking reads from our own `stdin`, writes straight to the pty master's writer. No
  processing, no buffering beyond the OS's. Detached (not joined) - it's expected to still be blocked in a read
  when the process exits, and that's fine, since it's reading a strictly local fd.
- **`forward_output`**: blocking reads from the pty master's reader. Each chunk is, in order: written to our own
  `stdout` (requirement 7 - this happens *before* anything else touches it, so the emulator parsing can never
  delay or alter what the human sees), appended to the raw log if one is open, then fed into the emulator while
  holding `TapState`'s lock, incrementing `bytes_seen` before releasing it. That lock ordering - write raw bytes
  out, *then* touch shared state - is what the "the emulator only watches, never alters what's forwarded"
  guarantee actually rests on in the code (not just a design intention).

A third thread (`watch_resize`, Unix only) blocks on `SIGWINCH` via `signal_hook`, and on each one re-reads the
real terminal's size and pushes it to both the pty (`MasterPty::resize`) and the emulator (`Term::resize`) so
they can never disagree about size.

## Why `term` and `bytes_seen` share one lock

`TapState` (private to `src/lib.rs`) is `{ term: Term<ObserverListener>, bytes_seen: u64 }` behind a single
`Mutex`. This is deliberate, not just convenient: requirement 4 wants a snapshot "as of byte N" to never disagree
with itself. If `bytes_seen` were a separate `AtomicU64` (an earlier version of this code did exactly that),
there'd be a window - however small - between the emulator being updated and the counter catching up, where a
concurrent `snapshot()` call could read new state paired with a stale offset. One lock, acquired once per chunk
in `forward_output`, makes that pairing atomic by construction instead of "usually fine in practice."

## The emulator boundary: `ObserverListener`

`alacritty_terminal::Term` takes an `EventListener` type parameter and calls back into it synchronously, from
inside the same `advance()` call that's already holding `TapState`'s lock (`src/lib.rs`, `ObserverListener`).
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
`Tap::wait() -> ExitStatus`, `Tap::watch_resize()` (Unix; no-op stub on other platforms, so callers don't need to
`cfg`-gate the call site), `Tap::bytes_seen() -> u64`, `Tap::send_bytes`/`send_text`/`send_key`,
`Tap::wait_for_text`, `Tap::app_cursor_mode`. `SpawnOptions::interactive` is the one switch between this
project's two modes (`roadmap.md`): `true` forwards our own stdin to the child and the child's raw output to our
own stdout (requirement 7, the human-at-a-real-terminal case); `false` does neither - the caller drives
everything through `Command`s instead (the batch-CLI case below). `RawModeGuard` stays separate from `Tap` on
purpose either way - it manages *our own* controlling terminal's mode, not the child's pty, and nothing in
non-interactive mode has a real terminal of its own to manage.

There is currently **no process boundary** for the *live* case - an in-process `Tap` is still the only way to
drive one interactively. The batch CLI below is a process boundary of a much narrower kind: one script, run to
completion, no ongoing connection. A general out-of-process protocol for a live/interactive session is still v1
work, scoped in `roadmap.md`, not built yet.

## The command core: `src/command.rs`, `src/keys.rs`

`Command` is the one vocabulary this project uses for "do something to a running tap and maybe get a `Snapshot`
back" - `Send { text }`, `SendKey { key }`, `Resize { cols, rows }`, `WaitMs { ms }`,
`WaitFor { text, timeout_ms }`, `Snapshot`. `Command::execute(&self, tap: &Tap)` runs one against a `Tap` and
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
(`SpawnOptions::interactive = false` - no stdin forwarding, no raw passthrough to our own stdout, so stdout stays
clean for structured output), read `<path>` as newline-delimited JSON, run each line as a `Command` in order, and
write one `CommandResult` JSON line to stdout for every `Snapshot`/`WaitFor` command. `--log <path>` still works
the same way in this mode, for anyone who also wants the raw byte log alongside the script's structured output.

## Known rough edges (tracked, not silently ignored)

- `Tap::wait()` doesn't block forever waiting for the pty master's reader to see EOF, because it isn't guaranteed
  to happen promptly on every platform (measured: it doesn't on Windows ConPTY) - it waits up to 500ms via a
  channel and moves on. On Linux this is a non-issue in practice (EOF arrives essentially immediately); the
  timeout exists for robustness against platforms/situations where it doesn't, not because Linux needs it.
- Terminal restoration (`RawModeGuard`) only covers normal exit and panic unwind. `SIGKILL` and `SIGTERM`
  survival are real gaps - see `roadmap.md`.
- Only the active screen buffer is exposed by `Snapshot`, not both - see `roadmap.md`.
- `Tap::spawn` doesn't let a caller set `TERM` or other environment variables on the child yet (requirement 6).
