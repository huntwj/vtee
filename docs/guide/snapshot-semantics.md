# Snapshot semantics changelog

Every change to `SEMANTICS_VERSION` is an entry here: the number, the release, the input that changes (the exact escape
sequence or scalar), the old and the new cells, and which conformance cases changed. A reader who finds a stored golden
refused for a semantics mismatch looks here to see whether the change touches what the golden compares. How the two
version numbers work and what each change costs: [versioning](versioning.md).

## Semantics 1 (0.1.0)

The first frozen meaning. It is whatever the 80 conformance cases, the width sample and the reported vocabulary in
`tests/fixtures/conformance/` say, produced by `alacritty_terminal` 0.26.0. There is no earlier semantics: snapshots
recorded before 0.1.0 are not supported.
