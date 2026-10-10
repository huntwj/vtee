# Documentation

`vtee` runs a program in a pseudo-terminal, keeps its screen in memory, and lets scripts, AI agents and humans
query and drive it at the same time. The project front page is the [README](../README.md).

## Using vtee

| Page | Read it to |
| --- | --- |
| [Getting started](guide/getting-started.md) | Run your first interactive, headless and scripted session |
| [CLI reference](guide/cli-reference.md) | Look up a flag, a subcommand or an exit code |
| [Sessions](guide/sessions.md) | Name sessions, attach and detach, see where files live, set defaults |
| [JSON protocol](guide/protocol.md) | Drive a session from your own code: every command, the snapshot format, errors, limits |
| [Driving a session as an AI agent](guide/agents.md) | Use the send, wait, read loop well and avoid its traps |
| [Testing a TUI](guide/testing-tuis.md) | Make a screen repeatable, wait correctly, compare snapshots and fail a test |
| [Examples](../examples/README.md) | Copy a working client or script in Python, shell or PowerShell |
| [Versioning and stability](guide/versioning.md) | Know what you can rely on across versions, and how to pin one |
| [Windows](guide/windows.md) | Know what works on Windows and what does not |
| [Security](guide/security.md) | Know who can reach a session, what is trusted and what is not |
| [Troubleshooting](guide/troubleshooting.md) | Fix stale sessions, odd terminals and common error messages |
| [FAQ and glossary](guide/faq.md) | Look up a term (holder, tap, stale, attach, pen, golden) or a common question |

Which page first: a person running a program with an AI watching starts at *Getting started*, then *Sessions* and
*Driving a session as an AI agent*. Someone writing tests for their own full-screen program starts at *Testing a
TUI*, then the *JSON protocol* and the *Examples*.

## Working on vtee

| Page | Read it to |
| --- | --- |
| [CONTRIBUTING.md](../CONTRIBUTING.md) | Build, test, format, write commit messages, submit a change |
| [Extending vtee](development/extending.md) | Find the files, tests and docs for a new command, snapshot field, flag or emulation case |
| [Architecture](architecture.md) | Understand how the pieces fit, then the detailed reference |
| [Platform internals](development/internals.md) | See the OS-specific code, every `unsafe` block, and why |
| [Testing](development/testing.md) | Know what each test suite covers and what it needs |
| [Continuous integration](development/ci.md) | See what CI runs and what is still unproven |
| [Releasing](releasing.md) | Understand how version numbers and releases follow from commit messages |
| [Snapshot and golden contract](design/snapshot-and-golden-contract.md) | Read why the snapshot and golden-file formats are what they are, their versioning and the stability promise (implemented, except `--update=migrate`) |

## History and planning

These are dated records, kept because the reasoning is still useful. They describe the project as it was when each
entry was written.

| Page | Contents |
| --- | --- |
| [Requirements](requirements.md) | What `vtee` must do, what it does not do, and where each requirement stands |
| [Roadmap](roadmap.md) | What is done, what remains for a beta, and open decisions |
| [Decisions](decisions.md) | Dated, measured choices with their evidence: the emulator and pty crates, ConPTY, sessions, hardening |
| [Probe results](probe-results.md) | The survey of existing tools that led to this design |
| [Handoff](handoff.md) | The original background and research, written for a fresh reader on 2026-10-06 |
| `probe/` | The recordings and scripts behind the probe results |
