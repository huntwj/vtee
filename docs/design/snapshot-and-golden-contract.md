# Snapshot and golden contract (design)

Status: **decided by the owner on 2026-10-10 (section 8, "Decisions"); the golden library, the run command (`--expect`, `--update`, `--update=restamp`) and the test vectors are implemented (`src/golden/`, `src/expect.rs`, `tests/fixtures/golden-vectors/`); `--update=migrate` is not (OD7).** Written and revised
2026-10-10 after two independent reviews and after the owner chose the run-and-compare test model. When implemented,
the normative parts (sections 3 to 6) move to the user guide and this page stays as the record of why. Of the rest, only the library,
the run command and the vectors exist; where a section says "today" it describes `main` before this work. The reviews and what was done with each finding are in appendix C.

**The test model.** A test is an app, an input and an expected output: `vtee --script flow.ndjson --expect
flow.golden -- APP`. The script is the existing NDJSON script; each `snapshot` in it is a checkpoint; the screens a run
produces, in order, are its output; a **golden** is the file holding the expected output (one or more screens). Check,
record, restamp and migrate are all options of that one command (`--expect`, `--update[=MODE]`, 6.11).
There is no `golden` subcommand: "golden" names the file kind and its extension.

The snapshot JSON and the golden file format are a long-lived contract. A stored golden outlives any one `vtee`
version (the TinyFugue project will use goldens for years, across Windows and Linux, to check a faithful port of tf), so
the formats are designed once, carefully, before 0.1.0 is tagged. Internal code stays free to change behind the
contract.

## 1. Goals, non-goals and who depends on this

**Goals**

1. A stored golden either still means what it meant when it was recorded, or `vtee` says so clearly. It never
   silently passes or fails for a reason that is not about the program being tested.
2. Adding something to `vtee` that a golden compares (a new mode token, a new cell attribute) never makes a stored
   golden *fail* for a reason that is not about the program: it changes a version number, so the golden is *refused*
   with a clear message until it is restamped or re-recorded (section 4.1; findings TF1 and FA-A1 in appendix C; owner
   decision OD8). This is a goal of its own because the first draft of this document got it wrong.
3. A language-neutral format: no Rust-isms in the JSON, no tables a reader needs to parse the files (Unicode width or
   category data, for example).
4. A small frozen core, with named, reserved paths for everything we know we will add.
5. Goldens that are small, diff well in git and are readable by a person reviewing a change.
6. One meaning on Linux and Windows, apart from a short, named list of platform differences that a golden can opt out
   of explicitly (section 6.14).

**Non-goals**

- Pixels, fonts, themes or resolving palette colours to RGB. A cell says `idx:1`, never what red looks like.
- Grapheme-cluster rendering. A cell is what the emulator's width model says it is (section 3.4).
- Timing. A snapshot is a state, not a movie.
- The NDJSON command set. Commands are covered by [versioning](../guide/versioning.md); only the shape of the
  `snapshot` object, which commands return, is covered here.

**Who depends on it**

| Dependant | What it relies on |
| --- | --- |
| Stored goldens (TinyFugue and others) | The golden grammar, `schema_version`, `semantics_version`, the reported vocabulary (section 3.8), and the meaning of every cell |
| AI agents reading snapshots | Field names, types and defaults; `lines` as a convenient view |
| Procedural tests in any language | The same, plus the exit codes of an `--expect` run and its JSON report |
| `vtee`'s own conformance set | The meaning of cells for the 80 escape-sequence cases and the width sample (section 5.4) |

## 2. What was measured

These are the facts the decisions below rest on. Each was run on `main` (built natively on Windows 11 build 26300, and in
the `vtee-test` Docker image for Linux) unless it cites TinyFugue's recorded screens
(`tests/screens-draft/recorded/`, read-only) or an independent review. E1 to E14 are from the first draft and were
reproduced by the reviewers; E15 to E26 were added in the revision after the reviews, and E27 to E31 in the revision
for the run-and-compare model.

| # | Observation | How it was obtained |
| --- | --- | --- |
| E1 | An 80x24 snapshot is about 413 KB of JSON (413,173 to 413,187 bytes depending on platform), roughly 215 bytes per cell, because every cell repeats twelve fields. | `vtee --script s.ndjson --cols 80 --rows 24 -- cmd /c "echo hi"`, length of the output line; reproduced by the Fable review |
| E2 | With defaults omitted, a whole snapshot is 11,501 bytes for TinyFugue's `tf-session` screen and 14,996 for `top` (down from 311,658 and 311,119: 27 and 21 times smaller). The cell array alone is 9.3 to 14.3 KB in the first draft's converter and 9.3 to 12.8 KB in the review's. | Three independent converters; the first two over TinyFugue's `run1.json` files |
| E3 | `byte_offset` is not comparable across platforms or even across line disciplines. `printf 'hi\r\n'` under `sh` on Linux gives 5 (the tty turns `\n` into `\r\n`); `cmd /c echo hi` under ConPTY gives 65 (ConPTY adds its own sequences). | `--script` with `wait_exit` then `snapshot`, on both |
| E4 | `title` differs by platform: `null` on Linux for a program that sets none; under ConPTY it is the host program's path, for example `"C:\\windows\\System32\\WindowsPowerShell\\v1.0\\powershell.EXE"`. | Same runs |
| E5 | A wide character is two cells: the character, then a cell whose `ch` is the empty string (`你好X` gives `"你","","好","","X"`). | Linux probe, 40x6 |
| E6 | A ZWJ sequence is not clustered. Man, ZWJ, woman gives cells `"👨\u200d"`, `""`, `"👩"`, `""`: the ZWJ is appended to the emoji before it and each emoji takes two cells. | `probes.py`, a scratch probe script that is not in the repository |
| E7 | A combining mark is appended to the previous cell (`e` then U+0301 is one cell, `"é"`, two scalars). No normalisation is applied. | Same |
| E8 | Control characters never reach a cell: U+0001, U+007F, U+0085 and BEL were dropped (`Term::input` returns early when `c.width()` is `None`). A private-use character (U+E000) is stored verbatim and prints as nothing, so a JSON reader sees a cell that looks empty but is not (`ch` has length 1; the continuation cell has length 0). | Same; `alacritty_terminal-0.26.0/src/term/mod.rs` line 1062 |
| E9 | OSC 8 `id=x` is dropped; only the target URI survives (`hyperlink` is the URI). | Same |
| E10 | A line of 45 characters on 40 columns gives row 0 with 40 characters and row 1 with `ABCDE`. The snapshot does not say the break was a soft wrap, although `alacritty_terminal` records it (`Flags::WRAPLINE` on the last cell of the row). This is why `wait_for` cannot match a wrapped line. | Same; `term/mod.rs` `wrapline()` |
| E11 | `alacritty_terminal` also tracks SGR 8 (conceal, `Flags::HIDDEN`), which the snapshot does not report. | `term/cell.rs` |
| E12 | The emulator can report ?1000, ?1002, ?1003, ?1004, ?1005, ?1006, ?1007, ?2004, ?1, keypad mode and several more; `vtee` reports five today. | `term/mod.rs`, `TermMode` |
| E13 | The cursor can rest on the continuation cell of a wide character; the emulator's own renderer moves it back one column, `vtee` reports the raw position. | `RenderableCursor::new` against `src/snapshot.rs` |
| E14 | A grammar sketch round-trips real screens with zero differences. Reproduced independently by two reviewers: 50 screens (Fable) and TinyFugue's five tf screens plus `top` (both). | Two throwaway checkers written from the draft alone |
| E15 | **ConPTY absorbs private modes.** Under ConPTY `ESC =` (DECKPAM), `?1000h`, `?1002h` and `?7h` never reach the emulator (they are absent from the raw stream), while `?2004h` and `?25` do. The project's own measurement says the same for `?1` and `?1006`. Linux passes every one of them through. | `vtee --script ... --log w.cast -- powershell -File probe.ps1`, then `grep` of the raw log, natively on Windows 11; the same probe on Linux |
| E16 | **ConPTY injects modes of its own.** The raw stream of a plain `cmd /c "echo hi"` contains `ESC[?1004h` at the start and `ESC[?1004l` at the end, and `ESC[?9001h` / `ESC[?9001l` (win32 input mode). | Same, raw log |
| E17 | **ConPTY garbles SGR 58 (underline colour).** `ESC[58;2;200;10;20m` arrives as `ESC[2m` (dim) and `ESC[58;5;9m` arrives as `ESC[5m` then `ESC[9m` (blink, strike); the colour is lost and a spurious `dim` or `strike` appears. `ESC[4:3m` (curly underline) passes through. On Linux the colour is kept (`Rgb [200,10,20]`, `Indexed 9`). | Same probe, snapshot cells and raw log |
| E18 | **The emulator keeps one mouse report mode and one mouse encoding at most; setting another replaces the earlier one, and a reset clears only its own mode.** `?1000h` followed by `?1002h` leaves only `?1002`; `?1003h` then `?1000l` still leaves `?1003` (the reset names a mode that is not the current one, so nothing changes); `?1003h` then `?1003l` leaves none; `?1006h` then `?1005h` leaves only the later encoding, `?1005`; `?1006h` then `?1005l` leaves `?1006`; a report mode and an encoding are independent (`?1000h` plus `?1006h` reports both). Verified 2026-10-10 on Linux with 12 sequence pairs through the real binary. | Linux, `modes` after each sequence (today's vocabulary only reports `?1000`, `?1006`, so absence of `?1000` is the evidence) |
| E19 | **Width classes** of 1,062 scalars in about 30 groups (blocks and hand-picked ranges), asked of the real emulator: 303 are zero-width (appended to the previous cell), 430 are one cell, 329 are two. Notable: the soft hyphen U+00AD is zero-width; VS16 U+FE0F is zero-width and does not widen its base (`☺️` is one cell); the ZWJ is zero-width; each regional indicator is one cell; each skin-tone modifier is two cells; U+115F is two cells while U+1160 to U+11C2 are zero-width; the blank braille U+2800 and private-use U+E000 are one cell; U+3164 and U+FFA0 are zero-width. | `width.py` (scratch), one row `A<scalar>œ` per scalar, class = column of `œ` minus 1; result in appendix B |
| E20 | **A wide character at the last column** leaves a blank placeholder cell (flag `LEADING_WIDE_CHAR_SPACER`) and then the row is marked `WRAPLINE`; the character starts the next row. So a row can be soft-wrapped without its last real character reaching the last column. | `term/mod.rs` `input()` lines 1087 to 1101 and `wrapline()` line 968; probe: 19 `a`, then `你`, then `Z` on 20 columns gives row 0 ending in a blank and row 1 starting `你`, `""`, `Z` |
| E21 | A combining mark with nothing before it attaches to the blank cell under the cursor: printing only U+0301 at column 0 gives cell 0 = `" \u0301"` (a space plus the mark) and the cursor stays at column 0. If a base character follows it, that character overwrites the cell and the mark is lost. | Probe, Linux |
| E22 | `lines[r]` can have **more** scalars than `cols` (three `é` on a 3-column screen give 6 scalars), as well as fewer (a wide character adds a cell but one scalar). | Probe, Linux |
| E23 | **NDJSON line splitting trap.** `serde_json` writes U+2028 and U+2029 (and U+0085) unescaped. Python's `str.splitlines()` splits on them, so a snapshot line containing one of them breaks a reader that is not splitting on LF alone. Found while building the width sample. | `JSONDecodeError: Unterminated string` from `width.py` |
| E24 | **Regex engines differ.** Rust `regex` 1.x: `\d` matches U+0663, `\w` matches `é`, `\s` matches NBSP; `(?-u).` does not compile ("pattern can match invalid UTF-8"); `\b`, `(?i)`, `\x41`, `\p{L}` compile; look-around and back-references do not; `$` does not match before a trailing newline. Python 3.13 `re`: `\d`, `\w`, `\s` are Unicode by default, ASCII with `re.ASCII`; `$` also matches before a trailing newline; `finditer('a*','baa')` yields `(0,0)`, `(1,3)`, `(3,3)`. | Two scratch programs run in Docker |
| E25 | **Usage errors exit 64.** `src/main.rs` lines 198 to 202 map every clap parse error to `exitcode::USAGE` (64); clap's own default of 2 is never used. The new `--expect` and `--update` options go through the same parser, so exit 2 is free for "refused". | Source |
| E26 | **Unicode data for the escape set.** Python 3.13.5's `unicodedata` is Unicode 15.1.0. The categories of section 6.3 produce 876 ranges covering 966,490 scalars (almost all of them unassigned). | `escset.py` (scratch), output in appendix B |
| E27 | **Today's `--script` cannot fail a test.** A `wait_for` that times out prints its reply with `"matched":false` and the run still exits **0**, on Linux and on Windows. A program that cannot be started exits **71** (`failed to spawn ...`); a missing script file exits **66** (`failed to open script ...`). | Added in the run-and-compare revision: `vtee --script ok.ndjson --cols 20 --rows 3 -- sh -c 'echo hello; sleep 2'` with a `wait_for` for text that never appears, in Docker; the same on Windows 11; `-- /no/such/prog`; `--script /no/such.ndjson` |
| E28 | **A malformed script line stops the run late.** With `{"cmd":"snapshot"}`, `not json`, `{"cmd":"snapshot"}`, the first line runs and prints its reply, then the run stops with `vtee: bad command on script line 2: expected ident at line 1 column 2` and exit **65** (Linux and Windows). | Same runs |
| E29 | **A program that exits early does not stop the script.** With `-- sh -c 'exit 7'` and a script of `wait_for "zzz"` then `snapshot`, the `wait_for` times out (`matched:false`), the `snapshot` still returns the final (blank) screen, and `vtee` exits **0**: the child's status is not propagated in script mode. `wait_exit` after `sh -c 'exit 3'` replies `{"cmd":"wait_exit","running":false,"code":3,"timed_out":false}`. | Same runs, Docker |
| E30 | **What `--script` prints.** In JSON mode one line per reply: `{"cmd":"snapshot","snapshot":{...}}`, `{"cmd":"wait_for","matched":true,"snapshot":{...}}`, `{"cmd":"wait_exit",...}`; `send` prints nothing. With `--text` each result is `--- snapshot (byte_offset=N) ---` followed by the screen rows. Results go to standard output; errors to standard error. | Same runs |
| E31 | **The terminal size default depends on the terminal.** `--cols`/`--rows` default to the invoking terminal's size, else 80x24, and apply to `--script` too; so a script run's output can depend on where it was run. | `vtee --help` text (not measured separately) |
| E32 | **`--connect` silently ignores a program and `--script`.** `vtee --connect /tmp/none.sock --script ok.ndjson` and `vtee --connect /tmp/none.sock -- cmd` both go straight to connecting (and fail to connect); neither the script nor the program is looked at, and the parser reports no conflict. A script without a program says `vtee: a command is required`. | Same binary, Windows 11 |

## 3. The snapshot schema, decided

### 3.1 Shape at a glance

Schema version 1. Every top-level field is present in every snapshot; cell fields are written only when they differ
from their default (section 3.3). The order of keys has no meaning; `vtee` writes them as below, with
`schema_version` and `semantics_version` first so a reader can refuse before reading the rest.

```
{
  "schema_version": 1,
  "semantics_version": 1,
  "byte_offset": 2466,
  "cols": 80, "rows": 24,
  "lines": ["", "..."],
  "cells": [[{}, {"ch": "R", "fg": "idx:1", "bold": true}, ...], ...],
  "wrapped_rows": [],
  "cursor": {"row": 22, "col": 19, "shape": "block", "hidden": false},
  "modes": ["?1", "DECKPAM"],
  "alt_screen": true,
  "title": null,
  "scrollback": ["...", "..."],
  "producer": {"vtee": "0.1.0", "emulator": "alacritty_terminal 0.26.0"}
}
```

A real excerpt, cells 0 to 16 of one row of a 40x6 Linux screen holding a combining mark, a ZWJ family emoji, a
private-use character, a red bold `R`, a curly underlined `c` with an RGB underline colour, and a reverse-video
wide character (generated from a real snapshot by the first draft's throwaway converter, with the underline field
renamed as decided in section 3.2):

```
[{"ch":"é"},{},{"ch":"👨‍"},{"ch":""},{"ch":"👩‍"},{"ch":""},{"ch":"👧"},{"ch":""},{},{"ch":""},{},
 {"ch":"R","fg":"idx:1","bold":true},{},{"ch":"c","underline":"curly","underline_color":"rgb:#c80a14"},{},
 {"ch":"你","reverse":true},{"ch":"","reverse":true}]
```

Cell 9 is the private-use character U+E000: it looks the same as cell 3, a continuation cell, but its `ch` has one
scalar, not none.

### 3.2 Field by field

Today's shape on `main` is a **provisional** layout that also says `schema_version` 1 and was never released. The
shape in this document is the first frozen one and **keeps the number 1** (owner decision OD1): the two are
indistinguishable by number, so snapshots and goldens recorded before 0.1.0 are *unsupported* (there is no migration
from them; they are re-recorded). The right-hand column is the decision.

| Field | Today (provisional) | Problem | Decision (schema 1, frozen at 0.1.0) |
| --- | --- | --- | --- |
| `schema_version` | integer, first | none | Keep, first field. Value is 1 for the shape in this document. |
| `semantics_version` | absent | An output fix (TAB, ED 1) changes what a stored golden means but not the layout | **New**, second field. Integer, starts at 1. See section 4. |
| `byte_offset` | u64 | Meaningless across sessions and platforms (E3), and tempting to store | Keep. Defined as an opaque, per-session, monotonic count of bytes read from the pty. Valid only for ordering two snapshots of one session. Never stored in a golden. |
| `cols`, `rows` | integers | none | Keep. Integers 1 to 1000 (the resize limit). |
| `lines` | strings padded with spaces | Redundant with `cells`; the docs say "padded to `cols`" but with wide or combining characters the string has a different number of scalars than `cols` (E22) | Keep as a derived convenience (cheap, and `wait_for` and agents use it). Define: `lines[r]` is the concatenation of `cells[r][c].ch` for every `c`, trailing blanks included; its length in scalars is **not** `cols`. Writers must keep it consistent with `cells`; readers prefer `cells`. |
| `cells` | `rows` by `cols` dense objects | 215 bytes per cell (E1), growing with every new field | Keep `rows` by `cols`, but sparse: section 3.3. |
| cell `ch` | string | Continuation cell is `""`, easily confused with a private-use character visually | Keep. Default `" "`. Rules in section 3.4. |
| cell `fg`, `bg` | `"Default"`, `{"Indexed":N}`, `{"Rgb":[r,g,b]}` | Rust's default enum encoding; three shapes for one field; verbose | Strings: `"default"` (omitted), `"idx:N"`, `"rgb:#rrggbb"`. Same vocabulary as the golden file. `idx:1` and `rgb:#cd0000` stay different values on purpose: a program that changes how it asks for red is a visible change. |
| cell `bold`, `dim`, `italic`, `reverse`, `strike` | booleans, always written | verbose | Booleans, written only when `true`. |
| cell `underline` and `underline_style` | a boolean and a string that say the same thing | two ways to say one thing | One field, named **`underline`**, a string: `"single"`, `"double"`, `"curly"`, `"dotted"` or `"dashed"`, omitted for none. `underline_style` goes away. The name `underline` is kept (not `underline_style`) because of how old readers fail: a typed reader expecting a boolean fails loudly on a string, and a dynamic reader (Python, JavaScript) sees a truthy string, which is correct for "has an underline". Dropping the boolean under the old name would instead make an old reader silently see "no underline" everywhere. |
| cell `underline_color` | Color | none | Keep, colour string, omitted for default. May appear on a cell with no `underline` (the emulator tracks them independently); it is then an ordinary attribute and is compared like any other. |
| cell `blink` | always `false` | the emulator has no blink flag | Remove. The name is reserved: it will appear only if the emulator gains support. |
| cell `conceal` | not reported | SGR 8 is tracked by the emulator (E11) and ignoring it hides a real attribute | **Add**: boolean, written only when `true`. The text stays in `ch`. |
| cell `hyperlink` | string or null | `null` on every cell | Keep the name `hyperlink`, written only when set. URI only (E9). `hyperlink_id` is reserved. |
| `wrapped_rows` | absent | A soft wrap is invisible (E10) | **New**, always present: sorted array of row indices whose last cell wraps into the next row (`WRAPLINE`). It is also set when a wide character was pushed to the next row (E20). Empty array when none. |
| `cursor.row`, `cursor.col` | i32, usize | `row` is signed for no reason | Non-negative integers inside the screen. `col` may point at a continuation cell (E13); this is reported as is. |
| `cursor.shape` | `"Block"`, `"Underline"`, `"Bar"`, `"HollowBlock"`, `"Hidden"` | Mixed case; `HollowBlock` is for an unfocused window and `Hidden` duplicates `cursor.hidden` | `"block"`, `"underline"`, `"bar"`. Blinking variants collapse to the steady shape (blink is not tracked). `hollow_block` is reserved. |
| `cursor.hidden` | boolean | none | Keep: true when DECTCEM (?25) is off. |
| `cursor.pending_wrap` | absent | After a character is written in the last column the cursor stays there with a "wrap pending" flag that changes where the next character goes, and is invisible | **Reserved** (not emitted in 0.1.0): additive when added, as a boolean written only when true. |
| `modes` | strings in a fixed, arbitrary order | The order is an accident; vocabulary is mixed (`?1000`, `DECKPAM`) | A set, written sorted: private modes by number, then names alphabetically. Vocabulary in section 3.6. |
| `alt_screen` | boolean | Cannot express both buffers | Keep. It means "the active screen is the alternate one". The inactive buffer is reserved as `inactive` (section 10). |
| `title` | string or null | ConPTY sets one where Linux does not (E4) | Keep. From OSC 0 and 2 only; `null` when never set or reset. Platform difference is handled by the golden directive `ignore title`. |
| `scrollback` | strings, oldest first, text only | Large (up to 10,000 rows by the emulator's default); not part of the screen | Keep as informational, bounded by the emulator's history. Not compared by goldens. Cell-level history is reserved as `scrollback_cells`. |
| `producer` | absent | Debugging needs to know the build; but it must not become part of the contract | **New**: `{"vtee": "...", "emulator": "..."}`. Informational only: no reader may depend on it and a check never fails on it. |

### 3.3 Sparse cells

A cell is an object. **A field whose value equals its default may be omitted by the writer, and must be read as its
default when absent.** The defaults are: `ch` is `" "`, colours are `"default"`, flags are `false`, `underline` is none,
`hyperlink` is none. A blank default cell is `{}`.

The writer in `vtee` always omits defaults. Readers must accept both forms, which is the rule they already need for
older snapshots without a field. A writer must never write a default explicitly in one place and omit it in another
of the same document.

Why now: E1 and E2. Dense cells cost 215 bytes each, and every attribute we add later would grow every cell of every
snapshot by another 15 to 25 bytes; the two fields added last week cost TinyFugue 100 KB per screen. With omitted
defaults a new attribute costs nothing for the cells that do not use it. Measured on real screens a whole snapshot
drops from about 311 KB to 11 to 15 KB (E2). The alternative considered, a client-selectable "compact" flag, would
freeze two shapes; a single canonical sparse shape is simpler.

### 3.4 What `ch` may contain

- `ch` is the text of one cell: a base scalar, then any zero-width scalars the emulator appended to it (combining
  marks, ZWJ, variation selectors). No Unicode normalisation is applied (E7).
- A **blank cell** is a cell whose `ch` is exactly `" "` (U+0020) and nothing else. A cell holding a space plus a
  combining mark (E21) is not blank.
- A wide character occupies two cells: the first holds the character, the second is the **continuation cell**, whose
  `ch` is `""`. A cell with `ch == ""` is a continuation cell and nothing else is ever empty (E5, E8). The cell before
  a continuation cell is wide. The continuation cell keeps its own attributes.
- Control characters (C0, DEL, C1) and BEL never appear in a cell (E8). Private-use scalars and format characters do,
  verbatim.
- When a wide character does not fit in the last column, the emulator leaves that cell blank and moves the character to
  the next row. The blank cell is reported as an ordinary blank, and its row is in `wrapped_rows` (E20).
- Cells follow the emulator's width model: one scalar, one `unicode-width` value. A ZWJ sequence is **not** clustered
  into one glyph (E6): every emoji in it takes its own cells. A terminal that renders the sequence as one glyph
  occupies different columns; a golden records what the emulator did, not what a screen would show.
- The width tables belong to the `unicode-width` version the emulator was built with (0.2.2 at the time of writing).
  Changing them is an output change: `semantics_version` bumps (section 4). The frozen width sample of section 5.4
  is what makes that rule enforceable.

### 3.5 Colours and attributes

- Colours are `"default"`, `"idx:N"` (N from 0 to 255) or `"rgb:#rrggbb"` (lower-case hex). Palette indices 0 to 15 are
  the ANSI palette entries; SGR 30 to 37 and 90 to 97 map to `idx:0` to `idx:15`, the same as the 256-colour form of the
  same entry, because they are the same palette entry. The named foreground, background and cursor colours map to
  `"default"`. The dim variants map to their index and carry the `dim` flag separately. Bold is never converted to
  bright.
- The attributes a golden compares are: `bold`, `dim`, `italic`, `reverse`, `strike`, `conceal`, `underline`,
  `ulcolor` (the golden's name for `underline_color`) and `link` (the golden's name for `hyperlink`), plus the colours
  `fg` and `bg`, which are part of the core. The full frozen list is in section 3.8. Reserved, never emitted until the
  emulator supports them: `blink`, `overline`, `hyperlink_id`.
- A new attribute is a new optional field, which JSON readers ignore (section 4.3); but because a golden compares
  attributes, **reporting a new attribute is a `semantics_version` bump** (section 4.1), as is a new value of an
  existing one.

### 3.6 Modes

`modes` reports the input-affecting modes the program has turned on, as a set. The vocabulary at the 0.1.0 freeze
(the code reports exactly the five "reported today" tokens before this change: `?1000`, `?1006`, `?2004`, `?1` and
`DECKPAM`, checked against `src/snapshot.rs`):

| Token | Meaning | Status |
| --- | --- | --- |
| `?1` | application cursor keys (DECCKM) | reported today |
| `DECKPAM` | application keypad | reported today |
| `?1000` | mouse click reporting | reported today |
| `?1002` | mouse drag reporting | **new** |
| `?1003` | mouse motion reporting | **new** |
| `?1005` | UTF-8 mouse encoding | **new** |
| `?1006` | SGR mouse encoding | reported today |
| `?2004` | bracketed paste | reported today |

The emulator keeps at most one mouse report mode (`?1000`, `?1002` or `?1003`) and at most one mouse encoding (`?1005`
or `?1006`). Setting another replaces the earlier one, and a reset clears only the mode it names, so resetting a mode
that is not the current one changes nothing; the set reflects the emulator's model, not the sequences sent (E18).

The first draft proposed five new tokens: `?1002`, `?1003`, `?1004`, `?1005` and `?1007`. Two are **withdrawn for 0.1.0**
and reserved (owner decision OD9): `?1004` (focus reporting) is injected by ConPTY at the start of every session
whether or not the program asked for it (E16), so it would be set on Windows and not on Linux for the same program, and
`?1007` (alternate scroll) has no consumer. **Adding either later is a `semantics_version` bump** (section 4.1): every
newly reported token can change a stored golden's comparison, so each one arrives as a deliberate, changelogged
event. Autowrap (`?7`) is reserved with the other output-affecting modes (origin, insert, newline) and the kitty
keyboard flags (section 10); nearly every program leaves it on, so it would be the commonest token in every golden.

The platform differences in this vocabulary (E15) are in section 6.14.

### 3.7 JSON encoding of snapshots

- UTF-8, one JSON value per line for the NDJSON protocol, the **record separator is U+000A alone**. Readers must split
  on LF and nothing else (E23).
- Writers escape U+0085, U+2028 and U+2029 as `\u0085`, `\u2028`, `\u2029` in addition to what JSON requires, so that a
  reader that splits lines on Unicode line boundaries is not broken. (This needs a small custom formatter: a
  required change.)
- Numbers are integers. No fractions, no exponents.

### 3.8 The reported vocabulary at 0.1.0 (frozen)

Because every newly reported name is a `semantics_version` bump (section 4.1), the complete list of what a 0.1.0
snapshot reports and a golden compares is stated once, here. A name not in this table is not reported at semantics 1.

| Kind | Frozen list at 0.1.0 | Compared by a golden |
| --- | --- | --- |
| Mode tokens (`modes`) | `?1`, `?1000`, `?1002`, `?1003`, `?1005`, `?1006`, `?2004`, `DECKPAM` | yes, as a set, minus `ignore modes` |
| Cell attributes | `bold`, `dim`, `italic`, `reverse`, `strike`, `conceal`, `underline`, `ulcolor`, `link`, and the colours `fg`, `bg` | yes, as a set per cell, minus `ignore attrs` |
| Colour forms | `default`, `idx:0` to `idx:255`, `rgb:#rrggbb` | part of `fg`, `bg`, `ulcolor` |
| `underline` values | `single`, `double`, `curly`, `dotted`, `dashed` | part of `underline` |
| `cursor.shape` values | `block`, `underline`, `bar` | yes |
| Screen fields | `size`, `cursor` (row, col, shape, hidden), `modes`, `alt_screen`, `title`, `wrapped` | yes, each unless ignored |
| Never compared | `byte_offset`, `scrollback`, `producer`, `x-` keys | no |

**Reserved names** (never emitted at semantics 1; the emulator or a consumer would have to ask for them): modes `?1004`,
`?1007`, `?7` and the other output-affecting modes of section 10; attributes `blink`, `overline`; fields
`hyperlink_id`, `cursor.pending_wrap`, `inactive`, `scrollback_cells`, `cursor.shape` value `hollow_block`. **Each
reserved name arrives with a `semantics_version` bump**, a changelog entry and a `feat(snapshot)` or `fix(snapshot)`
commit, and it makes every stored golden refused until it is restamped or re-recorded (section 5.3).

## 4. The versioning model

Two numbers describe a snapshot, and a golden records both.

| Number | Says | Changes when |
| --- | --- | --- |
| `schema_version` | **Layout**: names, types and shapes of the JSON, and what each field means | A field or token is removed, renamed, retyped, or its default changes |
| `semantics_version` | **Meaning and reporting**: what an existing cell, mode or position denotes for given program output, **and which names are reported at all** | An emulation fix, an emulator upgrade, or a newly reported name (mode token, attribute, attribute value) changes what a stored golden would compare |

**A golden is only ever read against a build with the same two numbers.** Any difference is a refusal (exit 2, nothing
compared; sections 4.3 and 6.8). The way forward is `--update=restamp` (safe when nothing the golden compares would
differ), `--update` (re-record) or, for a layout change, `--update=migrate` (section 5.3). There is no overlap window
and no "compare anyway" mode (owner decisions OD3 and OD4).

Schema 1 and semantics 1 are the values at the 0.1.0 freeze. The provisional shape on `main` today also says schema 1
and was never released; **snapshots and goldens recorded before 0.1.0 are unsupported** and are re-recorded (owner
decision OD1). TinyFugue's recordings of the provisional shape fall under this and are re-recorded after step 2 of
section 9.

### 4.1 What bumps what

The first draft said that adding a mode token or an attribute needs no bump because old goldens "simply do not contain
it". Both reviews showed that this is false: a check run compares `modes` as a set and cells as full attribute sets,
so an old golden **fails** against a snapshot that reports the addition (demonstrated: expected `?1 DECKPAM`, actual
`?1 ?1004 DECKPAM`, exit 1, even with `ignore modes ?1 ?1000 ?1006`). Two repairs were considered.

1. *Every newly reported thing is a semantics bump.* Simple to state and to implement: one rule, no extra header lines,
   no undeclared-token handling. The cost is that additions make every stored golden refused until it is restamped
   (which is a scripted pass for goldens of programs that never use the new thing) or re-recorded (for those that do).
2. *Goldens declare the vocabulary they were written with, and the checker compares only what is declared.* An
   addition is then additive: an old golden keeps passing with a warning. The cost is two header lines, a checker rule,
   and the risk that a golden silently does not check something new.

**Decision: option 1** (owner decision OD8, which overrode the recommendation of option 2; the owner prefers strict,
simple rules). The consequence is accepted knowingly: each addition after 0.1.0 is a refusal of every stored golden
until restamped, so additions are deliberate and rare, and the full 0.1.0 vocabulary is frozen in one table
(section 3.8) so that anything we expect to want is decided before the freeze. The table that follows uses it.

| Change | `schema_version` | `semantics_version` | Notes |
| --- | --- | --- | --- |
| Add an optional field that goldens do **not** compare (a `producer` key, a new report field) | no | no | Readers ignore unknown fields |
| Add an optional cell or header field that goldens compare (a cell attribute, `cursor.pending_wrap`) | no | **yes** | A stored golden now has something it did not record; the comparison result of an unchanged program can change |
| Report a new **mode token** or a new **attribute name** | no | **yes** | Both reviews demonstrated the failure; the rule is one line: if it can change a comparison, it bumps |
| Add a new **value** of an existing attribute (a new underline style) | no | **yes** | A reported attribute now says something different for input that produced the old value |
| The emulator starts reporting an **existing** token for input where it did not (it now sets `?1000` for a sequence it used to ignore) | no | **yes** | The same program produces a different result |
| Remove or rename a field, token or attribute | **yes** | no | A name that no longer exists cannot be compared |
| Change a field's type or default | **yes** | no | |
| Change what an existing value means (for example `cursor.row` becomes 1-based) | **yes** | no | |
| An emulation fix that changes cells for existing input (TAB becoming a space, the ED 1 fix) | no | **yes** | The layout is unchanged; stored goldens may now differ |
| Upgrading `alacritty_terminal`, `unicode-width` or `vte` | no | **yes**, if the conformance set or the width sample changes | The frozen data decides (section 5.4) |
| A bug in reporting that is fixed (named colours reported as default, fixed earlier) | no | **yes** | |
| A platform difference (ConPTY hides modes, sets a title) | no | no | Handled by `ignore` directives and the `--portable` profile, not by versions |
| A new golden directive that changes how a golden is compared | the golden **grammar** number | no | Unknown header keys are an error (section 6.2) |
| A change to the golden grammar | the golden **grammar** number | no | |

Every real change should land in a row of this table. If one does not, that is a defect in the table, and the table is
amended in the same change (the review asked for counter-examples and found none that were unclassifiable once the
rule "if it can change a comparison, it bumps `semantics_version`" was added; a fix that changes only `wrapped_rows` is
"an emulation fix that changes existing output", so it bumps semantics).

### 4.2 The golden file's own numbers

The first line of a golden is `vtee-golden N`: the version of the golden **grammar**. It is 1 for the grammar in this
document. The draft in TinyFugue's `tests/screens-draft` is `vtee-golden 0` and, like every artefact recorded before
0.1.0, is unsupported (section 4). The header also records the `schema` and `semantics` the golden was recorded under
(section 6.2).

### 4.3 Reader rules

1. A JSON reader ignores any field it does not know. A golden reader ignores `x-` header keys and treats any other
   unknown header key as an error (section 6.2).
2. A snapshot or golden whose schema number is **higher** than the reader supports is refused, never partly read. One
   whose schema number is **lower** is refused as well, immediately, with exit 2 and a message that names both numbers
   and points to `--update=migrate` or a re-record. The reader supports only the current schema (owner decision OD3:
   there is no deprecation window and no second reader).
3. A golden whose `semantics` differs from the snapshot's is **refused, with nothing compared** (exit 2). There is no
   "compare anyway" mode and no flag to override the refusal (owner decision OD4). The message names both numbers and
   points to `--update=restamp` and to the semantics changelog (section 4.4).
4. A golden file produced by `--update=migrate` from an older schema carries the `semantics` number it was recorded
   under (a layout migration does not change meaning), or the word `unknown` where that is not recoverable;
   `unknown` behaves like a mismatch for every value, so the golden is refused until restamped or re-recorded.
5. A snapshot that arrives without a `schema_version` or a `semantics_version` is refused (exit 65 for a snapshot
   file): there is no unnumbered legacy form. A snapshot of the provisional pre-0.1.0 shape says `schema_version` 1 but
   has no `semantics_version` and uses the old field shapes, so it fails the same check and is refused as malformed.
6. `producer` and `byte_offset` never take part in a comparison.
7. A reader supports **every grammar number up to its own** (parsing an old grammar is cheap, and the grammar number
   says how to read a file, not what its screens mean; the schema and semantics numbers still have to match). A grammar
   number is bumped only for a change old readers cannot understand, never to retire old goldens. This applies to
   **released** grammar numbers (1 and later); the unreleased draft `vtee-golden 0` is not one and is rejected
   (OD1). It is the one place where the rules are more permissive than the strict stance of rules 2 and 3; see the
   open question Q1 in section 8.

### 4.4 The semantics changelog

Every `semantics_version` bump is an entry in `docs/guide/snapshot-semantics.md`: the number, the release, the input
that changes (the exact escape sequence or scalar), the old and the new cells, and which conformance cases changed, so
that a consumer can search their raw output for it. Semantics 1 is the state of 0.1.0 and includes the fixes already
made (TAB, named colours, wide continuation cells, underline styles), the reported vocabulary of section 3.8, and the
three known deviations of section 8 (OD6), each listed precisely.

## 5. The stability promise

### 5.1 Frozen at 0.1.0

- The schema 1 field names, types, defaults and meanings of section 3, and the reported vocabulary of section 3.8.
- The golden grammar of section 6: the file syntax, the header keys, the escape set (section 6.3 and appendix B), the
  region and comparison rules, the regex dialect, the report shapes, the exit codes and the run command of 6.11
  (`--expect`, `--update`).
- The rules of section 4.1, including the rule that anything that can change a comparison bumps `semantics_version`.
- The semantics of the frozen data of section 5.4 at semantics 1: the 80 escape-sequence cases, the width sample and
  the test vectors.

### 5.2 Allowed without a version bump

Only what no golden compares: new optional JSON fields that goldens ignore, new `producer` keys, new report fields
(readers of the JSON report ignore unknown fields), new `vtee` subcommands and options that do not change a
comparison. **New mode tokens, new attributes and new attribute values are not in this list**: they are
`semantics_version` bumps (section 4.1). New golden *sections* and new comparing directives need a new grammar number.
Internal changes are unrestricted so long as the frozen data does not change.

### 5.3 Version mismatches, and the way forward

There is no deprecation window and no second reader (owner decisions OD3 and OD4). A reader understands the current
`schema_version` and `semantics_version` only; every other golden is **refused at once** (exit 2, nothing compared),
and the refusal names the way forward:

| The golden says | The tool's number differs because | Way forward |
| --- | --- | --- |
| `schema` lower than the tool's | the layout changed in a later release | `--update=migrate` (below), or re-record with `--update` |
| `schema` higher than the tool's | the golden was written by a newer `vtee` | upgrade `vtee`; never partly read |
| `semantics` different | what existing output means, or what is reported, changed | `--update=restamp` if nothing the golden compares would differ (it checks, and says if so), otherwise re-record with `--update`; the semantics changelog (4.4) says which inputs are affected |
| `semantics unknown` | a migration could not recover the number | the same as a different number |

- **`--update=restamp`** compares the golden with a fresh run under the current build. If every compared cell, header
  field and assertion of every screen is identical it rewrites only the `semantics` line (and `x-producer`) and exits 0;
  otherwise it writes nothing, prints the differences and exits 1. It is the safe answer to "an upgrade changed
  nothing my golden checks", and the only override there is: no flag makes a check ignore a mismatch.
- **`--update=migrate` is kept for every released schema and grammar, with no end date.** It converts a golden from an
  older layout to the current one mechanically and **never invents facts**: where the old form cannot supply a fact it
  adds the matching `ignore` (for example `ignore wrapped` when an older layout had no wrap flags) or writes
  `semantics unknown`, so the migrated golden is honest about what it no longer verifies. Each schema or grammar bump
  ships its own migration table in the user guide, tested by vectors. **There is no migration table today: 0.1.0 is the
  first release, and artefacts recorded before it (the provisional snapshot shape, TinyFugue's `vtee-golden 0` draft)
  are unsupported and are re-recorded** (owner decision OD1). `migrate` itself is not built for 0.1.0 (owner decision
  OD7); it follows no later than the first schema or grammar bump.
- **Grammar numbers.** A reader supports all grammar numbers up to its own (section 4.3, rule 7), flagged as an open
  question in section 8.
- A **semantics** difference is never a layout change, so `migrate` does not apply to it.

### 5.4 The frozen conformance data

Four sets of files are reference data. They are what makes "a stored golden still means what it meant" testable.

1. **The 80 escape-sequence cases** in `tests/fixtures/differential/cases.json`, with two files per case: the input
   (already exists), and `tests/fixtures/conformance/expected/<case>.json`, vtee's own snapshot of that input in
   canonical compact form without `byte_offset` and `producer`. (Today the oracle files hold `avt` and `vt100`'s
   output, not vtee's, so a change that moved vtee and both oracles together would not be caught.) The expected files
   are compared as parsed values, so key order and whitespace never matter. The cases feed bytes straight to the
   emulator, so they are platform independent; the resize cases need a live pty and run on Unix only, as they do today.
2. **The width sample** (appendix B): the width class of 1,062 scalars chosen in about 30 groups (blocks and hand-picked ranges) (E19). The 80 cases
   exercise escape sequences and almost no Unicode data, so a `unicode-width` or emulator upgrade that changes how wide a
   scalar is would pass them and silently change goldens (finding FA-A3 in appendix C). The sample is run through the real
   emulator by a test and compared to the file.
3. **The escape set** (appendix B): the exact scalar ranges a writer escapes in a golden. It is data, not a function of
   the tool's own Unicode tables.
4. **The test vectors** (appendix A): golden, snapshot, expected verdict and report triples; regex vectors; round-trip
   files. They let a second implementation of the checker, in another language, prove it agrees.

**The frozen data is the contract:** behaviour that no case exercises is not promised. When a consumer finds output that
matters and is not covered, we add a case (which is not a bump) and from then on the output is frozen. The rules:

1. Changing an expected file, the width sample or an expected verdict requires a `semantics_version` bump in the same
   change, an entry in `docs/guide/snapshot-semantics.md`, and a commit whose scope is `snapshot`
   (`fix(snapshot): ...`, or `feat(snapshot)!: ...`).
2. Adding a case, a vector or a scalar to the sample is not a bump.
3. Fixing a recorded deviation (ED 1 on the second row, origin mode reset, mode ?47) is a bump by definition.
4. **Any upgrade of `unicode-width`, `vte` or `alacritty_terminal` is a semantics bump if and only if the frozen data
   changes**, and the upgrade's pull request must show the test run that proves it either way.
5. Enforcement: a small CI job fails if a pull request changes anything under `tests/fixtures/conformance/` or
   `tests/fixtures/golden-vectors/` without also changing `SEMANTICS_VERSION` in `src/snapshot.rs` and the semantics
   changelog; commitlint gets a `snapshot` scope; `release-plz` groups `snapshot`-scope commits under "Snapshot output
   changes" in the changelog.

The differential oracles and `adjudications.json` stay as they are: they answer "is vtee right?", while the
conformance files answer "did vtee change?".

## 6. The golden file format and the run command, version 1

A golden is a text file holding the **expected output of a run**: the screens that a test script snapshots, in order,
with the volatile parts declared. A test is three things, `app + input + expected output`:

```
vtee --script flow.ndjson --expect flow.golden -- tf          check (the default)
vtee --script flow.ndjson --expect flow.golden --update -- tf  record, or re-record
```

The script is the existing NDJSON script and is not changed: each `snapshot` command in it is a checkpoint, and the
screens the run produces, in order, are its output. There is no `golden` command: "golden" names the file kind and its
extension, and everything that creates, updates, checks or migrates one is an option of the single run command
(6.11). This section is **normative** for the `vtee-golden 1` grammar and the run command. Words in capitals in a
grammar line are placeholders; `[x]` is optional. All row and column indices are **0-based**, like `cursor.row` and
`cursor.col` in the snapshot; screens are numbered from **1**, in the order the run produced them.

### 6.1 File syntax

- UTF-8 without a byte order mark; LF line endings; the file ends with a newline. These are what writers produce.
- **Readers accept CRLF** by removing one CR immediately before each LF, and accept a missing final newline. Reason:
  git's `autocrlf` and Windows editors change line endings silently and outside the author's control, and a carriage
  return cannot otherwise appear in a line (a CR in cell text is written `\u{d}`, in a JSON string `\r`). A CR anywhere
  else, a NUL byte, or invalid UTF-8 is an error. Repositories should still say `*.golden text eol=lf` in
  `.gitattributes`; `--update` writes LF and so shows the change.
- **Readers reject a leading byte order mark** with a message that names it and says to remove it. Unlike a line
  ending, a BOM is added only by a deliberate save in an editor, `vtee` never writes one, and accepting it would let
  two files that differ in their first bytes mean the same thing.
- Line oriented. A line `[name]` starts a section. Lines before the first section are the **file header**.
- A line whose first character is `#` is a comment. Comments are allowed in both headers, in `[masks]` and in
  `[assert]`; they are not allowed in `[text]` or `[attrs]`, whose content is regenerated (an error there).
- Blank lines are ignored everywhere. They cannot be mistaken for rows, because a `[text]` row always contains `|`.
  Writers emit exactly one blank line before each section.
- Trailing whitespace on header, `[attrs]`, `[masks]` and `[assert]` lines is ignored by readers and never written.
  In `[text]` a literal space is a blank cell (6.3).

**The shape of a file.** A golden always has this shape, even when it holds one screen:

```
FILE      =  FILE-HEADER  [ "[masks]" ... ]  SCREEN { SCREEN }
SCREEN    =  "[screen N]"  SCREEN-HEADER  "[text]" ...  [ "[attrs]" ... ]  [ "[masks]" ... ]  [ "[assert]" ... ]
```

- The **file header** (6.2) holds what is true of the whole run: the versions, `ignore` directives that
  apply to every screen.
- A `[masks]` section **before the first `[screen]`** is the **shared** mask list: it applies to every screen (6.5).
- `[screen N]` starts a screen block. `N` is the screen's 1-based position and **must equal** the number of screen
  blocks so far plus one, like a row label (a reader checks it; it makes a reordered or half-merged file an error and a
  diff readable). A file has at least one screen block.
- A screen block has its own **screen header** (the lines up to its first section), then `[text]` (required), and
  `[attrs]`, `[masks]`, `[assert]` (each optional, each at most once per screen, in any order). A writer omits `[attrs]`
  when no cell of the screen has a non-default attribute; a reader treats an absent and an empty `[attrs]` the same.
  The sections after a `[screen N]` line belong to that screen until the next `[screen]` line.
- **Errors:** a section outside this shape (a `[text]`, `[attrs]` or `[assert]` before the first `[screen]`; two shared
  `[masks]`; two of the same section in a screen), an unknown section name, a screen header key in the file header or
  a file header key in a screen header, and the **reserved** section names `[exit]` and `[screen]` without a number.

*Why a one-screen golden is a composite of one.* The first draft of this section described one screen with a flat
layout, and the run-and-compare model needs several. Two shapes (flat for one screen, blocks for more) would mean two
parsers, two canonical forms, and a conversion the day a second checkpoint is added to a script, and a conversion is
a rewrite of a stored file. One shape costs one extra line (`[screen 1]`) and a small header split; because nothing
has been released, nothing needs migrating, and no grammar number is spent later. The flat layout of TinyFugue's draft
(`vtee-golden 0`) was never released and is unsupported (5.3); those goldens are re-recorded.

### 6.2 File header and screen header

One `key value` pair per line. The first line of the file must be `vtee-golden N`; the other lines of each header may
come in any order (duplicates are an error except for `ignore` and `x-` keys). The writer's order, for the **file
header**:

```
vtee-golden 1                      required, first line
schema 1                             required: the snapshot schema the golden was recorded under
semantics 1                          required: an integer, or the word unknown (a migration could not recover it)
ignore modes ?1 ?1000                optional, repeatable; applies to every screen; see below
ignore title
x-producer vtee 0.1.0 alacritty_terminal 0.26.0     advisory
```

and for each **screen header** (the lines after `[screen N]`):

```
size 80x24                           required: columns x rows
cursor row=22 col=19 shape=block hidden=false      required
modes ?1 DECKPAM                     required: zero or more tokens, space separated (a bare `modes` for none)
alt_screen true                      required: true or false
title null                           required: null or a JSON string literal, "my title"
wrapped 22                           optional: row indices that soft-wrap (omitted when none)
name "long-line"                     optional: a JSON string, the checkpoint's label (6.8)
ignore wrapped                       optional, repeatable; applies to this screen only
```

- **`schema` and `semantics`** are the two version numbers of section 4. A check requires both to equal the running
  build's, before it compares anything (6.8); there is no vocabulary header, because a newly reported mode token or
  attribute is itself a `semantics` bump (section 4.1). A `modes` token or an attribute that is not in the frozen
  vocabulary of section 3.8 is an error in a golden (6.13). The numbers are per file, not per screen: all screens of a
  run were recorded by one tool.
- **`ignore`** names a part the check skips: `ignore title`, `ignore alt_screen`, `ignore wrapped`,
  `ignore cursor` (all of position, shape and hidden), `ignore cursor.position`, `ignore cursor.shape`,
  `ignore cursor.hidden`, `ignore modes TOKEN...` (the listed tokens are removed from both sides) and
  `ignore attrs NAME...` (the listed attributes, including `fg` and `bg`, are removed from every cell comparison; the
  cell *text* is still compared). Several `ignore` lines for the same part are merged. `ignore modes` without a token
  is an error. **Scope:** an `ignore` in the file header applies to every screen; an `ignore` in a screen header applies
  to that screen; a screen ignores the **union** of both. There is no way to cancel a file-level `ignore` in one
  screen; an `ignore` that only some screens need belongs in those screens' headers.
- A key beginning with `x-` is informational and ignored, in either header. **Any other unknown key is an error**, so a
  golden written by a newer tool that adds a directive changing comparison can never be silently misread. A key in
  the wrong header (`size` in the file header, `schema` in a screen header) is an error.
- `byte_offset` and `scrollback` are never stored.
- `wrapped` lists the rows whose last cell wraps into the next row (`wrapped_rows` in the snapshot). Order is not
  significant to a reader; writers sort ascending.
- `name` is a label for the checkpoint, written when the script gave the `snapshot` command a `name`. Names are not
  required to be unique. They are compared only when both sides have one (6.8).
- `title` uses the JSON string syntax of section 6.12.
- `cursor` always carries all four fields. `size` is `WxH` in decimal with no leading zeros.

### 6.3 `[text]`

Exactly `rows` lines, one per row: the row index (decimal, right-aligned with spaces to the width of the largest index),
then `|`, then the row's **cell tokens**. A reader splits the line at the **first** `|` and requires the label, trimmed,
to be the decimal index of the row, in order.

A row is a sequence of tokens, one token per cell, except that appended scalars share the cell they follow:

| Token | Meaning |
| --- | --- |
| a literal scalar | one cell whose `ch` is that scalar. Writers use it for every scalar that is neither a backslash nor in the escape set; readers accept any scalar except a backslash |
| `\\` | one cell holding a backslash |
| `\u{H}` | one cell whose first scalar is U+H (1 to 6 hex digits, a Unicode scalar value: not a surrogate, at most 10FFFF). **Writers** use it for every scalar in the escape set, in lower-case hex without leading zeros, and only for those. **Readers** accept it for any scalar, any case, any leading zeros, so `\u{41}` means `A` |
| `\.` | a **continuation cell**, `ch` is `""`. It is an error at the start of a row or directly after another `\.` |
| `\+{H}` | **appends** scalar U+H to the previous cell (a combining mark, ZWJ, variation selector). It is an error at the start of a row or after a `\.`, and H follows the same rules as in `\u{H}` |

Any other backslash sequence is an error. Every scalar of a cell after the first is written as `\+{H}`, even a printable
one. This is what lets a reader find cell boundaries without any Unicode table (the width of a scalar, whether it is a
mark): the writer knows the cells and the file says so. A reader that sees `\.` after a literal scalar does not check
that the scalar is wide: the golden says what the cells are, and the comparison finds a disagreement.

**Trailing blank cells are omitted** (a blank cell is `ch == " "` exactly, section 3.4), whatever their attributes
(those live in `[attrs]`), so a row's tokens may cover fewer cells than `cols`; the missing cells are `" "`. A row
with nothing to say is `NN|`. A row therefore never ends in a space, which keeps a golden safe from editors that strip
trailing whitespace. A row that describes more cells than `cols` is an error. A reader accepts trailing literal spaces
and treats them as blank cells.

**The escape set** is defined by rule and frozen as data, so that it never depends on the Unicode tables of the tool
that wrote the file. A scalar must be written as `\u{H}` if it is any of: a code point whose general category is `Cc`,
`Cf`, `Cs`, `Co`, `Cn`, `Zs` (except U+0020), `Zl`, `Zp`, `Mn` or `Me`; or one of U+2800, U+115F, U+1160, U+3164,
U+FFA0 (visually blank scalars outside those categories). The categories are taken from **Unicode 15.1.0**; the
resulting 876 inclusive ranges (966,490 scalars, nearly all unassigned) are in appendix B and are the normative list.
A scalar that a later Unicode version assigns stays in the set: the set never shrinks, and a golden never changes
meaning because the tool that read it knows more characters. These are the scalars a reviewer cannot see or an editor
may alter: controls, invisible and combining characters, spaces other than U+0020, private use and unassigned
code points. Extending the set is a change of the grammar number.

### 6.4 `[attrs]`

Lists the cells whose attributes are not all default, as **runs**: `ROW: SPAN TOKENS`.

- `ROW` is the row index (right-aligned to the same width as in `[text]`), then `:`.
- `SPAN` is a column `N` or an inclusive range `N-M`, counting cells (a wide character is two columns).
- `TOKENS` are the attribute set of every cell in the span, separated by spaces. Writers use the order `fg=C`, `bg=C`,
  `underline=STYLE`, `ulcolor=C`, `link="URI"`, then the flags `bold`, `dim`, `italic`, `reverse`, `strike`, `conceal`;
  readers accept any order. `C` is `default`, `idx:N` or `rgb:#rrggbb` (lower-case hex digits; readers accept either
  case). A token equal to its default is never written.
- A run is a maximal sequence of adjacent cells with identical non-empty attribute sets. Writers emit runs in row,
  then column order. A blank cell with an attribute (the 80 reverse cells of `top`'s header) is listed.
- **Errors:** two runs that overlap; a run outside the screen; a run with no tokens or only default values; a token
  repeated within a run; a token for an attribute that is not in the frozen vocabulary of section 3.8.

### 6.5 Regions and `[masks]`

A mask relaxes the comparison for a region of a screen. Everything outside every mask is compared in full: text and
attributes (except those named by an `ignore attrs` line).

```
mask REGION [text=ignore | text=/RE/] [attrs=ignore]
REGION  =  ROWS [COLS]  |  find=/RE/ [ROWS] [COLS]
ROWS    =  row=N | rows=N | rows=N-M          (a single index is the same as N-N)
COLS    =  cols=N | cols=N-M
```

Tokens after the keyword (`mask` or `assert`) may appear in any order, and each may appear once. A mask must say at
least one of `text=` or `attrs=ignore`. `row=` and `rows=` are the same. `N <= M`. All indices must lie inside the
screen's `size` (checked when the golden is read: an error, exit 65, otherwise).

**Scope: shared and per-screen masks.** A `[masks]` section before the first `[screen]` is **shared** and applies to
every screen; a `[masks]` section inside a screen block applies to that screen. A screen is masked by the **union** of
both lists, and where regions overlap a part is skipped if any covering mask skips it (below). There is no negation: a
per-screen mask cannot cancel a shared one, because that would make the effect of a line depend on where in the file
it sits. A mask that only some screens need belongs in those screens. A shared mask whose region is given by rows and
columns must lie inside the size of **every** screen (an error otherwise); a shared mask that must follow a layout
that differs per screen uses `find=`.

**Evaluation.** The definitions are normative; the pseudocode is the intended algorithm. It runs once per mask per
screen.

```
cell(r, c)        the cell at row r, column c of the ACTUAL screen
text(span)        concatenation of cell(r, c).ch for c in the span; a continuation cell contributes ""
row_text(r)       text of the whole row with trailing blank cells removed   (blank: ch == " " exactly)

spans(region):                          # a Span is (row, first_col, last_col), inclusive
  if region is ROWS [COLS]:
      for r in rows:                    # ONE SPAN PER ROW, never one rectangle
          yield Span(r, cols.first or 0, cols.last or width-1), whole_row = (COLS absent)
  if region is find=RE [ROWS] [COLS]:
      for r in (rows or all rows):
          slice  = cells cols.first..cols.last of row r (or the whole row)
          s      = text of slice with trailing blank cells removed       # `$` anchors to the last visible character
          pos    = 0
          while pos <= len(s):                                         # positions are scalar offsets
              m = search(RE, s, from=pos)       # leftmost-first; `^` only at offset 0 of s, `$` only at end of s
              if m is none: break
              if m is empty: pos = m.start + 1; continue
              first = the cell holding scalar m.start                 # a match starting inside a cell's appended
              last  = the cell holding scalar m.end - 1                # scalars covers the whole cell
              extend last over directly following continuation cells
              yield Span(r, first, last), matched text = m
              pos = m.end

span_text(span)   = row_text(r) if span.whole_row else text(span)      # a whole row ignores trailing blanks,
                                                                         # an explicit cols= range keeps its blanks
```

- `text=ignore`: the cells of every span are not compared for text.
- `text=/RE/`: the cells of every span are not compared with the stored text, but `span_text(span)` must match the
  whole of RE, that is `^(?:RE)$`, for **every** span. A failure is a `mask` mismatch.
- `attrs=ignore`: the cells of every span are not compared for attributes.
- **Stale masks.** A mask is stale, and reported as a failure, when its region is a `find=` that yields **no span at
  all**. Staleness is judged over the mask's range of rows (not "no span in some row": a figure may be missing from one
  row of `rows=2-4`), and, for a **shared** mask, over **all screens** of the run (below). A region written with rows and
  columns cannot be stale; a `text=/RE/` that does not match is reported as a mismatch.
- **A shared `find=` mask is stale only if it yields no span in any screen.** The alternative, "in every screen", would
  force the clock mask of a tf run into the screens that have a clock and out of the startup screen that does not yet
  (and so defeat sharing). The cheaper rule is safe because a mask that finds nothing in a particular screen simply
  masks nothing there: if the thing it was meant to hide is missing or has moved, the stored text no longer equals the
  actual text and the ordinary comparison reports it. Only a mask that matches in *no* screen has become meaningless,
  and that is the case worth failing. A per-screen `find=` mask is stale when it yields no span in its own screen.
- Where regions overlap, a part is skipped if any covering mask skips it.

### 6.6 `[assert]`

Sparse assertions on the **actual** screen paired with this screen block, independent of `[text]` and `[attrs]`. There
is no shared `[assert]` (a section of that name before the first `[screen]` is an error in grammar 1; a shared
assertion list is a reserved, additive path, section 10):

```
assert REGION [text="EXACT" | text=/RE/] [ATTR ...]
ATTR  =  fg=C | bg=C | underline=STYLE | underline=none | ulcolor=C | link="URI" | !link | FLAG | !FLAG
FLAG  =  bold | dim | italic | reverse | strike | conceal
```

- `REGION` is as in a mask. A missing `cols=` means the whole row.
- `text="..."` is a JSON string literal (6.12) and must equal `span_text(span)` as a **sequence of scalars**: `e` and the
  precomposed `é` are different. `text=/RE/` must match all of it. `text="EXACT"` needs a single-row region and cannot
  be combined with `find=` (the pattern is the text constraint); both are errors when the golden is read.
- Each `ATTR` must hold for **every cell** of every span. For a whole-row span that means every cell of the row, so an
  assertion about a coloured prefix says `cols=`. Unlisted attributes are unconstrained. `!bold` means the flag is off;
  `underline=none` means no underline; `fg=default` means the default colour. `ulcolor=` on a cell with no underline is
  meaningful (the emulator tracks them independently).
- With `find=`, **every** match must satisfy the constraints, not just one.
- An assertion whose region yields **no span** fails.
- An assert needs at least one of `text=` or an `ATTR`.

### 6.7 The regex dialect

Patterns are in `text=/RE/` and `find=/RE/`. The pattern ends at the next `/` that is not preceded by a backslash; a
literal slash is written `\/`.

**Allowed.** Literal scalars; the escapes `\\ \/ \. \+ \* \? \( \) \[ \] \{ \} \| \^ \$ \-` (an escaped metacharacter is
that character; the parser unescapes them, so every implementation sees the same pattern); `.` (any one scalar); classes
`[...]` and `[^...]` with ranges and the escapes above; `\d \D \w \W \s \S`; groups `( )` and `(?: )`; alternation `|`;
the quantifiers `* + ? {m} {m,} {m,n}` and their lazy forms (`*?`, `+?`, `??`, `{m,n}?`); the anchors `^` and `$`.

**ASCII only.** `\d` is `[0-9]`, `\w` is `[0-9A-Za-z_]`, `\s` is the scalars U+0009 to U+000D and U+0020, and `\D \W
\S` are their complements within all scalars; the same sets apply **inside a class** (`[\d.]` is `[0-9.]`). `^` and `$`
match only at the start and end of the subject (there is no multi-line mode and the subject has no line breaks). The
search is **leftmost-first** (Perl, Rust `regex`, Python `re`, JavaScript, RE2 in its default mode), never POSIX
leftmost-longest.

**Rejected** (an error when the golden is read, exit 65): `\b \B \A \z \Z \G`, inline flags such as `(?i)` and
`(?s)`, named groups `(?P<n>...)`, `\p \P`, numeric and hexadecimal escapes `\x \u \U \N \0` and back-references
`\1` to `\9`, look-ahead and look-behind, atomic and possessive forms, POSIX classes `[[:alpha:]]`, `\h \R \K`, and any
escape not listed above. Rejecting them is what keeps the dialect the same in every language.

**Implementing it** (measured, E24): in Rust `regex`, `\d`, `\w` and `\s` are Unicode (`\d` matches U+0663) and
`(?-u)` fails to compile with `.`, so the parser must rewrite the shorthand classes into explicit ASCII sets, including
inside classes, before compiling; in Python `re` use `re.ASCII` or the same rewrite; in JavaScript `\s` is Unicode too
(from the language definition, not measured here), so rewrite all three. The scan loop above skips empty matches
explicitly, because engines disagree about what follows an empty match (E24: Python yields `(0,0)`, `(1,3)`, `(3,3)` for
`a*` on `baa`).

### 6.8 Comparison

A run produces an ordered sequence of **actual screens** (snapshot objects) and possibly **run-level problems** (a
`wait_for` that timed out, 6.11). `--expect` compares them with the golden's screens. In this order:

1. **Parse** the golden and validate it (section 6.13). A missing file is exit 66 (unless `--update` is creating it); a
   malformed one is exit 65. Errors stop here, before anything is run.
2. **Versions**, decided from the **file header alone, before the program is started or the session is touched** (so a
   refusal never costs a run). Each refusal is exit 2 with nothing compared, and its message names both numbers and the
   way forward (section 5.3):
   - `grammar`: a number above the reader's is refused. Lower numbers are read (4.3, rule 7).
   - `schema`: any number other than the build's own is refused (there is no deprecation window and no second reader,
     OD3).
   - `semantics`: any number other than the build's own `semantics_version`, and the word `unknown`, is refused
     (OD4). There is no flag that overrides it.
3. **Run** the source (6.11) and collect the actual screens. In forms (b), (c) and (d) the numbers come from the
   snapshots themselves (a live session or a saved file may have been produced by another build): a snapshot whose
   `schema_version` or `semantics_version` differs from the golden's is refused in the same way, as soon as the first
   screen arrives; a snapshot with a missing number is malformed (exit 65). In form (a) the snapshots are the running
   build's own, so step 2 already decided it.
4. **Pair the screens by position:** actual screen *i* is compared with the golden's screen *i*. If the counts differ,
   the common prefix is paired and compared, and one `screens` mismatch (kind in 6.9) names the golden screens that got
   no output (`missing`) and the actual screens that have no golden (`extra`). Pairing is by position only: an inserted
   checkpoint shifts everything after it, which is exactly what a diff of the golden should show. If **both** sides carry
   a `name` for a pair (the script's `snapshot` gave one and the golden block has one) and the names differ, one `name`
   mismatch is reported for that pair and the comparison of the pair still runs.
5. **Each pair**, in order:
   1. **Size.** If it differs, report it and stop comparing the screen: the header fields of step 5.2 are still compared,
      but cells, masks and assertions (5.3 to 5.5) are **not** (the grids no longer line up, and stale-mask reports would
      be noise).
   2. **Header:** cursor, `alt_screen`, `modes` (as sets, minus `ignore modes`), `title`, `wrapped`, each unless
      ignored (file-level and screen-level `ignore` are united, 6.2).
   3. **Cells.** Every cell outside masks: text (`ch`) and the attribute set (minus `ignore attrs`).
   4. **Masks:** stale-mask checks and `text=/RE/` checks, shared masks first then the screen's own (6.5).
   5. **Assertions** of the screen.
6. **Staleness of shared masks** is decided after all pairs have been compared (6.5).

`byte_offset`, `scrollback`, `producer` and `x-` keys never take part. Regions are validated against each **golden
screen's** size when the golden is read, so a size mismatch is the only reason a region can fall outside the actual
screen. With `--fail-fast` the run stops at the first mismatch, refusal or run-level problem: the program (or nothing,
for `--connect`) is stopped, the remaining screens are reported as `skipped`, and the exit code is that of the first
problem.

**Version mismatches have one outcome.** A check never partly compares, never "compares anyway" and never downgrades a
refusal to a warning (owner decisions OD3 and OD4). Reason: every bump then means exactly one thing to a user, "this
golden was recorded under another meaning", and the answer is always the same two commands, `--update=restamp` (cheap,
scripted, and only succeeds when nothing the golden checks changed) or `--update` (re-record). The cost, accepted
knowingly, is that every bump after 0.1.0 refuses every stored golden until it is restamped, including goldens of
programs that never touch the change; that is why the reported vocabulary is frozen as one table (section 3.8) and why
bumps are expected to be rare. A repository restamps all its goldens in one pass, one command per test.

### 6.9 The report

Plain text on standard output by default; `--format json` writes one JSON object. **NAME** is the final component of the
golden's path, with its extension (`tf-session.golden`). The first line is exactly one of:

```
PASS NAME (K screens)                  ("1 screen" or "K screens")
FAIL NAME: 4 mismatches in 2 of 3 screens        ("1 mismatch" or "N mismatches"; "of 1 screen" when K is 1)
REFUSED NAME: REASON
```

`K` is the number of screens in the golden. `N` counts blocks over the whole run (a run of identical cell differences is
one block). **There are no warnings in grammar 1**: every condition passes, fails or refuses, so a `PASS` line is
followed by nothing. Then the **run-level** blocks (kinds `screens` and `wait`), then, for each
screen that has mismatches, a line `screen I` (or `screen I "NAME"` when the golden's block has a name) followed by that
screen's blocks, indented two more spaces. Screens without mismatches are not listed. Each mismatch is a block whose
first word names the kind. At most 20 blocks are shown (`--max-mismatches N`); the first line gives the true total and a
last line says `showing 20 of 413`. A `REFUSED` run has no mismatch blocks: its first line gives the reason and the
following lines the way forward (below).

A mismatch block for each kind (the text forms are the specification; the exact column alignment is not significant):

```
FAIL flow.golden: 17 mismatches in 2 of 3 screens
  screens    expected 3   actual 2   missing 3
  wait       script line 7   wait_for "10/10mv>" timed out after 5000 ms
screen 1 "startup":
  name       expected "startup"   actual "login"
  size       expected 80x24   actual 100x30   (cells, masks and assertions not compared)
  cursor     position   expected row=22 col=19   actual row=22 col=20
  cursor     shape      expected block           actual bar
  cursor     hidden     expected false           actual true
  modes      expected ?1 DECKPAM   actual ?2004 DECKPAM   missing ?1   extra ?2004
  alt_screen expected true   actual false
  title      expected null   actual "tf"
  wrapped    expected (none)   actual 22   missing (none)   extra 22
  text       row 20 col 79   expected '8'   actual '9'
             20| _________(unnamed1)_____...____03:18   (expected)
             20| _________(unnamed1)_____...____03:19   (actual)
                                                   ^
screen 2:
  attrs      row 16 col 0-13 (14 cells)   expected fg=idx:2 bold   actual fg=idx:2
  mask       golden line 37   mask find=/\d\d:\d\d$/ text=/\d\d:\d\d/   matched nothing (stale)
  mask       golden line 38   mask row=20 cols=75-79 text=/\d\d:\d\d/   row 20 col 75-79 actual '_____' does not match
  assert     golden line 47   assert find=/goblin/ fg=idx:1 bold   row 17 col 2   actual fg=idx:3   (first failing cell of 8)
  assert     golden line 49   assert row=30 text="x"   matched nothing
```

(The example lists more kinds than a real report would show at once; it is in the order a report prints them: run-level
blocks, then per screen size, name, cursor, modes, alt_screen, title, wrapped, then cells, masks and assertions. `golden
line` is the line of the *file*, which for a mask in the shared list is a line before the first `[screen]`.)

A `text` block shows both rows with a caret line under the differing columns; a `text` block for a continuation cell
shows `\.`. Refusals (exit 2; nothing was compared), each with its way forward:

```
REFUSED flow.golden: semantics 1 in the golden, 2 in this vtee; nothing was compared
  restamp (only if nothing the golden checks changed):  vtee --script flow.ndjson --expect flow.golden --update=restamp -- tf -n
  or re-record with --update; what changed is listed in docs/guide/snapshot-semantics.md

REFUSED flow.golden: schema 1 in the golden, 2 in this vtee; nothing was compared
  migrate the file:  vtee --expect flow.golden --update=migrate      (or re-record with --update)
```

`--format json`:

```
{"golden":"flow.golden","result":"fail","grammar":1,"schema":1,"semantics":1,
 "snapshot":{"schema":1,"semantics":1},"screens_expected":3,"screens_actual":2,
 "total":17,"shown":17,                       (the example is abbreviated: the screen entries below list only some objects)
 "run":[{"kind":"screens","expected":3,"actual":2,"missing":[3],"extra":[]},
        {"kind":"wait","line":7,"text":"10/10mv>","timeout_ms":5000}],
 "screens":[{"index":1,"name":"startup","result":"pass"},
            {"index":2,"name":null,"result":"fail","mismatches":[ ...objects below... ]},
            {"index":3,"name":"long-line","result":"skipped"}]}
```

`result` is `pass`, `fail` or `refused` for the run; a refusal adds `"reason"` (a string, the text after
`REFUSED NAME: `) and `"way_forward"` (an array of command strings). `run` holds the run-level mismatch
objects; each entry of `screens` has `index` (1-based, the golden's), `name` (the golden block's, or `null`), `result`
(`pass`, `fail`, `refused` or `skipped`: not compared because the run ended, `--fail-fast` stopped it, or no actual
screen existed) and, for `fail`, `mismatches`. Every mismatch object has `kind`. Fields by kind (rows and columns
0-based, `col_start` and `col_end` inclusive; unknown fields must be ignored by readers):

| kind | Fields |
| --- | --- |
| `screens` (run) | `expected` and `actual` (screen counts), `missing` (1-based golden screens with no output), `extra` (1-based actual screens with no golden) |
| `wait` (run) | `line` (script line number), `text`, `timeout_ms` |
| `name` | `expected` and `actual` (strings) |
| `size` | `expected` and `actual`, each `{"cols":N,"rows":N}` |
| `text` | `row`, `col_start`, `col_end`, `expected` and `actual` (the cell text of the span, strings), `expected_row` and `actual_row` (the whole row text) |
| `attrs` | `row`, `col_start`, `col_end`, `cells` (count), `expected` and `actual` (arrays of attribute tokens as in `[attrs]`) |
| `cursor` | `field` (`position`, `shape` or `hidden`), `expected` and `actual` (`{"row":N,"col":N}`, a shape string, or a boolean) |
| `modes` | `expected`, `actual`, `missing`, `extra` (arrays of tokens) |
| `alt_screen` | `expected` and `actual` (booleans) |
| `title` | `expected` and `actual` (string or null) |
| `wrapped` | `expected`, `actual`, `missing`, `extra` (arrays of row indices) |
| `mask` | `line` (golden file line number), `mask` (the line text), `problem` (`stale` or `text`), and for `text`: `row`, `col_start`, `col_end`, `actual` |
| `assert` | `line`, `assert` (the line text), `problem` (`no_match`, `text` or `attr`), and for `text` and `attr`: `row`, `col` (first failing cell), `expected`, `actual`, `failing_cells` (count) |

A `mask` mismatch with `problem` `stale` for a *shared* mask is a run-level fact (it was judged over all screens): it is
listed in `run`, not under a screen.

The reports of `--update` runs are in 6.11.

### 6.10 Exit codes of a run with `--expect`

| Code | Meaning |
| --- | --- |
| 0 | Pass; an update that succeeded, including one that changed nothing |
| 1 | The output differs from the golden: any mismatch, including a `screens`, `name` or `wait` problem; or an `--update` that refused to write (a failed run, a restamp that would change a compared value) |
| 2 | Refused: the grammar, schema or semantics number prevents a comparison (6.8); nothing was compared. Deliberately not a sysexits code, so a script can tell "differs" and "cannot compare" apart from errors |
| 64 | Usage error: an illegal option combination (6.11), a missing value, a `--screen` that does not fit the run |
| 65 | A golden, a snapshot input or a script is malformed (data error); the message says which, and for a snapshot the first line is `snapshot: ...`, for a script `script line N: ...` |
| 66 | A file named on the command line does not exist or cannot be read (the golden without `--update`, the script, the snapshot file) |
| 69 | The session cannot be reached (`--connect`) |
| 71 | The program could not be started (what `vtee --script` already returns today, E27) |
| 74 | Another I/O error, including a golden that cannot be written |

**How the code of a run is chosen.** Errors that prevent a run (64, 65, 66 for a golden, script or snapshot file) are
reported before anything starts, with nothing run: **in `--expect` mode the whole script is parsed and validated before
the program is started** (today's `--script`, without `--expect`, executes the lines before a malformed one and then
stops with 65, E28). An error *during* a run (71, 69, 74) aborts it and is the exit code; no comparison report is
printed, only a message on standard error. A run that completes is judged by its comparison: **any refusal gives 2,
else any mismatch or run-level problem gives 1, else 0.** A refusal from the file header (grammar, schema or
semantics number against the build's own) is decided before the run starts and the program is never started (form
(a)). In forms (b), (c) and (d) the snapshots' numbers come from a session or a file, so a mismatch there is decided
when the first screen arrives.

**Usage errors are 64, not 2.** clap's own default exit code for a usage error is 2. `src/main.rs` (lines 198 to 202,
E25) maps every parse error to `exitcode::USAGE` before anything else, and the new options are options of the same
parser, so the mapping covers them; the implementation must keep a test that a bad `--expect` option exits 64 and
that a refusal exits 2.

### 6.11 The run command

```
vtee [SESSION-OPTIONS] --script FILE --expect GOLDEN [RUN-OPTIONS] -- PROGRAM [ARGS]...     (a) spawn and run
vtee --connect SOCK --script FILE --expect GOLDEN [RUN-OPTIONS]                              (b) run against a live session
vtee --connect SOCK --expect GOLDEN [RUN-OPTIONS]                                            (c) the live session's screen now
vtee --snapshot FILE|- --expect GOLDEN [RUN-OPTIONS]                                         (d) offline: finished output
vtee --expect GOLDEN --update=migrate [--format text|json]                                   rewrite an old golden in place

RUN-OPTIONS  =  [--update[=MODE]] [--screen N] [--format text|json] [--portable] [--fail-fast] [--max-mismatches N]
MODE         =  record | restamp | migrate        (bare `--update` means `--update=record`)
```

**What 0.1.0 ships** (owner decision OD7): `--expect` (check), `--update` and `--update=record`, `--update=restamp`,
and the other options above. `--update=migrate` is specified here and ships **no later than the first schema or
grammar bump** after 0.1.0, because there is nothing before 0.1.0 to migrate from (5.3); until then it is rejected with
`vtee: --update=migrate is not available in this release` (exit 64). There are no `--strict-semantics`,
`--accept-semantics` or `--accept-vocab` options (owner decisions OD4 and OD8): a version mismatch has no override.

`SESSION-OPTIONS` are the existing options that shape the program's terminal in form (a): `--cols`, `--rows`, `--env`,
`--log`. **With `--expect` the size defaults to 80x24**, not to the invoking terminal's size (without `--expect` a
script run still uses the terminal's size when there is one, which makes a result depend on where it ran; the help text
says so today: "Default: this terminal's width, else 80"). A run with `--expect` is **headless in the sense of
`--headless`**: it reads no standard input and does not write the program's output to the terminal. In form (a) the
run inherits every conflict `--script` already has with the interactive-session options (`--listen`, `--name`,
`--writable`, `--headless`, `--attach-if-exists` and the others are errors, exactly as they are today); the legality
table below lists only what `--expect` adds.

**The sources of the output.**

| | Form | Screens produced | The program or session afterwards |
| --- | --- | --- | --- |
| (a) | a program after `--` and `--script FILE` | one per `snapshot` command executed in the script | stopped when the script ends, as `--script` does today |
| (b) | `--connect SOCK` and `--script FILE` | the same, against a live session | left running |
| (c) | `--connect SOCK` alone | exactly one: the session's current screen | left running |
| (d) | `--snapshot FILE`, or `-` for standard input | one per JSON document in the input (below) | none: no program is involved |

Form (b) is **new**: today `--connect` ignores `--script` without a word (E32), so the combination is not a
compatible extension of anything that worked; with this document both `--connect` with `--script` and `--connect` with a
program become well defined (the first runs the script, the second is an error), with or without `--expect`.

The input of (d) is NDJSON: each line is either a bare snapshot object, or a reply line `{"cmd":"snapshot","snapshot":{...}}`
as `vtee --script` prints it, so the output of an earlier script run can be saved and checked offline. A reply with
another `cmd` (`wait_for`, `exit_status`, ...) is not a screen; a `wait_for` reply with `"matched":false` is a `wait`
problem, as in a live run (so the same run gives the same verdict live and offline). Lines that are neither are
exit 65.

**Illegal combinations** (exit 64; clap conflicts where clap can express them, otherwise a check made after parsing).
The first clause of each message is normative, the rest is a hint:

| Combination | Message |
| --- | --- |
| `--expect` with no source (no program, no `--connect`, no `--snapshot`) and not `--update=migrate` | `vtee: --expect needs a source of screens: a program after --, --connect SOCK or --snapshot FILE` |
| a program with no `--script` (and no `--snapshot`/`--connect`) | `vtee: a program needs --script FILE to say what to type; to look at it by hand use --connect` |
| `--script` with neither a program nor `--connect` | `vtee: --script needs a program to run or --connect SOCK` |
| `--connect` with a program | `vtee: --connect talks to a running session; it cannot be combined with a program` |
| `--snapshot` with `--connect`, `--script`, a program, `--cols`, `--rows`, `--env` or `--log` | `vtee: --snapshot reads finished output; it cannot be combined with --connect, --script, a program or session options` |
| `--update`, `--screen`, `--format`, `--portable`, `--fail-fast` or `--max-mismatches` without `--expect` | `vtee: --OPTION needs --expect GOLDEN` |
| `--text` with `--expect` | `vtee: --text has no effect with --expect, which prints a report; use --format text or --format json` |
| `--update=migrate` with a program, `--script`, `--connect`, `--snapshot`, `--screen`, `--portable` or a run option other than `--format` | `vtee: --update=migrate rewrites the file in place and runs nothing; give only --expect GOLDEN` |
| `--portable` without `--update` (record), or with `--update=restamp` | `vtee: --portable applies when a golden is created; use it with --update` |
| `--screen` with `--update=restamp` | `vtee: --screen applies to checking and to --update (record)` |
| any `--update` while the `CI` environment variable is set and non-empty, unless `VTEE_ALLOW_UPDATE=1` | `vtee: --update is refused when CI is set (it would make a pipeline pass by rewriting its expectations); set VTEE_ALLOW_UPDATE=1 to allow it` (OD18) |
| `--update --screen N` when GOLDEN does not exist | `vtee: --screen N updates one screen of an existing golden; GOLDEN does not exist` (detected after parsing, still exit 64) |
| an unknown `--update` value | `vtee: unknown --update mode 'X'; expected record, restamp or migrate` |
| `--screen N` when the run produced more than one screen, or N is outside the golden | `vtee: --screen N selects one screen, but the run produced K` / `... the golden has K screens` (detected after the run, still exit 64) |

**Check (the default).** `--expect GOLDEN` compares (6.8). A missing GOLDEN is exit 66 with `vtee: GOLDEN: no such
file (give --update to record it)`. Nothing is written. A one-screen run (form (c), or (d) with one document) is paired
with a one-screen golden; to compare it with one screen of a composite, `--screen N` pairs it with golden screen N and
reports only that screen's mismatches. (Without `--screen`, a one-screen run against a composite shows a `screens`
mismatch whose message adds the hint `use --screen N to compare one screen`.)

**Update.** `--update[=MODE]` rewrites the expected file from the run. Common rules: it runs the source exactly as
a check does, **and writes only if the run succeeded**: a timed-out `wait_for` or an aborted run is exit 1 (or the
error's code) with `not written`, because recording a broken run would bake the failure into the golden. Header
comments, `ignore` lines, shared and per-screen `[masks]` and `[assert]` sections, their comments, and each block's
order are **kept verbatim**; only the generated parts are rewritten. Screens are paired with the existing blocks by
position. `x-producer` is rewritten only when something else in the generated part changes, so a re-record with a newer
`vtee` that changes nothing produces an empty diff and the line names the `vtee` that last changed the golden, which
is what an audit of a pin needs.

**`--update` always prints what it changed** (owner decision OD18), on standard output; `--format json` gives the same
facts as an object with `result` and `screens`. A reviewer sees in the log, and in the diff, exactly which expectations
moved:

```
CREATED NAME: 3 screens
UPDATED NAME: 3 screens
  screen 2: 14 cells changed (text 3, attrs 11), cursor moved
  screen 3: new (no masks, no asserts: add them by hand)
  screen 4: dropped (it had 2 masks and 1 assert, which are lost)
UNCHANGED NAME: 3 screens
```

**The CI guard** (OD18). A pipeline that runs `--update` rewrites its own expectations and then passes, so a stray flag
would make every test pass forever. When the `CI` environment variable is set and non-empty, every `--update` mode
(`record`, `restamp`, `migrate`) refuses with exit 64 and the message in the table above, unless
`VTEE_ALLOW_UPDATE=1` is also set. The guard looks only at the environment, so a deliberate update job opts in
explicitly. It applies before the program is started, so a refused update never costs a run.

| Mode | What it does | Refuses (exit 1) when | Notes |
| --- | --- | --- | --- |
| `record` (bare `--update`) | Creates the golden if it is missing (no masks, no assertions: those are written by hand) or regenerates `[screen]` headers, `[text]` and `[attrs]` from the run. Screens beyond the existing blocks are added without masks; existing blocks with no output are **dropped** and listed in the message, so a lost checkpoint is visible | the run failed | `--portable` (6.14) applies when the file is created. With `--screen N`, only screen N is regenerated (the run must produce one screen); it cannot create a file |
| `restamp` | Rewrites only the `semantics` line (to the number the run's snapshots carry) and `x-producer` | any compared cell, header field or assertion of any screen would differ (a full check with the version numbers ignored runs first) | Confirms that a semantics bump changed nothing this golden checks. Prints `RESTAMPED NAME: semantics 1 -> 2`, or `NOT RESTAMPED NAME: 3 differences` followed by the check report. A golden with a different `schema` is not restamped: it needs `migrate` or a re-record |
| `migrate` | Rewrites a golden recorded under an older grammar or schema, in place (5.3). Needs no source: it runs nothing. **Not in 0.1.0** (see above) | (never: it does not compare) exit 2 if the file is *newer* than this tool | **Never invents facts**: unrecoverable ones are written as unknown (an `ignore` for that part, or `semantics unknown`). Prints `MIGRATED NAME: schema N -> M` and lists the unknowns, or `UNCHANGED NAME` |

*Why these value names.* `record` is what the default does, so a bare `--update` and `--update=record` mean the same
and scripts can spell it out. `restamp` says it rewrites the stamp and nothing else; `semantics` would suggest it
changes semantics. `migrate` is an `--update` mode and not an option of its own because it too rewrites the
expected file in place and is bound to the same file argument; it is the one mode that takes no source, which the table
of illegal combinations states. The values are part of the interface and are frozen at 0.1.0; adding a mode later is
additive.

**A timed-out `wait_for` fails the run.** In every `vtee --script` run, with or without `--expect`, a `wait_for` that
times out makes the run fail. Without `--expect` the output is unchanged (the reply with `"matched":false` is still
printed, as in E27) but the exit code is **1** and standard error gets `vtee: script line N: wait_for "TEXT" timed out
after T ms`. With `--expect` it is a run-level `wait` mismatch (6.9), exit 1. The script **continues** after a
timed-out wait (later lines may still be informative) unless `--fail-fast` is given. A `wait_for` that is allowed to
time out says so, in the script: `{"cmd":"wait_for","text":"...","timeout_ms":500,"optional":true}`. `optional` is a
runner-side field that servers ignore, which is safe because it has no effect on what the server does. `--connect` read
from standard input (no `--script`) is not a script and is unchanged. This is a deliberate behaviour change of `--script`
(it exited 0 on a timed-out wait, E27); it is acceptable only because nothing is released (owner decision OD12).

**Names.** A script's `snapshot` command may carry an optional string `name` (`{"cmd":"snapshot","name":"connected"}`).
Servers ignore it; the runner attaches it to the screen. A name is a label, not an address (pairing is by position),
and the golden block records it as `name "connected"`. A name that is present on both sides and differs is a `name`
mismatch. A script without names works unchanged.

**Reserved, not defined in grammar 1.** An `[exit]` section would record the program's exit status as part of the
expected output (the script can already ask for it with `wait_exit`, but a status is not a screen, so it is not part of
the output today). The name is reserved: a grammar-1 reader rejects `[exit]` as a reserved section (6.1), and a later
grammar number defines it (section 10).

### 6.12 Determinism rules for writers

The order of a file is: the file header (lines in the order of section 6.2), the shared `[masks]` if any, then the
screens in numeric order, each with its screen header (the order of 6.2), `[text]`, `[attrs]` (when needed), `[masks]`
and `[assert]` (when present). Exactly one blank line before every section. `[attrs]` runs in row, then column order,
tokens in the order of section 6.4. Every row of `[text]` present, with no trailing blank cells. No line anywhere ends
in whitespace. One newline at the end. Nothing in the file depends on the Unicode version of the tool that wrote it.

**JSON string literals** (the header `title`, `link="URI"`, `text="EXACT"`) have one canonical encoding: UTF-8; escape only
`"` as `\"` and `\` as `\\`; the short escapes `\n`, `\r`, `\t`; every other scalar in the escape set of 6.3 (this includes
all other C0 controls and DEL) as `\uXXXX` in lower-case hex, with a surrogate pair `\ud83d\ude00` for scalars above
U+FFFF; everything else raw; a solidus `/` is never escaped. Readers accept any valid JSON string.

### 6.13 What a reader must reject, and what it must accept

Two independent implementations must agree on every file. A reader **rejects** (exit 65, naming the line):

- an invalid UTF-8 sequence, a NUL, a lone CR, a leading BOM;
- a first line that is not `vtee-golden N`, a missing required header key (in the file header or in a screen
  header), a duplicated key (other than `ignore` and `x-`), an unknown key that does not begin with `x-`, a key in the
  wrong header (`size` in the file header, `schema` in a screen header), an unknown section, a section duplicated within
  its scope (two shared `[masks]`, two `[text]` in one screen);
- a file with no `[screen]` block; a `[screen N]` whose `N` is not the number of screen blocks so far plus one; a
  `[text]`, `[attrs]` or `[assert]` before the first `[screen]`; the reserved sections `[exit]` and `[screen]` without a
  number; a first line `vtee-golden 0` (the unreleased draft layout, unsupported, OD1); a shared mask whose rows
  and columns fall outside any screen's size;
- a `[text]` with the wrong number of rows, a row label that is not its index, a row with no `|`, more cells than `cols`,
  a malformed or unknown backslash sequence, a surrogate or out-of-range `\u{H}`, `\.` at the start of a row or after
  `\.`, `\+` at the start of a row or after `\.`;
- an `[attrs]` run that overlaps another, is outside the screen, has no tokens or only defaults, repeats a token, or
  uses an attribute that is not in the frozen vocabulary of section 3.8; any comment in `[text]` or `[attrs]`;
- a region outside its screen's size, `N > M` in a range, a mask with neither `text=` nor `attrs=ignore`, `text="EXACT"`
  on a multi-row region or with `find=`, an assert with no constraint, a regex outside the dialect (6.7), an invalid
  JSON string, a repeated token in one `mask` or `assert` line, `ignore modes` with no token;
- a `modes` token that is not in the frozen vocabulary of section 3.8, or an `ignore` naming a mode or attribute that
  is not in it (a golden written by a newer build that reports more tokens already differs in `semantics` and is
  refused at step 2 of 6.8, before this check matters).

A reader **accepts and normalises**: CRLF; a missing final newline; blank lines anywhere; trailing whitespace on
non-`[text]` lines; the sections or header lines in any order; `modes` tokens in any order or repeated (a set);
`wrapped` in any order; tokens of an `[attrs]` run in any order; region tokens of a mask or assert in any order; `\u{H}`
for a scalar outside the escape set, in either case with leading zeros; trailing literal spaces in a `[text]` row; the
sections of one screen in any order. Screen blocks themselves must be in numeric order. Only a run with `--update`
normalises a file on disk; a check never writes.

### 6.14 Portability across platforms

A golden is portable between Linux and Windows to the extent the program's output is. Measured differences (E3, E4,
E15 to E17), and how a golden handles each:

| Difference | Linux | Windows (ConPTY) | Handling |
| --- | --- | --- | --- |
| `?1`, `?1000`, `?1002`, `?7` | reported when set | never arrive (E15; `?1` and `?1006` from the project's earlier measurement; `?1003` and `?1005` are untested and assumed to behave the same) | `ignore modes ?1 ?1000 ?1002 ?1003 ?1005 ?1006` (the portable profile) |
| `DECKPAM` (`ESC =`) | reported | never arrives (E15) | `ignore modes DECKPAM` |
| `?2004`, `?25` | reported | arrive normally | portable |
| `?1004`, `?9001` | not set unless the program does | injected by ConPTY at the start and removed at the end (E16) | neither is in the 0.1.0 reported vocabulary (section 3.8), so no golden ever sees them |
| SGR 58 (underline colour) | colour kept | garbled into `dim`, or `blink` and `strike` (E17) | do not use SGR 58 in a portable golden; if a program emits it, goldens are per-platform |
| SGR 4:N (curly, dotted ...) | kept | kept | portable |
| `title` | `null` unless the program sets one | the host program's path unless set | `ignore title` |
| `byte_offset` | differs wildly (E3) | | never stored |
| Anything else | | | a real difference to investigate and report |

`--update --portable` (record mode, when the file is created) writes the **portable profile** as file-level `ignore`
lines, which apply to every screen of the run:

```
ignore modes ?1 ?1000 ?1002 ?1003 ?1005 ?1006 DECKPAM
ignore attrs ulcolor
ignore title
```

`ulcolor` is ignored because ConPTY garbles SGR 58 (E17), so underline colour is not portable; the *text* of the cells
and every other attribute are still compared. A program that emits SGR 58 also gets a spurious `dim` or `strike` under
ConPTY, which no `ignore` can separate from a real one, so goldens of such programs are per-platform. The mode tokens
left compared are `?2004` only.
A golden recorded that way on one platform can pass on the other when the program's output is the same. **The profile's
list is frozen at 0.1.0 and is part of the contract**: adding an `ignore` to it is a change of what `--portable`
writes, announced in the semantics changelog but not a version bump, because it only loosens goldens recorded
afterwards and never changes how a stored one compares. Removing one is a semantics bump (owner decision OD10).

## 7. Worked examples

The screens below were produced by running real snapshots through converters that implement this document's grammar,
then parsing the result back and comparing it to the snapshot (E14). The first draft's examples (one screen each, flat
layout) were independently implemented from the text by two reviewers, who reproduced them byte for byte; the revision
for the run-and-compare model re-laid them out as composites, using TinyFugue's recordings (`tests/screens-draft/`)
for the content, and the commands and reports were checked against the behaviour of today's `--script` (E27 to E30).

### 7.1 A tf session: three screens in one golden

TinyFugue starts `tf`, connects to a fake server and types a long line. The script has three `snapshot` commands, so a
run produces three screens, and the golden holds all three. The script, `flow.ndjson`:

```text
{"cmd":"wait_for","text":"Loading commands from","timeout_ms":5000}
{"cmd":"snapshot","name":"startup"}
{"cmd":"send","text":"/connect 127.0.0.1 4000\r"}
{"cmd":"wait_for","text":"10/10mv>","timeout_ms":5000}
{"cmd":"snapshot","name":"connected"}
{"cmd":"send","text":"this is a long line that I am typing at the prompt and it should run past the end of the eighty column screen so that the input line wraps"}
{"cmd":"wait_for","text":"input line wraps","timeout_ms":3000}
{"cmd":"snapshot","name":"long-line"}
```

Recording and checking it:

```text
vtee --script flow.ndjson --expect flow.golden --update -- tf -n     # prints: CREATED flow.golden: 3 screens
vtee --script flow.ndjson --expect flow.golden -- tf -n              # prints: PASS flow.golden (3 screens), exit 0
```

The recorded golden follows. Row 20 is tf's status row with a clock at its right end; one **shared** mask lets the clock
vary on every screen and still compares the attributes of those cells. The startup screen has two lines that change with
the build, not with the program under test, so it masks them itself. Screen 2 carries assertions about colours. (`name`
is the label the script gave each `snapshot`; whether tf's long line is a terminal soft wrap is not established by the
recording, so screen 3 has no `wrapped` line: it appears only where the snapshot reports it.)

```text
vtee-golden 1
schema 1
semantics 1
x-producer vtee 0.1.0 alacritty_terminal 0.26.0

[masks]
# The clock at the right end of tf's status row, on every screen: any HH:MM. Its attributes are still compared.
mask row=20 cols=75-79 text=/\d\d:\d\d/

[screen 1]
size 80x24
cursor row=21 col=0 shape=block hidden=false
modes ?1 DECKPAM
alt_screen true
title null
name "startup"

[text]
 0|
 1|
 2|
 3|
 4|
 5|
 6|TinyFugue version development
 7|Copyright (C) 1993, 1994, 1995, 1996, 1997, 1998, 1999, 2002, 2003, 2004,
 8|    2005, 2006-2007 Ken Keys (kenkeys@users.sourceforge.net)
 9|Type `/help copyright' for more information.
10|Ingwar Swenson (ingwar@ingwar.eu.org) +ansi_log +log_timestamp +lua +gmcp
11|    +ATCP +GMCP +option102
12|Using PCRE version 10.42 2022-12-11
13|Type `/help', `/help topics', or `/help intro' for help.
14|Type `/quit' to quit tf.
15|
16|% LC_CTYPE category set to "en_US.UTF-8" locale.
17|% LC_TIME category set to "en_US.UTF-8" locale.
18|% Warning: Can't figure out name of mail file.
19|% Loading commands from /usr/local/share/tf-development-lib/stdlib.tf.
20|_________(no world)________________________________________________________03:18
21|
22|
23|

[masks]
# These two lines change with the build, not with the program under test.
mask row=12 text=ignore
mask row=19 text=/% Loading commands from \S+/

[screen 2]
size 80x24
cursor row=22 col=19 shape=block hidden=false
modes ?1 DECKPAM
alt_screen true
title null
name "connected"

[text]
 0|
 1|
 2|
 3|
 4|
 5|
 6|
 7|
 8|
 9|
10|
11|
12|
13|
14|% Trying to connect to (unnamed1): 127.0.0.1 4000.
15|% Connected to (unnamed1).
16|A Quiet Meadow
17|Tall grass sways here. A path leads north.
18|Exits: [north, east]
19|A goblin snarls at you.
20|_________(unnamed1)________________________________________________________03:18
21|/connect 127.0.0.1 4000
22|<42/100hp 10/10mv>
23|

[attrs]
16: 0-13 fg=idx:2 bold
17: 0-41 fg=idx:6
19: 0-7 fg=idx:1 bold
22: 0-17 fg=idx:3

[assert]
assert row=16 cols=0-13 text="A Quiet Meadow" fg=idx:2 bold
assert find=/goblin/ fg=idx:1 bold
assert row=17 cols=0-41 fg=idx:6 !bold
assert row=16 text="A Quiet Meadow"

[screen 3]
size 80x24
cursor row=23 col=78 shape=block hidden=false
modes ?1 DECKPAM
alt_screen true
title null
name "long-line"

[text]
 0|
 1|
 2|
 3|
 4|
 5|
 6|
 7|
 8|
 9|
10|
11|
12|
13|
14|% Trying to connect to (unnamed1): 127.0.0.1 4000.
15|% Connected to (unnamed1).
16|A Quiet Meadow
17|Tall grass sways here. A path leads north.
18|Exits: [north, east]
19|A goblin snarls at you.
20|_________(unnamed1)________________________________________________________03:18
21|/connect 127.0.0.1 4000
22|<42/100hp 10/10mv> this is a long line that I am typing at the prompt and it sh
23|ould run past the end of the eighty column screen so that the input line wraps

[attrs]
16: 0-13 fg=idx:2 bold
17: 0-41 fg=idx:6
19: 0-7 fg=idx:1 bold
22: 0-17 fg=idx:3
```

Read against 6.5 and 6.8: the shared `mask row=20 cols=75-79` (line 10) is applied to all three screens, in addition to
the two masks of screen 1 (lines 48 and 49) which apply to screen 1 only. The last assertion of screen 2 (line 95) has
no `cols=`: a whole-row region compares the row text with trailing blanks removed (6.5), so the natural form passes on
an 80-column screen. Assertions are checked against the actual screen, whatever is stored in `[text]`.

If the goblin line lost its colour and bold in a later run, the report is:

```text
FAIL flow.golden: 2 mismatches in 1 of 3 screens
screen 2 "connected":
  attrs      row 19 col 0-7 (8 cells)   expected fg=idx:1 bold   actual fg=idx:3
  assert     golden line 93   assert find=/goblin/ fg=idx:1 bold   row 19 col 2   actual fg=idx:3   (first failing cell of 6)
```

(exit code 1). Screens 1 and 3 passed and are not listed. If the run had produced only two screens, for example because
the second `wait_for` timed out and the script was edited to stop there, the first lines would instead be:

```text
FAIL flow.golden: 1 mismatch in 0 of 3 screens
  screens    expected 3   actual 2   missing 3
```

### 7.2 A `top` screen with masks

`top` refreshes itself, so about a tenth of its cells are volatile. The masks name them by the columns of the header
row and let figures vary within their own pattern; the assertions keep what matters regardless of the figures. The
bold attribute of row 7 and 8 is masked because a running task is bold and a sleeping one is not. The script,
`top.ndjson`, waits for the header and takes one snapshot, and the command is
`vtee --script top.ndjson --expect top.golden --env TERM=xterm-256color -- top`. The masks are inside the screen block
because this golden has one screen; in a longer run the same masks would be shared if every screen were a `top` screen.

```text
vtee-golden 1
schema 1
semantics 1

[screen 1]
size 80x24
cursor row=9 col=0 shape=block hidden=true
modes ?1 DECKPAM
alt_screen false
title null

[text]
 0|top - 03:18:50 up 26 min,  0 user,  load average: 0.43, 0.26, 0.20
 1|Tasks:   2 total,   1 running,   1 sleeping,   0 stopped,   0 zombie
 2|%Cpu(s):  0.5 us,  0.0 sy,  0.0 ni, 99.5 id,  0.0 wa,  0.0 hi,  0.0 si,  0.0 st
 3|MiB Mem :  31944.6 total,  29885.9 free,   1623.8 used,    772.2 buff/cache
 4|MiB Swap:   8192.0 total,   8192.0 free,      0.0 used.  30320.8 avail Mem
 5|
 6|    PID USER      PR  NI    VIRT    RES    SHR S  %CPU  %MEM     TIME+ COMMAND
 7|      1 root      20   0  278048   4416   4020 S   0.0   0.0   0:00.00 vtee
 8|      8 root      20   0   12264   5472   3332 R   0.0   0.0   0:00.00 top
 9|
10|
11|
12|
13|
14|
15|
16|
17|
18|
19|
20|
21|
22|
23|

[attrs]
 1: 6-10 bold
 1: 17-21 bold
 1: 30-34 bold
 1: 44-48 bold
 1: 57-61 bold
 2: 8-13 bold
 2: 17-22 bold
 2: 26-31 bold
 2: 35-40 bold
 2: 44-49 bold
 2: 53-58 bold
 2: 62-67 bold
 2: 71-76 bold
 3: 9-18 bold
 3: 25-34 bold
 3: 40-49 bold
 3: 55-64 bold
 4: 9-18 bold
 4: 25-34 bold
 4: 40-49 bold
 4: 55-64 bold
 6: 0-79 reverse
 8: 0-79 bold

[masks]
# Row 0: clock, uptime, users, load averages
mask rows=0 text=ignore
# Rows 2-4: CPU and memory figures; a figure that gains a digit shifts its bold run
mask rows=2-4 find=/\d+\.\d/ text=/\d+\.\d/ attrs=ignore
# Process rows, by the columns of their headers (VIRT RES SHR, state, %CPU, %MEM, TIME+)
mask rows=7-8 cols=26-45 text=/[ \d]+/
mask rows=7-8 cols=47 text=/[SRDZTt]/
mask rows=7-8 cols=49-53 text=/ *\d+\.\d/
mask rows=7-8 cols=55-59 text=/ *\d+\.\d/
mask rows=7-8 cols=61-69 text=/[ \d:.]+/
# A running task is drawn bold, a sleeping one is not: the bold row depends on the data
mask rows=7-8 attrs=ignore

[assert]
assert row=6 text=/ +PID +USER +PR +NI +VIRT +RES +SHR S +%CPU +%MEM +TIME\+ +COMMAND */ reverse
assert find=/%CPU/ reverse !bold
```

Read against 6.5: `mask rows=7-8 cols=26-45 text=/[ \d]+/` is **two spans** (one per row), each of which must match the
pattern on its own. `mask rows=2-4 find=/\d+\.\d/ ...` is not stale if row 3 has no figure, but is stale if none of
rows 2 to 4 has one. `cols=47` is the same as `cols=47-47`.

### 7.3 A screen with wide, combining and private-use characters

Row 0 is a path with a backslash, a wide pair and an ASCII letter (`C:\dir  你好X`). Row 1 holds, in order: `e` with a
combining acute accent (one cell), a space, the family emoji written man, ZWJ, woman, ZWJ, girl (three wide characters,
the first two carrying the ZWJ), a space, a private-use character, a space, a red bold `R`, a space, a curly underlined
`c` with an RGB underline colour, a space, and a wide reverse-video character.

```text
vtee-golden 1
schema 1
semantics 1

[screen 1]
size 40x6
cursor row=2 col=0 shape=block hidden=false
modes
alt_screen false
title null

[text]
0|C:\\dir  你\.好\.X
1|e\+{301} 👨\+{200d}\.👩\+{200d}\.👧\. \u{e000} R c 你\.
2|
3|
4|
5|

[attrs]
1: 11 fg=idx:1 bold
1: 13 underline=curly ulcolor=rgb:#c80a14
1: 15-16 reverse

[assert]
assert row=1 cols=11 text="R" fg=idx:1 bold
assert row=1 cols=13 underline=curly ulcolor=rgb:#c80a14
assert row=1 cols=0 text="e\u0301"
```

The last assertion compares scalar sequences: `"e\u0301"` is `e` plus the combining acute, and would not equal the
precomposed `é`.

### 7.4 A portable golden

What `--update --portable` records for a program that printed `ok` and turned on bracketed paste. The file-level
`ignore` lines are the portable profile (6.14, frozen at 0.1.0): the mode tokens ConPTY hides, the underline colour it
garbles, and the title. They apply to every screen the run has, so the same golden passes on Linux and under ConPTY.

```text
vtee-golden 1
schema 1
semantics 1
ignore modes ?1 ?1000 ?1002 ?1003 ?1005 ?1006 DECKPAM
ignore attrs ulcolor
ignore title
x-producer vtee 0.1.0 alacritty_terminal 0.26.0

[screen 1]
size 40x3
cursor row=1 col=0 shape=block hidden=false
modes ?2004
alt_screen false
title null

[text]
0|ok
1|
2|
```

### 7.5 What a version difference looks like

A hypothetical future `semantics 2` that stops reporting tab-stop cells as spaces. A golden recorded under semantics 1
is **refused**, whether or not its own screens contain a tab, and nothing is compared:

```text
REFUSED tf-edit.golden: semantics 1 in the golden, 2 in this vtee; nothing was compared
  restamp (only if nothing the golden checks changed):  vtee --script edit.ndjson --expect tf-edit.golden --update=restamp -- tf -n
  or re-record with --update; what changed is listed in docs/guide/snapshot-semantics.md, entry 2
```

For a golden whose screens do not contain a tab, restamp succeeds and rewrites only the `semantics` line:

```text
RESTAMPED tf-startup.golden: semantics 1 -> 2
```

For one whose third screen does contain a tab, restamp refuses, writes nothing, and shows what would differ:

```text
NOT RESTAMPED tf-edit.golden: 1 difference
screen 3:
  text      row 4 col 8   expected ' '   actual '\u{9}'
```

That golden has to be re-recorded with `--update` (and the reviewer sees the same change in the diff).

## 8. Open questions, closed

Decided here, with the reason. The first block is from the first draft; the second block was added or changed by the
reviews.

| Question | Decision | Reason |
| --- | --- | --- |
| Keep `lines`? | Yes, as a derived view | Cheap, and agents and `wait_for` use it. It is defined, not duplicated state |
| Booleans or an `attrs` array? | Booleans, written only when true | Same extensibility (new field = new attribute), simpler for typed readers, and the golden tokens are the field names |
| `""` for the continuation cell, or a flag? | `""` | Unambiguous (E8), no new field; derived width |
| One cell per token, or a `[wide]` side table in goldens? | One token per cell, `\.` for the continuation | A side table needs width tables to read; this needs none |
| How do multi-scalar cells show in a golden? | Base scalar, then `\+{H}` for each further scalar | Cell boundaries must not depend on Unicode data |
| Normalise Unicode in cells? | No | A normaliser changes meaning and ties the format to a Unicode version |
| Cluster ZWJ sequences? | No; record the emulator's model (E6) | The emulator is the source of truth; a golden records what it did |
| Resolve colours to RGB? | No | Themes are not ours |
| `hyperlink` id? | URI only; `hyperlink_id` reserved | Rare; additive later |
| Keep `blink`? | Remove; name reserved | Always false |
| Report `conceal`? | Yes | The emulator tracks it; ignoring it hides a real attribute |
| `modes`: numeric or symbolic names? | Keep the xterm numeric names plus `DECKPAM` | They are the documented standard; symbolic names would be our invention |
| `modes` order? | A set, written sorted; readers accept any order | The old order was accidental; order carries no meaning |
| Platform differences in the version number? | No: `ignore` directives and the `--portable` profile | Versions are for meaning, not for the OS |
| Where do `assert` lines live? | Their own `[assert]` section | They compare the actual screen, not the stored one |
| Header `title` as `-` or a literal? | `null` or a JSON string | `-` is a legal title |
| Comments in `[text]`? | Not allowed (nor in `[attrs]`) | Rows and runs are regenerated |
| `byte_offset` in goldens? | Never | E3 |
| Scrollback in goldens? | No | Not part of the screen; `scrollback_cells` reserved |
| Both screen buffers? | `alt_screen` stays; `inactive` reserved | The library cannot read the inactive buffer without destroying it |
| Versions of the golden grammar | `vtee-golden N`; a reader supports all released N up to its own (open question Q1) | Separates grammar from snapshot layout without retiring old files |
| Is "add a token" free? | No: every newly reported mode token or attribute is a semantics bump (OD8) | Reviews TF1 and FA-A1 demonstrated that additions break stored goldens; the owner chose the one-sentence rule over declared vocabularies, and froze the 0.1.0 vocabulary in one table (3.8) |
| What happens to a golden of an older or newer schema? | Refused at once (exit 2) with a pointer to `--update=migrate` or a re-record; no window, no second reader (OD3) | One reader and one rule |
| What happens on a `semantics` mismatch? | Refused, nothing compared (exit 2); the remedy is `--update=restamp` or a re-record; no override flags (OD4) | One outcome for every bump |
| What about snapshots and goldens recorded before 0.1.0? | Unsupported: no migration; they are re-recorded (OD1) | The provisional shape also says schema 1 and was never released |
| Which new mode tokens now? | `?1002`, `?1003`, `?1005`; `?1004` and `?1007` reserved | `?1004` is injected by ConPTY (E16); `?1007` has no consumer (OD9) |
| Underline field name | `underline` (a string), not `underline_style` | Failure mode for readers that skip the version check (3.2) |
| Row-range regions: one rectangle or one span per row? | One span per row | A `text=/RE/` must match each row on its own (6.5) |
| Whole-row text: trailing blanks kept? | Removed for a whole-row region; kept for an explicit `cols=` | The natural `assert row=16 text="..."` must pass (6.5) |
| `find=` with `cols=`: filter or search only those cells? | Search only those cells | `$` then means the end of the range; no clipping rules (6.5) |
| A stale `find=` mask | No span at all in its whole row range | A figure may be missing from one row (6.5) |
| Several `find=` matches in one assert | Every match must satisfy | A highlight regression on one of them must fail (6.6) |
| `\u{41}` for `A` in a reader | Accepted | Same meaning, no ambiguity; makes extending the escape set a canonical-form change, not a grammar bump (6.3) |
| BOM and CRLF | BOM rejected; CRLF accepted | CRLF is altered silently by tools; a BOM is a deliberate save (6.1) |
| JSON string encoding in goldens | Canonical, one form (6.12) | Otherwise `write(parse(g)) == g` holds only within one implementation |
| Escape set | By category rule, frozen as data from Unicode 15.1 | Does not depend on the writing tool's Unicode tables (6.3) |
| Regex `$` and classes | ASCII only, search leftmost-first, rejection list | Same meaning in every language (6.7, E24) |
| `row_runs` | Not additive as a replacement of `cells`; reserved as a request parameter instead | Section 10 |
| One screen or several per golden? | Several: a golden holds one or more screens, always as `[screen N]` blocks | The test model produces an ordered sequence of screens; one grammar and one canonical form (6.1) |
| A one-screen golden: flat layout or a composite of one? | A composite of one | Least risk of a later breaking change; nothing to convert when a script gains a second checkpoint (6.1, OD11) |
| Shared and per-screen masks and ignores | Shared `[masks]` and file-level `ignore` apply to every screen, per-screen ones to that screen; the union applies; no negation | A clock mask is written once; the effect of a line does not depend on where it sits (6.2, 6.5) |
| Staleness of a shared `find=` mask | Stale only if it matches in no screen | A mask that finds nothing in one screen masks nothing there, and the ordinary comparison reports the difference (6.5, OD13) |
| How are run screens paired with golden screens? | By position; a `name` is compared only when both sides carry one | An inserted checkpoint should show as a diff; names are labels (6.8) |
| Does the script change? | No. Only an optional `name` on `snapshot` and `optional` on `wait_for`, both ignored by servers | No script command carries comparison fields, so a raw server cannot silently skip a comparison (6.11) |
| A `golden` subcommand family? | No: `--expect` and `--update[=MODE]` of the one run command | The model is `app + input + expected output`; "golden" is only the file kind (6.11, OD15 to OD17) |
| A guard on `--update`? | Yes: it prints what it changed, and refuses in CI unless `VTEE_ALLOW_UPDATE=1` | A stray flag in a pipeline would make every test pass (6.11, OD18) |
| Does a timed-out `wait_for` fail a script? | Yes, in every `--script` run; `"optional":true` opts out | E27: otherwise a script cannot fail a test (6.11, OD12) |
| Default terminal size of an `--expect` run | 80x24 unless `--cols`/`--rows` | E31: a verdict must not depend on the invoking terminal (6.11) |
| Is the child's exit status part of the output? | Not in 0.1.0; `[exit]` is a reserved section | A status is not a screen; the script can read it (6.11, OD14) |
| Whole script validated before the run? | Yes, in `--expect` mode | A malformed line must not leave a half-run (E28, 6.10) |

### Decisions

Recorded 2026-10-10. The owner answered each question separately, one at a time. Where the answer differs from the
recommendation this document made, the decision is marked **overrode** and the consequence is stated in the reason
column; the document was rewritten to follow the owner (the owner prefers strict, simple rules). The reviewers' verdicts
on the original questions are in appendix C (TF = TinyFugue, FA = the Fable reviewer).

| # | Question | Owner's choice | Reason and consequence |
| --- | --- | --- | --- |
| OD1 | Schema number of the frozen shape | **Keep it at `schema_version` 1.** *Overrode* the recommendation of 2 | Nothing has been released, so 1 simply means "the 0.1.0 shape". Consequence: snapshots recorded from `main` before 0.1.0 also say 1 but have the provisional layout, so they cannot be told apart by number; they are **unsupported** (no migration from them) and fail validation as malformed (exit 65). TinyFugue re-records anyway |
| OD2 | Sparse cells | **Yes, now** | Real screens shrink 20 to 27 times; moving to sparse later would be a breaking change |
| OD3 | How long an older schema stays readable | **No window.** *Overrode* the recommendation of 12 months and until the next major | One reader and one rule: an older (or newer) schema is refused at once, exit 2, with a pointer to `--update=migrate` or a re-record. `--update=migrate` is kept for every released schema, so "refused" always comes with a way forward. Contradicts TinyFugue's request for a stated period (TF OD3); their answer is that they re-record from a baseline |
| OD4 | A `semantics_version` mismatch | **Always refuse**, exit 2, nothing compared. *Overrode* the recommendation "compare anyway" | `--accept-semantics`, `--strict-semantics` and the `WARN semantics` machinery are removed; the remedy is `--update=restamp` (compares internally, rewrites the number only if nothing the golden checks would differ) or `--update`. Consequence: every bump refuses every stored golden until restamped, including those that never touch the change; accepted, and the reason the reported vocabulary is frozen up front (3.8). Contradicts TF and FA, who both proposed compare-anyway |
| OD5 | Regex dialect | **RE2 common subset**, ASCII classes, a rejection list, about 30 vectors | Portable across languages (E24) |
| OD6 | The three known emulator deviations (ED 1 on row 2, origin-mode reset, mode ?47) | **Freeze and document** in semantics 1 | No fork of `alacritty_terminal`; each is listed precisely in the semantics changelog and fixed by a semantics bump when upstream fixes it. TinyFugue confirmed none touches tf |
| OD7 | What 0.1.0 ships | **The formats, the test vectors, `--expect`, `--update` and `--update=restamp`**; `--update=migrate` follows no later than the first schema or grammar bump | The reference implementation validates the formats before the tag; there is nothing to migrate from yet |
| OD8 | How new mode tokens and attributes are handled | **Every newly reported name is a semantics bump; no `vocab` header and no `--update=vocab`.** *Overrode* the recommendation of declared vocabularies | Simplest rule: one sentence, no extra header lines, no undeclared-token handling. Consequence: each addition after 0.1.0 refuses every stored golden until restamped (OD4), so the 0.1.0 vocabulary is frozen as one table (3.8) and additions are deliberate and rare |
| OD9 | Mode vocabulary at 0.1.0 | **`?1`, `DECKPAM`, `?1000`, `?1002`, `?1003`, `?1005`, `?1006`, `?2004`**; `?1004` and `?1007` reserved | `?1004` is injected by ConPTY (E16); `?1007` has no consumer. Adding either later is a semantics bump. The code reports five of these today (checked against `src/snapshot.rs`) |
| OD10 | `--portable` profile in 0.1.0 | **Yes**; its list is frozen | One flag records a golden that passes on Linux and under ConPTY for the known differences (6.14) |
| OD11 | Layout of a one-screen golden | **Always the composite layout** | One grammar, one parser, no conversion when a script gains a second `snapshot` |
| OD12 | A timed-out `wait_for` | **Fails every `--script` run** (exit 1, kind `wait`), unless the line has `"optional": true` | Scripts become able to fail a test (E27) |
| OD13 | Staleness of a shared `find=` mask | **Stale only if it matches in no screen** | A shared clock mask must not fail on the screen that has no clock yet |
| OD14 | The child's exit status as expected output | **Deferred; `[exit]` is reserved** | A status is not a screen; the script can read it with `wait_exit` |
| OD15 | Option names | **Keep `--script`** (not renamed to `--in`); `--expect FILE`, `--update[=MODE]`, `--screen N`, `--snapshot FILE\|-`, `--connect`, `--format text\|json`, `--portable`, `--fail-fast` (and `--max-mismatches N`) | No rename of an existing flag; no aliases |
| OD16 | `--update` mode values | **`record` (bare default), `restamp`, `migrate`**; no `vocab` | Follows OD8 |
| OD17 | `migrate` as an `--update` mode | **Yes**, the one mode that needs no source | Everything that rewrites the expected file is an option of the one command |
| OD18 | A guard on `--update` in CI | **Yes**: `--update` always prints what it changed, and refuses (exit 64) when `CI` is set and non-empty unless `VTEE_ALLOW_UPDATE=1` | A stray `--update` in a pipeline would make every test pass by rewriting its expectations |

### New open question (the owner has not answered it)

**Q1. Should a reader still read older *grammar* numbers?** Section 4.3 rule 7 keeps the earlier rule that a reader
supports every grammar number up to its own, because parsing an old grammar is cheap and the grammar number says only
how to read a file. That is more permissive than the owner's strict stance on schema (OD3) and semantics (OD4), which
refuse an older number at once. The two are consistent in one respect, since a file read under an older grammar still
has to match the build's `schema` and `semantics`, but they differ in spirit. Recommendation: keep the lenient rule for
**released** grammar numbers (1 and later), because a grammar bump is for changes old readers cannot understand and never
to retire files. The alternative is to refuse an older grammar with a pointer to `--update=migrate`, which costs a
migration at every grammar bump. "Grammar 0" (TinyFugue's unreleased draft) is not a released grammar: it is
unsupported (OD1), and the rule applies from grammar 1.

## 9. Required changes before 0.1.0

Sizes are rough: S under a day, M a day or two, L several days. Dependencies are marked. This list follows the owner's
decisions of 2026-10-10 (section 8): no vocabulary machinery, no compare-anyway path and no second reader make items 6
and 7 smaller than in the previous revision, and the CI guard (OD18) is new.

1. **Versions and producer.** `SCHEMA_VERSION = 1` (the frozen shape; OD1), a new `SEMANTICS_VERSION = 1`, both first
   in the snapshot; the `producer` object; a snapshot without the two numbers is malformed. **S** (do first; everything
   else assumes it)
2. **Sparse cells and the new cell shape** (OD2; sections 3.2 to 3.5): colours as strings, `underline` as a style
   string (removing `underline_style` and the old boolean), `conceal` added, `blink` and null `hyperlink` removed,
   defaults omitted, cursor shapes lower-case and `row` unsigned, `lines` documented, the JSON writer escaping U+0085,
   U+2028 and U+2029 (3.7). `src/snapshot.rs`, `redraw_sequence`, serde, the differential harness's reader,
   `docs/guide/protocol.md`, `examples/`, the text-mode printers. **L** (after 1). Because the provisional shape on
   `main` also says schema 1 (OD1), the reader must reject it as malformed with a message that says snapshots recorded
   before 0.1.0 are unsupported.
3. **`wrapped_rows`** from `WRAPLINE` (including the wide-character placeholder case, E20), and an opt-in `wait_for` mode
   that matches against logical (joined) lines using it. **M** (after 2; parallel with 4)
4. **Modes:** a sorted set and the three new tokens (`?1002`, `?1003`, `?1005`), with the exclusivity rules of 3.6 and a
   test. **S** (parallel with 3)
5. **Conformance data** (section 5.4): `tests/fixtures/conformance/expected/`, the exact-comparison test, the width
   sample and the test that runs it through the emulator, the escape-set data, a test that fails if the reported
   vocabulary of 3.8 changes without a `SEMANTICS_VERSION` change, the CI guard for the frozen files, the `snapshot`
   commit scope in commitlint and the release-plz changelog groups. **M** (after 2 and 4)
6. **The golden library:** a module that parses, writes, compares and reports **composite** goldens (screen blocks,
   shared and per-screen masks and ignores, position pairing, `name`), with the region evaluation of 6.5, the regex
   dialect (an ASCII-rewriting layer over a regex engine plus the rejection parser of 6.7), the strict version logic of
   6.8 (grammar above refused, `schema` and `semantics` must equal the build's, `unknown` refused) and the reports of
   6.9 (text and JSON, per-screen). There is no vocabulary handling, no compare-anyway path and no second schema reader.
   **L** (after 2; the largest item)
7. **The run command:** `--expect` (check), `--update` and `--update=record`, `--update=restamp`, `--screen`,
   `--snapshot`, `--portable`, `--format`, `--fail-fast`, `--max-mismatches`; the four sources of 6.11, including the
   **new combination** `--connect` with `--script`; the table of illegal combinations with its messages; the **CI guard
   on `--update`** (OD18) and the change summary it prints; whole-script validation before the program starts; the 80x24
   default; headless behaviour; the exit codes of 6.10 and a test that every bad option exits 64 and a refusal exits 2.
   `--update=migrate` is rejected as "not available in this release". **L** (after 6; it replaces what was four
   subcommands by one runner that integrates the script runner, with fewer code paths than before: no strict/accept/
   vocab options)
8. **A timed-out `wait_for` fails the run** (OD12): the runner change in `src/client.rs` (exit 1, the stderr message,
   the `optional` field, the `wait` mismatch kind), accepting `name` on `snapshot`, and the tests of every existing
   script test that relied on exit 0. **S** (with 7; it is the same code path)
9. **Test vectors** (appendix A): the region/mask/assert triples, the composite and pairing vectors, the command-line
   vectors (including schema and semantics refusal, restamp success and refusal, the CI guard), the regex vectors, the
   round-trip files, and a runner test; confirm that a second implementation (TinyFugue's checker) passes them. **M**
   (written alongside 6 and 7; the triples are the acceptance test for them)
10. **`--update=migrate`** (5.3). **M**, **not in 0.1.0** (OD7): it follows no later than the first schema or grammar
    bump, with that bump's migration table and vectors. There is nothing to migrate from at 0.1.0.
11. **Documentation:** the golden spec promoted to `docs/guide/golden-format.md`, the run command into
    `docs/guide/cli-reference.md` and `docs/guide/testing-tuis.md`, `docs/guide/snapshot-semantics.md` created with the
    semantics 1 entry, the frozen vocabulary and the three known deviations, `protocol.md` and `versioning.md` updated,
    the platform table in `windows.md`, `examples/`. **M** (after 7)
12. **Platform CI:** a few repository goldens (one of them a multi-screen script run) checked on both Linux and Windows
    jobs, recorded with `--portable`. **S** (after 7)
13. **Tell TinyFugue** the changes from their draft (below), that their `vtee-golden 0` recordings and the
    provisional snapshot shape are unsupported and are re-recorded, and the owner decisions that differ from their
    review (OD3, OD4, OD8), before they record real goldens, and again when the shape lands on `main`. **S**

The order is 1, 2, then 3 and 4 and 5 together, then 6, then 7, 8 and 9 together, then 11 and 12, with 10 following the
first bump. TinyFugue's recordings are re-recorded after step 2.

### Changes from TinyFugue's draft

- Assertions moved to their own `[assert]` section.
- `[text]` is cell-per-token with escapes (`\.`, `\u{H}`, `\+{H}`, `\\`); the draft joined the cells' `ch` values and had no
  rule for wide characters.
- A golden holds one or more screens, always as `[screen N]` blocks: the draft's flat single-screen layout is now
  `[screen 1]`. The header is split into a **file header** (versions, file-wide `ignore`) and a **screen header**
  (`size`, `cursor`, `modes`, `alt_screen`, `title`, `wrapped`, `name`, screen `ignore`). A shared `[masks]` section
  before the first screen applies to every screen.
- Header: `schema`, `semantics`, `wrapped`, `ignore`, `x-producer` added (there is no vocabulary header); `title` is
  `null` or a JSON string, and the cursor line is `hidden=B`.
- `modes-ignore` and `title-ignore` became `ignore modes ...` and `ignore title`; `ignore attrs` and the cursor forms
  are new.
- Regex classes are ASCII-only, `$` anchors to the last visible character of the searched cells, and a rejection list
  and test vectors fix the dialect.
- A whole-row region compares the row text without trailing blanks; `rows=A-B` is one span per row; the mask and
  assertion rules are the pseudocode of 6.5.
- The tooling is one command, not a checker script: `vtee --script S --expect G -- APP` to check, `--update` to
  record, instead of a prototype `golden.py check`.
- The grammar number is `vtee-golden 1`; the draft's `vtee-golden 0` was never released and is **unsupported**: it
  is rejected, and those goldens are re-recorded (OD1).

## 10. Deferred, with the additive path reserved

Each path below was checked against the rule "an existing JSON reader keeps working unchanged"; the last column of the
table says whether it is also free for stored goldens. **Anything a golden compares is a `semantics_version` bump when
it is first reported** (4.1, OD8): the reserved names of 3.8 are not free for goldens.

| Item | Reserved name or path | Why it is additive | Why deferred |
| --- | --- | --- | --- |
| The inactive screen buffer | top-level `inactive` object with the same shape as the snapshot | A new optional field; goldens do not read it | The emulator cannot read it without destroying it |
| Cell-level scrollback | `scrollback_cells` | A new optional field | No consumer yet |
| Hyperlink ids | cell `hyperlink_id` | An optional cell field that JSON readers ignore. **Not free for goldens**: if goldens compare it, reporting it is a semantics bump | Rare |
| Blink, overline | cell `blink`, `overline` | New attribute names. **Not free for goldens**: a semantics bump when first reported (4.1) | Emulator support |
| Focus reporting, alternate scroll, autowrap and other output-affecting modes, kitty keyboard flags | more tokens in `modes` (`?1004`, `?1007`, `?7`, ...) | New mode tokens. **Not free for goldens**: a semantics bump when first reported; stored goldens are refused until restamped (4.1, OD8) | No consumer; `?1004` is also injected by ConPTY |
| `cursor.pending_wrap` | cursor field, written only when true | A new optional field that JSON readers ignore; the golden *grammar* gains a header field behind a new grammar number. **Not free for goldens**: a semantics bump when first reported | Invisible today; no consumer |
| A more compact row encoding | a `snapshot` request parameter, `"encoding":"runs"`, that returns `row_runs` **instead of** `cells` only when asked | The default reply is unchanged. (The first draft listed `row_runs` as a field; replacing `cells` by default would have been a schema bump, so the *request* is the reserved path) | Sparse cells already give 20 to 30 times; add only if measured to matter |
| `snapshot` command parameters (`scrollback`, `lines`, a cell region) | optional request fields | Absent means today's reply | Each is additive |
| Grapheme clustering | a different width model, behind `semantics_version` | **Not additive**: it changes meaning, so it is a semantics bump by the table in 4.1 | Changes meaning |
| Structural queries for tables (columns by header alignment, numeric ranges) | new assertion forms behind a new grammar number | Old goldens parse unchanged; only goldens using the new forms need a newer reader | TinyFugue's own recordings suggest the form; build after masks |
| The program's exit status as expected output | an `[exit]` section at the end of the file (the name is reserved: a grammar-1 reader rejects it) | A new section behind a new grammar number; an old reader refuses the new file loudly, and old goldens parse unchanged | A status is not a screen and needs its own rules and report kind; the script can read it with `wait_exit` today (OD14) |
| A shared `[assert]` list | `[assert]` before the first `[screen]` (rejected in grammar 1) | A new section behind a new grammar number | No consumer yet |
| Pairing screens by name, several goldens per run, an overall run timeout | `--pair-by name`, repeated `--expect`, `--timeout` | New options; the default is today's behaviour | Position pairing is enough for the recorded cases; CI jobs have their own time limits |
| A shared `ignore` that a screen can cancel | none | Would need a negation form | Not needed; an `ignore` only some screens need belongs in those screens (6.2) |
| Images (sixel, kitty graphics), selection, timestamps | none | | Not in the emulator |

## Appendix A. Test vectors

A second implementation of the checker, in any language, proves it agrees by passing these. They are frozen data
under the rules of section 5.4. A vector is a **run**: a golden, the actual screens (an NDJSON file in the format of
source (d), 6.11, so every vector can be run offline with `--snapshot`), the command-line options, and the expected
verdict. Layout, in the repository when item 9 of section 9 is done:

```
tests/fixtures/golden-vectors/
  README.md                         how to run the vectors; the version of this document they belong to
  runs/<id>-<name>/
      golden.golden                 the golden under test (one or more screens)
      run.ndjson                    the actual screens, one snapshot document per line (schema 1)
      args                          the options after `--expect golden.golden --snapshot run.ndjson`, one per line (may be empty)
      expected.txt                  the exact text report
      expected.json                 the exact JSON report
      exit                          the expected exit code, one number
  cli/<id>-<name>/
      args                          the whole command line, one argument per line
      stderr-first-line             the normative first clause of the message (illegal combinations)
      exit                          the expected exit code
  update/<id>-<name>/
      before.golden  run.ndjson  args  after.golden  stdout  exit     the file rewritten by an `--update` run, byte for byte
  regex/vectors.tsv                 pattern <TAB> subject <TAB> operation <TAB> result   (A.3)
  roundtrip/*.golden                canonical files: write(parse(g)) must equal g byte for byte
  reject/*.golden                   one malformed file per rule of 6.13, expected exit 65 (name says which rule)
  width/width-sample.txt            appendix B
  escapes/escape-set.txt            appendix B
```

### A.1 The region, mask, assert and version vectors

About 40, each pinning one rule of sections 6.5 to 6.8. The `runs/` directories are named `<id>-<name>`:

| Id | Name | Pins |
| --- | --- | --- |
| R01 | mask-text-ignore-pass | `text=ignore` skips text, attributes still compared |
| R02 | mask-rows-range-per-row-regex | `rows=7-8` is two spans; each matches the pattern on its own |
| R03 | mask-rows-range-one-row-fails | a regex failure in the second row is a `mask` mismatch |
| R04 | mask-find-pass | a `find=` yields spans and the figures vary |
| R05 | mask-find-stale | a per-screen `find=` with no span at all is a failure |
| R06 | mask-find-missing-in-one-row-ok | `rows=2-4`, one row without a match: not stale |
| R07 | mask-find-cols-anchors-inside | `$` anchors at the end of the `cols=` range |
| R08 | mask-find-empty-match-skipped | `a*` yields only non-empty spans |
| R09 | mask-find-wide-char-covers-continuation | the span includes the continuation cell |
| R10 | mask-find-inside-appended-scalars | a match starting inside a cell's appended scalars covers the whole cell |
| R11 | mask-attrs-ignore-keeps-text | `attrs=ignore` still compares text |
| R12 | mask-text-ignore-keeps-attrs | `text=ignore` still compares attributes |
| R13 | mask-overlap-skips-if-any-covers | overlapping masks |
| R14 | mask-whole-row-regex-trims | a whole-row `text=/RE/` ignores trailing blanks |
| R15 | mask-cols-range-keeps-blanks | an explicit `cols=` range keeps its blanks |
| R16 | assert-exact-whole-row-trimmed | `assert row=16 text="A Quiet Meadow"` passes on 80 columns |
| R17 | assert-exact-cols-keeps-blanks | with `cols=` the blanks count |
| R18 | assert-exact-scalar-sequence | `e` plus U+0301 differs from `é` |
| R19 | assert-find-every-match | the second match fails the attribute |
| R20 | assert-find-no-match | no span is a failure |
| R21 | assert-attr-every-cell | one cell without the attribute fails |
| R22 | assert-underline-none | `underline=none` |
| R23 | assert-not-flag | `!bold` |
| R24 | assert-link-and-not-link | `link="..."` and `!link` |
| R25 | assert-ulcolor-without-underline | a colour on a cell with no underline |
| R26 | header-ignore-cursor-position-only | `ignore cursor.position` keeps shape and hidden compared |
| R27 | schema-lower-refused | a golden with a lower `schema`: exit 2, nothing compared, the message points to `--update=migrate` |
| R28 | wrapped-mismatch-and-ignore | `wrapped` differs, then `ignore wrapped` |
| R29 | size-mismatch-skips-cells-masks-asserts | only the `size` block (and header blocks) |
| R30 | semantics-mismatch-refused-though-screens-equal | a different `semantics` number with identical screens: still REFUSED, exit 2, nothing compared (OD4) |
| R31 | semantics-mismatch-refused-with-differences | a different `semantics` number with differing screens: the same refusal, no differences listed |
| R32 | semantics-unknown-refused | `semantics unknown`: REFUSED, exit 2 |
| R33 | snapshot-semantics-differs-from-golden | form (d): the snapshots' number differs from the golden's: REFUSED as the first screen is read |
| R34 | schema-higher-refused | exit 2 |
| R35 | provisional-shape-snapshot-malformed | a snapshot of the pre-0.1.0 layout (says schema 1, no `semantics_version`): exit 65, "recorded before 0.1.0" |
| R36 | crlf-accepted | the same golden with CRLF line endings passes |
| R37 | portable-golden-passes-both | a `--portable` golden against a Linux and a ConPTY-shaped screen |
| R38 | grammar-0-golden-rejected | a first line `vtee-golden 0` (the unreleased draft): exit 65, unsupported (OD1) |

**Composite, pairing and run-level vectors.** About 25 more, in the same `runs/` layout:

| Id | Name | Pins |
| --- | --- | --- |
| C01 | composite-three-screens-pass | the three-screen tf run of 7.1 passes against itself |
| C02 | shared-mask-applies-to-every-screen | a clock mask written once masks all three screens |
| C03 | per-screen-mask-applies-to-its-screen-only | the startup screen's build-line masks do not mask screens 2 and 3 |
| C04 | shared-and-screen-masks-union | a part is skipped if either list covers it |
| C05 | shared-find-matches-in-one-screen-not-stale | a shared `find=` that matches in only one screen is not stale |
| C06 | shared-find-matches-in-no-screen-stale | the same mask matching in none: a run-level `mask` mismatch (`problem` `stale`) |
| C07 | per-screen-find-stale-in-its-screen | a per-screen `find=` with no span in its screen is stale |
| C08 | shared-ignore-applies-to-every-screen | a file-level `ignore title` |
| C09 | screen-ignore-applies-to-one-screen | a screen-level `ignore wrapped` |
| C10 | shared-region-mask-outside-a-screen-rejected | a shared `row=`/`cols=` mask outside the smallest screen: exit 65 |
| P01 | pair-by-position | screen 2 of the run is compared with screen 2 of the golden |
| P02 | fewer-screens-than-golden | a `screens` mismatch with `missing`; the common prefix is compared |
| P03 | more-screens-than-golden | a `screens` mismatch with `extra` |
| P04 | inserted-checkpoint-shifts-everything | one extra screen early: the later screens differ |
| P05 | names-equal-pass | both sides name a screen and the names match |
| P06 | names-differ | a `name` mismatch for that screen; its cells are still compared |
| P07 | name-on-one-side-only-ignored | only the golden names the screen: no mismatch |
| P08 | one-screen-run-against-composite | a `screens` mismatch whose message adds the `--screen N` hint |
| P09 | screen-selector | `--screen 2` against a one-screen run compares golden screen 2 only |
| P10 | screen-selector-out-of-range | `--screen 4` on a three-screen golden: exit 64 |
| W01 | wait-timeout-fails | a `wait_for` reply with `"matched":false`: a `wait` mismatch, exit 1 |
| W02 | wait-timeout-optional-passes | the same with `"optional":true` in the script line: PASS |
| W03 | wait-timeout-with-matching-screens | screens equal the golden but a wait timed out: still exit 1 |
| W04 | fail-fast-stops-at-first-problem | later screens are `skipped`, the exit code is the first problem's |
| W05 | other-replies-are-not-screens | `wait_exit` and `exit_status` replies in `run.ndjson` do not become screens |

**Command-line vectors** (`cli/`, no run): one per row of the table of illegal combinations in 6.11 (about 14), plus
the CI guard (`CI=true` refuses `--update` with exit 64; `CI=true VTEE_ALLOW_UPDATE=1` allows it; `CI` unset or empty
needs nothing; a check is never guarded), `--update=migrate` rejected as not available in this release (64), an unknown
`--update` mode, `--expect` with a missing golden (exit 66, message with the `--update` hint), a malformed golden
(65), a missing script (66), and a bad flag exits 64 not 2.

**Update vectors** (`update/`): `--update` creating a file (`CREATED`, no masks); re-recording a screen and keeping
masks, assertions and comments verbatim; adding a screen; dropping a screen (the message lists what it had); an
`UNCHANGED` re-record produces an empty diff including `x-producer`; `--update` refusing to write after a timed-out
wait (exit 1, `not written`); `--update --screen 2` rewriting one block; `--update=restamp` succeeding (only the
`semantics` line changes) and refusing (writes nothing, shows the differences); the change summary lines of `UPDATED`
(cells changed, new screen, dropped screen). `--update=migrate` has no vectors at 0.1.0 (OD7): its vectors arrive with
the first schema or grammar bump.

The `reject/` files cover each rule of 6.13: BOM, lone CR, NUL, bad first line, missing and duplicated keys (in the file
header and in a screen header), a key in the wrong header, unknown and duplicated sections, no `[screen]`, a wrong screen
number, a `[text]` before the first `[screen]`, `[exit]`, a shared `[masks]` after a screen, wrong row count, bad label,
`\.` at the start of a row and after `\.`, `\+` at the start of a row and after `\.`, a bad `\u{}`, overlapping runs, an
empty run, a repeated token, an attribute outside the frozen vocabulary, `vtee-golden 0`, a comment in `[text]`, a region outside the size, a mask with
nothing to do, `text="EXACT"` on several rows and with `find=`, a regex outside the dialect, a bad JSON string, a
repeated token in a `mask` line, an empty `ignore modes`.

### A.2 Two vectors in full

A 6 by 2 screen whose row 0 reads `ab 12 ` with the digits bold. The golden (one screen, so `[screen 1]`):

```text
vtee-golden 1
schema 1
semantics 1

[screen 1]
size 6x2
cursor row=0 col=0 shape=block hidden=false
modes
alt_screen false
title null

[text]
0|ab 12
1|

[attrs]
0: 3-4 bold

[masks]
mask row=0 cols=3-4 text=/[0-9]+/
```

The mask is on line 20 of the file. The run (source (d)) is the single document of `run.ndjson`.

**R-a (pass, exit 0).** The actual screen's row 0 is `ab 99 ` with the `99` bold. Cells 0, 1, 2 and 5 equal the golden's; cells
3 and 4 are masked for text, the span text `99` matches `^(?:[0-9]+)$`, and the bold attribute equals the stored one.
Report: `PASS g.golden (1 screen)`.

**R-b (fail, exit 1).** The actual row 0 is `ab x7 ` with the `x7` bold. The span text `x7` does not match. Report:

```text
FAIL g.golden: 1 mismatch in 1 of 1 screen
screen 1:
  mask       golden line 20   mask row=0 cols=3-4 text=/[0-9]+/   row 0 col 3-4   actual 'x7' does not match
```

and the JSON report:

```json
{"golden":"g.golden","result":"fail","grammar":1,"schema":1,"semantics":1,"snapshot":{"schema":1,"semantics":1},"screens_expected":1,"screens_actual":1,"total":1,"shown":1,"run":[],"screens":[{"index":1,"name":null,"result":"fail","mismatches":[{"kind":"mask","line":20,"mask":"mask row=0 cols=3-4 text=/[0-9]+/","problem":"text","row":0,"col_start":3,"col_end":4,"actual":"x7"}]}]}
```

### A.3 Regex vectors

Positions are scalar offsets, `[a,b)` is start inclusive and end exclusive. A subject shown with `\u{H}` stands for the
scalar U+H (the table is escaped so that no invisible character hides in it). `search` is the leftmost-first search of
6.7, `full` is `^(?:RE)$`, `find` is the scan loop of 6.5 that skips empty matches. The results were computed with
Python 3.13 `re` under `re.ASCII`; the Rust `regex` crate with the ASCII rewriting of 6.7 gives the same for the
subset (the tricky rows are V04 to V06, V12 and V14 to V17).

| # | Pattern | Subject | Operation | Result |
| --- | --- | --- | --- | --- |
| V01 | `abc` | `xabcx` | search | match [1,4) |
| V02 | `a.c` | `abc` | full | match |
| V03 | `\d+` | `ab 123 x` | search | match [3,6) |
| V04 | `\d+` | `\u{663}\u{664}` | search | no match |
| V05 | `\w+` | `\u{e9}_x` | search | match [1,3) |
| V06 | `\s` | `a\u{a0}b` | search | no match |
| V07 | `[\d.]+` | `v1.25x` | search | match [1,5) |
| V08 | `[^a-c]` | `abcd` | search | match [3,4) |
| V09 | `^ab` | `ab` | search | match [0,2) |
| V10 | `b$` | `ab` | search | match [1,2) |
| V11 | `^$` | `` | full | match |
| V12 | `a*` | `baa` | find | [1,3) |
| V13 | `a+?` | `aaa` | search | match [0,1) |
| V14 | `a\|ab` | `ab` | search | match [0,1) |
| V15 | `(a\|ab)(c\|bcd)` | `abcd` | search | match [0,4) |
| V16 | `x{2,3}` | `xxxx` | search | match [0,3) |
| V17 | `x{2,3}?` | `xxxx` | search | match [0,2) |
| V18 | `(?:ab)+` | `ababab` | search | match [0,6) |
| V19 | `\+\d` | `a+1` | search | match [1,3) |
| V20 | `a\/b` | `a/b` | search | match [0,3) |
| V21 | `\.\.` | `a..b` | search | match [1,3) |
| V22 | `[ \d:.]+` | `  1:23.4 x` | search | match [0,9) |
| V23 | `[A-Z]` | `abC` | search | match [2,3) |
| V24 | `\D` | `12a` | search | match [2,3) |
| V25 | `\S+` | `  ab c` | search | match [2,4) |
| V26 | `\W` | `a-b` | search | match [1,2) |
| V27 | `.` | `\u{4f60}` | search | match [0,1) |
| V28 | `[a-c]{2}` | `xabcx` | search | match [1,3) |
| V29 | `(ab\|cd)+` | `abcdab` | search | match [0,6) |
| V30 | `\d\d:\d\d` | `at 03:18 now` | search | match [3,8) |

Rejected patterns (the pattern is invalid in the dialect, exit 65 when it appears in a golden):

| # | Pattern | Why |
| --- | --- | --- |
| V31 | `\bfoo` | word boundary |
| V32 | `(?i)abc` | inline flag |
| V33 | `\x41` | hexadecimal escape |
| V34 | `\p{L}` | Unicode property |
| V35 | `(?=a)` | look-ahead |
| V36 | `(a)\1` | back-reference |
| V37 | `[[:alpha:]]` | POSIX class |
| V38 | `(?P<n>a)` | named group |
| V39 | `\Aa` | start-of-text anchor |
| V40 | `a\z` | end-of-text anchor |
| V41 | `\u0041` | Unicode escape |
| V42 | `\h` | unknown escape |
| V43 | `a**` | nothing to repeat |
| V44 | `a/b` | unescaped delimiter: the pattern ended at the slash and `b` follows |
| V45 | `(a` | unbalanced group |
| V46 | `[a` | unterminated class |

(In the tables the vertical bar of an alternation is written `\|` only to keep the table intact; the pattern itself is
`a|ab`, not `a\|ab`.)

## Appendix B. Frozen data files

Two data files accompany this document. They are generated, then frozen; they move to
`tests/fixtures/golden-vectors/` when the implementation lands. The width sample has landed: it is now
`tests/fixtures/conformance/width-sample.txt`, run by `tests/conformance.rs`.

- **`tests/fixtures/golden-vectors/escapes/escape-set.txt`** (moved there from `docs/design/`): the escape set of section 6.3. 876 inclusive hexadecimal ranges, 966,490
  scalars, from Unicode 15.1.0 (Python 3.13.5's `unicodedata`), with the SHA-256 of the range lines in its header.
  Generated by a scratch script (`escset.py`) from the categories `Cc Cf Cs Co Cn Zs Zl Zp Mn Me`, minus U+0020, plus
  U+2800, U+115F, U+1160, U+3164, U+FFA0.
- **`docs/design/golden-width-sample.txt`** (now `tests/fixtures/conformance/width-sample.txt`): the width class of 1,062 scalars (E19), as run-length ranges: `U+0020..U+0041 1`
  means every scalar from U+0020 to U+0041 occupies one cell; `0` is zero-width (appended to the previous cell), `2` is
  wide. Measured by writing one row `A<scalar>œ` per scalar through the real emulator and reading the column of `œ`.
  The test of item 5 in section 9 repeats that measurement and compares it to the file.

## Appendix C. Review dispositions

Two independent reviews of the first draft: **TF** is the TinyFugue session's review (a review agent built a checker from
sections 6 and 7 alone and converted their five screens and `top`), **FA** is a fresh adversarial reviewer who built a
checker, ran it on 37 probe screens plus all 12 of TinyFugue's recordings, and reproduced the document's measurements.
Both found the grammar implementable from the text and agreed the shape changes are sound. Where they disagreed, the
decision and the reason are in the table.

| Finding | Disposition | Reason and where it landed |
| --- | --- | --- |
| TF1 / FA-A1: additive vocabulary is not free for goldens | **Accepted; resolved as TF proposed** | Both demonstrated the break. TF suggested treating newly reported things as semantics bumps; FA suggested self-describing goldens. The earlier revision chose FA's; **the owner overrode it (OD8)**: every newly reported name is a semantics bump, there is no `vocab` header, and the 0.1.0 vocabulary is frozen in one table. 3.8, 4.1 |
| TF2: report only specified for text | **Accepted** | An example and a JSON field table per kind; exit 65 for a malformed snapshot; NAME defined. 6.9, 6.10 |
| TF3: region and mask evaluation ambiguous (a to g) | **Accepted** | Pseudocode and fixed answers: (a) one span per row, (b) whole-row text trims, explicit `cols=` keeps, (c) `find=` with `cols=` searches only those cells, (d) stale = no span in the whole range, (e) every match, (f) forms and free token order, (g) a match inside appended scalars covers the whole cell. 6.5, 6.6; vectors R01 to R25 |
| TF4: JSON string literals have no canonical encoding | **Accepted** | 6.12 |
| TF5: canonical form implied, reader strictness unstated | **Accepted, two points changed** | Blank-line rule, optional `[attrs]`, hex rules, comment rules, `ignore` merging, update's treatment of header comments, a must-reject list. TF wanted `\u{41}` and an unsorted `modes` line rejected; they are **accepted** (FA wanted `\u{H}` accepted too) because the meaning is the same and it keeps extending the escape set a canonical-form change. 6.1, 6.2, 6.3, 6.13 |
| TF6: `x-producer` contradicts the empty-diff promise | **Accepted** | `update` rewrites it only when something else changes. 6.11 |
| TF7: CRLF on Windows | **Accepted** | Readers accept CRLF, reject a BOM (both reviewers wanted the BOM rejected); `.gitattributes` advice. 6.1 |
| TF8: migration mapping unwritten and lossy | **Accepted as a rule; the table is withdrawn for 0.1.0** | `migrate` never invents facts. There is no mapping table because nothing before 0.1.0 is supported (OD1) and `migrate` ships with the first bump (OD7). 5.3 |
| TF9 / FA-A3: conformance set does not cover width tables | **Accepted** | A frozen width sample of 1,062 scalars and the rule for upgrades. E19, 5.4, appendix B |
| TF10: regex dialect is prose only | **Accepted** | Exact construct list, rejection list, ASCII rewriting, 46 vectors. 6.7, appendix A.3, E24 |
| TF11: exit code 2 may collide with clap | **Checked: no collision** | `src/main.rs` maps every clap error to 64. A test is required. E25, 6.10 |
| TF12: size mismatch and masks | **Accepted** | Masks and assertions are skipped too. 6.8 |
| TF13: `--accept-semantics` with no differences | **Withdrawn** | The flag no longer exists: a semantics mismatch is always refused (OD4). 6.8 |
| TF14: escape set gaps | **Accepted** | Defined by category rule, frozen from Unicode 15.1. 6.3, appendix B, E26 |
| TF15: minor items | **Accepted** | 0-based stated (6); "1 mismatch" wording (6.9); the header key is `schema`, the JSON field `schema_version` (4.3, 6.2); `ignore cursor` split into position, shape, hidden (6.2); `ulcolor` without `underline` is meaningful (3.2, 6.6) |
| TF: wrapped_rows is also set by a wide char pushed to the next row | **Accepted** | 3.2, E20 |
| TF: `\\.` and `\.` look alike | **No change** | Parsing is unambiguous (the grammar is by token). 6.3 |
| TF: `idx:1` and `rgb:#cd0000` are different values | **Stated as intentional** | 3.2 |
| TF OD3 (state the window in time) | **Rejected by the owner** | No window at all: an older schema is refused at once (OD3). 5.3 |
| TF OD4 (compare anyway) | **Rejected by the owner** | A semantics mismatch is always refused, with no override flag (OD4). 6.8 |
| TF OD7 (formats, write, check, update in 0.1.0) | **Accepted**, in the run-command form | `--expect` and `--update` with `record` and `restamp`; `migrate` follows (OD7) |
| FA-A2: platform list in 6.12 is wrong | **Accepted and re-measured on Windows 11** | DECKPAM is also absorbed, ConPTY injects `?1004h`, SGR 58 is garbled. E15 to E17, 6.14, OD10 |
| FA-A4: `golden migrate` cannot produce a valid grammar-1 file | **Accepted as a rule; deferred with `migrate`** | `ignore` for unrecoverable facts, `semantics unknown`. There is nothing to migrate from at 0.1.0 (OD1, OD7). 5.3 |
| FA-A5: grammar bumps fall under the two-minor refusal | **Accepted; the window itself is gone** | There is no deprecation window (OD3). Readers support all *released* grammar numbers up to their own; whether that should be stricter is open question Q1. 4.3, 5.3, 8 |
| FA-B: blank cell definition | **Accepted** | `ch == " "` exactly (E21). 3.4 |
| FA-B: a single index where a range is allowed | **Accepted** | 6.5 |
| FA-B: `\u{H}` accepted for any scalar | **Accepted** (TF opposed) | See TF5. 6.3 |
| FA-B: `\.`, `\+` and row-label rules | **Accepted** | 6.3, 6.13 |
| FA-B: regions over multi-scalar cells; `text="EXACT"` compares scalars | **Accepted** | 6.5, 6.6 |
| FA-B: regex rejection list and ASCII classes inside classes | **Accepted** | 6.7 |
| FA-B: `modes` with zero tokens; mouse-mode exclusivity | **Accepted** | 6.2, 3.6, E18 |
| FA-B: report counting, first lines for 65 and 66 | **Accepted** | 6.9, 6.10 |
| FA-B: `lines` can have more scalars than `cols`; `underline=none` in asserts | **Accepted** | 3.2, E22, 6.6 |
| FA-C: `pending_wrap` is invisible | **Reserved** | 3.2, section 10 |
| FA-C: `row_runs` is not additive | **Accepted** | Reserved as a request parameter instead. Section 10 |
| FA-C: `underline` changes type under the same name | **Decided: keep the name** | Failure modes for readers that skip the version check argue for it. 3.2 |
| FA OD1 to OD7 | **Agreed in the earlier revision; the owner overrode OD1, OD3 and OD4** | Schema stays 1, no window, always refuse. Section 8 |
| FA-E: not verified (`wrapped_rows` live, Linux ConPTY equivalent, Windows vim and less) | **Open** | `wrapped_rows` is inferred from the emulator source (E20) and is verified when item 3 of section 9 is built |

### The run-and-compare model (owner change after the reviews)

After the two reviews the owner chose the test model `app + input + expected output` and removed the `golden`
subcommand family. These are the changes and their reasons; the earlier rows above that mention `golden write`,
`golden check`, `golden update` or `golden migrate` refer to the options below.

**Mapping of the earlier names.**

| Earlier (in the reviews and in rows above) | Now |
| --- | --- |
| `vtee golden check G (--connect S \| --snapshot F)` | `vtee --expect G (--connect S \| --snapshot F)`; with a script, `--script X --expect G -- APP` |
| `vtee golden write -o G ...` | `vtee ... --expect G --update` (creates the file if it is missing) |
| `vtee golden update G ...` | `vtee ... --expect G --update` (keeps masks, assertions, comments, `ignore`) |
| `golden update --restamp` | `--update=restamp` |
| `golden update --refresh-vocab` | withdrawn: there is no vocabulary header (OD8) |
| `vtee golden migrate FILE...` | `vtee --expect FILE --update=migrate` (ships no later than the first bump, OD7) |
| `golden write --portable` | `--update --portable` |

| Change | Disposition | Reason and where it landed |
| --- | --- | --- |
| A golden holds one or more screens | **Accepted (owner)** | The output of a script is a sequence of screens. Composite grammar: 6.1, 6.2 |
| A one-screen golden keeps the flat layout, or is a composite of one | **Decided: composite of one** | One grammar and one canonical form; a flat form would need a conversion when a script gains a second `snapshot`. 6.1, OD11 |
| Shared masks across screens; how they combine; staleness | **Decided** | Union, no negation; a shared `find=` mask is stale only if it matches in no screen. 6.5, OD13 |
| Pairing by position; names as labels | **Decided** | An inserted checkpoint shows as a diff; a `name` is compared only when both sides have one. 6.8 |
| No golden fields in script commands | **Accepted (owner)** | An optional `name` on `snapshot` and `optional` on `wait_for` are ignored by servers. 6.11 |
| A timed-out `wait_for` fails a script | **Decided**, owner decision | E27 shows a script cannot fail today. 6.11, OD12 |
| The four sources, the legality table and its messages | **Decided** | 6.11 |
| Update modes `record`, `restamp`, `migrate` (`vocab` was withdrawn with OD8) | **Decided**, owner decision | 6.11, OD16, OD17 |
| Whole-script validation and the 80x24 default under `--expect` | **Decided** | E28 and E31. 6.10, 6.11 |
| The child's exit status as expected output | **Deferred; name `[exit]` reserved** | 6.11, section 10, OD14 |
| The earlier reviews' test-vector, regex, escape-set and report requirements | **Carried over unchanged** | Appendices A and B were extended, not replaced, for composites, pairing, the command line and update modes |

### The owner's final decisions (2026-10-10) and what they changed

The owner answered the ten format questions and the new run-command questions one at a time (section 8, "Decisions").
Four answers differ from this document's earlier recommendations and change findings above:

| Decision | Effect on the earlier findings |
| --- | --- |
| OD1: the frozen shape keeps schema 1 | The provisional-schema-1 migration table is gone. Snapshots and goldens recorded before 0.1.0, including TinyFugue's `vtee-golden 0` draft and the provisional JSON shape, are unsupported; the provisional shape is rejected as malformed because it lacks `semantics_version`. TF8, FA-A4 |
| OD3: no deprecation window | The 12-month rule and the second schema reader are removed; `migrate` is the only way forward for an older schema, and it follows the first bump. TF OD3, FA-A5 |
| OD4: a semantics mismatch is always refused | `--accept-semantics`, `--strict-semantics` and the `WARN semantics` path are removed. `--update=restamp` is the cheap, safe remedy. TF13, TF OD4, FA OD4 |
| OD8: every newly reported name is a semantics bump | The `vocab` header, `WARN vocab`, `--accept-vocab` and `--update=vocab` are removed. The 0.1.0 reported vocabulary is frozen in 3.8, and the reserved names (`?1004`, `?1007`, `blink`, `overline`, `hyperlink_id`, `pending_wrap`, ...) arrive with a bump each. TF1, FA-A1 |

The other decisions (OD2, OD5 to OD7, OD9 to OD18) follow the earlier recommendations.
