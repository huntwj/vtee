# vtee

A terminal multiplexer built for AI and for testing. A Rust crate.

`tmux` lets a human juggle terminals. `vtee` runs a program in a pseudo-terminal, keeps the terminal's state in
memory, and lets a script or an AI agent query it and (later) send input, while a human, if there is one, uses
their own terminal as usual. Think of it as `tee` for a TUI: the stream is copied without disturbing the original,
but the copy is a screen you can query, not raw bytes.

**Status: nothing implemented yet.** This README is the plan. The name is a working name. `HANDOFF.md` has the
research, the context from TinyFugue, decisions made and not made, and working conventions: read it before starting.

## Why

It came out of testing TinyFugue (`~/ai/mud`). Its display code, status line and input editing cannot be tested
without a terminal, and the regression harness has no way to see one. The same tool would let an AI agent see and
drive a full-screen program, and let a human run something while an agent watches it.

## Requirements

1. **Driveable without a human, and also interactively.** Scriptable from CI inside a Linux container, and usable by
   an agent across many calls. The process model (library, daemon, socket) is a trade-off, not a rule.
2. **Broad, extensible emulation.** VT100 and VT220 up to the xterm superset: alternate screen, scroll regions,
   256-colour and truecolour, mouse reporting, bracketed paste, DEC special graphics, OSC (title, hyperlinks), wide
   and combining characters. We should not cap the target at what one program uses today.
3. **Full state exposure.** Every cell attribute (including dim, italic, strikethrough, underline styles), cursor
   shape and visibility, active modes, both screen buffers, scroll regions, scrollback, title.
4. **Synchronisation that does not rely on the screen going quiet.** A server may stream constantly. So: wait for a
   condition, synchronise on markers the test controls, snapshot "as of output byte N" so a query never sees a
   half-drawn frame, and mask volatile regions such as a clock.
5. **Raw-stream capture and replay.** Record the bytes the program wrote, and replay them into an emulator later, so
   a golden test is "emulator state after these bytes" and timing drops out.
6. **Deterministic.** Control over size, `TERM`, environment, working directory, resize and timing.
7. **Transparent observer mode.** The human keeps their own terminal and sees exactly what they would without the
   tool. Output is forwarded raw; the emulator only watches.
8. **Linux now, Windows later** (ConPTY).

## Not goals, for now

Writing a terminal emulator from scratch (reuse one), panes and windows, a status bar, or being a general `tmux`
replacement.

## Staged plan

- **v0, the tap.** Run a program in a pty, forward everything to the real terminal, keep emulator state, write a raw
  byte log, answer snapshot queries (cells, attributes, cursor, modes). Read-only for the observer.
- **v1.** Input injection from the observer, wait-for-predicate, a documented protocol.
- **Later.** An MCP front end, Windows, multiple sessions.

If v0 turns out harder than it looks, the fallback is to adopt an existing tool for the test side and keep only a
thin live-watching tap.

## Decisions

- **Emulator crate: `alacritty_terminal`**, decided 2026-10-06 by measuring all four candidates directly against
  requirements 2 and 3 (`docs/decisions.md` has the evidence). `avt`, `vte` and `termwiz` were ruled out: `avt`
  confirmed (from source) to expose no mouse/paste/keypad mode state, no title, no OSC 8 and no cursor shape; `vte`
  and `termwiz` are parsers, not emulators, and would need a VT100+ state machine built and maintained from scratch.

## Open decisions

- **pty crate.** `portable-pty` is the obvious candidate for requirement 8, not yet measured against a real
  spawn/forward/resize loop.
- **Protocol.** Probably line-delimited JSON over a Unix socket first, like `ht`.
- **Input merging** when both a human and an agent can type.
- **Licence.** Not chosen. `publish = false` in `Cargo.toml` until it is.

## Prior art

Surveyed 2026-10-06, from READMEs unless stated: `ht` (Rust, JSON over stdin/stdout, built on `avt`; owns a virtual
terminal, not a transparent proxy), `pilotty`, `agent-tui`, `NPCterm` (agent-oriented, daemon or MCP), `tui-test`
(Microsoft, multiple emulator backends, Windows), `pyte` and `pexpect` (Python), `Hecate` (tmux-based, old). None
documented a transparent observer mode, which is the gap this fills. An empirical probe of four of them (pyte, tui-test, ht, pilotty) is in `docs/probe-results.md`: pyte lacks the alternate
screen, and tui-test was the only one to pass the broad-emulation and full-state bar on every backend.
