# Continuous integration

Status: **running on GitHub Actions since 2026-10-09.** The first run passed every job on `ubuntu-latest` and
`windows-latest`. The workflow is `.github/workflows/ci.yml`; check that file for the current truth, this page
describes it. The release workflow is separate (see [releasing](../releasing.md)).

## What it runs

Triggers: a push to `main`, every pull request, and a manual run (`workflow_dispatch`). Five jobs, none depending on
another, so a failure in one does not hide the others:

| Job | Runner | What it does |
| --- | --- | --- |
| `lint` | `ubuntu-latest` | `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo doc --no-deps` with `RUSTDOCFLAGS=-D warnings`. Platform-independent, so checked once. |
| `fast` | `ubuntu-latest` and `windows-latest` | Clippy with warnings denied, then `cargo test --lib --bins --test cli --test replay --test differential`. On Windows it also runs `cargo doc` with warnings denied, because rustdoc output differs for items that exist on one platform only. |
| `linux-pty` | `ubuntu-latest` | Installs `less`, `vim-tiny`, `procps` and `python3`, builds the tests, and runs `cargo test --test linux_sessions` with `VTEE_REQUIRE_TOOLS=1` and `RUST_TEST_THREADS=2`. |
| `windows-pty` | `windows-latest` | Puts Git for Windows' `less` and `vim` on `PATH` (and fails if either is missing), builds the tests, and runs `windows_conpty`, `windows_console`, `windows_behavior` and `windows_sessions` with `VTEE_REQUIRE_TOOLS=less,vim`. |
| `msrv` | `ubuntu-latest` | Reads `rust-version` from `Cargo.toml` (1.89) and runs `cargo check --all-targets` on that toolchain. If `Cargo.toml` has no `rust-version` it prints a notice and does nothing. |

Together they run every test suite in the repository: `differential` in the `fast` jobs on both platforms, and
`windows_console` in `windows-pty`. See [testing](testing.md) for what each suite covers.

Details that apply to all of them:

- The Rust toolchain comes from `dtolnay/rust-toolchain@stable` (`msrv` uses the pinned version) and build output
  is cached with `Swatinem/rust-cache`.
- Commands use `--locked`, so a change to `Cargo.lock` that was not committed fails the run.
- `permissions: contents: read`, so the workflow cannot push or write anywhere.
- A concurrency group cancels a superseded run on the same branch.
- Every job and the long steps have timeouts, and `RUST_BACKTRACE=1` is set.

A manual run has an option, `repeat_pty_tests`, that repeats the pty and ConPTY test files ten extra times in the
two pty jobs, to hunt flakiness without changing the workflow.

## What has happened on real runners

- The first run passed all six job runs (`lint`, `msrv`, `fast` on two platforms, `linux-pty`, `windows-pty`). The
  Windows jobs were the slow ones, about four minutes each; `linux-pty` took about a minute and a half and `lint`
  and `msrv` under a minute each.
- Two later pushes failed `lint`: `cargo doc` reported an unresolved link to `restore_console_modes`, a private
  item. That is what the doc job is for; the link was fixed and the job passes.
- One push failed `fast (ubuntu-latest)` because the test
  `a_stalled_tty_client_does_not_block_other_clients_or_snapshots` timed out waiting for the well-behaved client to
  receive 4 MiB. It passed on the next push. It is a timing test on a shared runner; treat a second failure as a
  real bug, not noise.
- The latest push passed every job.

## What to expect on a hosted runner

These are the differences from a development machine that can cause a failure there:

- **Windows ConPTY.** The tests create their own ConPTY, so no console window is needed, and `windows-latest`
  passes. The Ctrl-C test depends on the runner's process tree not leaving the "ignore Ctrl-C" setting on an
  ancestor; the code resets it at spawn, which covers this.
- **Tools.** The Windows tests that drive Git for Windows' `vim` and `less`, or `edit.exe`, look the programs up.
  The `windows-pty` job puts Git's tools on `PATH` and checks them before testing. `VTEE_REQUIRE_TOOLS` turns a
  missing tool into a visible failure instead of a silent skip (see
  [testing](testing.md#tests-that-need-external-programs)).
- **Ubuntu.** `bash`, `less` and `vim` are on the image already and `/proc` exists; the apt step guarantees
  `vim-tiny`, `procps` and `python3`.
- **Timing.** Hosted runners are slow and have few cores, which is why `linux_sessions` runs with
  `RUST_TEST_THREADS=2`. See the timeouts in the file.
- **Config isolation.** Hosted runners start with no user config, which is the clean case. Locally, a leftover
  config file can change a test's outcome (see [testing](testing.md#isolation)).

## Running the same checks locally

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
cargo test
```

Use the [Linux test image](../../CONTRIBUTING.md#the-linux-test-image) to reproduce the Ubuntu jobs from another
OS. Run `cargo doc` on both platforms if you touch code that only exists on one of them.

## Releases

Releases are not part of this workflow. Version numbers and release notes are driven by commit messages and are
documented in [docs/releasing.md](../releasing.md).

## Not covered yet

Nothing checks supply-chain advisories (`cargo audit` would be the useful one; the vendored `portable-pty` is
invisible to it). macOS is not built. The `msrv` job checks the library and tests build; it does not run them on
the old toolchain.
