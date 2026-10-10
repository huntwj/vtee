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
| `cli/<id>-<name>/` | One command line that is refused or fails before it compares anything: the exit code and the first words of the message |
| `update/<id>-<name>/` | One `--update` run on saved output: the golden before, the run, and the golden and the report afterwards |
| `reject/*.golden` | A malformed golden, one per rule of design section 6.13. A reader must reject each (exit 65) |
| `roundtrip/*.golden` | Canonical goldens. Reading one and writing it back must give the same bytes |
| `regex/vectors.tsv` | The regex dialect: pattern, subject, operation, result (the header of the file explains it) |
| `escapes/escape-set.txt` | The scalars a writer escapes in `[text]` and in string literals (876 ranges, Unicode 15.1.0) |

The width sample is `tests/fixtures/conformance/width-sample.txt`, with the conformance cases.

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

## A command-line vector

Run the command `vtee` with the lines of `args` as its arguments, in a directory that holds a copy of the vector's
files (a golden, a script and so on are working files of the vector; the other files below are its description),
with `CI` and `VTEE_ALLOW_UPDATE` unset except as `env` says.

| File | Content |
| --- | --- |
| `args` | The whole command line after `vtee`, one argument per line |
| `env` | Environment variables to set, `KEY=VALUE` per line. Present only when there are any |
| `exit` | The expected exit code |
| `stderr-first-line` | The normative first clause of the first line of standard error (the rest of the line is a hint). Absent when the command prints no message |
| other files | Files the command finds in its working directory (`g.golden`, `bad.ndjson`, ...) |

The vectors are one per row of the table of illegal combinations in design section 6.11, plus the guard on `--update`
(`CI` set refuses it, `VTEE_ALLOW_UPDATE=1` or an empty `CI` does not, a check is never guarded), `--update=migrate`,
an unknown `--update` mode, a missing or malformed golden, a missing script, a malformed script line, a program that
cannot be started, a session that cannot be reached, and a parser error (exit 64, not clap's 2). The message of
`L37-program-cannot-start` is preceded by `-qqq` because on Windows ConPTY logs its own error before `vtee` speaks.

## An update vector

Put `before.golden` (if the vector has one) and `run.ndjson` in an empty directory as `golden.golden` and
`run.ndjson`, run `vtee --expect golden.golden --snapshot run.ndjson ARGS` there, and compare.

| File | Content |
| --- | --- |
| `before.golden` | The golden before. Absent when the run creates it |
| `run.ndjson` | The saved output of the run, as for a run vector |
| `args` | The options of the run (`--update`, `--update=restamp`, `--portable`, `--screen N`, `--format json`), one per line |
| `after.golden` | The file `golden.golden` afterwards, byte for byte. Absent if the run leaves no file |
| `stdout` | The exact standard output |
| `exit` | The expected exit code |

They pin what a re-record keeps (masks, assertions, comments, `ignore` lines, `x-producer`), what it says moved
(`cells changed`, `new`, `dropped`, `stamp`), when nothing is written (`NOT WRITTEN`, `NOT RESTAMPED`, a golden of a
newer grammar), and the restamp.

## Regenerating

`cargo test --test golden_vectors -- --ignored bless` writes everything except the escape set from
`tests/golden_vectors/defs.rs`, `more.rs` and `commands.rs` (the last runs the real command to produce what it records). Each definition states the verdict it must produce, so regenerating cannot
write down a wrong answer unnoticed. It refuses to change a recorded file while `SEMANTICS_VERSION` is unchanged.
