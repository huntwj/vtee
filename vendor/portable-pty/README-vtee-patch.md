# Local patch: this is not upstream `portable-pty`

Vendored from `portable-pty` 0.9.0 (crates.io), unmodified except for one line in
`src/win/psuedocon.rs`'s `CreateProcessW` call: adds `CREATE_NEW_PROCESS_GROUP` to the creation
flags. Everything else - source, `Cargo.toml`, `LICENSE.md` - is the published crate as-is.

**Why**: without that flag, a spawned child inherits `vtee`'s own process group, and Windows
delivers console-control events (Ctrl+C/Break) by process-group membership rather than strictly
by which console a process is attached to - so even though ConPTY attaches the child to its own
hidden console, a break event aimed at the parent (or generated during normal console
setup/teardown) can still reach the child, killing it almost instantly
(`STATUS_CONTROL_C_EXIT`). Full writeup, including how this was found and the real-pty
verification behind it: `docs/decisions.md`.

**Upstream**: tracked at [wezterm/wezterm#7025](https://github.com/wezterm/wezterm/issues/7025)
(filed June 2025, unaddressed as of 2026-10-08 - not because the project is abandoned; `wezterm`
has roughly 1,500 open issues and this one has no clear, universally-reproducible repro).

**Maintenance note**: wired in via `[patch.crates-io]` in the workspace `Cargo.toml`, so it's
picked up automatically wherever `portable-pty = "0.9.0"` is depended on - no code elsewhere
needs to know this exists. Drop this directory and the `[patch.crates-io]` entry the moment
upstream actually fixes #7025 and a new release ships; don't let a vendored fork outlive the
bug it exists for.
