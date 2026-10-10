# vtee

A tee for terminal programs: watch, share, drive and test any TUI's screen. A Rust crate and command-line tool.

`vtee` runs a program in a pseudo-terminal and keeps the terminal's screen in memory. A script or an AI agent
can read that screen as structured data (text, colours, cursor, title, modes) and send keystrokes to the program.
A human can attach to the same session from another shell and see and type into it, the way `tmux attach` works.

- **Tests and agents** get a queryable screen instead of raw bytes, and a way to wait for text to appear instead
  of sleeping.
- **Humans** keep their own terminal. In the default interactive mode the program's output is forwarded unchanged;
  the emulator only watches.
- **Several clients at once.** One session can have any number of attached terminals and any number of JSON
  clients.

It started as a way to test TinyFugue, whose display code cannot be tested without a terminal. See
[docs/README.md](docs/README.md) for the full documentation index.

## Status

Pre-1.0. The core feature set works on Linux and Windows and is covered by automated tests that run in CI on
Ubuntu and Windows runners. The interface may still change.

Versions 0.0.x are early previews that reserve the crate name. The snapshot shape is frozen. Screen tests with
golden files (`vtee --script S --expect GOLDEN -- APP`) are new in 0.1.0, and the input-policy options are still being
designed. Version numbers follow semantic versioning driven by commit messages: see
[docs/releasing.md](docs/releasing.md) and the [stability policy](docs/guide/versioning.md).

License: dual-licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option. Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
vtee by you, as defined in the Apache-2.0 license, is dual licensed as above, without any additional terms or
conditions.

## Install

There are no binary releases yet. Build from source with a current stable Rust toolchain (the minimum is Rust
1.89, set as `rust-version` in `Cargo.toml`):

```
cargo build --release
```

The binary is `target/release/vtee` (`vtee.exe` on Windows).

## Quick start

Start a session that an agent will drive (no human terminal, JSON socket open for writing). The `--env` gives
the shell a short fixed prompt, which makes the screen small and easy to wait on:

```
vtee --headless --name demo --writable --env PS1='$ ' -- bash --norc
```

From another shell, list it and read the screen (the 23 blank screen lines after the prompt are left out):

```
$ vtee ls
demo	pid=23	live

$ echo '{"cmd":"snapshot"}' | vtee --connect "${TMPDIR:-/tmp}/vtee-$(id -u)/demo.sock" --text
--- snapshot (byte_offset=2) ---
$
```

To type into it, send `send` and `wait_for` commands on the same socket; the
[agents guide](docs/guide/agents.md) shows the loop and its traps.

Attach a terminal to the same session (closing the window detaches; the session keeps running):

```
vtee attach demo
```

End it:

```
vtee kill demo
```

To test a program's screens, record them once into a golden file and then check against it (the script is the
JSON commands to type; each `snapshot` in it is a checkpoint). The check exits 0 on a pass, 1 on a difference and 2
when the golden was recorded under other version numbers; see [testing TUIs](docs/guide/testing-tuis.md#golden-files) and the [golden file format](docs/guide/golden-format.md):

```
vtee --script flow.ndjson --expect flow.golden --update -- tf     # CREATED flow.golden: 3 screens
vtee --script flow.ndjson --expect flow.golden -- tf              # PASS flow.golden (3 screens)
```

To run a program with your own terminal as usual while still exposing it to other tools, leave out `--headless`:
`vtee -- vi notes.txt` on Linux, or `vtee -- powershell` on Windows (any program works). To drive a program
from a script file without any interactive session, use `--script`. See
[Getting started](docs/guide/getting-started.md).

## Platforms

| Platform | State |
| --- | --- |
| Linux | Supported. Primary development and test platform. |
| Windows 10 1803 or later | Supported through ConPTY. Sessions, `attach`, `ls` and `kill` work. Known gaps are listed in [the Windows guide](docs/guide/windows.md). |
| macOS | Not tested. It is Unix, so it may work. |

## Known limitations

- Mouse and bracketed-paste modes are tracked in snapshots, but there is no command to send mouse or paste input.
- Only the active screen buffer is exposed (not the primary and alternate buffers separately).
- `wait_for` waits for text on a single screen row, or on a soft-wrapped line with `"join_wrapped":true`; there is no
  richer wait condition yet.
- Blink is never reported (the emulator has no blink flag).
- Three escape-sequence behaviours differ from xterm; see
  [Known terminal-emulation differences](docs/guide/protocol.md#known-terminal-emulation-differences).
- If the process holding a session is killed with `SIGKILL`, the wrapped program is orphaned. See
  [Troubleshooting](docs/guide/troubleshooting.md).

The roadmap has the full list: [docs/roadmap.md](docs/roadmap.md).

## Related tools

Related tools: [teetty](https://github.com/mitsuhiko/teetty) (a tee with a fake tty, for watching and remote
controlling a process), [tmux](https://github.com/tmux/tmux) (the session and attach model `vtee` borrows),
`script` and `ttyrec` (record a terminal session), `expect` and [pexpect](https://github.com/pexpect/pexpect)
(script interactive programs, with no screen model). `vtee` adds an in-memory screen you can query, several
simultaneous clients, and golden files for screen-level tests.

## Documentation

Start where your use is:

- **I run a TUI and want an AI (or a colleague) to watch or help:**
  [getting started](docs/guide/getting-started.md), [sessions](docs/guide/sessions.md) (naming, attach, detach,
  several clients), [driving a session as an AI agent](docs/guide/agents.md),
  [Windows](docs/guide/windows.md), [security](docs/guide/security.md) (and [SECURITY.md](SECURITY.md) for
  reporting), [troubleshooting](docs/guide/troubleshooting.md).
- **I write a TUI and want a test harness:** [testing TUIs](docs/guide/testing-tuis.md) (a determinism and test
  recipe), the [golden file format](docs/guide/golden-format.md), [JSON protocol](docs/guide/protocol.md) (every command and the snapshot format),
  [CLI reference](docs/guide/cli-reference.md), runnable [examples](examples/README.md) in Python, shell and
  PowerShell, [versioning and stability](docs/guide/versioning.md).
- **I want to contribute:** [CONTRIBUTING.md](CONTRIBUTING.md), [extending vtee](docs/development/extending.md),
  [architecture](docs/architecture.md), [testing](docs/development/testing.md), [CI](docs/development/ci.md),
  [platform internals](docs/development/internals.md), [releasing](docs/releasing.md).
- **Questions and terms:** [FAQ and glossary](docs/guide/faq.md).
- History: [decisions](docs/decisions.md), [requirements](docs/requirements.md), [roadmap](docs/roadmap.md).
