# Contributing

Thanks for looking at `vtee`. This page covers how to build, test and submit changes. For how the code is
organised, read [docs/architecture.md](docs/architecture.md); for the test suites, [docs/development/testing.md](docs/development/testing.md).

## Workflow

Use ordinary Git: fork or branch, commit, open a pull request against `main`. The maintainer happens to use
Jujutsu (`jj`) locally, but nothing `jj`-specific is committed and you do not need it.

Keep each commit a coherent change that builds, passes `cargo fmt --check` and passes its tests. Small commits
are easier to review than large ones.

## Commit messages

Commits follow [Conventional Commits](https://www.conventionalcommits.org/). The message decides the next version
number, so the prefix matters. **A pull request's title must be a Conventional Commit too**, because pull requests
are squash-merged and the title becomes the commit on `main`. CI checks both: the type must be one of `feat`, `fix`,
`perf`, `refactor`, `test`, `docs`, `chore`, `ci`, `style`, `build`, `revert`, and the whole header line (type, scope
and summary) at most 100 characters (`.commitlintrc.yml`, `.github/workflows/pr-title.yml`).

```
<type>(<optional scope>): <short summary in the imperative>

<optional body: why, not what>
```

| Type | Use for | Effect on the version |
| --- | --- | --- |
| `feat` | A new user-visible capability | Minor bump |
| `fix` | A bug fix | Patch bump |
| `perf` | A performance improvement | Patch bump |
| `feat!`, `fix!`, or a `BREAKING CHANGE:` footer | An incompatible change to the CLI, the JSON protocol or the library API | Major bump (minor while the version is `0.x`; see [releasing](docs/releasing.md)) |
| `docs`, `test`, `refactor`, `build`, `ci`, `chore`, `style`, `revert` | Everything else | No release by themselves |

Releases are cut automatically from these messages; see [docs/releasing.md](docs/releasing.md) for the exact
rules and the [stability policy](docs/guide/versioning.md) for what each kind of change promises. Write the summary
so it reads well in a changelog.

## Setup

You need a current stable Rust, at least the version in `rust-version` in `Cargo.toml` (1.89), with `rustfmt` and
`clippy`:

```
rustup component add rustfmt clippy
cargo build
```

Linux is the primary platform. On Windows the same commands work natively.

## Checks to run before opening a pull request

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

- **Formatting.** `rustfmt.toml` sets `max_width = 120` and `use_small_heuristics = "Max"`. Run `cargo fmt` and
  the formatting question is closed. Every commit in the history is formatted.
- **Line endings.** Files are LF. `.gitattributes` normalises them for Git users. Do not commit CRLF; some Windows
  editors and tools add it silently. Files under `vendor/` are left as upstream wrote them.
- **Clippy.** Warnings are errors. Fix them rather than silencing them; if an `allow` is genuinely right, say why in
  a short comment.
- **Comments.** The default is no comment. Write one only for a non-obvious reason, such as a constraint or a
  workaround. History and rationale belong in the commit message or [docs/decisions.md](docs/decisions.md), not in
  source comments.

## Running the tests

Run the platform's whole suite with `cargo test`. Some suites only exist on one platform:

| Command | Platform | What it covers |
| --- | --- | --- |
| `cargo test --lib --test cli --test replay` | both | Unit tests, CLI behaviour, replay against probe recordings |
| `cargo test --test differential` | both | 80 escape-sequence cases compared with two independent emulators ([how it works](docs/development/testing.md#differential-emulator-tests)) |
| `cargo test --test linux_sessions` | Linux | Sessions, multi-client, full-screen programs, lifecycle, stress |
| `cargo test --test windows_conpty --test windows_behavior --test windows_sessions` | Windows | ConPTY, signals, resize, sessions over AF_UNIX |
| `cargo test --test windows_console` | Windows | The console VT flags and that arrow keys arrive as escape sequences |

Test files are gated by platform, so on the wrong platform they compile to nothing. A plain `cargo test` runs all
of them for your platform. CI does not run `differential` or `windows_console` yet, so run them yourself when you
touch the emulator, the snapshot or the console code.

### The Linux test image

Several Linux tests drive `less`, `vim` and `python3`, and one checks process cleanup through `/proc`. A Docker image
with those tools is the easiest consistent environment, and the only way to run the Linux suite from a Windows
machine. There is no Dockerfile in the repository root; the image is defined in `docker/Dockerfile.test`:

```
docker build -f docker/Dockerfile.test -t vtee-test .
docker run --rm -v "$PWD:/work" -w /work -e CARGO_TARGET_DIR=/tmp/target -e VTEE_REQUIRE_TOOLS=1 vtee-test cargo test
```

Setting `CARGO_TARGET_DIR` inside the container keeps its build output out of your checkout. Under Git Bash on
Windows, set `MSYS_NO_PATHCONV=1` so the volume path is not rewritten, and use a path like `C:\path\to\vtee:/work`.

`VTEE_REQUIRE_TOOLS=1` makes a missing `less`, `vim` or `python3` fail the test instead of printing `skipping` and
passing. Without it, a green run in a bare image does not prove the full-screen tests ran. The image installs
`vim-tiny`, which provides `vi` and `vim.tiny`; it has no command named `vim`.

## Continuous integration

GitHub Actions runs the checks on Ubuntu and Windows for every push to `main` and every pull request: formatting,
clippy with warnings denied, the documentation build, the fast suites on both platforms, the pty suites on both
(with the Linux tools required), and a build on the minimum Rust version. It has run on real runners and is green.
See [docs/development/ci.md](docs/development/ci.md). A failure that only happens on a runner is most likely a
timing assumption; see [flakiness](docs/development/testing.md#hunting-flakiness).

## Adding a feature or a test

[Extending vtee](docs/development/extending.md) lists the files, tests and docs that move together when you add a
protocol command, a snapshot field, a command-line flag or an emulation case. For tests:

- Prefer the existing helpers. `tests/cli.rs` (the `unix_behavior` module) and `tests/linux_sessions.rs` have
  helpers that start a session in an isolated `TMPDIR` and wait for it. `tests/linux_sessions.rs` wraps a session
  in a guard that kills it even when an assertion fails; use that pattern for new tests.
- Isolate everything. Point `TMPDIR` (Linux), `LOCALAPPDATA` and `APPDATA` (Windows) and `XDG_CONFIG_HOME` at a
  per-test directory so tests never touch the real session directory or config file and can run in parallel.
- Wait for a condition, never a fixed delay. Poll for the socket, or use `wait_for`, with a timeout that fails the
  test.
- Make a test fail when it cannot run, not pass silently. If a test needs `vim` or `less`, see
  [testing](docs/development/testing.md#tests-that-need-external-programs).
- A test that reproduces a real bug before the fix lands can be marked `#[ignore = "bug: ..."]` with the reason, then
  enabled in the fix.

## Code layout

| Path | Contents |
| --- | --- |
| `src/main.rs` | CLI parsing and the spawn path |
| `src/session_cmds.rs`, `src/client.rs`, `src/output.rs` | `attach`/`ls`/`kill`; `--connect`/`--script`; error and exit-code helpers |
| `src/tap.rs` | `Tap`: the pty, the emulator, the forwarding threads |
| `src/serve.rs`, `src/sockets.rs` | Serving a `Tap` over the two sockets; write policies |
| `src/session.rs` | Session directory, names, liveness |
| `src/rawmode.rs` | Raw-mode guard and the terminal-restore watchdog |
| `src/command.rs`, `src/snapshot.rs`, `src/keys.rs`, `src/log.rs` | Protocol types, snapshot and redraw, named keys, asciicast log |
| `src/net.rs` | Unix sockets on both platforms |
| `vendor/portable-pty/` | A patched copy of `portable-pty` (see its README) |
| `tests/` | Integration tests |
| `examples/` | Runnable examples of driving a session |
| `docs/` | Documentation; `docs/decisions.md` is the dated record of why things are the way they are |

## Reporting problems

Open an issue with the command you ran, what you expected, what happened, your OS and the output of
`vtee --version`. For a hang or a screen that looks wrong, `vtee --log run.cast ...` records the raw output,
which makes it reproducible.
