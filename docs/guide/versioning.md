# Versioning and stability

`vtee` is pre-1.0. This page says what you can rely on across versions and what you cannot, for each thing a
user or a program can depend on. It states the project's policy; where the policy is a plan and not yet a track
record, it says so. There has been no release yet.

## How a version number is chosen

Version numbers come from commit messages on `main`, not from a person deciding at release time. While the version
is `0.x`:

| Commit type | Next version |
| --- | --- |
| `fix`, `perf` | patch (`0.1.0` to `0.1.1`) |
| `feat` | minor (`0.1.0` to `0.2.0`) |
| any type marked breaking (`feat!`, a `BREAKING CHANGE:` footer) | minor |
| `docs`, `test`, `refactor`, `build`, `ci`, `chore`, `style` | no release |

From `1.0` a breaking change is a major bump. The mechanics, the first release and the maintainer's checklist are
in [releasing](../releasing.md). Every release gets a changelog entry generated from the same messages, so read
`CHANGELOG.md` before upgrading across a minor version in the `0.x` series.

## What is covered, and how

| Surface | Before 1.0 | From 1.0 (the intent) |
| --- | --- | --- |
| **Command line**: flags, subcommands, exit codes | May change in any minor release. A change is announced in the changelog and marked breaking in the commit. | Semantic versioning. |
| **JSON protocol**: commands and replies | Additive changes (a new command, a new field in a reply) keep old clients working if they ignore fields they do not know. Renaming or removing a command or field, or changing what it means, is a breaking change. | Semantic versioning. |
| **Snapshot schema** | The `schema_version` field says which layout you have. See below. | Semantic versioning, with `schema_version` as the contract. |
| **Rust library** (`vtee`) | No stability promise. The crate is `publish = false`, so it is not on crates.io; depend on it by git revision. Any release may change it. | To be decided before 1.0. |
| **Session files and the config file** | Internal. Do not parse `<name>.pid` or the socket paths yourself; use `vtee ls` and the CLI. The config file is read only for the options documented in [Sessions](sessions.md). | Same. |

## The snapshot schema

A snapshot carries `schema_version`, currently `1`. It changes only when an existing field is renamed, removed or
changes meaning. Adding a field never changes it, so write readers that ignore fields they do not know. If you
store snapshots or expected screens, store the version with them; when a snapshot arrives with a higher number
than the one you recorded, regenerate and review instead of trusting a comparison across versions. The field list
and the fields added so far are in the [protocol reference](protocol.md#schema-versioning).

A fix to wrong output is not a schema change. For example, tab-stop cells used to contain a literal TAB character
and now contain a space; the layout is the same, so `schema_version` stayed at `1`. Such fixes appear in the
changelog as `fix`. If you recorded a screen before a fix, you may need to record it again.

## Pinning

Until there are releases, pin a commit: build that revision from source, and name it in your test configuration.
Once releases exist, pin the tag (`v0.1.0` and so on). Either way, keep the pinned version and `schema_version`
next to anything you store.

## Platforms and toolchain

- The minimum supported Rust version is set as `rust-version` in `Cargo.toml` (1.89). Raising it is a normal
  change and is noted in the changelog; CI builds with stable Rust.
- Windows needs Windows 10 1803 or later, for ConPTY and AF_UNIX sockets. Linux is the primary platform. macOS is not
  tested.
