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

## Open

- **Protocol**, **input merging**, **licence**: unchanged from README's "Open decisions."
