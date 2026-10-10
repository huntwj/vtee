# vtee

A terminal multiplexer for AI agents and for testing. A Rust crate and command-line tool.

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

Pre-1.0. The core feature set works on Linux and Windows and is covered by automated tests, but the project has
not had a release yet, the CI workflow has not run on real runners, and the interface may still change. Version
numbers follow semantic versioning driven by commit messages: see [docs/releasing.md](docs/releasing.md).

License: not yet chosen.

## Install

There are no binary releases yet. Build from source with a current stable Rust toolchain (edition 2024, so Rust
1.85 or newer):

```
cargo build --release
```

The binary is `target/release/vtee` (`vtee.exe` on Windows).

## Quick start

Start a session that an agent will drive (no human terminal, JSON socket open for writing):

```
vtee --headless --name demo --writable -- bash --norc
```

From another shell, list it, read the screen, and type into it:

```
$ vtee ls
demo	pid=2221	live

$ echo '{"cmd":"snapshot"}' | vtee --connect "${TMPDIR:-/tmp}/vtee-$(id -u)/demo.sock" --text
--- snapshot (byte_offset=2) ---
$
```

Attach a terminal to the same session (closing the window detaches; the session keeps running):

```
vtee attach demo
```

End it:

```
vtee kill demo
```

To run a program with your own terminal as usual while still exposing it to other tools, leave out `--headless`:
`vtee -- vim notes.txt`. To drive a program from a script file without any interactive session, use
`--script`. See [Getting started](docs/guide/getting-started.md).

## Platforms

| Platform | State |
| --- | --- |
| Linux | Supported. Primary development and test platform. |
| Windows 10 1803 or later | Supported through ConPTY. Sessions, `attach`, `ls` and `kill` work. Known gaps are listed in [the Windows guide](docs/guide/windows.md). |
| macOS | Not tested. It is Unix, so it may work. |

## Known limitations

- Mouse and bracketed-paste modes are tracked in snapshots, but there is no command to send mouse or paste input.
- Only the active screen buffer is exposed (not the primary and alternate buffers separately).
- `wait_for` waits for text on a single line; there is no richer wait condition yet.
- Blink is never reported (the emulator has no blink flag).
- Three escape-sequence behaviours differ from xterm; see
  [Known terminal-emulation differences](docs/guide/protocol.md#known-terminal-emulation-differences).
- If the process holding a session is killed with `SIGKILL`, the wrapped program is orphaned. See
  [Troubleshooting](docs/guide/troubleshooting.md).

The roadmap has the full list: [docs/roadmap.md](docs/roadmap.md).

## Documentation

- User guide: [getting started](docs/guide/getting-started.md), [CLI reference](docs/guide/cli-reference.md),
  [sessions](docs/guide/sessions.md), [JSON protocol](docs/guide/protocol.md),
  [driving a session as an AI agent](docs/guide/agents.md), [Windows](docs/guide/windows.md),
  [security](docs/guide/security.md) (and [SECURITY.md](SECURITY.md) for reporting),
  [troubleshooting](docs/guide/troubleshooting.md).
- Development: [CONTRIBUTING.md](CONTRIBUTING.md), [architecture](docs/architecture.md),
  [testing](docs/development/testing.md), [CI](docs/development/ci.md),
  [platform internals](docs/development/internals.md).
- History: [decisions](docs/decisions.md), [requirements](docs/requirements.md), [roadmap](docs/roadmap.md).
