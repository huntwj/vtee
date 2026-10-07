# Roadmap

Staged plan, what v1 actually requires (scoped, not built), the standing open decisions, and the open questions -
including the ones only the TinyFugue side can answer. See `requirements.md` for what each stage is aiming at and
`architecture.md` for how the done parts work.

## v0 - done, 2026-10-06 (Linux; Windows blocked, see `decisions.md`)

Spawn, bidirectional forward, resize, terminal restore on normal exit/panic, a snapshot API (cells, attributes,
cursor, modes, offset, title, scrollback), a raw byte log, and replay - verified against the live snapshot for
all 15 probe files (`tests/replay.rs`).

## Hardening v0 (in progress, 2026-10-06)

Three concrete gaps found by actually testing, not yet closed:

1. **Both screen buffers in the snapshot** (requirement 3). Currently `Snapshot` only reads `term.grid()` (the
   active buffer); `alacritty_terminal` also tracks an inactive one. Needs exposing both, with a flag for which
   is currently active (already have `alt_screen` for that).
2. **`SIGTERM`-safe shutdown.** Catchable, straightforward: a signal handler (`signal_hook`, Unix) that disables
   raw mode before exiting, same as `RawModeGuard` already does for normal exit and panic unwind.
3. **`SIGKILL`-safe terminal restoration.** `SIGKILL` cannot be caught by the dying process - no handler, no
   exception, that's a Unix guarantee, not a gap in this code. The only way to survive it is a second process: a
   small watchdog that blocks reading a pipe whose write end only the main process holds. When the main process
   dies for *any* reason, including `-9`, the OS closes that fd automatically (process death always closes a
   process's fds, with no cooperation needed from the dying process), the watchdog's blocking read returns EOF,
   and it restores the terminal and exits. This is the standard technique other terminal-wrapping tools use for
   exactly this problem.

## v1 (scoped, not built)

### Input injection

`Tap` currently has one writer to the pty master, owned by the internal thread that forwards our own `stdin`
(`forward_input`, `architecture.md`). Letting an external caller (`send_keys`-style) inject input too means that
writer needs to be shared - likely `Arc<Mutex<Box<dyn Write + Send>>>`, with both `forward_input`'s loop and a new
`Tap::send_input(&self, bytes: &[u8])` locking it per write. Each call already writes one complete chunk under
the lock, so individual injections and individual stdin reads can't interleave *within* themselves - what's
still an open policy question, not a data-corruption risk, is *when* an agent should be allowed to inject while a
human is also typing (steal focus immediately? queue behind the human? make it configurable?). That's the
"input merging" open decision below, and it needs a real answer from how this actually gets used, not a
guessed-at default.

### Wait-for-predicate

`Tap::wait_for(predicate: impl Fn(&Snapshot) -> bool, timeout) -> Result<Snapshot, Timeout>`. Cheapest correct
design: pair `TapState`'s `Mutex` with a `Condvar`; `forward_output` calls `notify_all()` after each chunk's
state update (it already holds the lock there), and `wait_for` loops on `condvar.wait_timeout()` re-checking the
predicate - no polling, wakes up exactly when there's new state to check. Marker synchronisation (watching for
TF's own `@@BEGIN@@`/`@@END@@`-style output) is just `wait_for` with a "does the text contain this marker"
predicate, not a separate primitive. Masking volatile regions (a clock in a status line) is a snapshot-comparison
concern, not a `Tap` one - probably a small helper that takes two `Snapshot`s and a set of masked regions, usable
by whatever eventually does golden-testing against this.

### Wire protocol

No process boundary exists yet (`architecture.md`) - everything is in-process. Options, roughly in order of how
much they fit what's already decided: a Unix domain socket with line-delimited JSON (what `ht` does, and what
the original design notes already leaned toward) - simple, and a Python harness needs nothing but `socket` +
`json` to speak it; stdio-based JSON-RPC - doesn't need a socket path, but conflicts with stdin/stdout already
being raw-forwarding channels to the human/child, so it'd need its own fd, which is extra plumbing for no clear
win over a socket; an HTTP/WebSocket server - more flexible, but nothing here needs that flexibility yet. Leaning
socket, not decided. Core commands it would need to carry: `snapshot`, `wait_for`, `send_keys`, `resize`, maybe
`stream(from_offset)` for tailing raw output live. Windows equivalent (named pipes) is later, same as ConPTY
itself.

**Before building any of this**, it's worth checking whether the first real use (TinyFugue) even needs a
protocol at all - see "Open questions for TinyFugue" below. A non-interactive CLI that runs a script through
`Tap` and dumps one final JSON snapshot might cover the actual near-term need with none of this.

## Later

An MCP front end (wraps whichever protocol v1 picks), Windows (ConPTY, blocked on an upstream bug - see
`decisions.md`), multiple sessions (one process managing several `Tap`s, vs. one process per `Tap` - not decided,
see open questions).

## Open decisions

- **Protocol.** See v1 scoping above. Leaning Unix socket + line-delimited JSON, not decided.
- **Input merging** when both a human and an agent can type. See "Input injection" above - a real policy
  decision, not yet made.
- **One process per session, or a daemon managing several?** Affects the protocol design (a socket per process is
  simplest; a daemon multiplexing several `Tap`s needs session IDs in every message). Not decided.
- **Licence.** Not chosen. `publish = false` in `Cargo.toml` until it is.
- **Whether the name `vtee` is free to use.** Not checked.

## Open questions for TinyFugue

Grounded in the actual gap between what v0 already does and what v1 is scoped to add - worth asking the session
working on TinyFugue testing before building any of v1, since the answers would change its shape or priority:

1. Does the TF test harness need *live* wait-for-predicate at all, or does "feed a scripted sequence, then take
   one final snapshot" (which v0 already supports, replay included) cover the display/status-line/input-editing
   cases you have in mind? This is the single biggest thing v1 adds over v0 - worth knowing if it's actually
   needed before it gets built.
2. TF's existing harness synchronises via printed markers (`@@BEGIN@@`/`@@END@@`) in a non-interactive run - does
   testing with a terminal attached change that, or would the same marker-based approach still work, just with a
   screen to inspect afterward instead of (or alongside) captured stdout text?
3. For a first integration, would a non-interactive CLI - run a script through `Tap`, dump one final JSON
   snapshot - actually be enough, or does the harness need a live, out-of-process protocol (`send_keys` etc.)
   from day one? This determines whether any of the v1 wire-protocol work needs to happen before TF can use this
   at all.
4. For input-editing/`/bind` tests specifically: does the harness need to send literal special keys (arrows,
   function keys) as their escape sequences programmatically? If so, is there already a key-name-to-escape-
   sequence table on the TF testing side, or would `vtee` need to provide one?
5. The snapshot currently only exposes the *active* screen buffer, not both (a known v0 gap, being fixed now) -
   does anything about testing TF's status line or display actually depend on inspecting state while TF is in
   its alternate screen, or is the main screen what matters for the tests you're picturing?
6. Does TF's status line (or anything else in its display) include something that changes on its own, like a
   clock, that would need masking for a text-comparison golden test - or was that a general concern from the
   original design notes that doesn't actually apply to TF's real output?
