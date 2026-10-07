# Handoff: research and context for vtee

Written 2026-10-06 so a fresh agent can start without the conversation that produced it. Read `README.md` first
(goals, requirements, staged plan). This file is the background: why the project exists, what we learned, what is
decided, what is not, and how to work in this repo.

Provenance matters here. Every claim about an external tool below was read from its README, docs or source through a
summarising fetch tool on 2026-10-06 and **not run**, unless it says "measured". Treat them as claims to verify.

## 1. Where this came from

The user is working on TinyFugue (TF), a C MUD client, in `~/ai/mud` (their notes, tests and tooling, a jj repo) with
the code in `~/ai/mud/tinyfugue/` (a separate jj repo). The goal there is to clean up and eventually migrate the
code, which needs strong tests first.

The existing regression harness (`~/ai/mud/tests/`: `run.py` on the host, `inside.sh` and `harness.py` in a Linux
container; `just test`, `just test-coverage`) runs TF headlessly: `tf -n -f -v` with `TERM=dumb`, a script on stdin,
output compared with recorded goldens, between `@@BEGIN@@` and `@@END@@` marker lines. A fake MUD server
(`tests/fixtures/fakeserver.py`) feeds it network data. That is good for the macro language and weak everywhere else.

Measured 2026-10-06 with gcov (`just test-coverage`, candidate = the `dev` branch, full build), lines hit:

| | golden cases only | plus finding tests |
|---|---|---|
| All files | 49.4% lines, 60.9% functions | 58.0% lines |
| `output.c` (display) | 38% | 46% |
| `socket.c` (event loop, telnet) | 8% | 39% |
| `msdp.c` | 0% | 0% |
| `expr.c`, `expand.c`, `macro.c`, `pattern.c` | 70-80% | 72-80% |

So the display and network code, which a port or a new feature would touch hardest, is the least protected.
`tests/cases/CATALOG.md` lists the known gaps. The display, status line, input editing and `/bind` cannot be tested
without a terminal: that is what this crate is for. Its second use is letting an AI agent see and drive a
full-screen program, and a third is letting an agent watch while a human uses one.

Notes pages in `~/ai/mud/site/src/content/docs/`: `workflow/testing.md` (test design and the coverage section),
`project/ideas.md` (idea 8 is this tool), `project/todo.md`.

## 2. Decisions made

- **The tool is a Rust crate at `~/ai/vtee`** (the user, 2026-10-06). Working name, may change.
- **Reuse an emulator; do not write one.** The pty plumbing is small. Terminal emulation is the hard part.
- **Transparent observer mode is a core requirement**: the human keeps their own terminal, output is forwarded raw,
  the emulator only watches. This came from the user: "kind of a TUI version of tee".
- **The requirements were deliberately broadened by the user**, who rejected an earlier draft that capped them at
  what TF uses today: broad VT100/VT220/xterm emulation, full state exposure, a daemon or MCP server is acceptable
  (a trade-off, not a gate), and "wait until the screen settles" is the wrong primitive because a server may stream
  constantly. See `README.md`.
- **Staged plan**: v0 the tap (read-only observer, raw byte log, snapshot queries), v1 input injection and
  wait-for-predicate, later MCP, Windows and multiple sessions.
- **Emulator crate decided 2026-10-06: `alacritty_terminal`.** See `docs/decisions.md` for the measured comparison
  against `avt`, `vte` and `termwiz`, plus follow-up checks (chunked-feed equivalence, malformed-input robustness).
- **pty crate decided 2026-10-06: `portable-pty`.** Built v0's tap on it; works on Linux (all 15 probe files
  through a real pty, survives `SIGWINCH`), found and fixed two real bugs in the process (zero-size grid panic,
  an EOF-doesn't-always-follow-child-exit assumption). Windows (ConPTY) isn't usable from this MSYS2/Git-Bash host
  yet, cause not isolated - see `docs/decisions.md`.
- **Not decided**: wire protocol, how to merge human and agent input, licence.

## 3. What TinyFugue needs from a terminal

From `tinyfugue/src/output.c`, lines 510-550 (read, not run): it queries termcap for `cl ce cd` (clear),
`ti`/`te` (**enter and exit the alternate screen**, so the emulator must honour it), `cm` (cursor addressing), `cs`
(**scroll region**, the status line relies on it), `dc ic im ei` (insert and delete character, insert mode), `al dl`
(insert and delete line), `ks ke` (keypad mode), `bl`, and the attributes `us mr mb mh md ZH so ue se me sa`
(underline, reverse, blink, dim, bold, italic, standout and their resets). It also reads key codes from terminfo.

The harness chooses `TERM`, so it decides which of those are active. `TERM=dumb` today means none. Screen tests need
a terminfo with cursor addressing, for example `xterm`, `xterm-256color`, `vt100` or `linux`. TF's status line may
contain a clock, so a screen golden needs it disabled or masked.

## 4. Research

### Existing tools

| Tool | What | Evidence | Notes |
|---|---|---|---|
| [pyte](https://github.com/selectel/pyte) | Pure-Python in-memory VT emulator | Source of `pyte/screens.py` read | **No alternate screen** (no handling of modes 47/1047/1049). Scroll regions (`set_margins`), `insert_lines`/`delete_lines`, wide and combining characters (via `wcwidth`), 256-colour and truecolour SGR, per-cell `Char(data, fg, bg, bold, italics, underscore, strikethrough, reverse, blink)` with **no dim**, `HistoryScreen` scrollback. LGPL-3. Latest 0.8.2, Nov 2023, Python >=3.8 ([PyPI JSON](https://pypi.org/pypi/pyte/json)). |
| [ht](https://github.com/andyk/ht) | Rust binary, Apache-2.0, built on asciinema's `avt` | README read | JSON over stdin/stdout; commands `sendKeys`, `input`, `takeSnapshot`, `resize`; events `init`, `output`, `resize`, `snapshot`. `--size WxH`. Built-in HTTP live preview (`-l`) and WebSocket (`/ws/events`). Snapshot has `text` and raw `seq`. **No settle or wait primitive.** Owns a virtual terminal, **not** a transparent proxy. Linux and macOS only. 909 stars. |
| [tui-test](https://github.com/microsoft/tui-test) | Microsoft | README read twice (two fetches agreed) | README describes Rust, Python and Node bindings plus a CLI; backends Alacritty, Ghostty, Rio, xterm.js; cell colours and attributes; `wait idle`; locator-style `expect` with retry; snapshot testing; Windows, Linux, macOS; MIT. An earlier web search described it as Node-only with xterm.js, so the project has probably evolved. The Rust crate is 0.1.0, so maturity is unknown. |
| [pilotty](https://cdn.jsdelivr.net/gh/msmps/pilotty@main/README.md) | Rust daemon plus CLI, MIT | README read | JSON snapshots with text, cursor and a content hash; `--await-change --settle`, `wait-for`; spawn with `--cwd`; resize. Colours not stated. Unix only. Agent-only, no live human view stated. Calls itself "built with AI… edge cases may exist". |
| [agent-tui](https://cdn.jsdelivr.net/gh/pproenca/agent-tui@master/README.md) | Rust daemon, MIT | README read | `run`, `press`, `type`, `screenshot`, `wait`. Cursor and colours not stated. Unix only. Attach/detach listed as future work. |
| [NPCterm](https://docs.rs/crate/npcterm/1.3.3) | MCP server, Apache-2.0 | docs.rs read | Built on `portable-pty` and `vte`. Idle detection. Windows binaries exist. Optional web viewer. MCP only, awkward from Python. |
| [Hecate](https://hecate.readthedocs.io/en/latest/) | Python, drives tmux | docs read | About 2015, "semi experimental", tmux 1.6-2.0. Effectively dead. |
| tmux (`send-keys`, `capture-pane -e`) | The classic way to share a terminal with an agent | **Not researched here**, from the model's own knowledge | Unix only. |

Not researched at all, only named as candidates: the Rust crates `avt`, `vte`, `alacritty_terminal`, `termwiz`,
`portable-pty`. Conformance suites such as `vttest` and `esctest` are mentioned from memory and unverified.

**None of the tools above documented a transparent observer mode** where the human keeps their own terminal. That is
the gap this crate fills.

### Empirical probe (measured 2026-10-06)

Full matrix, evidence and reproduce commands: `docs/probe-results.md`, with the byte files and drivers in `docs/probe/`.
Four candidates were run in throwaway containers against 15 probe files at 40x12, `TERM=xterm-256color`. agent-tui and
NPCterm were not run. Condensed (all measured):

| | pyte | tui-test (xterm.js, alacritty, ghostty, rio) | ht (avt) | pilotty |
|---|---|---|---|---|
| Text, scroll region, IL/DL/ICH/DCH/IRM | partial (a `delete_lines` bug) | pass | pass | partial (IRM ignored) |
| SGR attributes, 256-colour, truecolour | no dim | pass (alacritty and rio cannot report blink) | pass, but only as escape text in `seq` | not exposed |
| Alternate screen | **fail** | pass | pass | pass |
| DEC special graphics | fail by default | pass | pass | fail |
| Mode state (mouse, paste, keypad...) | 4 of 5 | 4 of 5 (no `?1006`) | 1 of 5 | not exposed |
| Title and OSC 8 hyperlink | title only | both | neither | neither |
| Cursor visibility and shape | partial | pass | partial | partial |
| Scrollback | pass | pass | not exposed | pass |

What matters for this project:

- **pyte is ruled out as the emulator.** No alternate screen at all, a `delete_lines` bug that leaves stale text unless
  `screen.display` is read between feeds, DEC graphics ignored in its default UTF-8 mode, no dim attribute.
- **tui-test is the only tool that passed the broad-emulation, full-state bar on every backend**, and its multiple
  backends allow differential runs (the same bytes through two engines). It exposes `State.modes`, `mouse_mode`, cursor
  shape, title and hyperlinks. Gaps: version 0.1.0, no raw byte capture (its recordings are re-rendered screen history,
  not the program's bytes), and it is untested against a long-running program like TinyFugue.
- **ht has real raw capture**: its `output` events were byte-identical to the program's output. Thin state otherwise, no
  wait primitive, Unix only.
- **pilotty's snapshot is only text, cursor and a hash.** Too thin for tests.

**This section's engines were measured only through tools**: tui-test's backends (Alacritty, Ghostty, Rio, xterm.js)
and ht (`avt`). The Rust crates themselves were fed the same probes directly in a follow-up round, 2026-10-06 —
see `docs/decisions.md` for that matrix and the emulator-crate decision it produced.

**Not measured:** repeated-run determinism, Windows/ConPTY (tui-test only claims it), resize reflow, mouse input, and the
real behaviour of every wait primitive (read from help text only).

## 5. Design notes (proposals, not decisions)

- **Shape.** Spawn the child in a pty (`portable-pty` is the obvious candidate). Two-way forward bytes between the
  child and the user's real terminal, raw, so the human sees exactly what they would without the tool. A copy of the
  child's output also goes to an emulator that keeps the state. The emulator is a passive observer and must never
  alter what is forwarded.
- **Snapshots tied to a byte offset.** Number the output stream. A snapshot is "state after byte N", so a query
  cannot see a half-drawn frame and two runs can be compared.
- **Raw log and replay.** Write the child's output to a file (with timestamps, asciicast-like). Replaying it into an
  emulator reproduces the state with no timing involved, which is what makes golden tests stable.
- **Wait primitives.** Wait for a predicate on the screen with a timeout. Synchronise on markers a test controls
  (TF's harness already prints `@@BEGIN@@`/`@@END@@`). Allow masking regions that change on their own.
- **Observer protocol.** Probably line-delimited JSON over a Unix socket first, as `ht` does. Commands such as
  `snapshot`, `wait_for`, `stream(from_offset)`, `modes`, later `send_keys` and `resize`. An MCP front end can wrap it.
- **Input.** v0 is read-only for the observer. With both a human and an agent able to type, one merged stream is
  needed with a stated policy for simultaneous input.
- **Details that bite.** Propagate `SIGWINCH` and keep the child pty, the emulator and the real terminal the same
  size. Restore the user's terminal mode on crash or Ctrl-C. Mouse, bracketed paste and the alternate screen must
  reach the real terminal untouched, which raw forwarding gives for free. On Windows the equivalent is ConPTY.
- **Split the agent-query layer into its own crate (brainstorm, 2026-10-06, not a requirement).** The "snapshot/wait/
  modes" query protocol is logically separable from the "spawn in a pty and tap the bytes" layer. If they're two
  crates, an app that already owns its own screen state (TinyFugue parses its own display) could expose the same
  query protocol in-process, with no pty and no emulator involved, while `vtee` composes that query crate on top
  of its own emulator for the general "wrap anything" case. Premature to act on: we don't know what the query API
  needs to look like until v0's snapshot API exists and has been used once. Revisit then; don't split crates before.

## 6. Suggested first steps

1. ~~**Choose the emulator by measuring.**~~ **Done, 2026-10-06.** `docs/decisions.md` has the comparison and the
   follow-up chunked-feed/garbage-input checks. `alacritty_terminal` won.
2. ~~**The tap.** Spawn, forward both ways, restore the terminal on exit, handle resize.~~ **Done, 2026-10-06** on
   Linux (`src/lib.rs`, `Tap`): spawns via `portable-pty`, forwards both ways, feeds a copy of the child's output
   into `alacritty_terminal`, restores the real terminal's mode on exit (`RawModeGuard`), watches `SIGWINCH`.
   Windows not working yet from this host - see `docs/decisions.md`. Still missing from this step, deferred to 3:
   a real way to query the emulator (`snapshot_lines()` is a debug stand-in, not the real API) and a true
   `kill -9`/`SIGTERM`-safe terminal restoration (only normal exit and panic-unwind are covered).
3. **The snapshot API** (cells, attributes, cursor, modes, offset) and the raw byte log.
4. **Replay.** Rebuild a snapshot from a log, and check it matches the live one.
5. **Tests that need no TinyFugue**: run `cat` on sequence files, and add conformance cases. Only after the crate works
   on its own, add an integration with TF's harness (`~/ai/mud/tests/harness.py`), probably a thin Python wrapper.

If step 2 or 3 turns out much harder than expected, say so rather than push on: the fallback in the README is to adopt
an existing tool for the test side and keep only a thin live-watching tap.

## 7. Working conventions

These come from the user's other repos and standing preferences; they are not inherited automatically.

- **Version control is Jujutsu (`jj`), never `git`.** The repo is non-colocated: there is no `.git`, so git commands
  fail by design. Default branch is `main`. Bookmarks do not auto-advance; move them explicitly. Check `jj <cmd> --help`
  on the installed version rather than answering from memory.
- **Commits.** Conventional Commits (`type(scope): subject`). **Small commits: one logical change each**, not
  bundled by file or theme (the user said so explicitly on 2026-10-06). **No `Co-Authored-By:` or `Claude-Session:`
  trailers**, and no "Generated with" footers (a standing user decision, 2026-10-05, from the TinyFugue repos).
- **Branches.** One branch per unit of work, `type/short-name`, off `main`, merged with a no-ff merge commit titled
  `Merge branch '<name>'`.
- **Privacy.** The repo is private and has no remote yet. Push nothing anywhere, and create no remote, without asking.
- **Build and test with `cargo` only.** The host is Windows 11 with Git Bash, Rust 1.98 is installed, and Docker
  Desktop provides Linux for anything Unix-specific. Git Bash rewrites paths in Docker commands: use
  `MSYS_NO_PATHCONV=1` and `C:/...` style mount paths.
- **Verify, do not assume.** The user repeatedly prefers claims checked against the source or by running something.
  Say plainly whether a statement was measured or read.
- **Record decisions** with a date when they are made (for example in a `docs/decisions.md`), and keep this file
  current when facts change instead of only noting it elsewhere.

## 8. Open questions

- Can the human's input and an agent's input be merged safely, and what should simultaneous input do?
- Build on `tui-test`'s Rust crate (`tui-test-rs`, 0.1.0, which already wraps several engines), or on an emulator crate
  directly plus our own pty layer? The first is less work and gets differential backends; the second keeps control and
  gives us the raw-byte capture and transparent observer mode that tui-test lacks. (Leaning toward the second, now that
  `alacritty_terminal` is chosen and measured as robust — see `docs/decisions.md` — but not yet decided.)
- One session per process, or a daemon that manages several?
- Does the snapshot "as of byte N" need the emulator to be re-runnable from a checkpoint, or is replay from the
  start fast enough?
- Licence, and whether the name `vtee` is free to use.
