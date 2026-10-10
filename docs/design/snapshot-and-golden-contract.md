# Snapshot and golden contract (design)

Status: **proposed, for owner review before 0.1.0.** Written 2026-10-10. When accepted, the normative parts (sections 3
to 6) move to the user guide and this page stays as the record of why. Nothing here is implemented yet except where a
section says "today".

The snapshot JSON and the golden file format are a long-lived contract. A stored golden outlives any one `vtee`
version (the TinyFugue project will use goldens for years, across Windows and Linux, to check a faithful port of tf), so
the formats are designed once, carefully, before 0.1.0 is tagged. Internal code stays free to change behind the
contract.

## 1. Goals, non-goals and who depends on this

**Goals**

1. A stored golden either still means what it meant when it was recorded, or `vtee` refuses it with a clear
   message. It never silently passes or fails for a different reason.
2. A language-neutral format: no Rust-isms in the JSON, no tables a reader needs to parse the files (Unicode width or
   category data, for example).
3. A small frozen core, with named, reserved paths for everything we know we will add.
4. Goldens that are small, diff well in git and are readable by a person reviewing a change.
5. One meaning on Linux and Windows, apart from a short, named list of platform differences that a golden can opt out
   of explicitly.

**Non-goals**

- Pixels, fonts, themes or resolving palette colours to RGB. A cell says `idx:1`, never what red looks like.
- Grapheme-cluster rendering. A cell is what the emulator's width model says it is (section 3.4).
- Timing. A snapshot is a state, not a movie.
- The NDJSON command set. Commands are covered by [versioning](../guide/versioning.md); only the shape of the
  `snapshot` object, which commands return, is covered here.

**Who depends on it**

| Dependant | What it relies on |
| --- | --- |
| Stored goldens (TinyFugue and others) | The golden grammar, `schema_version`, `semantics_version`, and the meaning of every cell |
| AI agents reading snapshots | Field names, types and defaults; `lines` as a convenient view |
| Procedural tests in any language | The same, plus exit codes of `vtee golden check` and its JSON report |
| `vtee`'s own conformance set | The meaning of cells for 80 escape-sequence cases (section 5.4) |

## 2. What was measured

These are the facts the decisions below rest on. Each was run on the current `main` (built natively on Windows 11, and
in the `vtee-test` Docker image for Linux), unless it cites TinyFugue's recorded screens
(`tests/screens-draft/recorded/`, read-only).

| # | Observation | How it was obtained |
| --- | --- | --- |
| E1 | An 80x24 snapshot is 413,187 bytes of JSON on Windows (`cmd /c echo hi`). About 215 bytes per cell, because every cell repeats twelve fields. Adding `underline_style` and `underline_color` account for the difference between TinyFugue's 311 KB recordings (made before they existed) and 413 KB today. | `vtee --script s.ndjson --cols 80 --rows 24 -- cmd /c "echo hi"` (`MSYS_NO_PATHCONV=1`), length of the output line |
| E2 | The `cells` array alone is 309,475 bytes for TinyFugue's `tf-session` screen and 308,856 bytes for `top`. Written with defaults omitted it is 9,382 and 14,334 bytes (33 and 21.5 times smaller). | A throwaway converter run over TinyFugue's `run1.json` files |
| E3 | `byte_offset` is not comparable across platforms or even across line disciplines. `printf 'hi\r\n'` under `sh` on Linux gives 5 (the tty turns `\n` into `\r\n`); `cmd /c echo hi` under ConPTY gives 65 (ConPTY adds its own sequences). | `--script` with `wait_exit` then `snapshot`, on both |
| E4 | `title` differs by platform: `null` on Linux for a program that sets none, `"C:\\windows\\system32\\cmd.EXE"` under ConPTY. | Same runs |
| E5 | A wide character is two cells: the character, then a cell whose `ch` is the empty string (`你好X` gives `"你","","好","","X"`). | Linux probe, 40x6 |
| E6 | A ZWJ sequence is not clustered. The family emoji (man, ZWJ, woman, ZWJ, girl) gives cells `"👨‍"`, `""`, `"👩‍"`, `""`, `"👧"`, `""`: the ZWJ is appended to the emoji before it, and each emoji takes two cells. | Same |
| E7 | A combining mark is appended to the previous cell (`e` then U+0301 is one cell, `"é"`, two scalars). No normalisation is applied. | Same |
| E8 | Control characters never reach a cell: U+0001, U+007F, U+0085 and BEL were dropped. A private-use character (U+E000) is stored verbatim, and prints as nothing in a terminal, so a JSON reader sees a cell that looks empty but is not (`ch` has length 1, the continuation cell has length 0). | Same |
| E9 | OSC 8 `id=x` is dropped; only the target URI survives (`hyperlink` is the URI). | Same |
| E10 | A line of 45 characters on 40 columns gives row 0 with 40 characters and row 1 with `ABCDE`. The snapshot does not say the break was a soft wrap, although `alacritty_terminal` records it (`Flags::WRAPLINE` on the last cell of the row). This is why `wait_for` cannot match a wrapped line. | Same; `alacritty_terminal-0.26.0/src/term/cell.rs` |
| E11 | `alacritty_terminal` also tracks SGR 8 (conceal, `Flags::HIDDEN`), which the snapshot does not report. | Same source file |
| E12 | The emulator can report ?1000, ?1002, ?1003, ?1004, ?1005, ?1006, ?1007, ?2004, ?1, keypad mode and several more; `vtee` reports five. | `alacritty_terminal-0.26.0/src/term/mod.rs`, `TermMode` |
| E13 | The cursor can rest on the continuation cell of a wide character; the emulator's own renderer moves it back one column, `vtee` reports the raw position. | `RenderableCursor::new` against `src/snapshot.rs` |
| E14 | A grammar sketch (section 6) round-trips seven real screens with zero differences: TinyFugue's `tf-session` and `top`, and five character probes (wide, ZWJ, control and private-use, combining, a composite). | A throwaway script, `spike.js`, not in the repository |

## 3. The snapshot schema, decided

### 3.1 Shape at a glance

Schema version 2. Every top-level field is present in every snapshot; cell fields are written only when they differ
from their default (section 3.3). The order of keys has no meaning; `vtee` writes them as below, with
`schema_version` and `semantics_version` first so a reader can refuse before reading the rest.

```
{
  "schema_version": 2,
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
wide character (generated from a real snapshot by the throwaway converter):

```
[{"ch":"é"},{},{"ch":"👨‍"},{"ch":""},{"ch":"👩‍"},{"ch":""},{"ch":"👧"},{"ch":""},{},{"ch":""},{},
 {"ch":"R","fg":"idx:1","bold":true},{},{"ch":"c","underline":"curly","underline_color":"rgb:#c80a14"},{},
 {"ch":"你","reverse":true},{"ch":"","reverse":true}]
```

Cell 9 is the private-use character U+E000: it looks the same as cell 3, a continuation cell, but its `ch` has one
scalar, not none.

### 3.2 Field by field

Today's shape is `schema_version` 1 on `main`. The right-hand column is the decision.

| Field | Today | Problem | Decision (schema 2) |
| --- | --- | --- | --- |
| `schema_version` | integer, first | none | Keep, first field. Value becomes 2 for the shape in this document. |
| `semantics_version` | absent | An output fix (TAB, ED 1) changes what a stored golden means but not the layout | **New**, second field. Integer, starts at 1. See section 4. |
| `byte_offset` | u64 | Meaningless across sessions and platforms (E3), and tempting to store | Keep. Defined as an opaque, per-session, monotonic count of bytes read from the pty. Valid only for ordering two snapshots of one session. Never stored in a golden. |
| `cols`, `rows` | integers | none | Keep. Integers 1 to 1000 (the resize limit). |
| `lines` | strings padded with spaces | Redundant with `cells`; the docs say "padded to `cols`" but with wide or combining characters the string has fewer scalars than `cols` | Keep as a derived convenience (cheap, and `wait_for` and agents use it). Define: `lines[r]` is the concatenation of `cells[r][c].ch` for every `c`, trailing blanks included. Writers must keep it consistent with `cells`; readers prefer `cells`. |
| `cells` | `rows` by `cols` dense objects | 215 bytes per cell (E1), growing with every new field | Keep `rows` by `cols`, but sparse: section 3.3. |
| cell `ch` | string | Continuation cell is `""`, easily confused with a private-use character visually | Keep. Default `" "`. Rules in section 3.4. |
| cell `fg`, `bg` | `"Default"`, `{"Indexed":N}`, `{"Rgb":[r,g,b]}` | Rust's default enum encoding; three shapes for one field; verbose | Strings: `"default"` (omitted), `"idx:N"`, `"rgb:#rrggbb"`. Same vocabulary as the golden file. |
| cell `bold`, `dim`, `italic`, `reverse`, `strike` | booleans, always written | verbose | Booleans, written only when `true`. |
| cell `underline` and `underline_style` | a boolean and a string that say the same thing | two ways to say one thing | One field, `underline`: `"single"`, `"double"`, `"curly"`, `"dotted"` or `"dashed"`, omitted for none. `underline_style` goes away. |
| cell `underline_color` | Color | none | Keep, colour string, omitted for default. |
| cell `blink` | always `false` | the emulator has no blink flag | Remove. The name is reserved: it will appear only if the emulator gains support. |
| cell `conceal` | not reported | SGR 8 is tracked by the emulator (E11) and ignoring it hides a real attribute | **Add**: boolean, written only when `true`. The text stays in `ch`. |
| cell `hyperlink` | string or null | `null` on every cell | Keep the name `hyperlink`, written only when set. URI only (E9). `hyperlink_id` is reserved. |
| `wrapped_rows` | absent | A soft wrap is invisible (E10) | **New**, always present: sorted array of row indices whose last cell wraps into the next row. Empty array when none. |
| `cursor.row`, `cursor.col` | i32, usize | `row` is signed for no reason | Non-negative integers inside the screen. `col` may point at a continuation cell (E13); this is reported as is. |
| `cursor.shape` | `"Block"`, `"Underline"`, `"Bar"`, `"HollowBlock"`, `"Hidden"` | Mixed case; `HollowBlock` is for an unfocused window and `Hidden` duplicates `cursor.hidden` | `"block"`, `"underline"`, `"bar"`. Blinking variants collapse to the steady shape (blink is not tracked). `hollow_block` is reserved. |
| `cursor.hidden` | boolean | none | Keep: true when DECTCEM (?25) is off. |
| `modes` | strings in a fixed, arbitrary order | The order is an accident; vocabulary is mixed (`?1000`, `DECKPAM`) | A set, written sorted: private modes by number, then names alphabetically. Vocabulary in section 3.6. |
| `alt_screen` | boolean | Cannot express both buffers | Keep. It means "the active screen is the alternate one". The inactive buffer is reserved as `inactive` (section 10). |
| `title` | string or null | ConPTY sets one where Linux does not (E4) | Keep. From OSC 0 and 2 only; `null` when never set or reset. Platform difference is handled by the golden directive `ignore title`. |
| `scrollback` | strings, oldest first, text only | Large (up to 10,000 rows by the emulator's default); not part of the screen | Keep as informational, bounded by the emulator's history. Not compared by goldens. Cell-level history is reserved as `scrollback_cells`. |
| `producer` | absent | Debugging needs to know the build; but it must not become part of the contract | **New**: `{"vtee": "...", "emulator": "..."}`. Informational only: no reader may depend on it and `golden check` never fails on it. |

### 3.3 Sparse cells

A cell is an object. **A field whose value equals its default may be omitted by the writer, and must be read as its
default when absent.** The defaults are: `ch` is `" "`, colours are `"default"`, flags are `false`, `underline` is none,
`hyperlink` is none. A blank default cell is `{}`.

The writer in `vtee` always omits defaults. Readers must accept both forms, which is the rule they already need for
older snapshots without a field.

Why now: E1 and E2. Dense cells cost 215 bytes each, and every attribute we add later would grow every cell of every
snapshot by another 15 to 25 bytes; the two fields added last week cost TinyFugue 100 KB per screen. With omitted
defaults a new attribute costs nothing for the cells that do not use it. Measured on real screens the cell array drops
from about 309 KB to 9 to 14 KB (E2). The alternative considered, a client-selectable "compact" flag, would freeze two
shapes; a single canonical sparse shape is simpler.

### 3.4 What `ch` may contain

- `ch` is the text of one cell: a base scalar, then any zero-width scalars the emulator appended to it (combining
  marks, ZWJ, variation selectors). No Unicode normalisation is applied (E7).
- A wide character occupies two cells: the first holds the character, the second is the **continuation cell**, whose
  `ch` is `""`. A cell with `ch == ""` is a continuation cell and nothing else is ever empty (E5, E8). The cell before
  a continuation cell is wide. The continuation cell keeps its own attributes.
- Control characters (C0, DEL, C1) and BEL never appear in a cell (E8). Private-use scalars and format characters do,
  verbatim.
- When a wide character does not fit in the last column, the emulator leaves that cell blank and moves the character to
  the next row. The blank cell is reported as an ordinary blank, and its row is in `wrapped_rows`.
- Cells follow the emulator's width model: one scalar, one `unicode-width` value. A ZWJ sequence is **not** clustered
  into one glyph (E6): every emoji in it takes its own cells. A terminal that renders the sequence as one glyph
  occupies different columns; a golden records what the emulator did, not what a screen would show.
- The width tables belong to the emulator's version. Changing them is an output change: `semantics_version` bumps
  (section 4).

### 3.5 Colours and attributes

- Colours are `"default"`, `"idx:N"` (N from 0 to 255) or `"rgb:#rrggbb"` (lower-case hex). Palette indices 0 to 15 are
  the ANSI palette entries; SGR 30 to 37 and 90 to 97 map to `idx:0` to `idx:15`, the same as the 256-colour form of the
  same entry, because they are the same palette entry. The named foreground, background and cursor colours map to
  `"default"`. The dim variants map to their index and carry the `dim` flag separately. Bold is never converted to
  bright.
- The attribute vocabulary of schema 2: `bold`, `dim`, `italic`, `reverse`, `strike`, `conceal`, `underline`
  (style), `underline_color`, `hyperlink`, plus `fg` and `bg`. Reserved, never emitted until the emulator supports
  them: `blink`, `overline`, `hyperlink_id`.
- New attributes are new optional fields. Readers ignore fields they do not know (section 4.3).

### 3.6 Modes

`modes` reports the input-affecting modes the program has turned on, as a set. The vocabulary of schema 2:

| Token | Meaning |
| --- | --- |
| `?1` | application cursor keys (DECCKM) |
| `DECKPAM` | application keypad |
| `?1000` | mouse click reporting |
| `?1002` | mouse drag reporting |
| `?1003` | mouse motion reporting |
| `?1004` | focus in and out reporting |
| `?1005` | UTF-8 mouse encoding |
| `?1006` | SGR mouse encoding |
| `?1007` | alternate scroll |
| `?2004` | bracketed paste |

`?1002`, `?1003`, `?1004`, `?1005` and `?1007` were not reported before (E12). They are added now because they decide
how mouse input has to be encoded, and a golden of a mouse-aware program is incomplete without them. Output-affecting
modes (origin, autowrap, insert, newline) and the kitty keyboard flags are not reported (section 10).

On Windows ConPTY consumes `?1`, `?1000` and `?1006` and does not forward them, so they never appear; a golden that
must pass on both uses `ignore modes ?1 ?1000 ?1006` (section 6.2).

## 4. The versioning model

Two numbers, with different jobs.

| Number | Says | Changes when |
| --- | --- | --- |
| `schema_version` | **Layout**: names, types and shapes of the JSON, and the grammar of the golden file | A field or token is removed, renamed, retyped, or its default changes |
| `semantics_version` | **Meaning**: what an existing cell, mode or position denotes for given program output | An emulation fix or an emulator upgrade changes what existing output produces |

Schema 2 and semantics 1 are the values at the 0.1.0 freeze. Schema 1 is the provisional shape that exists on `main`
today; it was never released, and it is readable only by `golden migrate` (section 5.3). The number is bumped to 2 so
that anything recorded against the provisional shape, including TinyFugue's recordings, is identifiable.

### 4.1 What bumps what

| Change | `schema_version` | `semantics_version` | Notes |
| --- | --- | --- | --- |
| Add an optional cell or top-level field | no | no | Readers ignore unknown fields |
| Add a new value to an existing field (a new underline style, a new mode token) | no | no | Goldens recorded earlier simply do not contain it |
| Remove or rename a field | **yes** | no | |
| Change a field's type or default | **yes** | no | |
| Change what an existing value means (for example `cursor.row` becomes 1-based) | **yes** | no | |
| An emulation fix that changes cells for existing input (TAB becoming a space, the ED 1 fix) | no | **yes** | The layout is unchanged; stored goldens may now differ |
| Upgrading `alacritty_terminal` or its width tables | no | **yes**, if the conformance set changes | The conformance set decides (section 5.4) |
| A bug in reporting that is fixed (named colours reported as default, fixed earlier) | no | **yes** | |
| A platform difference (ConPTY hides modes, sets a title) | no | no | Handled by `ignore` directives, not by versions |
| A new golden directive that changes how a golden is compared | the golden **format** number | no | Unknown header keys are an error (section 6.2) |
| A change to the golden grammar | the golden **format** number | no | |

### 4.2 The golden file's own number

The first line of a golden is `vtee-golden N`: the version of the golden **grammar**. It is 1 for the grammar in this
document. The draft in TinyFugue's `tests/screens-draft` is `vtee-golden 0`. The golden also records the
`schema` and `semantics` it was recorded under.

### 4.3 Reader rules

1. A reader ignores any JSON field it does not know, and any attribute or mode token it does not know.
2. A snapshot or golden whose `schema_version` is **higher** than the reader supports is refused, never partly read.
3. A golden whose `schema` is **lower** than the reader's current one is read through the migration path while the
   deprecation window lasts (section 5.3), and refused after it with a pointer to `golden migrate`.
4. A golden whose `semantics` differs from the snapshot's is refused by `golden check` by default (exit 2), with an
   explicit way to look at the difference (section 6.9).
5. A schema 1 snapshot has no `semantics_version`. It is read as "unknown": a comparison against it is refused like a
   semantics mismatch, so it can be inspected with `--accept-semantics` but never silently trusted.
6. `producer` and `byte_offset` never take part in a comparison.

### 4.4 The semantics changelog

Every `semantics_version` bump is an entry in `docs/guide/snapshot-semantics.md`: the number, the release, the input
that changes, the old and the new cells, and which conformance cases changed. Semantics 1 is the state of 0.1.0
and includes the fixes already made (TAB, named colours, wide continuation cells, underline styles).

## 5. The stability promise

### 5.1 Frozen at 0.1.0

- The schema 2 field names, types, defaults and meanings of section 3.
- The golden grammar of section 6, including the escape set and the exit codes of `vtee golden check`.
- The rule of section 4.1.
- The semantics of the conformance set (section 5.4) at semantics 1.

### 5.2 Allowed without a version bump

Everything the first two rows of section 4.1 permit: new optional fields, new attribute and mode tokens, new `producer`
keys, new golden *sections* only behind a new grammar number. Internal changes are unrestricted so long as the
conformance set does not change.

### 5.3 Deprecation and tools

- When a new schema number ships, the previous one stays readable for at least **two minor releases** while the
  version is 0.x, and for the whole major series from 1.0. After that, readers refuse it and say which release last
  supported it.
- `vtee golden migrate FILE...` converts mechanically between layouts: schema 1 JSON, `vtee-golden 0` files from the
  draft, and every later bump. It never changes meaning; a semantics difference is not migratable.
- For a semantics difference, `vtee golden check --accept-semantics` compares anyway, prints the differences and
  exits 1 if there are any. A reviewer then re-records with `golden update`. The changelog entry says in advance which
  inputs are affected.

### 5.4 The frozen conformance set

The 80 cases in `tests/fixtures/differential/cases.json` become reference data with two files per case:

- the input (already exists),
- `tests/fixtures/conformance/expected/<case>.json`: vtee's own snapshot of that input, in canonical compact form
  without `byte_offset` and `producer`. (Today the oracle files hold `avt` and `vt100`'s output, not vtee's, so a
  change that moves vtee and both oracles together would not be caught.)

The expected files are pretty-printed JSON in the key order of section 3.1 and are compared as parsed values, so key
order and whitespace never matter. The cases feed bytes straight to the emulator, so they are platform independent;
the resize cases need a live pty and run on Unix only, as they do today.

A new test compares vtee's live output to every expected file exactly. **The conformance set is the contract:**
behaviour that no case exercises is not promised. When a consumer finds output that matters and is not covered, we add
a case (which is not a bump) and from then on the output is frozen. The rules:

1. Changing an expected file requires a `semantics_version` bump in the same change, an entry in
   `docs/guide/snapshot-semantics.md`, and a commit whose scope is `snapshot` (`fix(snapshot): ...`, or
   `feat(snapshot)!: ...`).
2. Adding a case is not a bump.
3. Fixing a recorded deviation (ED 1 on the second row, origin mode reset, mode ?47) is a bump by definition.
4. Enforcement: a small CI job fails if a pull request changes anything under `tests/fixtures/conformance/expected/`
   without also changing `SEMANTICS_VERSION` in `src/snapshot.rs` and the semantics changelog; commitlint gets a
   `snapshot` scope. `release-plz` groups `snapshot`-scope commits under "Snapshot output changes" in the changelog.

The differential oracles and `adjudications.json` stay as they are: they answer "is vtee right?", while the
conformance files answer "did vtee change?".

## 6. The golden file format, version 1

A golden is a text file describing one screen, with the volatile parts declared. It is **normative** for the
`vtee-golden 1` grammar.

### 6.1 File syntax

- UTF-8 without a byte order mark; LF line endings; the file ends with a newline.
- Line oriented. A line `[name]` starts a section. Lines before the first section are the **header**.
- A line whose first character is `#` is a comment, allowed in the header and in `[masks]` and `[assert]`.
- Blank lines are ignored everywhere. They cannot be mistaken for rows, because a `[text]` row always contains `|`.
- Sections appear in the order `[text]`, `[attrs]`, `[masks]`, `[assert]`. `[masks]` and `[assert]` are optional.
  A reader accepts any order; a writer always uses this one. An unknown section name is an error.

### 6.2 Header

One `key value` pair per line. The writer's order:

```
vtee-golden 1                         required, first line
schema 2                                required
semantics 1                             required
size 80x24                              required: columns x rows
cursor row=22 col=19 shape=block hidden=false      required
modes ?1 DECKPAM                        required; zero or more tokens, space separated, sorted
alt_screen true                         required: true or false
title null                              required: null or a JSON string literal, "my title"
wrapped 3 4                             optional: row indices that soft-wrap (omitted when none)
ignore modes ?1 ?1000 ?1006             optional, repeatable; see below
ignore title
x-producer vtee 0.1.0 alacritty_terminal 0.26.0     advisory
```

- **`ignore`** names a part of the header the check skips: `ignore title`, `ignore cursor`, `ignore alt_screen`,
  `ignore wrapped`, or `ignore modes TOKEN...` (the listed tokens are removed from both sides before the comparison).
  This is how a golden opts out of a platform difference (section 3.6, E4).
- A key beginning with `x-` is informational and ignored. **Any other unknown key is an error**, so a golden written by
  a newer tool that adds a directive changing comparison can never be silently misread.
- `byte_offset` and `scrollback` are never stored.

### 6.3 `[text]`

Exactly `rows` lines, one per row: the row index (decimal, right-aligned to the width of the largest index), then `|`,
then the row's **cell tokens**. The label must equal the row's index.

The row is a sequence of tokens, one token per cell, except that appended scalars share the cell they follow:

| Token | Meaning |
| --- | --- |
| a literal scalar | one cell whose `ch` is that scalar. Allowed for any scalar that is neither a backslash nor in the escape set below |
| `\\` | one cell holding a backslash |
| `\u{H}` | one cell whose first scalar is U+H (1 to 6 hex digits, lower case). Required for every scalar in the escape set |
| `\.` | a **continuation cell**, `ch` is `""` |
| `\+{H}` | **appends** scalar U+H to the previous cell (a combining mark, ZWJ, variation selector). It is an error after a continuation cell or at the start of a row |

Every scalar of a cell after the first is written as `\+{H}`, even a printable one. This is what lets a reader find cell
boundaries without any Unicode table (the width of a scalar, whether it is a mark): the writer knows the cells and the
file says so.

**Trailing blank cells are omitted**, whatever their attributes (those live in `[attrs]`), so a row's tokens may cover
fewer cells than `cols`; the missing cells are `" "`. A row with nothing to say is `NN|`. A row therefore never ends in a
space, which is what keeps a golden safe from editors that strip trailing whitespace.

**The escape set** is fixed by this document and independent of any Unicode version. A scalar must be written as
`\u{H}` if it is in one of these ranges (all hexadecimal, inclusive): 0000 to 001F; 007F to 00A0; 00AD; 0300 to 036F;
034F; 0600 to 0605; 061C; 06DD; 070F; 08E2; 115F to 1160; 1680; 17B4 to 17B5; 180B to 180F; 1AB0 to 1AFF;
1DC0 to 1DFF; 2000 to 200F; 2028 to 202F; 205F to 206F; 20D0 to 20FF; 2800; 3000; 3164; E000 to F8FF; FE00 to FE0F;
FE20 to FE2F; FEFF; FFA0; FFF0 to FFFF; 1BCA0 to 1BCA3; 1D173 to 1D17A; E0000 to E0FFF; F0000 to 10FFFF. These are
controls, spaces other than U+0020, invisible or combining characters (including the blank braille pattern U+2800 that
graph-drawing programs use), and private-use scalars: the ones a reviewer cannot see or an editor may alter. Extending
the set is a change of the grammar number.

### 6.4 `[attrs]`

Lists the cells whose attributes are not all default, as **runs**: `ROW: SPAN TOKENS`.

- `ROW` is the row index (right-aligned to the same width as in `[text]`), then `:`.
- `SPAN` is a column `N` or an inclusive range `N-M`, counting cells (a wide character is two columns).
- `TOKENS` is the attribute set of every cell in the span, in this order, separated by spaces: `fg=C`, `bg=C`,
  `underline=STYLE`, `ulcolor=C`, `link="URI"`, then the flags `bold`, `dim`, `italic`, `reverse`, `strike`,
  `conceal`. `C` is `default`, `idx:N` or `rgb:#rrggbb`. A token equal to its default is not written.
- A run is a maximal sequence of adjacent cells with identical non-empty attribute sets. Runs are in row, then column
  order, do not overlap, and never describe a default cell. A blank cell with an attribute (the 80 reverse cells of
  `top`'s header) is listed.

### 6.5 `[masks]`

A mask relaxes the comparison for a region. Everything outside every mask is compared in full: text and attributes.

```
mask REGION [text=ignore | text=/RE/] [attrs=ignore]
REGION  =  (row=N | rows=N-M) [cols=N | cols=A-B]
        |  find=/RE/ [rows=N-M] [cols=A-B]
```

- A mask must say at least one of `text=` or `attrs=ignore`.
- `find=/RE/` is matched against the **row text** of each row in range of the **actual** screen: the cells' `ch`
  concatenated, continuation cells dropped, trailing blanks removed (so `$` anchors to the last visible character). Each
  non-empty match is a region, from the first matched cell to the last cell of the last matched character, including
  that character's continuation cell.
- `text=ignore` skips the text comparison in the region. `text=/RE/` skips the comparison with the stored text but
  requires the actual text of each region to match the whole of `RE` (anchored).
- `attrs=ignore` skips the attribute comparison.
- **A stale mask is a failure.** A `find=` that matches nothing, a `text=/RE/` that does not match, and a region outside
  the screen are all reported (section 6.8). A mask cannot silently stop meaning anything.
- Where regions overlap, a part is skipped if any covering mask skips it.

**Regex dialect.** The common subset of RE2: literals, `.`, character classes `[...]`, groups, alternation,
`* + ? {m,n}` and the anchors `^` `$`. `\d`, `\w` and `\s` mean their **ASCII** sets only. No backreferences or
lookaround. A `/` inside a pattern is written `\/`. Implementations in other languages must restrict themselves to this
subset.

### 6.6 `[assert]`

Sparse assertions on the **actual** screen, independent of `[text]` and `[attrs]`:

```
assert REGION [text="EXACT" | text=/RE/] [ATTR ...]
ATTR  =  fg=C | bg=C | underline=STYLE | ulcolor=C | link="URI" | flag | !flag | !link
```

- `REGION` is as in a mask. A missing `cols=` means the whole row.
- `text="..."` is a JSON string literal and must equal the region's text exactly; `text=/RE/` must match all of it.
- Each `ATTR` must hold for **every cell** of the region. Unlisted attributes are unconstrained. `!bold` means the flag
  must be off.
- An assertion whose region finds nothing fails.

### 6.7 Comparison

`golden check` performs, in this order:

1. **Parse** the golden. Syntax errors stop here (exit 65).
2. **Versions.** Compare `schema` (refuse on a difference outside the deprecation window) and `semantics` (refuse unless
   `--accept-semantics`). Refusals exit 2 and print `REFUSED`.
3. **Size.** If it differs, report it and skip cell comparison (the grids no longer line up).
4. **Header:** cursor, `alt_screen`, `modes` (as sets, after `ignore modes`), `title`, `wrapped`, each unless `ignore`d.
5. **Cells.** Every cell outside masks: text (`ch`) and attribute set.
6. **Masks:** stale-mask checks and `text=/RE/` checks.
7. **Assertions.**

`byte_offset`, `scrollback`, `producer` and `x-` keys never take part.

### 6.8 The report

Plain text on standard output. The first line is `PASS NAME` or `FAIL NAME: N mismatches` or
`REFUSED NAME: REASON`. Each mismatch is a block whose first word names the kind: `size`, `text`, `attrs`, `cursor`,
`modes`, `alt_screen`, `title`, `wrapped`, `mask`, `assert`. Contiguous cells with the same difference are one report.
A `text` block shows both rows and a caret line under the differing columns. At most 20 blocks are shown by default
(`--max-mismatches N`); the first line still gives the true total and a final line says `showing 20 of 413`.

```
FAIL tf-session.golden: 1 mismatch
  text      row 20 col 79   expected '8'   actual '9'
            20| _________(unnamed1)_____...____03:18   (expected)
            20| _________(unnamed1)_____...____03:19   (actual)
                                                  ^
```

`--format json` writes one JSON object instead:

```
{"golden":"tf-session.golden","result":"fail","schema":2,"semantics":1,"total":1,"shown":1,
 "mismatches":[{"kind":"text","row":20,"col":79,"expected":"8","actual":"9"}]}
```

`result` is `pass`, `fail` or `refused`; a refusal adds `"reason"`. Every mismatch object has `kind`, and `row`/`col`
(or `col_start`/`col_end`) where a position applies.

### 6.9 Exit codes of `vtee golden check`

| Code | Meaning |
| --- | --- |
| 0 | Pass |
| 1 | The screen differs from the golden |
| 2 | Refused: schema or semantics versions do not allow a comparison. Deliberately not a sysexits code, so a script can tell "differs" and "cannot compare" apart from errors |
| 64 | Usage error |
| 65 | The golden is malformed (data error) |
| 66 | A file named on the command line does not exist |
| 69 | The session cannot be reached |
| 74 | Another I/O error |

### 6.10 Recording, updating, migrating

- `vtee golden write -o NAME.golden (--connect SOCKET | --snapshot FILE | -)` records the header, `[text]` and
  `[attrs]` from a snapshot. It writes no `[masks]` or `[assert]`: those are written by hand.
- `vtee golden update NAME.golden (--connect SOCKET | --snapshot FILE | -)` regenerates header, `[text]` and
  `[attrs]` and **keeps** the `ignore` lines, comments, `[masks]` and `[assert]` verbatim.
- `vtee golden migrate FILE...` rewrites a golden recorded under an older grammar or schema (section 5.3).
- Both writers are deterministic: `write(parse(g)) == g` for a canonical file, so a re-record that changes nothing
  produces an empty diff.

### 6.11 Determinism rules for writers

Header lines in the order of section 6.2. `[attrs]` runs in row, then column order. Every row of `[text]` present, with
no trailing blank cells. No line anywhere ends in whitespace. One newline at the end. Nothing in the file depends on
the Unicode version of the tool that wrote it.

### 6.12 Portability across platforms

A golden is portable between Linux and Windows to the extent the program's output is. The known differences are `modes`
(ConPTY hides three) and `title`; both are opted out with `ignore`. `byte_offset`, which differs wildly (E3), is not
stored. Anything else that differs is a real difference to investigate and report.

## 7. Worked examples

The three examples below were produced by running a real snapshot through a converter that implements this
document, then parsing the result back and comparing it to the snapshot (E14).

### 7.1 A tf screen

Recorded by TinyFugue from tf running under `vtee` at 80x24 (their `tf-session` screen). Row 20 is tf's status row,
with a clock at its right end; the mask lets the clock vary and still compares the attributes of those cells.

```text
vtee-golden 1
schema 2
semantics 1
size 80x24
cursor row=22 col=19 shape=block hidden=false
modes ?1 DECKPAM
alt_screen true
title null

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

[masks]
# The clock at the right end of tf's status row: any HH:MM. Its attributes are still compared.
mask row=20 cols=75-79 text=/\d\d:\d\d/

[assert]
assert row=16 cols=0-13 text="A Quiet Meadow" fg=idx:2 bold
assert find=/goblin/ fg=idx:1 bold
assert row=17 cols=0-41 fg=idx:6 !bold
```

### 7.2 A `top` screen with masks

`top` refreshes itself, so about a tenth of its cells are volatile. The masks name them by the columns of the header
row and let figures vary within their own pattern; the assertions keep what matters regardless of the figures. The
bold attribute of row 7 and 8 is masked because a running task is bold and a sleeping one is not.

```text
vtee-golden 1
schema 2
semantics 1
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
mask rows=7-8 cols=47-47 text=/[SRDZTt]/
mask rows=7-8 cols=49-53 text=/ *\d+\.\d/
mask rows=7-8 cols=55-59 text=/ *\d+\.\d/
mask rows=7-8 cols=61-69 text=/[ \d:.]+/
# A running task is drawn bold, a sleeping one is not: the bold row depends on the data
mask rows=7-8 attrs=ignore

[assert]
assert row=6 text=/ +PID +USER +PR +NI +VIRT +RES +SHR S +%CPU +%MEM +TIME\+ +COMMAND */ reverse
assert find=/%CPU/ reverse !bold
```

### 7.3 A screen with wide, combining and private-use characters

Row 0 is a path with a backslash, a wide pair and an ASCII letter (`C:\dir  你好X`). Row 1 holds, in order: `e` with a
combining acute accent (one cell), a space, the family emoji written man, ZWJ, woman, ZWJ, girl (three wide characters,
the first two carrying the ZWJ), a space, a private-use character, a space, a red bold `R`, a space, a curly underlined
`c` with an RGB underline colour, a space, and a wide reverse-video character.

```text
vtee-golden 1
schema 2
semantics 1
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
```

## 8. Open questions, closed

Decided here, with the reason:

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
| `modes` order? | A set, written sorted | The old order was accidental |
| Platform differences in the version number? | No: `ignore` directives | Versions are for meaning, not for the OS |
| Where do `assert` lines live? | Their own `[assert]` section | They compare the actual screen, not the stored one; mixing them with masks confused two ideas |
| Header `title` as `-` or a literal? | `null` or a JSON string | `-` is a legal title |
| Comments in `[text]`? | Not allowed | Rows are regenerated |
| Regex dialect? | RE2 common subset, ASCII classes | Linear time, available almost everywhere, no surprises |
| `byte_offset` in goldens? | Never | E3 |
| Scrollback in goldens? | No | Not part of the screen; `scrollback_cells` reserved |
| Both screen buffers? | `alt_screen` stays; `inactive` reserved | The library cannot read the inactive buffer without destroying it |
| Versions of the golden grammar | `vtee-golden N` | Separates grammar from snapshot layout |

### OWNER DECISIONS

| # | Decision | Recommendation | If you choose otherwise |
| --- | --- | --- | --- |
| OD1 | **Schema number.** Call the frozen shape `schema_version` 2, leaving 1 as the provisional shape on `main` | 2 | Keep 1: simpler number, but every artefact already recorded against the provisional shape (TinyFugue's, ours) becomes indistinguishable from the frozen one |
| OD2 | **Sparse cells.** Omit defaults, as in 3.3, in the same change as the redesign | Yes, now | Keep dense: simpler to read in JSON, but every added attribute grows every cell, the snapshot stays at 400 KB, and moving to sparse later is a breaking change |
| OD3 | **Deprecation window** for an old schema number: two minor releases before 1.0, the whole major series after | Two minors | A longer window costs maintenance of old readers; none at all (refuse immediately) makes every bump break every stored golden |
| OD4 | **Semantics mismatch in `golden check`.** Refuse (exit 2) unless `--accept-semantics` | Refuse | Warn only: friendlier but a golden can then pass or fail for reasons that are not about the program |
| OD5 | **Regex dialect** pinned to the RE2 common subset with ASCII classes | Yes | A richer dialect (Python `re`) makes goldens depend on one language |
| OD6 | **Known emulation deviations** (ED 1 on row 2, origin-mode reset, mode ?47): freeze them in semantics 1 and bump when upstream fixes them, or fix them first (vendor `alacritty_terminal`) so semantics 1 is right from the start | Freeze and document; they are listed in the semantics changelog as known deviations | Fixing first costs a vendored fork of the emulator and a maintenance burden; goldens recorded before the fix would need `golden update` only if they exercised those sequences |
| OD7 | **What 0.1.0 contains.** The snapshot and golden formats frozen and `vtee golden write|check|update|migrate` shipped, or only the frozen formats with the commands to follow in 0.2 | Ship the formats and `golden check` and `write`; `update` and `migrate` may follow in 0.2 | Formats only: TinyFugue can keep its prototype checker meanwhile, but the formats would be frozen without the reference implementation having validated them |

## 9. Required changes before 0.1.0

1. **Sparse cells and the new cell shape** (OD1, OD2; sections 3.2, 3.3): colours as strings, `underline` as a style string
   (removing `underline_style` and the old boolean), `conceal` added, `blink` and null `hyperlink` removed, defaults
   omitted. `src/snapshot.rs`, `redraw_sequence`, serde, the differential harness's reader, `docs/guide/protocol.md`,
   `examples/`. **L**
2. **Versions:** `SCHEMA_VERSION = 2`, a new `SEMANTICS_VERSION = 1`, both in the snapshot; `producer` object. **S**
3. **`wrapped_rows`** from the emulator's wrap flag, and an opt-in `wait_for` mode that matches against logical (joined)
   lines using it. **M**
4. **Cursor:** lower-case shapes, drop `Hidden` and `HollowBlock`, non-negative row. **S**
5. **Modes:** a sorted set and the five new tokens. **S**
6. **Conformance set** (section 5.4): `tests/fixtures/conformance/expected/`, the exact-comparison test, the CI guard
   and the `snapshot` commit scope in commitlint and the release-plz changelog groups. **M**
7. **`vtee golden`:** a library module (parse, write, compare, report) and the subcommands `write` and `check`
   (OD7), with the JSON report. **L**
8. **`golden update` and `golden migrate`**, including reading schema 1 snapshots and `vtee-golden 0` files. **M**
9. **Documentation:** the golden spec promoted to `docs/guide/golden-format.md`, `docs/guide/snapshot-semantics.md`
   created with the semantics 1 entry and the known deviations, `protocol.md` and `versioning.md` updated, a testing
   page section. **M**
10. **Platform CI:** a few repository goldens checked on both Linux and Windows jobs, using `ignore` for the known
    differences. **S**
11. **`lines`:** correct the documentation claim that it is padded to `cols` characters. **S**
12. **Tell TinyFugue** the list of changes from their draft (below) before they record real goldens. **S**

### Changes from TinyFugue's draft

- Assertions moved to their own `[assert]` section.
- `[text]` is cell-per-token with escapes (`\.`, `\u{H}`, `\+{H}`, `\\`); the draft joined the cells' `ch` values and had no
  rule for wide characters.
- Header: `semantics`, `wrapped`, `ignore`, `x-producer` added; `title` is `null` or a JSON string, and the cursor line is
  `hidden=B`.
- `modes-ignore` and `title-ignore` became `ignore modes ...` and `ignore title`.
- Regex classes are ASCII-only and `$` anchors to the last visible character.
- The grammar number is `vtee-golden 1`; the draft's `vtee-golden 0` is read by `golden migrate`.

## 10. Deferred, with the additive path reserved

| Item | Reserved name or path | Why deferred |
| --- | --- | --- |
| The inactive screen buffer | top-level `inactive` object with the same shape as the snapshot | The emulator cannot read it without destroying it |
| Cell-level scrollback | `scrollback_cells` | No consumer yet |
| Hyperlink ids | cell `hyperlink_id` | Rare |
| Blink, overline | cell `blink`, `overline` | Emulator support |
| Output-affecting modes (origin, autowrap, insert, newline) and the kitty keyboard flags | more tokens in `modes` | No consumer; additive |
| A more compact row encoding than sparse cells (run-length rows) | `row_runs` | Sparse cells already give 20 to 30 times; add only if measured to matter |
| `snapshot` command parameters (`scrollback`, `lines`, a cell region) | optional request fields | Each is additive |
| Grapheme clustering | a different width model, behind `semantics_version` | Changes meaning |
| Structural queries for tables (columns by header alignment, numeric ranges) | new assertion forms behind a new grammar number | TinyFugue's own recordings suggest the form; build after masks |
| Images (sixel, kitty graphics), selection, timestamps | none | Not in the emulator |
