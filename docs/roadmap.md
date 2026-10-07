# Roadmap

Staged plan, what's being built now, what v1 still needs beyond that, the standing open decisions, and what
TinyFugue's testing side told us about its actual near-term needs. See `requirements.md` for what each stage is
aiming at and `architecture.md` for how the done parts work.

## v0 - done, 2026-10-06 (Linux; Windows blocked, see `decisions.md`)

Spawn, bidirectional forward, resize, terminal restore on normal exit/panic, a snapshot API (cells, attributes,
cursor, modes, offset, title, scrollback), a raw byte log, and replay - verified against the live snapshot for
all 15 probe files (`tests/replay.rs`).

## Hardening v0

1. ~~**Both screen buffers in the snapshot**~~ **Deferred, 2026-10-07** - not skipped, genuinely blocked: there's
   no safe way to read `alacritty_terminal`'s inactive buffer without destroying it, verified against the crate
   and alacritty's own upstream `main` (`decisions.md`). Revisit by vendoring a trivial local patch if TF's first
   real snapshot shows TF actually uses the alternate screen and the content underneath needs inspecting - not
   before, and not by any other workaround (the alternatives considered are worse than waiting).
2. **`SIGTERM`-safe shutdown.** Catchable, straightforward: a signal handler (`signal_hook`, Unix) that disables
   raw mode before exiting, same as `RawModeGuard` already does for normal exit and panic unwind.
3. **`SIGKILL`-safe terminal restoration.** `SIGKILL` cannot be caught by the dying process - no handler, no
   exception, that's a Unix guarantee, not a gap in this code. The only way to survive it is a second process: a
   small watchdog that blocks reading a pipe whose write end only the main process holds. When the main process
   dies for *any* reason, including `-9`, the OS closes that fd automatically (process death always closes a
   process's fds, with no cooperation needed from the dying process), the watchdog's blocking read returns EOF,
   and it restores the terminal and exits. This is the standard technique other terminal-wrapping tools use for
   exactly this problem.

## TinyFugue's answers, 2026-10-06 (asked via cross-session message, see `decisions.md`-style provenance: these
are TF's own judgments from their harness design and source, not measurements - they haven't run TF under any
terminal emulator yet)

1. **Live wait-for-predicate: not needed yet.** TF's tests are deterministic scripts against a fake MUD server.
   Feed a script, take one snapshot, done. Live predicates only matter for interactive agent use (the "other
   half" of this project's purpose - see below), not for TF's first integration.
2. **A non-interactive CLI is enough for the first integration** - run a script of steps (send bytes or keys,
   resize, wait N ms, snapshot) and dump JSON; their Python harness (`tests/harness.py`) can call it as a
   subprocess. Defer `send_keys`-over-a-live-protocol until an agent needs to watch a live session.
3. **No key-name-to-escape table exists on their side.** They want one shipped in `vtee`: Up/Down/Left/Right,
   Home/End/PgUp/PgDn, F1-F12, Ctrl-x, Alt-x - and it needs to be mode-aware, since application-cursor mode
   changes arrow keys from `ESC [ A` to `ESC O A` and TF reads that distinction via termcap/terminfo. We already
   track that mode in `Snapshot.modes` (`?1`/DECCKM), so this is a lookup against state we already have, not new
   plumbing.
4. **Marker sync needs rethinking, not dropping.** `@@BEGIN@@`/`@@END@@` would land as literal on-screen text
   now, not captured stdout. Their suggested cheap fix: a `wait_for <text>` *step inside the script itself* -
   not a live protocol, just a blocking step within one batch run.
5. **Both screen buffers**: confirmed needed (see Hardening above) - they don't know yet whether TF uses the
   alternate screen and want to find out from a first real snapshot.
6. **Clock masking: probably needed**, not confirmed (TF's status line can show a clock field, default not yet
   checked). Wants cell-region or regex row masking so goldens can ignore it. A snapshot-comparison concern, not
   core `Tap`.

Their stated next step: once the CLI exists, point them at the binary path and script format and they'll try a
first TF startup snapshot.

## Done, 2026-10-07: a shared command core, driving the batch CLI now and (later) the live protocol

TF's answers describe *half* of what this project exists for (HANDOFF.md §1): scripted, non-interactive testing.
The other half - an agent driving or watching a *live* session - still needs the same primitives eventually
(send input, resize, wait for a condition, snapshot), just driven from a different place and running
indefinitely instead of to completion. Building the batch CLI as a one-off, TF-shaped tool would have meant
redesigning all of this when the live half gets built, so it wasn't: built on a shared core instead, so the live
protocol (whenever it happens) is "the same commands, read from a socket instead of a script file" - not a
second design.

**The core** (`src/command.rs`): a `Command` enum, one variant per primitive TF asked for, each executable
against a running `Tap` via `Command::execute`:

```rust
enum Command {
    Send { text: String },
    SendKey { key: String },                     // resolved via the mode-aware key table, src/keys.rs
    Resize { cols: u16, rows: u16 },
    WaitMs { ms: u64 },
    WaitFor { text: String, timeout_ms: u64 },    // the wait-for-predicate design from before, scoped
                                                   // down to "text appears", which is all that's asked
                                                   // for right now
    Snapshot,
}
```

`Send`/`SendKey` needed `Tap`'s pty-master writer to become shareable (`Arc<Mutex<...>>`) rather than owned
solely by the interactive-mode stdin-forwarding thread - see `architecture.md` for why that doesn't introduce a
corruption risk, only an open *policy* question (still listed under "Open decisions" below).

**Driver 1, done: the batch CLI.** `vtee --script <path> -- <program> [args...]` (`architecture.md`). Reads
the script as newline-delimited JSON (one `Command` per line - the same convention this project already uses
for the raw log and the probes), runs each one in order against a non-interactive `Tap`
(`SpawnOptions::interactive = false`), and writes one `CommandResult` JSON line to stdout per `Snapshot`/
`WaitFor` command. Verified end to end against a real Linux pty: `send`/`send_key` (mode-aware arrow-key
resolution confirmed via the pty's own echo), `resize`, `wait_ms`, and `wait_for` in both outcomes (text found
before timeout, and genuinely timing out without hanging). TinyFugue has the binary path and script format now
(answer to their stated next step above) and can try a first TF startup snapshot.

**Driver 2, later (v1): the live protocol.** The same `Command` JSON objects, read from a socket connection
instead of a static file, executed against a `Tap` that keeps running instead of exiting after the script ends,
with `Snapshot` results streamed back over the same connection as they're produced. The "wire protocol" question
below is now really just "what carries these bytes" (a Unix socket is still the leading option), not "what
commands does it need to support" - that part's answered by the same enum the CLI already uses.

This doesn't retire "Input injection" or "Wait-for-predicate" as v1 line items - the *policy* questions under
them (human/agent input merging; whether `WaitFor` needs to grow into a full arbitrary predicate rather than
just "text appears") are still open and still only matter once something drives this live. It does mean that
work, whenever it happens, extends the same `Command` core rather than inventing a second one.

## v1 (scoped, not built - the live/interactive half)

### Input merging policy

The mechanism is already built: `Tap`'s pty-master writer is `Arc<Mutex<Box<dyn Write + Send>>>`, shared between
the interactive-mode stdin-forwarding thread and `Command::Send`/`SendKey` (`architecture.md`), so a script's
injected input and (in interactive mode) a human's own typing can't corrupt each other - each write is one
atomic chunk under the lock. What's still open is *policy*, not mechanism: once something drives `Command`s
live against a `Tap` that also has a human typing, when a human is typing and an agent also wants to inject, who
goes first, and does one block the other? That needs a real answer from how this actually gets used, not a
guessed-at default, and TF's answers don't resolve it since TF's use is batch-only with no human in the loop.

### Wait-for-predicate, beyond "text appears"

`Tap::wait_for_text`/`Command::WaitFor` only checks for a substring (built, `architecture.md`). A live/interactive
use might eventually want an arbitrary predicate over a full `Snapshot` (cursor position, a mode, a cell's
colour), not just text. The plumbing for that already exists too - `TapState`'s `Mutex` is already paired with a
`Condvar` that `forward_output` notifies after every chunk - so generalizing just means accepting a closure
instead of a fixed string, not new infrastructure. Not done because nothing's asked for it yet. Masking volatile
regions (a clock) for golden comparisons stays a separate, `Tap`-independent helper either way (TinyFugue's
answer 6).

### Wire protocol

What carries live `Command`s/`Snapshot`s between processes. Leaning Unix domain socket with line-delimited JSON
(what `ht` does, and what the `Command` design above already assumes) - a Python client needs nothing but
`socket` + `json`. Considered and set aside: stdio-based JSON-RPC (conflicts with stdin/stdout already being
raw-forwarding channels, would need its own fd for no clear win); HTTP/WebSocket (more flexible than anything
here needs yet). Not decided. Windows equivalent (named pipes) is later, same as ConPTY itself.

## Later

An MCP front end (wraps whichever protocol v1 picks), Windows (ConPTY, blocked on an upstream bug - see
`decisions.md`), multiple sessions (one process managing several `Tap`s, vs. one process per `Tap` - affects the
protocol: a socket per process is simplest, a daemon multiplexing several needs session IDs in every message -
not decided).

## Open decisions

- **Protocol.** See "Wire protocol" above. Leaning Unix socket + line-delimited JSON, not decided.
- **Input merging** when both a human and an agent can type. See "Input merging policy" above - a real policy
  decision, not yet made, and TF's batch-only use doesn't answer it.
- **One process per session, or a daemon managing several?** Not decided.
- **Licence.** Not chosen. `publish = false` in `Cargo.toml` until it is.
- **Whether the name `vtee` is free to use.** Not checked.
