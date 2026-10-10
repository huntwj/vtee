# Roadmap

Staged plan, what's being built now, what v1 still needs beyond that, the standing open decisions, and what
TinyFugue's testing side told us about its actual near-term needs. See `requirements.md` for what each stage is
aiming at and `architecture.md` for how the done parts work.

## Beta readiness

State as of 2026-10-09. The core feature set works on Linux and Windows. What is left before calling it a beta:

- [x] **First CI run.** `.github/workflows/ci.yml` ran on real runners on 2026-10-09 and all six jobs passed
  (lint, MSRV, and the fast and pty jobs on `ubuntu-latest` and `windows-latest`). See `development/ci.md`.
- [x] **A license.** `MIT OR Apache-2.0`, in `LICENSE-MIT` and `LICENSE-APACHE`.
- [ ] **Release automation.** Versions follow commit messages; the automation is described in `releasing.md`. Its
  first run failed with "GitHub Actions is not permitted to create or approve pull requests", so the repository
  setting listed in `releasing.md` must be turned on before the first release PR can open.
- [ ] **A live read-only socket rejecting writes.** `Command::authorize` has a table test over every policy and
  command, but no integration test sends `send` to a read-only session and checks the error. The other test gaps
  listed earlier (snapshot attributes, the `redraw_sequence` round trip, `--log` through the CLI, the config file,
  terminal restore after a signal) now have tests.
- [ ] **Mouse and bracketed-paste input.** The modes are tracked in snapshots; there is no way to send mouse or
  paste events. On Windows ConPTY hides the mouse modes from the emulator (`guide/windows.md`).
- [x] **Underline styles.** Snapshot cells carry `underline_style` (single, double, curly, dotted, dashed) and
  `underline_color`; `underline` stays as the on/off flag.
- [ ] **Both screen buffers.** Only the active buffer is exposed. Blocked by `alacritty_terminal` (see
  `decisions.md`, 2026-10-07).
- [ ] **Richer wait conditions.** `wait_for` is a substring match on one screen line; there is no regular
  expression, region, or match across a wrapped line. `exit_status` and `wait_exit` exist for the process itself.
  A "screen unchanged" wait was deliberately not built (requirement 4).
- [ ] **Masking volatile regions** (a clock) for golden comparisons. Left to the caller so far; a golden-file format
  with masks and attribute checks is proposed but not built.
- [ ] **Windows terminal-restore watchdog.** Nothing restores the console if the holder is force-killed.
- [x] **Windows `attach` console flags.** `RawModeGuard` sets `ENABLE_VIRTUAL_TERMINAL_INPUT` and
  `ENABLE_VIRTUAL_TERMINAL_PROCESSING` and restores the original modes on drop and from the console control
  handler. The output half is untestable under ConPTY (it is already on); a legacy `conhost` window is untested.
- [x] **Known defects.** `--listen=<path>` sessions are visible to `ls` and `kill`; bare `attach` picks the session
  attached to most recently; `attach` refuses a non-terminal.
- [x] **Snapshot schema version.** Snapshots start with `"schema_version":1`; the policy is in `guide/protocol.md`.
- [x] **Differential emulator tests.** 80 escape-sequence cases are compared with `avt` and `vt100`
  (`development/testing.md`). Three deviations inherited from `alacritty_terminal` are accepted for 0.1.0 and listed
  in `guide/protocol.md` ("Known terminal-emulation differences").

Done: spawn and bidirectional forwarding, snapshots, raw log and replay, the JSON socket with write policies,
named sessions with `attach`/`ls`/`kill`, several clients at once, terminal restore after `SIGTERM` and `SIGKILL` on
Linux, and native Windows support (ConPTY, resize, Ctrl-C, sessions over AF_UNIX).

## v0 - done, 2026-10-06 (Linux; Windows followed, see `decisions.md`)

Spawn, bidirectional forward, resize, terminal restore on normal exit/panic, a snapshot API (cells, attributes,
cursor, modes, offset, title, scrollback), a raw byte log, and replay - verified against the live snapshot for
all 15 probe files (`tests/replay.rs`).

## Hardening v0

1. ~~**Both screen buffers in the snapshot**~~ **Deferred, 2026-10-07** - not skipped, genuinely blocked: there's
   no safe way to read `alacritty_terminal`'s inactive buffer without destroying it, verified against the crate
   and alacritty's own upstream `main` (`decisions.md`). Revisit by vendoring a trivial local patch if TF's first
   real snapshot shows TF actually uses the alternate screen and the content underneath needs inspecting - not
   before, and not by any other workaround (the alternatives considered are worse than waiting).
2. ~~**`SIGTERM`-safe shutdown.**~~ **Done, 2026-10-07** (`Tap::watch_sigterm`, `architecture.md`). Catchable, so
   no second process needed: a `signal_hook` handler disables raw mode (a safe no-op if it was never enabled)
   and kills the child via `ChildKiller::clone_killer` - deliberately not `Tap::kill`, since this runs from its
   own thread and the thread holding `&mut Tap` may be blocked in `wait()` when the signal arrives. Verified on
   a real pty: `SIGTERM` to a running `vtee` restores cooked mode (`ICANON`) and exits, both in interactive
   mode and mid-way through a batch script.
3. ~~**`SIGKILL`-safe terminal restoration.**~~ **Done, 2026-10-07** (`RawModeGuard`'s watchdog, `architecture.md`).
   `SIGKILL` cannot be caught by the dying process - no handler, no exception, a Unix guarantee, not a gap in
   this code - so this needed a second process: a watchdog that blocks reading a socketpair whose other end only
   the main process holds, restoring the terminal and exiting once that read sees EOF (which happens whether the
   main process exited cleanly or was killed with `-9` - process death always closes its file descriptors, with
   no cooperation needed from the dying process). Two real, non-obvious failure modes found and fixed by actually
   testing this on a real pty (`python3`'s `pty.fork()` + inspecting `termios` from the master side), not by
   reasoning about it in the abstract - see `architecture.md` for both:
   - `crossterm::terminal::disable_raw_mode()` can't be called from the watchdog at all; it only works via a
     process-local static populated by `enable_raw_mode` in the *same* process, so calling it from a different
     process silently no-ops. Fixed by capturing the real termios with `libc::tcgetattr` before enabling raw
     mode and sending it to the watchdog directly.
   - Even with that fix, restoring still failed the first time: opening `/dev/tty` *after* the main process died
     returned `ENXIO`, because POSIX disassociates the controlling terminal from a session once its leader
     exits - even though the watchdog itself is still alive and still part of that session. Fixed by opening
     `/dev/tty` immediately at watchdog startup, while the session leader is still alive, and holding that fd
     rather than reopening it after the fact.
   - A third race (the watchdog itself being killed by the `SIGHUP` the kernel sends to the terminal's
     foreground process group when a session leader dies, before it could react to the EOF) was caught and
     fixed the same way, before the two above were even found - see `architecture.md`.
   Verified on a real pty, 5 consecutive runs: `SIGKILL` to the main process restores cooked mode every time, and
   a normal clean exit leaves no orphaned watchdog process behind.

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

## TinyFugue's first real run, 2026-10-07 - confirms two of the answers above, measured not guessed

Ran `vtee --script s.jsonl -- tf -L<libdir> -n -f` (Linux container, `TERM=xterm-256color`): resize, wait,
snapshot, send a command, `wait_for` its echo, snapshot again. "Build, spawn, send, wait_for and snapshot all
behaved. It covers the first integration."

- **Q5 (both screen buffers) answered, and the deferral holds**: `alt_screen` was `True` - TF's banner and
  status line render in the alternate screen - but TF's own read was "the unexposed main buffer is not a
  blocker." The thing being deferred (`decisions.md`) turned out not to be in the way.
- **Q6 (clock masking) confirmed needed**: the status row ends `...19:12` - a live clock, in the last 5 cells,
  exactly as suspected. TF will need row/regex masking for golden comparisons.
- **Two requests, handled differently, not both built in**:
  1. Plain-text `lines`-only output for quick eyeballing - small, clearly in scope, added: `--text` on the
     batch CLI (`architecture.md`).
  2. Row/regex masking - deliberately left as "a snapshot-comparison concern, not core `Tap`'s job" earlier in
     this document, and that holds: TF's harness already gets full structured `lines`/`cells` JSON, so masking a
     row range or regex against it is plain Python on their side, not a new `vtee` feature. Recommended that
     instead of building it in.
- **Not yet tried** (TF's own words): `send_key` (arrows, function keys, `/bind`), `resize` after the initial
  one, and the per-cell attribute fields (bold, colour) for attribute goldens - all built and unit/integration
  tested on this side, just not yet exercised against real TF.

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

## v1 (scoped, not built - the live/interactive half): crawl, walk, run

TF is one real consumer, asking from a testing perspective; the user is the other, asking from a "connect and
watch an active session" perspective (2026-10-07) - HANDOFF.md §1's third original use case ("letting an agent
watch while a human uses one"), now with an actual stakeholder behind it rather than a speculative one. Staged
by the user explicitly, in this order, after hardening:

1. ~~**Hardening**~~ **Done, 2026-10-07** (above) - first, regardless of consumer.
2. ~~**Observation**~~ **Done, 2026-10-07** (`vtee --listen <path>` / `vtee --connect <path>`,
   `architecture.md`): a separate process connects to an already-running `Tap` and reads live `snapshot`/
   `wait_for`/`wait_ms` state over a Unix domain socket, driving the exact same `Command` JSON the batch CLI
   reads from a script file. `Send`/`SendKey`/`Resize` are rejected with a clear error rather than ever reaching
   `execute` - read-only by design, so this didn't need the input-merging policy question below at all, only the
   wire protocol (now decided: a Unix socket, settling that open decision too). Verified end to end: a snapshot
   query, a write-command rejection, a `wait_for` match, and multiple commands over one persistent connection,
   all against a real running session.
3. **Full duplex**: splits into two genuinely different cases once actually scoped out. 3a is done; 3b (exclusive
   handoff, both `HumanPriority` settings) is also done, 2026-10-07.

### Full duplex

Splits into two cases that sound like one feature ("let a remote connection write too") but aren't, because only
one of them has an open question attached to it.

**3a. Remote-only control - no human competing.** ~~No policy question, buildable now~~ **Done, 2026-10-07**
(`vtee --listen <path> --writable`, `Command::authorize`, `WritePolicy`, `architecture.md`). When nothing else
is writing to the pty, a `--connect` client writing has no one to collide with. This is HANDOFF.md §1's *second*
use case, finally served live rather than only as a fixed batch script: "an AI agent see and drive a full-screen
program." Mechanically was almost free, as scoped: `Tap::send_bytes`'s shared, lock-guarded writer already
handled any number of concurrent callers without corruption; this was lifting the gate for connections that
asked for write access, behind a new server-side opt-in (`--writable`, default off, matching this project's
pattern everywhere else of defaulting to the narrower/safer behaviour). `Resize` got its own rule as scoped: a
remote `Resize` is rejected whenever the session is also interactive with a human whose real terminal drives
`watch_resize` (`resize_owned_by_terminal` in `ServeOptions`, set from the same `is_terminal()` check
`RawModeGuard` already uses) - there's only one real screen size that means anything once an actual human is
looking at one. Verified end to end: write rejected without `--writable`, accepted with it (text appeared on
screen), and a remote `Resize` actually changing the live session's size (80x24 to 100x40) when nothing else
owned it.

**3b. Human and remote both writing at once - the real open question.** Four shapes, not decided, genuinely the
user's call rather than an engineering one since it depends on how this actually gets used:

- **(a) Free-for-all.** ~~No arbitration~~ **Done, 2026-10-08 - turned out to already be built.** This is
  `WritePolicy::Open` (stage 3a) reused exactly as predicted below: `Command::authorize` never checked whether
  a human was also typing, so giving `--writable` (bare, i.e. `Open`) on an interactive, non-headless session
  already had no gate at all - confirmed, not assumed, with a real pty standing in for the human (`python3`'s
  `pty.fork()`, same technique as the signal/`Participant` tests): the simulated human types, a `--connect`
  client sends `send` concurrently with no `acquire_write` needed, and both land in the child. The existing
  lock already guarantees one write-call's bytes can't literally interleave with another's *bytes*, but says
  nothing about *order* or *intent* - a human mid-keystroke and an agent injecting at the same moment can still
  produce confusing, uncoordinated screen output (not corrupted, just badly timed). Fine if the real use is
  cooperative and low-frequency, wrong if it's "compete for the keyboard" - that tradeoff was always the
  point of making this a selectable mode rather than the only one.
- **(b) Exclusive/advisory handoff ("take the pen").** ~~A client must send an explicit acquire-write command
  before `Send`/`SendKey` are accepted~~ **Done, 2026-10-07** (`vtee --listen <path> --writable=exclusive` or
  `--writable=exclusive-shared`, `ExclusiveLock`, `Command::AcquireWrite`/`ReleaseWrite`, `architecture.md`). The
  server tracks one current holder (`ExclusiveLock { holder: Mutex<Option<u64>> }`, keyed by a per-connection id
  handed out by `Tap::serve`'s accept loop), auto-released on disconnect (`serve_connection` calls
  `release_if_held_by` on every exit path, not just a clean `ReleaseWrite`). `AcquireWrite`/`ReleaseWrite` reuse
  `CommandResult::WriteAccess { acquired }` for both a grant and a denial, and `CommandResult::Error` is reserved
  for a structurally-disallowed command instead. Matches how pair-programming/screen-sharing tools usually handle
  "who's driving" - whoever asked first keeps it until they let go. **Has its own sub-setting, also decided to be
  a choice rather than one fixed answer (2026-10-07)**: does the human's own typing ever need to hold this same
  lock, or does it always pass through unconditionally regardless of who (if anyone) currently holds it?
  `WritePolicy::Exclusive` carries a `HumanPriority` (`AlwaysWins` - the lock only arbitrates among remote
  clients, the human is never gated; or `Participant` - the human is just another holder that can be preempted)
  rather than picking one. Both are built and verified end to end, including `Participant`'s actual revoke path:
  a remote client acquires the lock (`write_access: true`), a human keystroke arrives on the real stdin-forwarding
  thread, and the remote client's next `Send` is rejected (`write access not held`) without it ever sending
  `ReleaseWrite` - confirming the keystroke itself revokes the lock rather than merely racing to be re-granted it.
  A `--connect` client can then reacquire once the human's own typing isn't actively holding it. Verified via a
  real pty (Python's `pty.fork()`, the same technique as the SIGTERM/SIGKILL watchdog tests) rather than named
  pipes - a FIFO-based harness hit two real ordering bugs of its own (opening the writer side too late, and
  insufficient settle time after unblocking it) before this project switched approaches; those were bugs in the
  test script, not in `vtee`.
- **(c) Human always wins.** A remote write is only accepted when the interactive stdin-forwarding thread hasn't
  seen a byte in the last N ms - an idle-based heuristic. **Not recommended**: this is the same shape of fragile
  primitive requirement 4 already rejected for *output* ("wait until the screen settles is the wrong primitive")
  applied to *input* instead - what counts as "idle enough" is just as arbitrary, and a human who pauses to think
  looks identical to a human who's finished typing.
- **(d) No simultaneous access at all.** `--writable` and a human typing become mutually exclusive *modes* of one
  `Tap`, not something that can both be true - starting with `--writable` disables stdin forwarding entirely.
  Simplest of all, but drops the "both at once" scenario completely; might be the honest answer if that scenario
  turns out to be nobody's actual need and (a)/(b)/(c) are solving a problem nobody has.

**Decided, 2026-10-07: not a single policy - all of (a)/(b)/(d) should be selectable modes** (the user's call,
made explicitly: "I think those should all be options"), and the same goes one level deeper for (b)'s own
`HumanPriority` sub-setting ("I think that's just one more setting/option, right?"). (c) stays excluded on its
merits (the idle-detection fragility argument above still holds regardless of this), but everything else that
sounds like a single policy call turns out to be a choice to expose, not a default to pick - this project settles
into "make it a setting" as its default answer to this category of question. This argues for shaping the
mechanism as an enum of write-policy modes from the start, even though 3a (no competing human) only exercises
the simplest one - so that 3b's remaining modes, and their own sub-settings, are additions to that enum later,
not a rework of it. (d) remains unbuilt - nothing about `WritePolicy`'s shape as an enum needs it built before
it's useful. (a) turned out to need nothing built at all, once checked - see above.

**Not new plumbing, whichever modes end up built**: the write mechanism (`Tap::send_bytes`, the shared writer
lock) already tolerates any number of concurrent callers; `Send`/`SendKey`/`Resize` already exist as `Command`
variants. **New regardless of which modes**: the mode selector itself, whatever state a given mode needs to track
"who's allowed to write right now" (nothing, for free-for-all; one `Option<ConnectionId>`, for exclusive handoff;
a boolean switch, for mutually-exclusive-with-the-human), and `Resize`'s own sub-rule once a human's terminal is
in the picture.

3a didn't need 3b resolved first, the same way observation (stage 2) didn't need either question answered -
`WritePolicy` already exists as the enum 3b's remaining modes extend, not rework (`WritePolicy::Open` is 3a's
mode; `(a)` free-for-all above is actually the same mode reused once a human's also typing, since nothing about
it is specific to there being no human - only `(b)` exclusive handoff and `(d)` mutually-exclusive need new
variants).

### Wait-for-predicate, beyond "text appears"

`Tap::wait_for_text`/`Command::WaitFor` only checks for a substring (built, `architecture.md`). A live/interactive
use might eventually want an arbitrary predicate over a full `Snapshot` (cursor position, a mode, a cell's
colour), not just text. The plumbing for that already exists too - `TapState`'s `Mutex` is already paired with a
`Condvar` that `forward_output` notifies after every chunk - so generalizing just means accepting a closure
instead of a fixed string, not new infrastructure. Not done because nothing's asked for it yet. Masking volatile
regions (a clock) for golden comparisons stays a separate, `Tap`-independent helper either way (TinyFugue's
answer 6).

### Wire protocol

~~What carries live `Command`s/`Snapshot`s between processes.~~ **Decided and built, 2026-10-07**: a Unix domain
socket with line-delimited JSON (what `ht` does, and what the `Command` design above already assumed) -
`Tap::serve`/`vtee --connect`, `architecture.md`. Considered and set aside: stdio-based JSON-RPC (conflicts
with stdin/stdout already being raw-forwarding channels, would need its own fd for no clear win); HTTP/WebSocket
(more flexible than anything here needs). On Windows the same design runs over AF_UNIX sockets (`uds_windows`, 2026-10-09; `architecture.md`, "Named sessions on
Windows"); named pipes were rejected.

## Later

An MCP front end (wraps whichever protocol v1 picks), Windows polish (ConPTY itself works now - see
`decisions.md`; still open: a terminal-restore watchdog and the VT console-mode flags for `attach`). Multiple sessions - **substantially addressed, 2026-10-08, without actually needing a
daemon**: named sessions, `vtee ls`/`attach`/`kill` (`docs/architecture.md`, "Named sessions and `vtee
attach`") give most of the practical value multiple-sessions-behind-one-process would have (discoverability,
attach-by-name, several clients per session) while staying one process per `Tap` - a socket per process really
was the simplest answer once sessions had names to find them by. Whether to ever build an actual daemon
multiplexing several `Tap`s behind one process remains undecided, and now looks like a "only if something
concrete needs it" question rather than a near-term one.

## Open decisions

- **Input merging** when both a human and an agent can type (stage 3b, "Full duplex" above): decided that
  free-for-all, exclusive handoff, and no-simultaneous-access should all be selectable modes rather than one
  fixed policy, and that exclusive handoff's own `HumanPriority` (always-wins vs. the human as just another
  participant) is likewise a setting, not a built-in answer (idle-based priority stays excluded on its merits
  either way). Exclusive handoff (both `HumanPriority` settings) and free-for-all are both built and verified
  (`--writable`/`--writable=open`); no-simultaneous-access remains unbuilt until something actually needs it.
  Still not decided: which mode and
  which `HumanPriority` are the defaults when none is requested (today: `ReadOnly` if `--writable` is absent at
  all, otherwise whatever `--writable=<mode>` names explicitly - there is no implicit default *among* the
  writable modes yet, since every caller so far has said which one it wants).
- **One process per session, or a daemon managing several?** Not decided. Leaning toward "not decided" staying
  fine for a while longer - `Tap::serve` already lets one process accept several concurrent observers, so the
  pressure to also multiplex several *sessions* behind one daemon hasn't shown up yet.
- **Licence.** `MIT OR Apache-2.0`, chosen 2026-10-10. `publish = false` stays in `Cargo.toml` until publishing to
  crates.io is decided separately.
- **Whether the name `vtee` is free to use.** Checked 2026-10-10: the crate name is free on crates.io and no GitHub
  project uses it for a terminal tool (searches turned up only empty or unrelated repositories).
