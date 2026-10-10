## What and why

<!-- What changed, and the reason. The title above becomes the commit message on `main`. -->

## Checklist

- [ ] The title is a [Conventional Commit](https://www.conventionalcommits.org/) such as `fix(session): ...` or
      `feat: ...` (at most 100 characters). It decides the next version number; add `!` for a breaking change.
- [ ] `cargo fmt --check` passes.
- [ ] `cargo clippy --all-targets -- -D warnings` passes.
- [ ] `cargo test` passes on the platform I changed (CI runs both).
- [ ] New behaviour has a test; a bug fix has a test that failed before the fix.
- [ ] Documentation is updated: [`docs/guide/`](https://github.com/huntwj/vtee/tree/main/docs/guide) for
      anything a user or a program can observe (flags, commands, snapshot fields), with the examples run for real.
- [ ] If this adds or changes a snapshot field, the rule in
      [Extending vtee](https://github.com/huntwj/vtee/blob/main/docs/development/extending.md#add-a-snapshot-field)
      is followed.
