# Golden test vectors

Reference data for the golden file format and its comparison, from
[the design](../../../docs/design/snapshot-and-golden-contract.md) (section 6 and appendix A). A second implementation
of the checker, in any language, proves it agrees with `vtee` by passing these. They belong to grammar 1, snapshot
schema 1 and semantics 1.

The files are frozen. `MANIFEST` records a hash of each and the `SEMANTICS_VERSION` they were written under, and
`tests/golden_vectors/main.rs` fails when one changes without that number. Adding a vector is not a change. Every file is
byte-exact data (see `.gitattributes`): one golden has CRLF line endings and one is not valid UTF-8 on purpose.

## Layout

| Path | What it is |
| --- | --- |
| `runs/<id>-<name>/` | One check: a golden, the saved output of a run, options, and the exact verdict |
| `reject/*.golden` | A malformed golden, one per rule of design section 6.13. A reader must reject each (exit 65) |
| `roundtrip/*.golden` | Canonical goldens. Reading one and writing it back must give the same bytes |
| `regex/vectors.tsv` | The regex dialect: pattern, subject, operation, result (the header of the file explains it) |
| `escapes/escape-set.txt` | The scalars a writer escapes in `[text]` and in string literals (876 ranges, Unicode 15.1.0) |

The width sample is `tests/fixtures/conformance/width-sample.txt`, with the conformance cases. The command-line vectors
(`cli/`) and the `--update` vectors (`update/`) of the design arrive with the `--expect` runner, which they test.

## A run vector

| File | Content |
| --- | --- |
| `golden.golden` | The golden under test |
| `run.ndjson` | The saved output of a run, in the format of source (d) of design section 6.11: one JSON document per line, a reply of a `snapshot` command or a bare snapshot object. A `wait_for` reply that did not match is a problem of the run unless it has `"optional":true`; replies of other commands are ignored. A reply may carry the `name` the script gave the `snapshot`, and a `wait_for` the `line`, `text` and `timeout_ms` of its script line |
| `args` | The options of the run, one per line (empty for none): `--screen N`, `--fail-fast`, `--max-mismatches N` |
| `exit` | The expected exit code |
| `expected.txt`, `expected.json` | The exact text and JSON reports (the JSON followed by a newline). Present when the run produced a report, that is exit 0, 1 or 2 |
| `error` | The reference implementation's message. Present instead of the reports when the run fails before a verdict (exit 64 or 65). Only its first words are normative, as design section 6.10 says: `snapshot: ...` for a snapshot, the golden's path and line for a golden, `vtee: ...` for a usage error |

Run the check as `vtee --expect golden.golden --snapshot run.ndjson ARGS`. The report text is the specification; the
exact column spacing of a block is not significant (design section 6.9), so a second implementation may compare the
reports after collapsing runs of spaces. The JSON is compared as a value: key order is not significant. `way_forward` and
the commands in a refusal assume the command line above.

A vector id says what it pins: `R` the region, mask, assertion and version rules, `C` composite goldens, `P` pairing,
`W` problems of the run itself. The ids and what each pins are listed in design appendix A.1.

## Regenerating

`cargo test --test golden_vectors -- --ignored bless` writes everything except the escape set from
`tests/golden_vectors/defs.rs` and `more.rs`. Each definition states the verdict it must produce, so regenerating cannot
write down a wrong answer unnoticed. It refuses to change a recorded file while `SEMANTICS_VERSION` is unchanged.
