# Continuous integration

Status: **designed but never run on real runners.** The workflow lives in `.github/workflows/ci.yml`. At the time
of writing it is not yet on `main` in this tree (it is being added with the CI hardening change), and nothing has
exercised it on GitHub's machines. Everything below describes the intended setup; check the file for the current
truth, and expect the first real run to need fixes.

## What it runs

Triggers: a push to `main`, every pull request, and a manual run (`workflow_dispatch`).

Two operating systems, each in its own job, `fail-fast` off so one platform's failure does not hide the other:

| Job | Runner | Steps |
| --- | --- | --- |
| Linux | `ubuntu-latest` | install `less`, `vim-tiny` and `procps`; `cargo build --tests`; `cargo test`; `cargo clippy --all-targets -- -D warnings` |
| Windows | `windows-latest` | `cargo build --tests`; `cargo test`; `cargo clippy --all-targets -- -D warnings` |

Details that apply to both:

- The Rust toolchain comes from `dtolnay/rust-toolchain@stable` and build output is cached with
  `Swatinem/rust-cache`.
- `permissions: contents: read`, so the workflow cannot push or write anywhere.
- A concurrency group cancels a superseded run on the same branch.
- Every step has a timeout, and `RUST_BACKTRACE=1` is set.
- `cargo fmt --check` is part of the plan. The tree now passes it, so it can be enabled.

A manual run has an option to repeat the pty and ConPTY test files ten extra times, to hunt flakiness without
changing the workflow.

## What to expect on a hosted runner

These are the differences from a development machine that are most likely to cause a first-run failure:

- **Windows ConPTY.** The tests create their own ConPTY, so a console window is not needed, but the runner
  has never been tried. The Ctrl-C test depends on the runner's process tree not leaving the "ignore Ctrl-C"
  setting on an ancestor; the code resets it at spawn, which should cover this.
- **Tools.** Windows tests that drive Git for Windows' `vim` and `less`, or `edit.exe`, look the programs up. They
  may not be on the runner's `PATH`; they skip when not found (see
  [testing](testing.md#tests-that-need-external-programs)). Setting `VTEE_REQUIRE_TOOLS` once it exists in the
  tree will turn that into a visible failure instead of a silent pass.
- **Ubuntu.** `bash`, `less` and `vim` are present on the image, and `/proc` is there, so the apt step mostly
  guarantees `vim-tiny` and `procps`.
- **Timing.** Hosted runners are slow and have few cores. The Windows behaviour suite takes about a minute on a
  development machine. Expect a few minutes there, and see the timeouts in the file.
- **Config isolation.** Hosted runners start with no user config, which is the clean case. Locally, a leftover
  config file can change a test's outcome (see [testing](testing.md#isolation)).

## Running the same checks locally

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Use the [Linux test image](../../CONTRIBUTING.md#the-linux-test-image) to reproduce the Ubuntu job from another
OS.

## Releases

Releases are not part of this workflow. Version numbers and release notes are driven by commit messages and are
documented in [docs/releasing.md](../releasing.md).

## Not covered yet

Nothing checks `cargo doc` warnings, supply-chain advisories (`cargo audit` would be the useful one; the vendored
`portable-pty` is invisible to it), or an MSRV build. macOS is not built.
