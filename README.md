# vtee

A terminal multiplexer built for AI and for testing. A Rust crate.

`tmux` lets a human juggle terminals. `vtee` runs a program in a pseudo-terminal, keeps the terminal's state in
memory, and lets a script or an AI agent query it and (later) send input, while a human, if there is one, uses
their own terminal as usual. Think of it as `tee` for a TUI: the stream is copied without disturbing the original,
but the copy is a screen you can query, not raw bytes.

**Status: v0 is done** on Linux and Windows (see `docs/decisions.md`), and being hardened now
(`docs/roadmap.md`). The name is a working name.

It came out of testing TinyFugue (`~/ai/mud`): its display code, status line and input editing cannot be tested
without a terminal, and the regression harness has no way to see one. The same tool lets an AI agent see and
drive a full-screen program, and lets a human run something while an agent watches it.

## Where to read next

- **`HANDOFF.md`** - start here if you're new to this project: where it came from, the research behind it, and
  the working conventions (jj, commits, Docker for anything Unix-specific) for this repo.
- **`docs/requirements.md`** - what `vtee` has to do, what it deliberately doesn't, and the current status of
  each requirement.
- **`docs/architecture.md`** - how it's actually built: the tap, the emulator boundary, the snapshot API, the
  raw log and replay, and the known rough edges.
- **`docs/roadmap.md`** - what's done, what's being hardened right now, what v1 needs (scoped, not yet built),
  the standing open decisions (protocol, input merging, licence), and open questions for TinyFugue's own testing
  work.
- **`docs/decisions.md`** - dated, measured, one-way-door choices (the emulator crate, the pty crate) with their
  evidence.
- **`docs/probe-results.md`** - the empirical survey of existing tools that led here.
