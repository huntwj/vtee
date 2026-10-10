# Extending vtee

Where to make the common changes: which files, which tests and which docs move together. The module map is in
[architecture](../architecture.md); the checks to run before a pull request are in
[CONTRIBUTING](../../CONTRIBUTING.md). Two rules apply to everything here:

- Anything a program can observe (a flag, a command, a reply, a snapshot field) is part of the interface.
  Additive changes are fine; renaming, removing or changing meaning is a breaking change. See
  [versioning](../guide/versioning.md).
- A change is not done until the test and the doc are in the same pull request.

## Add a protocol command

1. **`src/command.rs`.** Add a variant to `Command` (the `cmd` field is the variant name in `snake_case`). If it
   replies, add a variant to `CommandResult`; a command that returns nothing needs none. Then:
   - `authorize` says whether the command needs write access. Reads are allowed under every policy.
   - `execute` does the work, taking `&Tap`. If it needs new behaviour, add a method on `Tap` in `src/tap.rs`.
   - Add the command to the table in the unit test `authorize_matches_the_documented_rules_for_every_combination`,
     which checks every command against every write policy.
2. **`src/client.rs`.** `--text` output is written by `print_text`, which matches every `CommandResult`; the compiler
   will point at it. Script mode and `--connect` share the same `Command` type, so they need nothing else.
3. **`src/sockets.rs`.** Only commands that depend on *which connection* is asking (the write-access ones) need a
   special case in `handle_line`. Most do not.
4. **Docs.** [protocol.md](../guide/protocol.md): the command table, a section for the command, the error table if it
   adds an error, and the read-only row if it is a read. Add a line to [agents.md](../guide/agents.md) if an agent
   would use it.
5. **Tests.** A unit test next to the code; an end-to-end test in `tests/cli.rs` (runs on both platforms) or in
   `tests/linux_sessions.rs` / `tests/windows_sessions.rs` if it needs a live session.
6. **Commit** as `feat:`. A new command is additive, so `schema_version` does not change.

## Add a snapshot field

1. **`src/snapshot.rs`.** Add the field to `Snapshot` or `Cell`. Mark it `#[serde(default)]` (or give it a default
   function) so older JSON without it still reads; the unit tests in that file include a legacy-JSON case to copy.
2. Fill it in `build`. If it changes what the screen looks like (a colour, an attribute), make `redraw_sequence`
   reproduce it, because attached terminals get their first picture from there; the round-trip test checks this.
3. **The schema rule.** Adding a field does not change `SCHEMA_VERSION` (`src/snapshot.rs`). Renaming, removing or
   changing the meaning of an existing field does: bump the constant, and mark the commit breaking (`feat!`).
4. **Docs.** The field tables in [protocol.md](../guide/protocol.md#snapshot-schema), and the "added without a bump"
   list in [Schema versioning](../guide/protocol.md#schema-versioning).
5. **Tests.** A unit test that feeds escape bytes and asserts the field. If the field is a per-cell attribute the
   emulators agree on, extend the comparison in `tests/differential.rs` so the oracles check it too.

## Add a command-line flag

1. **`src/main.rs`.** Add the field to `Cli` (clap derive). The doc comment on the field becomes `--help`, so write
   it for a user, not for a maintainer. Declare conflicts with `conflicts_with`; `--script` lists the interactive-only
   flags it cannot be combined with, so a new session-level flag must be added there.
2. Thread the value into the spawn path (`run_spawn`) or the subcommand that uses it.
3. **Tests.** In `tests/cli.rs`: a parsing test (the flag is accepted, conflicts are rejected with exit code 64) and,
   for a behavioural flag, a test that starts a real session in an isolated `TMPDIR` and checks the effect.
4. **Docs.** [cli-reference.md](../guide/cli-reference.md); [sessions.md](../guide/sessions.md) if it changes naming,
   files or defaults; the [getting-started](../guide/getting-started.md) table if it is a new way to use the tool.
5. **Commit** as `feat:`; a flag that changes the meaning of an existing one is breaking.

## Add an emulation case

The [differential harness](testing.md#differential-emulator-tests) compares vtee's screen with two independent
emulators.

1. Add an entry to `tests/fixtures/differential/cases.json`: a unique `name`, the screen size, and an `input` string
   (`\\e` for ESC):

   ```json
   {"name": "sgr_bold", "cols": 20, "rows": 6, "input": "a\\e[1mbold\\e[0m plain"}
   ```

2. Regenerate the oracles with `sh tests/differential/regenerate.sh`, then triage any disagreement with
   `VTEE_DIFF_REPORT=/tmp/report.json cargo test --test differential`.
3. If all three emulators agree, you are done. If they differ, decide which is right by xterm's documented
   behaviour, and add an entry to `adjudications.json` with a verdict, a reason and the exact list of differences.
   An adjudicated bug that is later fixed makes the test fail until its entry is removed; that is intended.
4. If vtee is wrong and the fix is in vtee's mapping (`src/snapshot.rs`), fix it and remove the entry.

## Add a test

| You are testing | Put it in |
| --- | --- |
| A pure function, a table, a parser | A `#[cfg(test)]` module in the same file |
| Flag parsing, exit codes, what the binary prints | `tests/cli.rs` (cross-platform; some tests are Unix-only) |
| A live session with clients, on Linux | `tests/linux_sessions.rs` |
| A live session or a console feature, on Windows | `tests/windows_sessions.rs`, `windows_behavior.rs`, `windows_console.rs` |
| Escape-sequence handling | The differential harness, above |

Keep tests independent: a private `TMPDIR` (Linux) or `LOCALAPPDATA` (Windows) per test, a config directory pointed
at a temporary one, a guard that kills the processes the test starts, and waits on conditions with a timeout, never
fixed sleeps. A test that needs `less` or `vim` should fail, not skip, when `VTEE_REQUIRE_TOOLS` is set; copy
`tool_required` from `tests/linux_sessions.rs`. See [testing](testing.md#isolation).

## Before you open the pull request

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Run them on the platform you changed; CI runs both. The pull request title must be a Conventional Commit; see
[CONTRIBUTING](../../CONTRIBUTING.md#commit-messages).
