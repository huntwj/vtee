# Decisions

Dated, one-way-door choices. See `HANDOFF.md` for the research behind each, `requirements.md` for what each is
judged against, and `architecture.md`/`roadmap.md` for how they turned into the current build and what's next.

## 2026-10-06: emulator crate is `alacritty_terminal`

**Measured**, not read. Four Rust crates were probed directly (not through a wrapper) against the same `T1`-`T14`
byte files already used for pyte/tui-test/ht/pilotty in `docs/probe-results.md`, fed straight into each crate's own
API at 40x12, graded by the same `docs/probe/probe_eval.py` judge. Full per-crate detail, evidence and raw results
are in `docs/probe/results_*.json` and `docs/probe/rust/*/`.

| | `vte` 0.15.0 | `avt` 0.18.0 | `alacritty_terminal` 0.26.0 | `termwiz` 0.23.3 |
|---|---|---|---|---|
| Category | parser only | full emulator | full emulator | parser + rendering toolkit |
| Score (of 15) | 13 PASS, 1 FAIL (T8), 1 NOT EXPOSED (T14) | 10 PASS, 3 PARTIAL, 1 NOT EXPOSED, 1 FAIL-free overall | **14 PASS, 1 PARTIAL** | 15 PASS, but that's our own ~450-line emulator on top of its parser |
| Mode state (mouse/paste/keypad/DECCKM) | hand-rolled | DECCKM only (confirmed absent from source) | full `TermMode` bitflags, all 5 | hand-rolled |
| Title / OSC 8 | hand-rolled | absent from source | yes (`EventListener` for title, `Cell::hyperlink()`) | hand-rolled |
| Cursor shape | hand-rolled | absent from source | yes | hand-rolled |
| Scrollback | not implemented (scoped out) | yes (`Vt::lines()` vs `Vt::view()`) | yes (`grid.history_size()`) | hand-rolled |
| Known gap | needed ~430 lines of our own state layer first | blink absent; combining chars shift a column | **blink absent from `Flags` entirely** (structural, confirmed in source) | needed ~450 lines of our own state layer first |

`alacritty_terminal` is the only one of the four that satisfies README requirements 2 and 3 (broad emulation, full
state exposure) through its own public API, with no emulator of our own to write or maintain. Its one gap — no
blink attribute — is a rare SGR code and is accepted as a known limitation rather than a blocker. `vte` and
`termwiz` are parsers, not emulators: both would require building and maintaining a real VT100+ state machine
ourselves, which the `vte` probe's own fork called "a multi-week undertaking, not a probe-sized one," duplicating
work these crates have already hardened. `avt` is close behind `alacritty_terminal` but confirmed (by source, not
just testing) to expose no mouse/paste/keypad mode state, no title, no OSC 8, and no cursor shape — all required by
README requirement 3.

**Decision: build on `alacritty_terminal`.** Revisit only if it proves unworkable in practice (e.g. API instability,
licensing surprise, or a real-program integration — TinyFugue — exposing a gap the probe didn't catch).

### Follow-up: what T1-T14 didn't cover, checked before committing

T1-T14 all feed one byte file in a single call, which doesn't exercise `vtee`'s real usage (reading a pty in
arbitrary OS-sized chunks) or robustness to malformed input (a raw pty stream can contain anything). Measured
2026-10-06, `docs/probe/rust/alacritty_terminal/src/bin/stress.rs`:

- **Chunked-feed equivalence**: all 15 tests, fed via `Processor::advance()` split into 1, 3, 7 and 64-byte chunks
  (60 cases total) instead of one call, gave byte-for-byte identical final screen text and cursor position to the
  single-shot feed every time. Includes multi-byte UTF-8 and escape sequences deliberately split across chunk
  boundaries at chunk_size=1. **0 mismatches.**
- **Malformed-input robustness**: 12 adversarial inputs (truncated CSI, truncated OSC with no terminator, truncated
  UTF-8 at every byte-width, invalid continuation bytes, a nonsense-huge numeric SGR parameter, 10,000 bytes of bare
  ESC, pseudo-random 4KB of binary garbage), each fed both whole and byte-at-a-time. **0 panics.**

**Code-review finding, not just testing**: `avt`'s Rust API only accepts `&str` (`Vt::feed_str`), not raw bytes — the
avt probe had to convert the `.bin` files with `String::from_utf8_lossy` before feeding. A real consumer reading
raw bytes off a pty would have to buffer and validate UTF-8 itself before calling in, and naive lossy conversion
would silently mangle a multi-byte sequence split across a read boundary. `alacritty_terminal`'s `Processor::advance`
takes `&[u8]` directly and assembles UTF-8 internally, which is exactly what the chunked-feed check above exercised
and confirmed correct. Another point in `alacritty_terminal`'s favor, found by reading the probe source rather than
by a failing test.

Not yet checked (deferred to the v0 tap build itself, since they're about vtee's own code more than the crate):
resize/reflow, and whether `Term::resize()` behaves sanely mid-stream. Also still open, as rubric gaps rather than
candidate-disqualifiers: SGR attribute-off/reset codes (22/23/24/25/27/29, DECSTR) aren't exercised by T2, and T2/T9
don't distinguish underline *styles* even though `alacritty_terminal`'s `Flags::ALL_UNDERLINES` suggests it tracks
more than one. Bell (`bl`) isn't tested at all despite being in TinyFugue's own termcap list (HANDOFF §3). None of
these are blockers; they're candidates for probe_eval.py additions if a real gap bites later.

## 2026-10-06: pty crate is `portable-pty`

**Measured**, by building v0's tap on it (`src/lib.rs`: `Tap::spawn`/`resize`/`wait`/`watch_resize`) and running it
for real, not just reading its docs.

**Linux (the target platform for v0, requirement 8): works.** All 15 probe files (`T1`-`T14`, `T7` split into
`T7a`/`T7b`) run through a real pty via `cat T*.bin < /dev/null` inside a `rust:slim` container, forwarding
correctly to stdout and producing the same `alacritty_terminal` state as the direct-feed probes in the emulator
decision above (confirmed by eye against `probe_eval.py`'s expected rows for T1, T5, T6, T7a/T7b, T9, T14; sizes
differ from the canonical 40x12 for tests not explicitly sized, which is expected). Two `SIGWINCH` signals sent to
a running tap mid-command caused no crash, deadlock, or error.

**Two real bugs found and fixed by actually running it, not by reading the API:**
- `alacritty_terminal::Term` panics (`index out of bounds: the len is 0`) on a 0x0 grid. `crossterm::terminal::size()`
  genuinely reports `0x0` when stdout is a tty that hasn't negotiated a real size yet (observed via
  `docker run -t` without `-i`) - a real, reachable condition, not a hypothetical. Fixed with a hard floor
  (`cols.max(1)`, `rows.max(1)`) in `Tap::spawn`/`resize`, and by treating a `0x0` read as "no usable size" (skip
  the resize, or fall back to 80x24) at the two call sites that read the ambient terminal size.
- The output-forwarding thread's `read()` on the pty master doesn't necessarily return EOF just because the child
  process has exited - `child.wait()` and "the master's reader gets EOF" are two different events that can be far
  apart in time. `Tap::wait()` no longer blocks on that EOF; it waits up to 500ms via a channel and moves on
  regardless, so a platform where that EOF is delayed or absent can't hang `wait()` forever.

**Windows (ConPTY): not usable, and it is not an MSYS2/Git-Bash artifact.** Spawning even a no-argument,
no-output native binary (`whoami.exe`) through `portable-pty`'s ConPTY backend consistently exits with
`STATUS_CONTROL_C_EXIT` (`0xC000013A`) within milliseconds, never producing the program's real output - only
ConPTY's own injected startup/teardown sequences (`win32-input-mode` query, focus tracking, etc.) were seen.
**Measured 2026-10-06 from genuine PowerShell** (not through this session's Bash tool/MSYS2): identical failure,
byte-for-byte, ruling out the MSYS2-hosting theory entirely.

This is a known, open, upstream bug, not something ours: **[wezterm/wezterm#7025](https://github.com/wezterm/wezterm/issues/7025)**,
"Portably pty fails to launch commands on windows," filed against the same crate with the same trivial `whoami`
repro and the same exit code, unresolved, zero comments, and reported as machine-dependent (the reporter's
colleagues don't see it on their machines) - consistent with what we saw. Reading `portable-pty` 0.9.0's actual
`CreateProcessW` call (`src/win/pseudocon.rs`, confirmed unchanged on wezterm's unreleased main branch too, so
there's no newer release to upgrade to that fixes this): the creation flags are only
`EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT` - **no `CREATE_NEW_PROCESS_GROUP`**. Without that flag
a spawned child inherits the parent's process group, and Windows delivers console-control events (Ctrl+C/Break)
by process-group membership, not strictly by which console a process is visibly attached to - so even though
ConPTY attaches the child to its own hidden console, a break event aimed at the parent (or generated during
normal console setup/teardown) can still reach it. Plausible explanation for the machine-dependence too: whatever
makes one machine's ambient console/process-group state trigger this and not another's would vary host to host.
Not confirmed by a patched build - that would be the next step if this gets picked up, not read-the-source
speculation. Not investigated further now, consistent with requirement 8 deferring Windows.

**Decision: build on `portable-pty`.** Its Linux backend is solid and the issues found were in our own glue code,
not the crate. The Windows gap is real but expected and explicitly out of scope until the Windows work starts.

**2026-10-08: the patch was tried, and it's a real, confirmed, partial fix - not a full fix.** Vendored
`portable-pty` 0.9.0 into `vendor/portable-pty/` (self-contained once published - its registry `Cargo.toml` has
no path deps on sibling `wezterm` crates, confirmed by reading it, so no need to fork the whole monorepo),
added `CREATE_NEW_PROCESS_GROUP` to the one `CreateProcessW` call, wired in via `[patch.crates-io]`.
**Confirmed fixed**: spawning `whoami.exe` natively on Windows no longer dies with `STATUS_CONTROL_C_EXIT`
within milliseconds - `tasklist` shows the child process actually alive and running, where before it was gone
almost instantly. That's the exact, specific bug this entry diagnosed, and it's gone.

**But that wasn't the end of it.** With the crash fixed, a second, separate issue surfaced: the child's
(forwarded) output included a bare cursor-position query (`ESC[6n`, DSR) that ConPTY itself issues toward
whatever's on the other end of its pty, expecting a reply via stdin - and with no real terminal actually
attached in the test harness used to check this (`PowerShell`'s `Start-Process` with redirected pipes, not a
real console window), nothing ever answers it, so the session hangs waiting for a reply that will never come.
This is very likely *not* a new bug introduced by the patch, and probably resolves itself when run from an
actual terminal window (a real terminal answers a DSR query automatically, and `vtee`'s own `forward_input`
would relay that reply to the child exactly like any other keystroke) - but that's reasoning about what
*should* happen, not something verified, because nothing in this tool-driven environment can attach a real,
interactive Windows console to test it.

**Closed the loop, 2026-10-08, by the user running it from a real terminal**: `.\target\debug\vtee.exe --
whoami.exe` from an actual PowerShell window printed `whoami`'s real output (`HOST\user`) and returned to the
prompt cleanly - no hang, no crash. Confirms the hypothesis above exactly: a real terminal answers the DSR
query automatically, `forward_input` relays that reply to the child like any other keystroke, and the session
completes normally.

**Then automating that same check surfaced a residual, probabilistic rough edge.** Built an automated
regression test (`tests/windows_conpty.rs`) that opens a real ConPTY pty itself (via our own patched
`portable_pty`) and acts as a minimal terminal - answering `ESC[6n` the same way a real one would - so this
doesn't need a human each time. It hung the same way the manual pre-patch testing once did, but only
*sometimes*: identical back-to-back runs (`cargo test --test windows_conpty`, same machine, same binary)
passed, passed, then hung on the third. Found that the calling process having no console of its own makes
`openpty()` itself unreliable, independent of anything `vtee` does - confirmed by having both the test and
`Tap::spawn` call `AllocConsole()` defensively first (a no-op failure if one already exists; gated on
`SpawnOptions::forward_output` in `Tap::spawn` specifically so `--headless`/`--script` never risk a surprise
GUI window appearing during otherwise-automated/CI usage). That measurably helped - the failure rate dropped -
but did not eliminate it: three identical runs after the fix went 2 passed, 1 hung. This matches the
underlying GitHub issue's own description of "machine-dependent" almost exactly, just one level deeper -
apparently extending to run-to-run variance on a single machine, not only machine-to-machine.

**Asked directly why the residual flakiness couldn't just be fixed - investigated further rather than
shrugged at, and ruled out two concrete, plausible causes with actual evidence, not assumptions.** Added
diagnostics (`eprintln!` in the test's own responder) to see, on every run, whether the query was actually
seen and answered. Both hypotheses turned out wrong:
- *Could the query arrive split across two separate `read()` calls*, with the responder checking only the
  current chunk in isolation and missing a sequence straddling two reads? Fixed anyway (checking the full
  accumulated buffer is strictly more correct regardless), but the diagnostics show the query is always seen
  intact, as the very first 4 bytes read, in every single run - pass or fail alike. Not the cause.
- *Could this test's own nested ConPTY sessions* (the outer one hosting `vtee.exe` itself, created by the
  test's own `openpty()`; the inner one `vtee.exe` creates internally via `Tap::spawn` to host `whoami.exe`)
  *each emit their own independent startup query*, with the test's original single-shot "already answered"
  guard silently swallowing a second one? Fixed anyway (now replies to every occurrence found, not just the
  first), but the diagnostics show only one query ever appears at all, in every run - pass or fail alike. Not
  the cause either.
- The failure rate itself bounced around across batches of identical runs with *no code change between them*
  (2 of 3 failed, then 4 of 5, then 1 of 6) - the signature of a genuine lower-level race, not a deterministic
  logic bug sitting at the application level where this test (or `vtee`) can see or influence it.

**Status, precisely**: the originally-diagnosed crash (`STATUS_CONTROL_C_EXIT` within milliseconds) is fixed
and verified with no flakiness observed in re-testing specifically that. The user's own real-terminal use
worked cleanly, and this is the realistic case - that test harness itself (deliberately constructing an
artificial pty-within-a-pty to automate what a human did manually) may be exercising a Windows console corner
nobody's actual usage looks like. A separate, deeper ConPTY-creation reliability issue is real, measured, and
- after genuinely trying, not just asserting it's unfixable - not something pinned down at the application
level: both plausible, checkable causes were ruled out with diagnostic evidence, and what's left appears to
be lower in the stack than anything in this codebase or test harness can observe, let alone fix, without
tooling this project doesn't have (e.g. ETW tracing of the console subsystem). `tests/windows_conpty.rs` stays
`#[ignore]`d by default for exactly this reason (an unstable, data-free-to-explain failure rate is unfit as a
normal gate) rather than claimed as passing. Native Windows interactive use is meaningfully better than
"blocked," genuinely not yet "fully solved." `vendor/portable-pty/README-vtee-patch.md` has the
`CREATE_NEW_PROCESS_GROUP` patch's own rationale and the removal plan once upstream fixes #7025 for real; the
`AllocConsole` mitigation is a separate, independent finding layered on top of it, not part of that same
upstream issue.

## 2026-10-07: "both screen buffers" (requirement 3) deferred - a verified `alacritty_terminal` API gap

**Measured against the crate (0.26.0) and alacritty's own current upstream `main` branch** (not just assumed):
there is no public way to read the inactive screen buffer's content without destroying it. `Term` exposes
`grid()`/`grid_mut()` for the *active* buffer only; the one way to reach the other, `swap_alt()`, is not a safe
peek. Traced through its actual logic: swapping from the primary screen onto the alternate one always resets
the alternate buffer first (`self.inactive_grid.reset_region(..)`), matching real terminal behaviour (a DEC
alternate screen always starts blank on entry) but meaning any "peek at alt" also wipes it. Worse, a "peek, then
swap back" round trip isn't safe either: swapping back from a now-active primary screen onto the alternate one
re-triggers that same reset branch, destroying whatever was genuinely sitting in the alternate buffer. There is
no direction this is safe in every case, and no accessor that avoids `swap_alt()` entirely.

Real options considered: (a) defer and document - `alacritty_terminal` simply doesn't expose this safely yet,
same category of gap as the Windows ConPTY bug, not a flaw in this project's own code; (b) vendor a one-line
local patch adding `pub fn inactive_grid(&self) -> &Grid<Cell>` (behaviour-free, low risk, but a fork to
maintain); (c) reconstruct it ourselves by snapshotting before every chunk to catch the alt-screen transition,
expensive and not something anyone has actually asked for yet.

**Decision: defer (a), stay on upstream `alacritty_terminal` with no local patches.** TinyFugue's own answer on
this (`roadmap.md`) was "I'll find out if it matters from a first snapshot" - which only needs the active buffer
this project already exposes, not a confirmed present need for the inactive one. Keeping everything on the
unmodified upstream crate avoids taking on a fork to maintain for a need that hasn't materialized. Revisit via
the fork route (b) specifically - not (c) - if TF's first real snapshot shows TF actually uses the alternate
screen and the main-screen content underneath genuinely needs inspecting.

**Confirmed right, 2026-10-07, by TF's first real run** (`roadmap.md`): `alt_screen` was `True` with TF's own
startup banner and status line rendered in it. TF does use the alternate screen - but that's exactly the buffer
this project already exposes, and TF's own read of the result was "the unexposed main buffer is not a blocker."
The gap this entry defers turned out not to be in the way of the thing it was theoretically about. No patch
needed; closing the loop here rather than leaving it as a live risk.

## 2026-10-08: `--listen` with no path defaults to tmux's own socket-security model

**Gap found while being asked** "how is security handled for this?" - measured, not assumed, before answering:
`UnixListener::bind` creates the socket file with whatever the process umask produces (verified in the Linux
container: `srwxr-xr-x` under the default `umask 0022`), and `vtee` did nothing to narrow that. There is no
application-level authentication at all - `WritePolicy` governs what a connection may do once connected, not who
may connect; under `Exclusive`, the per-connection id used to arbitrate the write lock is sequential, not a
secret. Whoever could reach an explicit `--listen <path>` could do anything the server's policy allowed.

**Looked at how `tmux` handles the same problem** (confirmed against its actual source, not recalled from
memory - `server.c`): it isn't socket-file permissions either. `tmux` puts its socket in a per-UID directory
(`$TMPDIR/tmux-<uid>/`) and, before using that directory, checks that it's owned by the current user and that
group/other have no permission bits at all - refusing to start ("unsafe permissions") if an existing directory
fails either check. The protection is blocking *traversal into the directory* for anyone but the owner, not the
socket file's own mode bits - which also defeats a `/tmp` symlink-or-pre-plant race, where another local user
pre-creates that directory before the real one ever runs `tmux`.

**Decision: the same model, scoped to the default case only.** `vtee::default_socket_path()` (`src/lib.rs`)
creates `$TMPDIR/vtee-<uid>/` with mode `0700` passed directly to the `mkdir` call (no window where it exists
with looser permissions), or - if the directory already exists - verifies rather than trusts it: refuses to use
it if it isn't a plain directory (`symlink_metadata`, so a symlink is rejected the same way a plain file would
be), isn't owned by the current UID, or has any group/other bit set. `--listen` with no path argument
(`main.rs`'s `take_listen_flag`) resolves to this; `--listen <path>` is unchanged and stays the caller's own
responsibility, exactly as `tmux -S <path>` does for its own explicit paths - this only hardens the default,
not every possible invocation. The socket inside that directory is named by pid (`<pid>.sock`), not a fixed
name like tmux's `default` session, since nothing here manages multiple named sessions behind one directory yet;
`Tap::serve`'s existing stale-socket removal (`docs/architecture.md`) already covers a pid getting reused after
a killed session left its file behind.

Verified end to end in the Linux container: a bare `--listen` creates `/tmp/vtee-<uid>/` as `drwx------`; an
explicit `--listen <path>` is byte-for-byte the same as before; a pre-existing world-writable directory at the
default path is refused (`unsafe permissions`, exit code 71); a pre-existing directory owned by a different
user is refused (`isn't owned by the current user`, exit code 71).

**Not solved by this**: an explicit `--listen <path>` still has no protection at all - that remains the caller's
job, same as it was. There's still no way for a client to prove *who* it is once connected; this closes the
"who can reach the socket at all" gap, not an authentication gap on top of that. Nobody's asked for the latter
yet.

## 2026-10-08: observation is on by default now; the exit-time snapshot dump isn't

Two related corrections prompted by the same question - "what does plain `vtee -- vim` actually gain you over
running `vim` directly?" - asked in direct response to an honest answer about what the previous defaults did.

**The exit-time screen dump was never a real requirement.** Traced to the very first commit, before TF had
asked for anything - debug scaffolding to prove `Tap` was tracking state correctly, left on by default ever
since. For a human running something interactively, it's also actively redundant: `forward_output` already
rendered the real final screen to their actual terminal, live: the dump just repeats that, in uglier
`Debug`-escaped form, after the fact. **Decision: opt-in via `--dump-on-exit`, off by default.** Nothing in
`requirements.md` calls for it; `--script`'s structured results and `--listen`'s on-demand queries are the
real answers to "give me the state," not an unconditional dump nobody consuming `vtee` programmatically
asked for.

**Observation, on the other hand, turns out to be the actual common case - so make it the default.** The one
use case that exists today (TF's testing) never touches `--listen` at all; the one described but unbuilt
(watching a live session, `architecture.md`/`roadmap.md`) is the user's own stated reason for being "the other
consumer" of this project. Once `default_socket_path` made a default location safe to bind
(above), the cost of always having something to attach to dropped to "one background thread and one socket
file," while the value - any `vtee` session being attachable without having to restart it with `--listen`
in hindsight - is real. **Decision: `--listen` is implicit now, `WritePolicy::ReadOnly`, at the hardened
default path, unless `--no-listen` is given** (added specifically for a CI script driving `vtee` through
plain stdin/stdout, which has no use for an attachable socket and would rather not have one appear in a shared
`/tmp`). `--listen`/`--listen <path>` still work exactly as before and still mean what they meant; `--writable`
is still required to get anything beyond read-only, unchanged. The *implicit* default's own
`default_socket_path` failing (not Unix, today) is **not** treated as fatal - that would turn "you get
observation for free" into "the program won't even start" on a platform that can't have it yet, which is a
worse trade than just not being attachable. An *explicit* `--listen` failing the same way stays a hard error,
since the user asked for it by name.

Verified end to end in the Linux container: a plain run with no flags creates the default socket and prints
the same "listening for observers" line `--listen` always has; `--no-listen` suppresses it entirely; passing
both `--listen` and `--no-listen` is a usage error (exit code 64) rather than silently picking one.

## 2026-10-08: `--headless` (the third primary use case) and `--cols`/`--rows`

Prompted by actually naming the use cases this project serves: automated tests (`--script`), an interactive
session with a human and an optionally-attached AI (`--listen`, the default now), and an AI-only session with
no human at all. The third one already technically worked - `RawModeGuard`/`forward_input`/`forward_output`
already no-op correctly when stdin/stdout aren't a real tty - but only as an *accident* of however the process
happened to be launched, never a thing the CLI itself knew about or declared.

**Decision: `--headless` makes it explicit.** Forces `forward_input`/`forward_output` off outright (not just
stdin - there's no stdout worth forwarding to either, since the AI side is meant to use `--listen`/snapshot),
skips `RawModeGuard`/the SIGKILL watchdog and `watch_resize` entirely rather than leaving it to `is_terminal()`
to decide, and forces `resize_owned_by_terminal` false so a remote `resize` is never rejected as "owned by the
real terminal" when there isn't one. Refuses to start combined with `--no-listen` - a headless session with no
forwarding and no socket would be unreachable by anything, human or AI, which is never what's wanted.

**`--cols`/`--rows`** override the virtual tty's size independently of whatever the real terminal reports (or
the 80x24 fallback) - the first of what's meant to grow into a fuller virtual tty spec per the user ("width and
height are the obvious starters, but terminal emulation modes, color support, and other things might be good
down the road"); no abstraction for that was built yet, just the two flags actually asked for.

Verified end to end in the Linux container: `--headless --no-listen` together is a usage error (exit 64);
`--headless` alone still gets the default socket (read-only); a remote `resize` succeeds under `--headless`
where it would be rejected with a real terminal attached; `--cols 100 --rows 40` is reflected in a live
`snapshot`'s `cols`/`rows` fields over `--connect`.

## 2026-10-08: adopting the Rust CLI ecosystem's standard crates

Prompted by a direct question ("are you using standard rust crates... clap, color_eyre, etc.?") - the honest
answer was no, the CLI was entirely hand-rolled (`take_path_flag`, `take_switch`, a hand-written `--listen`
optional-value parser, a hand-written `--cols`/`--rows` numeric parser), which was a reasonable call at two or
three flags but had stopped being one: adding `--headless` just above needed its own hand-written conflict
check, and the growing flag list had already pushed `run_interactive`'s own signature past
`clippy::too_many_arguments` (fixed separately by bundling them into `InteractiveOpts`). Checked "Command Line
Applications in Rust" (the Rust CLI working group's book, `rust-cli.github.io/book`) against what this project
actually needs rather than adopting its whole list:

- **`clap`** (derive): replaces every hand-rolled flag parser. `--listen`/`--writable`'s optional-value shape
  (`num_args(0..=1)`, `require_equals(true)`, `default_missing_value(...)`) turns out to be exactly `grep
  --color[=WHEN]`'s own shape, and POSIX's `getopt_long(3)` is explicit that an optional-argument long option
  *requires* the `=` form - confirmed against the actual man page, not assumed. **Measured, not assumed, bug
  found along the way**: `default_missing_value("")` (empty string, for "bare `--listen` means use the
  default") was silently rejected by `clap` - it falls back to demanding a value rather than treating empty as
  the "no value given" sentinel. Fixed with `"\0"` instead (a real path can never contain a NUL byte, on any
  platform this runs on). **Second fix, also found while verifying this**: `Cli::parse()` exits with code `2`
  on a bad argument - `clap`'s own convention, not this project's `exitcode`/sysexits one adopted earlier this
  session specifically for consistency. Switched to `Cli::try_parse()` and route a real error through
  `exitcode::USAGE` (64) via `clap::Error::exit_code()` (0 for `--help`/`--version`, which stay on their own
  correct exit code and stream via `Error::print()`), so every usage error in this CLI still means the same
  thing regardless of which layer caught it.
- **`human-panic`**: a friendly crash report instead of a raw backtrace - relevant here specifically because
  the real terminal can be in raw mode when a panic happens. Not fully verified (no panic was deliberately
  triggered to check it) - a known gap in how far this was tested, not a claim it's flawless. One real caveat
  worth being honest about: `human-panic`'s own message prints the instant the panic hook runs, *before* stack
  unwinding drops `RawModeGuard`, so in the rare case of an actual panic during an interactive session that
  message can still land on the same raw-mode terminal the child is drawing on - better than a bare backtrace,
  but not the same clean separation the fix below gives ordinary error paths.
- **`log` + `env_logger` + `clap-verbosity-flag`**: leveled, `-v`/`-vv`-controlled diagnostics (connection
  accept/disconnect, `acquire_write`/`release_write`/revoke events in `serve_connection`/`forward_input`) where
  previously there was none. **The actual reason this needed its own design, not just "add a logging crate"**:
  during an interactive session with a real terminal, `vtee` only ever shows the *child's* screen - there is
  no safe moment to write anything of our own to that same stderr without interleaving with it. Tracing this
  through surfaced a real pre-existing bug: the `--listen` setup-failure message was already being printed via
  plain `eprintln!` *after* `RawModeGuard` had put the terminal in raw mode, with the child potentially already
  drawing - fixed by dropping the guard first, restoring cooked mode, *then* printing. Generalized into a rule:
  diagnostic/non-fatal messages (the "listening for observers" notice, all new `log::info!`/`debug!` calls) go
  to a file beside the session's own socket (`default_log_path`, sharing `default_socket_path`'s hardened
  per-uid directory via a new shared `default_session_dir` helper) whenever a real terminal is actively showing
  the child (`!headless && stdin is a tty`); fatal/abort messages stay on stderr directly, since the process is
  exiting right there regardless. `env_logger`'s `write_style(Never)` is forced for the file target specifically
  so colour escapes never land in a log file - verified nothing is misdetected by actually reading the file's
  bytes (`cat -A`), not just eyeballing it.
- **`owo-colors`** (`supports-colors` feature): status/error output coloured (red for `print_error`, dimmed for
  the one pre-session `print_info` notice) only when the destination stream actually supports it -
  auto-detected per-stream, so piped/redirected output and the log file both stay plain text. Verified by
  piping stderr to a file and checking for literal escape bytes (`cat -A`): none.
- **`assert_cmd` + `predicates`**: replaces the ad-hoc shell-script-in-Docker approach for CLI-*argument*
  behaviour (conflicts, bad values, the `--listen`-with-a-space pitfall) with real `#[test]`s
  (`tests/cli.rs`) that run on every platform, since `clap` validates before any pty is ever touched. Tests
  that actually spawn a child stay Unix-only, same reasoning as `tests/replay.rs`. **Found and fixed a test
  bug, not a product bug, while writing these**: the Unix-only tests initially shared one fixed default
  session directory and raced each other under `cargo test`'s default parallelism (`cargo_test` runs tests
  concurrently) - fixed by giving each test its own `TMPDIR`, the same isolation a real caller would use, not
  by serializing the tests.
- **Looked at and declined**: `anyhow`/`color_eyre` (this project's `main` deliberately returns `ExitCode`
  per-call-site rather than bubbling a `?`-propagated `Result`, which is the problem those crates solve);
  `indicatif` (nothing here has a progress-bar-shaped operation); `confy` (no persistent config exists);
  `tokio` (would mean rewriting the thread-per-connection model for no current need); `ctrlc` (redundant with
  `signal-hook`, already in use for the SIGHUP-immunization/watchdog tricks `ctrlc` doesn't offer); `ansi_term`
  (the book's own pick for colour, but archived/unmaintained - `owo-colors` above instead).

## 2026-10-08: antagonistic review of the day's work - one real CLI bug found and fixed

Four adversarial reviews, each explicitly briefed to try to disprove this project's own claims rather than
confirm them, covering: the socket-security hardening, `ExclusiveLock` concurrency and `--headless`'s
guarantees, and the new `clap` CLI surface. **Three areas held up; one didn't.**

**Socket security (default per-UID directory)**: every attack scenario tried - TOCTOU between the directory
check and `mkdir`, two same-uid processes racing, a symlink pre-planted at the predicted `<pid>.sock` path,
files already present inside the directory - either fails closed (the losing side of a race just gets "not
attachable," never a trusted-but-wrong directory) or is foreclosed structurally by the 0700 directory gate
before it's even reachable (a non-owning attacker has no `x`/`w` on the directory at all, so pid predictability
is irrelevant). One honest caveat the docs didn't state outright: this assumes a sticky-bit temp directory
(true of `/tmp` by default) and that root always bypasses Unix permission checks (standard, not a flaw here).

**`ExclusiveLock` concurrency and `--headless`**: both held up. Acquire/release/revoke are each one
operation under a single mutex, so two connections racing `AcquireWrite` cannot both win, and id reuse needs
2^64 connections to even become a question. The one real nuance - a remote write can land in the microsecond
gap between a human's keystroke landing and `revoke()` running, since those are two separate critical
sections, not one atomic unit - is a property of a *reactive* lock, not a broken promise: the design never
claimed byte-level-atomic preemption, only "a remote client finds out by its next command failing," which
still holds. `--headless` was verified to genuinely skip `RawModeGuard`/the watchdog/`watch_resize` outright
(not just leave them to no-op), not merely claim to.

**The `clap` CLI surface had a real, confirmed bug**: `allow_hyphen_values = true` on the trailing `command`
positional made clap treat *any* unrecognized leading flag - including `--version`/`-V` - as the start of the
program to run rather than rejecting it, so a typo'd `vtee` flag silently became "the program to run" and
failed with a confusing pty-spawn error instead of clap's own "unrecognized argument" message.
**Fixed by dropping `allow_hyphen_values`** - `trailing_var_arg` alone still lets a hyphen-prefixed argument
*after* the program name pass through untouched (verified: `vtee -- cat -n` and `vtee cat -n` both still
reach `cat` correctly, with or without the `--`), it just no longer greedily starts the positional at an
unrecognized flag *before* any program name has appeared. Investigating this also surfaced that `--version`
had never actually been enabled at all (`clap::Parser` requires the explicit `#[command(version)]` opt-in,
confirmed against its own docs) - added, so `--version`/`-V` now print `vtee 0.0.0` and exit 0 rather than
erroring either way.

**Two smaller, real inconsistencies fixed alongside it**: `--writable` had no `conflicts_with` tying it to a
socket existing at all, unlike `--headless`+`--no-listen` which was already validated - `--writable --no-listen`
used to be silently accepted and silently inert; now rejected the same way. `--script` silently accepted and
silently ignored every interactive-only flag (`--headless`, `--listen`/`--no-listen`, `--writable`,
`--no-stdin`, `--dump-on-exit`) since `run_script` never reads any of them - now `conflicts_with_all` on
`--script` rejects the combination instead, on the principle that accepting a flag `run_script` can't possibly
honor is misleading, not lenient.

All three fixes have regression tests in `tests/cli.rs` (`unknown_flags_are_rejected_not_swallowed_as_the_command`,
`version_flag_is_handled_by_clap_not_swallowed`, `writable_and_no_listen_conflict`,
`script_conflicts_with_every_interactive_only_flag`, plus a Unix-only
`hyphen_flags_after_the_program_name_still_pass_through` confirming the fix didn't break the feature
`trailing_var_arg` exists for). Full suite re-verified in the Linux container and natively on Windows after
every change in this entry.

## 2026-10-08: "free-for-all" (stage 3b option (a)) needed nothing built - confirmed, not assumed

The user asked for this mode specifically; `roadmap.md` had already predicted it would turn out to be
`WritePolicy::Open` reused as-is once a human is also typing, since `Command::authorize`'s `Open` arm never
checked whether anyone else was typing - but that was a read of the code, not a live test with an actual human
present. Verified for real: a simulated human (Python's `pty.fork()`, the same technique as the signal and
`Participant` tests) types into the real pty while a separate `--connect` client sends a `send` command with no
`acquire_write` - both land in the child. (First run of this test showed the remote write missing entirely; the
cause was too-short sleeps in the *test script* racing the connect against the server still starting up, not an
`vtee` bug - same class of mistake as the FIFO-timing lessons from the `Participant` verification earlier.)

No code changed. `roadmap.md` updated to mark (a) done alongside (b)/`HumanPriority`, leaving only (d)
(mutually-exclusive-with-a-human) as the one remaining unbuilt mode of `WritePolicy`.

## 2026-10-08: `--env KEY=VALUE` - requirement 6's last unmet piece

Asked directly, decided to build now rather than keep deferring. Measured the actual crate before writing
anything: `portable_pty::CommandBuilder` (the vendored source, `cmdbuilder.rs`, not assumed from memory) already
seeds its environment from `std::env::vars_os()` with `is_from_base_env: true`, and `.env(key, value)` simply
overrides/adds on top (`is_from_base_env: false`) without needing `env_clear()` first - exactly the
"inherit by default, override selectively" shape this needed, already built into the dependency.

`SpawnOptions` gained `env: &'a [(String, String)]` (default `&[]`, so existing callers are unaffected);
`Tap::spawn` just loops it into `cmd.env(k, v)`. `Cli` gained `--env KEY=VALUE`, repeatable, splitting on the
*first* `=` only so a value that itself contains `=` (e.g. a `PATH`-shaped override) isn't truncated or
rejected. Shared by every mode - unlike `--cols`/`--rows`, which `run_script` also takes but for consistency
rather than because the option itself is mode-specific, `--env` is equally meaningful everywhere a child gets
spawned at all. No dedicated `--term` flag: `TERM` is just another entry through this same mechanism, not a
special case - requirement 6 lists it separately from "environment" but there's no reason the implementation
should.

Verified end to end in the Linux container: a single `--env`, several at once, a value containing `=`
(`a=b=c` survives intact), overriding an inherited variable (`TERM=dumb` reaches the child, confirmed via
`printenv`), a malformed value with no `=` at all rejected with a clear usage error (exit 64), and the same
flag working identically under `--script` (`printenv` inside a scripted snapshot showed the overridden value).

## 2026-10-08: tmux as the explicit reference model for named sessions and attach

Decided, when asked to design this rather than just build the first thing that worked, to follow tmux's own
precedent deliberately rather than invent new conventions - checked against tmux's actual source/docs at each
fork, not recalled from memory:

- **A session name defaults to something, and a second one with the same name is a hard error** - tmux's own
  `new-session -s NAME` behaviour exactly (`duplicate session: NAME`, no auto-suffixing, no silent attach).
  Considered and rejected: auto-suffixing (`tf`, `tf-1`, `tf-2`, ...) - tmux itself doesn't do this for named
  sessions (only for its own anonymous numbered ones), and it would make `attach <name>` ambiguous about which
  one you meant.
- **`--attach-if-exists` is tmux's own `-A`** (`new-session` behaves like `attach-session` if the name already
  exists) - adopted directly, plus made configurable as a standing default via a new config file, which tmux
  itself doesn't offer but was asked for specifically and fit the same "make it a setting, not a fixed answer"
  pattern this project has used throughout (`WritePolicy`, `HumanPriority`).
- **Bare `attach` (no name) resolves the way tmux's own bare `attach-session` does**: the only live session if
  there's exactly one, else the most-recently-active. "Most-recently-active" is approximated by the pidfile's
  own mtime - a reasonable, stated proxy, not a reproduction of tmux's own richer activity tracking.
- **Detach deliberately has *no* keybinding**, the one deliberate departure from tmux rather than a borrowed
  precedent. tmux needs a prefix key (Ctrl-b `d`) because its server requires an explicit detach signal; here,
  the holder's lifetime depends on the child process, not on any client connection, so an attach client ending
  for *any* reason (Ctrl-C, closing the window) already is detaching, with nothing else to signal. Keeping that
  simplicity was judged more valuable than literal tmux-parity - it preserves requirement 7's "fully
  transparent passthrough" promise instead of stealing a keystroke from the child to implement a feature that
  doesn't structurally need one here.

**The one design that was tried and reverted during implementation, not just considered and declined**: making
the *originating* `vtee <program>` invocation's own display self-connect to its own new TTY socket, "instead
of" its existing direct `forward_input`/`forward_output`, so every client (including the original one) would
be symmetric. Implemented, then reverted on realizing the raw byte protocol has no framing for
`acquire_write`/`release_write` - `WritePolicy::Exclusive` can't mean anything there, so going fully symmetric
would have silently retired the already-built-and-tested `HumanPriority`/exclusive-handoff machinery (stage
3b) the instant a human attached via TTY instead of owning the original stdio. Kept the direct path unchanged
for the originating invocation and made the new socket purely additive instead (`architecture.md` has the
full mechanics) - a case where the architecturally "purer" design turned out to be the wrong one once actually
built and checked against what it would cost, not just how it looked on paper.

## Open

- **Protocol**, **licence**: unchanged from README's "Open decisions."
- **Input merging**: substantially decided and built 2026-10-07 (exclusive handoff, both `HumanPriority`
  settings - `roadmap.md`/`architecture.md`). Still open: free-for-all and mutually-exclusive-with-a-human as
  unbuilt modes of the same enum, and which mode/`HumanPriority` is the default when none is requested.
