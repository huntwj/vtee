# Requirements

What `vtee` has to do, and what it deliberately doesn't. See `roadmap.md` for what's built versus planned, and
`architecture.md` for how the built parts work. Moved out of `README.md` 2026-10-06 once there was enough real
status to track per-requirement rather than as one blob.

## Must do

1. **Driveable without a human, and also interactively.** Scriptable from CI inside a Linux container, and usable
   by an agent across many calls. The process model (library, daemon, socket) is a trade-off, not a rule.
   **Met.** The batch CLI (`vtee --script`, `architecture.md`) runs a newline-delimited-JSON script of commands
   non-interactively and dumps structured results, built for and verified with TinyFugue's testing needs. "Usable
   by an agent across many calls" against an ongoing, out-of-process session is also met, read-only and
   read-write: observation is on by default now (read-only, at a hardened per-user default path,
   `--no-listen` to opt out - `decisions.md`), so a separate process can query live state via `--connect`
   without the session having been started with `--listen` in anticipation of that, and
   inject input too - with `--writable` when no human is competing (`WritePolicy::Open`), or with
   `--writable=exclusive`/`exclusive-shared` when one is, via an explicit `acquire_write`/`release_write` handoff
   (`WritePolicy::Exclusive`, full duplex stage 3b, `roadmap.md`/`architecture.md`). What's still open:
   free-for-all and mutually-exclusive-with-a-human remain unbuilt modes of the same `WritePolicy` enum
   (nobody's needed them yet). "Without a human" is now a declared mode, not an accident of environment
   detection: `--headless` (`decisions.md`) turns off human stdin/stdout forwarding and the raw-mode/signal
   machinery outright, rather than leaving it to whatever `is_terminal()` happens to see. A human in a
   *separate* process can now attach to a live session too (on Linux and on Windows), not just query it - named sessions,
   `vtee attach`/`ls`/`kill`, a second raw-TTY protocol alongside the existing NDJSON one
   (`architecture.md`, "Named sessions and `vtee attach`"), multiple clients of either kind at once,
   verified end to end including a remote attach client's own typing reaching the child.
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
   **Mostly met**: size and working directory are controlled by `Tap::spawn`, and size is now explicitly
   overridable from the CLI too (`--cols`/`--rows`, `decisions.md`) rather than only ever auto-detected from a
   real terminal - the first of what's meant to grow into a fuller virtual tty spec (terminal emulation mode,
   colour support, ...) as those are actually asked for. Resize is handled live (`SIGWINCH`). `TERM` and
   arbitrary environment variables are now controllable too - `--env KEY=VALUE` (repeatable, `decisions.md`),
   layered on top of `vtee`'s own inherited environment via `portable_pty::CommandBuilder::env` rather than
   replacing it; `TERM` is just one entry here, not a separate mechanism. Timing is irrelevant to replay (by
   design) but not yet controllable for the live tap itself (no artificial delay/throttling, not clear it's
   needed).
7. **Transparent observer mode.** The human keeps their own terminal and sees exactly what they would without the
   tool. Output is forwarded raw; the emulator only watches.
   **Met.** Raw bidirectional forwarding, `RawModeGuard` for the human's own terminal mode, confirmed not to alter
   anything forwarded (the emulator reads a copy, never the original bytes). Terminal restoration now survives
   `SIGTERM` and `SIGKILL` too, not just normal exit and panic unwind (`architecture.md`), verified on a real pty.
8. **Linux now, Windows later** (ConPTY).
   **Met on both.** Linux was first. On Windows, a patch to `portable-pty` (`vendor/portable-pty/`,
   `decisions.md`) fixed the original startup crash. The later intermittent hang turned out to be ours: ConPTY's
   startup cursor query went unanswered (a stdin read begun in cooked mode, or nobody to answer in
   `--script`/`--headless`). Raw mode now comes before the spawn and the tap answers the child's terminal queries
   when no human terminal can. Resize (console-size polling) and Ctrl-C work, and named sessions, `attach`, `ls`,
   `kill` and both sockets run over AF_UNIX sockets (`architecture.md`, "Named sessions on Windows"). Remaining
   Windows gaps: no terminal-restore watchdog, `attach` does not set the console's virtual-terminal flags, and
   continuous integration has not run on a hosted runner (`roadmap.md`, "Beta readiness").

## Not goals, for now

Writing a terminal emulator from scratch (reuse one - done, `alacritty_terminal`), panes and windows, a status
bar, or being a general `tmux` replacement.
