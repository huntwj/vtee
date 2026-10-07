# Requirements

What `vtee` has to do, and what it deliberately doesn't. See `roadmap.md` for what's built versus planned, and
`architecture.md` for how the built parts work. Moved out of `README.md` 2026-10-06 once there was enough real
status to track per-requirement rather than as one blob.

## Must do

1. **Driveable without a human, and also interactively.** Scriptable from CI inside a Linux container, and usable
   by an agent across many calls. The process model (library, daemon, socket) is a trade-off, not a rule.
   **Met for the scriptable-from-CI case**: the batch CLI (`vtee --script`, `architecture.md`) runs a
   newline-delimited-JSON script of commands non-interactively and dumps structured results, built for and
   verified with TinyFugue's testing needs. Not yet met for "usable by an agent across many calls" in the sense
   of an ongoing, out-of-process session - that's the live protocol, v1 (`roadmap.md`).
2. **Broad, extensible emulation.** VT100 and VT220 up to the xterm superset: alternate screen, scroll regions,
   256-colour and truecolour, mouse reporting, bracketed paste, DEC special graphics, OSC (title, hyperlinks), wide
   and combining characters. We should not cap the target at what one program uses today.
   **Met**, inherited from `alacritty_terminal` (`decisions.md`) and exposed through `Tap::snapshot()`
   (`architecture.md`). Mouse/paste reporting is tracked as mode *state* but there's no input path to actually
   send mouse/paste sequences yet (v1).
3. **Full state exposure.** Every cell attribute (including dim, italic, strikethrough, underline styles), cursor
   shape and visibility, active modes, both screen buffers, scroll regions, scrollback, title.
   **Mostly met.** Open gaps: blink is structurally absent from `alacritty_terminal` (accepted, see
   `decisions.md`); underline *styles* (curly/double, not just on/off) aren't distinguished in the snapshot yet;
   **only the active screen buffer is exposed, not both** - deferred, 2026-10-07, a verified `alacritty_terminal`
   API gap rather than something to build around right now (`decisions.md`, `roadmap.md`).
4. **Synchronisation that does not rely on the screen going quiet.** A server may stream constantly. So: wait for
   a condition, synchronise on markers the test controls, snapshot "as of output byte N" so a query never sees a
   half-drawn frame, and mask volatile regions such as a clock.
   **Mostly met.** The byte-offset-stamped, race-free snapshot exists (`Tap::snapshot()`, `TapState`), and
   `Command::WaitFor`/`Tap::wait_for_text` (`architecture.md`) covers "wait for a condition" and marker
   synchronisation for the scoped-down case TinyFugue actually needs ("this text appears"), woken via a
   `Condvar` rather than polling. Open gaps: no arbitrary predicate beyond text containment (v1, `roadmap.md`),
   and masking volatile regions (a clock) doesn't exist yet - a snapshot-comparison concern, not `Tap`'s job.
5. **Raw-stream capture and replay.** Record the bytes the program wrote, and replay them into an emulator later,
   so a golden test is "emulator state after these bytes" and timing drops out.
   **Met.** `RawLog` (asciicast v2) + `replay()`, verified byte-for-byte against the live snapshot for all 15
   probe files (`tests/replay.rs`).
6. **Deterministic.** Control over size, `TERM`, environment, working directory, resize and timing.
   **Mostly met**: size and working directory are controlled by `Tap::spawn`; resize is handled live
   (`SIGWINCH`). **Not yet controlled**: `TERM` and arbitrary environment variables - `Tap::spawn` doesn't expose
   a way to set them on the child yet, so it inherits ours. Timing is irrelevant to replay (by design) but not
   yet controllable for the live tap itself (no artificial delay/throttling, not clear it's needed).
7. **Transparent observer mode.** The human keeps their own terminal and sees exactly what they would without the
   tool. Output is forwarded raw; the emulator only watches.
   **Met.** Raw bidirectional forwarding, `RawModeGuard` for the human's own terminal mode, confirmed not to alter
   anything forwarded (the emulator reads a copy, never the original bytes). Terminal restoration now survives
   `SIGTERM` and `SIGKILL` too, not just normal exit and panic unwind (`architecture.md`), verified on a real pty.
8. **Linux now, Windows later** (ConPTY).
   **Linux: met.** **Windows: blocked on an upstream bug** (`wezterm/wezterm#7025`), not just unimplemented -
   see `decisions.md`.

## Not goals, for now

Writing a terminal emulator from scratch (reuse one - done, `alacritty_terminal`), panes and windows, a status
bar, or being a general `tmux` replacement.
