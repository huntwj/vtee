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
| **Snapshot** | `schema_version` says which layout you have and `semantics_version` what it means. See below. | Semantic versioning, with both numbers as the contract. |
| **Rust library** (`vtee`) | No stability promise. The crate is on crates.io, but the library API is not part of the stability promise. Any release may change it. | To be decided before 1.0. |
| **Session files and the config file** | Internal. Do not parse `<name>.pid` or the socket paths yourself; use `vtee ls` and the CLI. The config file is read only for the options documented in [Sessions](sessions.md). | Same. |

## The snapshot: two numbers

A snapshot carries two version numbers, `schema_version` and `semantics_version`, both required and both first in
the JSON. Each answers a different question.

| Number | Question | Changes when |
| --- | --- | --- |
| `schema_version` (currently `1`) | Can I read this JSON? | A field is renamed, removed or changes type. Adding an optional field never changes it, so write readers that ignore fields they do not know. |
| `semantics_version` (currently `1`) | Does it mean what I recorded? | What a cell, mode or position denotes for given program output changes, or a name is reported that was not before. |

A fix to wrong output changes the meaning but not the layout. For example, if tab-stop cells used to contain a
literal TAB and now contain a space, the layout is the same and `semantics_version` goes up. So does upgrading the
terminal emulator when it changes any frozen output, and so does reporting a new mode, attribute or attribute value:
a stored screen compared against the new build could find something it did not find before. If you store snapshots or
expected screens, store both numbers with them. When either differs from the one you recorded, regenerate and review
instead of trusting a comparison across versions. Snapshots recorded before 0.1.0 have no `semantics_version` and an
older cell layout; they are not supported.

Each change to `semantics_version` is an entry in the [snapshot semantics changelog](snapshot-semantics.md), and the
commit that makes it has the `snapshot` scope (`fix(snapshot): ...`, or `feat(snapshot): ...`), which puts it under
"Snapshot output changes" in the release changelog. The field list and the layout are in the
[protocol reference](protocol.md#schema-versioning).

### What is frozen

`tests/fixtures/conformance/` holds what 0.1.0 promises: vtee's own snapshot for each of the 80 escape-sequence cases,
the width of 1,062 sample scalars, and the list of every field, mode, attribute and value a snapshot can report.
Behaviour that none of them exercises is not promised. When output that matters turns out to be uncovered, a case is
added and from then on it is frozen; adding a case or a scalar is not a version bump, changing or removing one is.
Tests fail if any of it changes without `SEMANTICS_VERSION` going up; see
[Conformance tests](../development/testing.md#conformance-tests).

## Pinning

Until there are releases, pin a commit: build that revision from source, and name it in your test configuration.
Once releases exist, pin the tag (`v0.1.0` and so on). Either way, keep the pinned version and both snapshot version numbers
next to anything you store.

## Platforms and toolchain

- The minimum supported Rust version is set as `rust-version` in `Cargo.toml` (1.89). Raising it is a normal
  change and is noted in the changelog; CI builds with stable Rust.
- Windows needs Windows 10 1803 or later, for ConPTY and AF_UNIX sockets. Linux is the primary platform. macOS is not
  tested.
