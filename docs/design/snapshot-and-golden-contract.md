# Snapshot and golden contract (design)

Status: **proposed, revised after two independent reviews, for owner decision before 0.1.0.** First written and
revised 2026-10-10. When accepted, the normative parts (sections 3 to 6) move to the user guide and this page stays
as the record of why. Nothing here is implemented yet except where a section says "today". The reviews and what was
done with each finding are in appendix C.

The snapshot JSON and the golden file format are a long-lived contract. A stored golden outlives any one `vtee`
version (the TinyFugue project will use goldens for years, across Windows and Linux, to check a faithful port of tf), so
the formats are designed once, carefully, before 0.1.0 is tagged. Internal code stays free to change behind the
contract.

## 1. Goals, non-goals and who depends on this

**Goals**

1. A stored golden either still means what it meant when it was recorded, or `vtee` says so clearly. It never
   silently passes or fails for a reason that is not about the program being tested.
2. Adding something to `vtee` (a new mode token, a new cell attribute) never makes a stored golden fail. This is a
   goal of its own because the first draft of this document got it wrong (section 4.1; findings TF1 and FA-A1 in appendix C).
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
| Stored goldens (TinyFugue and others) | The golden grammar, `schema_version`, `semantics_version`, the declared vocabulary, and the meaning of every cell |
| AI agents reading snapshots | Field names, types and defaults; `lines` as a convenient view |
| Procedural tests in any language | The same, plus the exit codes of `vtee golden check` and its JSON report |
| `vtee`'s own conformance set | The meaning of cells for the 80 escape-sequence cases and the width sample (section 5.4) |

## 2. What was measured

These are the facts the decisions below rest on. Each was run on `main` (built natively on Windows 11 build 26300, and in
the `vtee-test` Docker image for Linux) unless it cites TinyFugue's recorded screens
(`tests/screens-draft/recorded/`, read-only) or an independent review. E1 to E14 are from the first draft and were
reproduced by the reviewers; E15 to E26 were added in the revision.

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
| E18 | **Mouse modes are exclusive in the emulator.** `?1000h` followed by `?1002h` leaves no `?1000`; `?1003h` then `?1000l` leaves none; `?1006h` then `?1005h` leaves only the later encoding. The reported set therefore reflects one mouse mode and one mouse encoding at most. | Linux, `modes` after each sequence (today's vocabulary only reports `?1000`, `?1006`, so absence of `?1000` is the evidence) |
| E19 | **Width classes** of 1,062 scalars in about 30 groups (blocks and hand-picked ranges), asked of the real emulator: 303 are zero-width (appended to the previous cell), 430 are one cell, 329 are two. Notable: the soft hyphen U+00AD is zero-width; VS16 U+FE0F is zero-width and does not widen its base (`☺️` is one cell); the ZWJ is zero-width; each regional indicator is one cell; each skin-tone modifier is two cells; U+115F is two cells while U+1160 to U+11C2 are zero-width; the blank braille U+2800 and private-use U+E000 are one cell; U+3164 and U+FFA0 are zero-width. | `width.py` (scratch), one row `A<scalar>œ` per scalar, class = column of `œ` minus 1; result in appendix B |
| E20 | **A wide character at the last column** leaves a blank placeholder cell (flag `LEADING_WIDE_CHAR_SPACER`) and then the row is marked `WRAPLINE`; the character starts the next row. So a row can be soft-wrapped without its last real character reaching the last column. | `term/mod.rs` `input()` lines 1087 to 1101 and `wrapline()` line 968; probe: 19 `a`, then `你`, then `Z` on 20 columns gives row 0 ending in a blank and row 1 starting `你`, `""`, `Z` |
| E21 | A combining mark with nothing before it attaches to the blank cell under the cursor: printing only U+0301 at column 0 gives cell 0 = `" \u0301"` (a space plus the mark) and the cursor stays at column 0. If a base character follows it, that character overwrites the cell and the mark is lost. | Probe, Linux |
| E22 | `lines[r]` can have **more** scalars than `cols` (three `é` on a 3-column screen give 6 scalars), as well as fewer (a wide character adds a cell but one scalar). | Probe, Linux |
| E23 | **NDJSON line splitting trap.** `serde_json` writes U+2028 and U+2029 (and U+0085) unescaped. Python's `str.splitlines()` splits on them, so a snapshot line containing one of them breaks a reader that is not splitting on LF alone. Found while building the width sample. | `JSONDecodeError: Unterminated string` from `width.py` |
| E24 | **Regex engines differ.** Rust `regex` 1.x: `\d` matches U+0663, `\w` matches `é`, `\s` matches NBSP; `(?-u).` does not compile ("pattern can match invalid UTF-8"); `\b`, `(?i)`, `\x41`, `\p{L}` compile; look-around and back-references do not; `$` does not match before a trailing newline. Python 3.13 `re`: `\d`, `\w`, `\s` are Unicode by default, ASCII with `re.ASCII`; `$` also matches before a trailing newline; `finditer('a*','baa')` yields `(0,0)`, `(1,3)`, `(3,3)`. | Two scratch programs run in Docker |
| E25 | **Usage errors exit 64.** `src/main.rs` lines 198 to 202 map every clap parse error to `exitcode::USAGE` (64); clap's own default of 2 is never used. A new `golden` subcommand goes through the same parser, so exit 2 is free for "refused". | Source |
| E26 | **Unicode data for the escape set.** Python 3.13.5's `unicodedata` is Unicode 15.1.0. The categories of section 6.3 produce 876 ranges covering 966,490 scalars (almost all of them unassigned). | `escset.py` (scratch), output in appendix B |

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

Today's shape is `schema_version` 1 on `main`. The right-hand column is the decision.

| Field | Today | Problem | Decision (schema 2) |
| --- | --- | --- | --- |
| `schema_version` | integer, first | none | Keep, first field. Value becomes 2 for the shape in this document. |
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
| `producer` | absent | Debugging needs to know the build; but it must not become part of the contract | **New**: `{"vtee": "...", "emulator": "..."}`. Informational only: no reader may depend on it and `golden check` never fails on it. |

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
- The attribute vocabulary of schema 2 is the list of names a golden can declare and compare: `bold`, `dim`,
  `italic`, `reverse`, `strike`, `conceal`, `underline`, `ulcolor` (the golden's name for `underline_color`), `link` (the
  golden's name for `hyperlink`). The colours `fg` and `bg` are part of the core and are always compared. Reserved,
  never emitted until the emulator supports them: `blink`, `overline`, `hyperlink_id`.
- New attributes are new optional fields. Readers ignore fields they do not know (section 4.3). A golden does not
  fail because of an attribute it did not declare (section 4.1, row 2).

### 3.6 Modes

`modes` reports the input-affecting modes the program has turned on, as a set. The vocabulary of schema 2:

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

The mouse report modes (`?1000`, `?1002`, `?1003`) are mutually exclusive in the emulator and so are the mouse
encodings (`?1005`, `?1006`); the set reflects the emulator's model, not the sequences sent (E18).

The first draft proposed five new tokens: `?1002`, `?1003`, `?1004`, `?1005` and `?1007`. Two are **withdrawn for 0.1.0**
and reserved: `?1004` (focus reporting) is injected by ConPTY at the start of every session whether or not the program
asked for it (E16), so it would be set on Windows and not on Linux for the same program, and `?1007` (alternate scroll)
has no consumer. They can be added later without a bump, because goldens declare the vocabulary they were written with
(section 4.1). Autowrap (`?7`) is reserved with the other output-affecting modes (origin, insert, newline) and the kitty
keyboard flags (section 10); nearly every program leaves it on, so it would be the commonest token in every golden.

The platform differences in this vocabulary (E15) are in section 6.14.

### 3.7 JSON encoding of snapshots

- UTF-8, one JSON value per line for the NDJSON protocol, the **record separator is U+000A alone**. Readers must split
  on LF and nothing else (E23).
- Writers escape U+0085, U+2028 and U+2029 as `\u0085`, `\u2028`, `\u2029` in addition to what JSON requires, so that a
  reader that splits lines on Unicode line boundaries is not broken. (This needs a small custom formatter: a
  required change.)
- Numbers are integers. No fractions, no exponents.

## 4. The versioning model

Two numbers describe a snapshot, and a third describes what a golden declares it can check.

| Number | Says | Changes when |
| --- | --- | --- |
| `schema_version` | **Layout**: names, types and shapes of the JSON, and what each field means | A field or token is removed, renamed, retyped, or its default changes |
| `semantics_version` | **Meaning**: what an existing cell, mode or position denotes for given program output | An emulation fix or an emulator upgrade changes what existing output produces |
| the golden's declared **vocabulary** | **What the golden checks**: the mode tokens and cell attributes it was written knowing about | A newer `vtee` can report more; the golden does not change, and does not fail (4.1) |

Schema 2 and semantics 1 are the values at the 0.1.0 freeze. Schema 1 is the provisional shape that exists on `main`
today; it was never released, and it is readable only by `golden migrate` (section 5.3). The number is bumped to 2 so
that anything recorded against the provisional shape, including TinyFugue's recordings, is identifiable.

### 4.1 What bumps what

The first draft said that adding a mode token or an attribute needs no bump because old goldens "simply do not contain
it". Both reviews showed that this is false: `golden check` compares `modes` as a set and cells as full attribute sets,
so an old golden **fails** against a snapshot that reports the addition (demonstrated: expected `?1 DECKPAM`, actual
`?1 ?1004 DECKPAM`, exit 1, even with `ignore modes ?1 ?1000 ?1006`). Two repairs were considered.

1. *Every newly reported thing is a semantics bump.* Simple to state, but additions are frequent (every attribute the
   emulator learns, every mode). Each one would make every stored golden suspect, including goldens of programs that
   never use the new thing, and people learn to wave the warning through.
2. *Goldens declare the vocabulary they were written with, and the checker compares only what is declared.* An
   addition is then genuinely additive: an old golden keeps passing and says so; only the author who wants the new
   thing checked re-records. The cost is two header lines and a rule in the checker, and the risk that a golden
   silently does not check something new, which the checker mitigates with a warning (`WARN vocab`, section 6.9) and
   `golden update --refresh-vocab`.

**Decision: option 2** (owner decision OD8 records the alternative). The table that follows uses it.

| Change | `schema_version` | `semantics_version` | Notes |
| --- | --- | --- | --- |
| Add an optional cell or top-level field | no | no | Readers ignore unknown fields |
| Add a new **name** to the golden vocabulary (a new mode token, a new attribute) | no | no | Goldens compare only what they declare. A golden that predates the name passes and gets a `WARN vocab` |
| Add a new **value** of an existing declared attribute (a new underline style) | no | **yes** | A declared attribute now reports something different for input that produced the old value; the same rule as the next row |
| The emulator starts reporting an **existing declared** token for input where it did not (it now sets `?1000` for a sequence it used to ignore) | no | **yes** | The same program produces a different declared result |
| Remove or rename a field, token or attribute | **yes** | no | A declared name that no longer exists cannot be compared |
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
vocabulary rule was added; a fix that changes only `wrapped_rows` is "an emulation fix that changes existing output",
so it bumps semantics).

### 4.2 The golden file's own numbers

The first line of a golden is `vtee-golden N`: the version of the golden **grammar**. It is 1 for the grammar in this
document. The draft in TinyFugue's `tests/screens-draft` is `vtee-golden 0`. The header also records the `schema`
and `semantics` the golden was recorded under, and the vocabulary (`vocab modes ...`, `vocab attrs ...`, section 6.2).

### 4.3 Reader rules

1. A JSON reader ignores any field it does not know. A golden reader ignores `x-` header keys and treats any other
   unknown header key as an error (section 6.2).
2. A snapshot or golden whose schema number is **higher** than the reader supports is refused, never partly read.
3. A golden whose `schema` is **lower** than the reader's current one is read through the migration path while the
   deprecation window lasts (section 5.3) and refused after it, with a pointer to `golden migrate`.
4. A golden whose `semantics` differs from the snapshot's is **compared anyway**: identical results pass with a
   visible `WARN semantics`, different results are refused (exit 2) with the differences shown. Details and the two
   flags that change this are in section 6.8.
5. A schema 1 snapshot has no `semantics_version`. A golden migrated from schema 1 or from grammar 0 carries
   `semantics unknown`, which behaves like a mismatch for every value.
6. A golden that declares a vocabulary token the reader does not know (written by a newer `vtee`) is refused with the
   name of the token: the reader cannot know what it meant.
7. `producer` and `byte_offset` never take part in a comparison.
8. A reader supports **every grammar number up to its own** (parsing an old grammar is cheap). A grammar number is
   bumped only for a change old readers cannot understand, never to retire old goldens.

### 4.4 The semantics changelog

Every `semantics_version` bump is an entry in `docs/guide/snapshot-semantics.md`: the number, the release, the input
that changes (the exact escape sequence or scalar), the old and the new cells, and which conformance cases changed, so
that a consumer can search their raw output for it. Semantics 1 is the state of 0.1.0 and includes the fixes already
made (TAB, named colours, wide continuation cells, underline styles) and the three known deviations of section 8 (OD6),
each listed precisely.

## 5. The stability promise

### 5.1 Frozen at 0.1.0

- The schema 2 field names, types, defaults and meanings of section 3.
- The golden grammar of section 6: the file syntax, the header keys, the escape set (section 6.3 and appendix B), the
  region and comparison rules, the regex dialect, the report shapes and the exit codes of `vtee golden check`.
- The rules of section 4.1, including the vocabulary rule.
- The semantics of the frozen data of section 5.4 at semantics 1: the 80 escape-sequence cases, the width sample and
  the test vectors.

### 5.2 Allowed without a version bump

Everything the first rows of section 4.1 permit: new optional JSON fields, new vocabulary names (modes, attributes),
new `producer` keys, new report fields (readers of the JSON report ignore unknown fields), new
`vtee` subcommands. New golden *sections* and new comparing directives need a new grammar number. Internal changes are
unrestricted so long as the frozen data does not change.

### 5.3 Deprecation and tools

- **Schema numbers.** When a new schema number ships, the previous one stays readable for at least **12 months** from
  that release, and until the next major version if that is later. After the window readers refuse it and say which
  release last supported it. The window is stated in time because two minor releases can be two weeks apart in 0.x.
- **`golden migrate` is kept for every released schema and grammar, with no end date.** A golden can always be moved
  forward or re-recorded; a window only limits how long the *checker* reads the old form directly.
- **Grammar numbers.** A reader supports all grammar numbers up to its own (section 4.3, rule 8).
- `vtee golden migrate FILE...` converts mechanically between layouts and **never invents facts**. Where a fact is
  not recoverable it is written as unknown (table below), so the migrated golden is honest about what it no longer
  verifies.

| Old form | Becomes | Note |
| --- | --- | --- |
| `{"Indexed":N}`, `{"Rgb":[r,g,b]}`, `"Default"` | `idx:N`, `rgb:#rrggbb`, `default` | exact |
| shape `Block`, `Underline`, `Bar` | `block`, `underline`, `bar` | exact |
| shape `Hidden` | `cursor.hidden` true and shape `block` | the old shape lost the real shape; `ignore cursor.shape` is added |
| shape `HollowBlock` | `block` | `ignore cursor.shape` is added |
| `underline: true` with `underline_style` | `underline=STYLE` | exact |
| `underline: true` without `underline_style` | `underline=single` | the old boolean never said which |
| `blink`, `hyperlink: null` | dropped | the former was always false, the latter a default |
| rows soft-wrapped | **not recoverable** | `ignore wrapped` is added |
| `semantics` | `semantics unknown` | not recoverable |
| `vocab` | the vocabulary the old schema could report | so nothing new is compared |

- For a **semantics** difference there is no migration, because the meaning changed, not the layout. `golden check`
  compares anyway (section 6.8) and the changelog says which inputs are affected; a reviewer re-records with
  `golden update`, or confirms the difference does not matter with `golden update --restamp`.

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
   files. They let a second implementation of `golden check` in another language prove it agrees.

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

## 6. The golden file format, version 1

A golden is a text file describing one screen, with the volatile parts declared. It is **normative** for the
`vtee-golden 1` grammar. Words in capitals in a grammar line are placeholders; `[x]` is optional. All row and column
indices are **0-based**, like `cursor.row` and `cursor.col` in the snapshot.

### 6.1 File syntax

- UTF-8 without a byte order mark; LF line endings; the file ends with a newline. These are what writers produce.
- **Readers accept CRLF** by removing one CR immediately before each LF, and accept a missing final newline. Reason:
  git's `autocrlf` and Windows editors change line endings silently and outside the author's control, and a carriage
  return cannot otherwise appear in a line (a CR in cell text is written `\u{d}`, in a JSON string `\r`). A CR anywhere
  else, a NUL byte, or invalid UTF-8 is an error. Repositories should still say `*.golden text eol=lf` in
  `.gitattributes`; `golden update` writes LF and so shows the change.
- **Readers reject a leading byte order mark** with a message that names it and says to remove it. Unlike a line
  ending, a BOM is added only by a deliberate save in an editor, `vtee` never writes one, and accepting it would let
  two files that differ in their first bytes mean the same thing.
- Line oriented. A line `[name]` starts a section. Lines before the first section are the **header**.
- A line whose first character is `#` is a comment. Comments are allowed in the header, in `[masks]` and in
  `[assert]`; they are not allowed in `[text]` or `[attrs]`, whose content is regenerated (an error there).
- Blank lines are ignored everywhere. They cannot be mistaken for rows, because a `[text]` row always contains `|`.
  Writers emit exactly one blank line before each section.
- Sections, in the writer's order: `[text]` (required), `[attrs]`, `[masks]`, `[assert]` (each optional). A writer omits
  `[attrs]` when no cell has a non-default attribute; a reader treats an absent or empty `[attrs]` the same. A reader
  accepts the sections in any order; a section appearing twice or an unknown section name is an error.
- Trailing whitespace on header, `[attrs]`, `[masks]` and `[assert]` lines is ignored by readers and never written.
  In `[text]` a literal space is a blank cell (6.3).

### 6.2 Header

One `key value` pair per line. The first line must be `vtee-golden N`; the other lines may come in any order
(duplicates are an error except for `ignore` and `x-` keys). The writer's order:

```
vtee-golden 1                      required, first line
schema 2                             required: the snapshot schema the golden was recorded under
semantics 1                          required: an integer, or the word unknown
size 80x24                           required: columns x rows
cursor row=22 col=19 shape=block hidden=false      required
modes ?1 DECKPAM                     required: zero or more tokens, space separated (a bare `modes` for none)
alt_screen true                      required: true or false
title null                           required: null or a JSON string literal, "my title"
wrapped 3 4                          optional: row indices that soft-wrap (omitted when none)
vocab modes ?1 ?1000 ?1002 ?1003 ?1005 ?1006 ?2004 DECKPAM      required: the mode tokens this golden knows about
vocab attrs bold dim italic reverse strike conceal underline ulcolor link   required: the attributes it knows about
ignore modes ?1 ?1000                optional, repeatable; see below
ignore title
x-producer vtee 0.1.0 alacritty_terminal 0.26.0     advisory
```

- **`vocab`** is what makes additions additive (section 4.1). The writer records the full vocabulary of the tool that
  wrote the golden, whether or not the screen uses each token. The checker compares only declared tokens: `modes` are
  compared after both sides are restricted to the declared mode tokens, and cell attributes after both sides are
  restricted to the declared attributes (`fg` and `bg` are core and always compared). A token the snapshot reports that
  the golden does not declare is ignored and produces `WARN vocab modes ?NNNN` in the report; it does not fail. The
  `modes` line of the header lists only tokens that are in `vocab modes` (a token outside it is an error, 6.13).
- **`ignore`** names a part the check skips: `ignore title`, `ignore alt_screen`, `ignore wrapped`,
  `ignore cursor` (all of position, shape and hidden), `ignore cursor.position`, `ignore cursor.shape`,
  `ignore cursor.hidden`, `ignore modes TOKEN...` (the listed tokens are removed from both sides) and
  `ignore attrs NAME...` (the listed attributes, including `fg` and `bg`, are removed from every cell comparison; the
  cell *text* is still compared). Several `ignore` lines for the same part are merged. `ignore modes` without a token
  is an error.
- A key beginning with `x-` is informational and ignored. **Any other unknown key is an error**, so a golden written by
  a newer tool that adds a directive changing comparison can never be silently misread.
- `byte_offset` and `scrollback` are never stored.
- `wrapped` lists the rows whose last cell wraps into the next row (`wrapped_rows` in the snapshot). Order is not
  significant to a reader; writers sort ascending.
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
  repeated within a run; a token for an attribute the golden's `vocab attrs` does not declare.

### 6.5 Regions and `[masks]`

A mask relaxes the comparison for a region. Everything outside every mask is compared in full: text and attributes
(restricted to the declared, un-ignored attributes).

```
mask REGION [text=ignore | text=/RE/] [attrs=ignore]
REGION  =  ROWS [COLS]  |  find=/RE/ [ROWS] [COLS]
ROWS    =  row=N | rows=N | rows=N-M          (a single index is the same as N-N)
COLS    =  cols=N | cols=N-M
```

Tokens after the keyword (`mask` or `assert`) may appear in any order, and each may appear once. A mask must say at
least one of `text=` or `attrs=ignore`. `row=` and `rows=` are the same. `N <= M`. All indices must lie inside the
golden's `size` (checked when the golden is read: an error, exit 65, otherwise).

**Evaluation.** The definitions are normative; the pseudocode is the intended algorithm.

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
  all** in its whole row range (not "no span in some row": a figure may be missing from one row of `rows=2-4`). A
  region written with rows and columns cannot be stale; a `text=/RE/` that does not match is reported as a mismatch.
- Where regions overlap, a part is skipped if any covering mask skips it.

### 6.6 `[assert]`

Sparse assertions on the **actual** screen, independent of `[text]` and `[attrs]`:

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

`golden check` performs, in this order:

1. **Parse** the golden and validate it (section 6.13). Errors stop here (exit 65).
2. **Versions and vocabulary.**
   - Grammar: a number above the reader's is refused (exit 2).
   - `schema`: higher than the reader's is refused (exit 2); lower is read through the migration table (5.3) while the
     deprecation window lasts, with a `WARN schema`, and refused after it with a pointer to `golden migrate`.
   - A declared `vocab` token the reader does not know is refused (exit 2), naming it.
   - `semantics`: see below.
3. **Size.** If it differs, report it and stop comparing the screen: the header fields of step 4 are still compared,
   but cells, masks and assertions (steps 5 to 7) are **not** (the grids no longer line up, and stale-mask reports would
   be noise).
4. **Header:** cursor, `alt_screen`, `modes` (as sets, restricted to the declared vocabulary and minus `ignore modes`),
   `title`, `wrapped`, each unless ignored.
5. **Cells.** Every cell outside masks: text (`ch`) and the attribute set (restricted to declared, un-ignored
   attributes).
6. **Masks:** stale-mask checks and `text=/RE/` checks.
7. **Assertions.**

`byte_offset`, `scrollback`, `producer` and `x-` keys never take part. Regions are validated against the **golden's**
size when it is read, so a size mismatch is the only reason a region can fall outside the actual screen.

**Semantics mismatch.** When the golden's `semantics` differs from the snapshot's (or is `unknown`), the checker still
runs steps 3 to 7 and then decides:

| Option | No differences found | Differences found |
| --- | --- | --- |
| default | `PASS`, exit 0, with `WARN semantics golden=A snapshot=B: no differences` | `REFUSED`, exit 2: the differences are listed, with a pointer to `docs/guide/snapshot-semantics.md` |
| `--accept-semantics` | `PASS`, exit 0, with the same warning | `FAIL`, exit 1, an ordinary failure |
| `--strict-semantics` | `REFUSED`, exit 2, nothing compared | (not reached) |

Reason: one integer covers all emulation areas, so a strict refusal would turn every bump into a refusal of every
golden, including ones that never touch the changed behaviour, and teach people to pass `--accept-semantics`
blindly. Comparing anyway keeps the information (nothing differs) and the safety (different results are not
presented as the program's fault). Repositories that want the strict behaviour for their own goldens use
`--strict-semantics`.

### 6.9 The report

Plain text on standard output by default; `--format json` writes one JSON object. **NAME** is the final component of the
golden's path, with its extension (`tf-session.golden`). The first line is exactly one of:

```
PASS NAME
FAIL NAME: 1 mismatch            ("1 mismatch" or "N mismatches")
REFUSED NAME: REASON
```

`N` counts blocks (a run of identical cell differences is one block). Warnings, if any, follow on lines beginning with
`WARN `. Each mismatch is a block whose first word names the kind. At most 20 blocks are shown (`--max-mismatches N`);
the first line gives the true total and a last line says `showing 20 of 413`. For `REFUSED` caused by a semantics
difference the blocks are shown after the first line, headed `differences (informational):`.

A mismatch block for each kind (the text forms are the specification; the exact column alignment is not significant):

```
FAIL tf-session.golden: 14 mismatches
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
  attrs      row 16 col 0-13 (14 cells)   expected fg=idx:2 bold   actual fg=idx:2
  mask       golden line 37   mask find=/\d\d:\d\d$/ text=/\d\d:\d\d/   matched nothing (stale)
  mask       golden line 38   mask row=20 cols=75-79 text=/\d\d:\d\d/   row 20 col 75-79 actual '_____' does not match
  assert     golden line 47   assert find=/goblin/ fg=idx:1 bold   row 17 col 2   actual fg=idx:3   (first failing cell of 8)
  assert     golden line 49   assert row=30 text="x"   matched nothing
```

(The example lists more kinds than a real report would show at once; it is in the order a report prints them: size,
cursor, modes, alt_screen, title, wrapped, then cells, masks and assertions.)

A `text` block shows both rows with a caret line under the differing columns; a `text` block for a continuation cell
shows `\.`. A warning:

```
PASS tf-session.golden
WARN semantics golden=1 snapshot=2: no differences
WARN vocab modes ?1004: reported by this vtee, not declared by the golden (not compared)
```

`--format json`:

```
{"golden":"tf-session.golden","result":"fail","grammar":1,"schema":2,"semantics":1,
 "snapshot":{"schema":2,"semantics":1},"total":2,"shown":2,
 "mismatches":[ ...objects below... ],
 "warnings":[{"code":"vocab","message":"modes ?1004: reported by this vtee, not declared by the golden"}]}
```

`result` is `pass`, `fail` or `refused`; a refusal adds `"reason"`. Every mismatch object has `kind`. Fields by kind
(rows and columns 0-based, `col_start` and `col_end` inclusive; unknown fields must be ignored by readers):

| kind | Fields |
| --- | --- |
| `size` | `expected` and `actual`, each `{"cols":N,"rows":N}` |
| `text` | `row`, `col_start`, `col_end`, `expected` and `actual` (the cell text of the span, strings), `expected_row` and `actual_row` (the whole row text) |
| `attrs` | `row`, `col_start`, `col_end`, `cells` (count), `expected` and `actual` (arrays of attribute tokens as in `[attrs]`) |
| `cursor` | `field` (`position`, `shape` or `hidden`), `expected` and `actual` (`{"row":N,"col":N}`, a shape string, or a boolean) |
| `modes` | `expected`, `actual`, `missing`, `extra` (arrays of tokens) |
| `alt_screen` | `expected` and `actual` (booleans) |
| `title` | `expected` and `actual` (string or null) |
| `wrapped` | `expected`, `actual`, `missing`, `extra` (arrays of row indices) |
| `mask` | `line` (golden line number), `mask` (the line text), `problem` (`stale` or `text`), and for `text`: `row`, `col_start`, `col_end`, `actual` |
| `assert` | `line`, `assert` (the line text), `problem` (`no_match`, `text` or `attr`), and for `text` and `attr`: `row`, `col` (first failing cell), `expected`, `actual`, `failing_cells` (count) |

Warning objects have `code` (`semantics`, `vocab`, `schema`) and `message`.

### 6.10 Exit codes of `vtee golden`

| Code | Meaning |
| --- | --- |
| 0 | Pass (possibly with warnings) |
| 1 | The screen differs from the golden |
| 2 | Refused: grammar, schema, vocabulary or semantics prevent or qualify a comparison (6.8). Deliberately not a sysexits code, so a script can tell "differs" and "cannot compare" apart from errors |
| 64 | Usage error |
| 65 | A golden or a snapshot is malformed (data error); the message says which, and for a snapshot the first line is `snapshot: ...` |
| 66 | A file named on the command line does not exist or cannot be read |
| 69 | The session cannot be reached (`--connect`) |
| 74 | Another I/O error |

**Usage errors are 64, not 2.** clap's own default exit code for a usage error is 2. `src/main.rs` (lines 198 to 202,
E25) maps every parse error to `exitcode::USAGE` before anything else, and `golden` is a subcommand of the same parser,
so the mapping covers it; the implementation must keep a test that a bad `golden` flag exits 64 and that a refusal
exits 2.

### 6.11 Recording, updating, migrating

- `vtee golden write -o NAME.golden (--connect SOCKET | --snapshot FILE | -) [--portable]` records the header,
  `[text]` and `[attrs]` from a snapshot. It writes no `[masks]` or `[assert]`: those are written by hand.
  `--portable` declares the cross-platform profile of section 6.14 (a reduced `vocab`, `ignore title`).
- `vtee golden update NAME.golden (--connect SOCKET | --snapshot FILE | -) [--refresh-vocab]` regenerates the header,
  `[text]` and `[attrs]` and **keeps** header comments and `ignore` lines (placed after the generated header lines, in
  their original order), `[masks]` and `[assert]` verbatim, including comments. `--refresh-vocab` replaces the declared
  vocabulary with the current tool's, which starts comparing the new tokens.
- `vtee golden update --restamp NAME.golden (--connect ... | --snapshot ...)` rewrites only the `semantics` line (and
  `x-producer`), and **refuses with exit 1 if any compared cell, header field or assertion would differ**: it confirms
  that a semantics bump changed nothing this golden checks.
- `vtee golden migrate FILE...` rewrites a golden recorded under an older grammar or schema (section 5.3).
- Every command is deterministic, and `write(parse(g)) == g` byte for byte for a canonical file. **`x-producer` is
  rewritten by `update` only when something else in the generated part changes**, so a re-record with a newer
  `vtee` that changes nothing produces an empty diff. It therefore names the `vtee` that last changed the golden,
  which is what an audit of a pin needs; it is not "the latest check".

### 6.12 Determinism rules for writers

Header lines in the order of section 6.2. `[attrs]` runs in row, then column order, tokens in the order of section 6.4.
Every row of `[text]` present, with no trailing blank cells. No line anywhere ends in whitespace. One newline at the end.
Nothing in the file depends on the Unicode version of the tool that wrote it.

**JSON string literals** (the header `title`, `link="URI"`, `text="EXACT"`) have one canonical encoding: UTF-8; escape only
`"` as `\"` and `\` as `\\`; the short escapes `\n`, `\r`, `\t`; every other scalar in the escape set of 6.3 (this includes
all other C0 controls and DEL) as `\uXXXX` in lower-case hex, with a surrogate pair `\ud83d\ude00` for scalars above
U+FFFF; everything else raw; a solidus `/` is never escaped. Readers accept any valid JSON string.

### 6.13 What a reader must reject, and what it must accept

Two independent implementations must agree on every file. A reader **rejects** (exit 65, naming the line):

- an invalid UTF-8 sequence, a NUL, a lone CR, a leading BOM;
- a first line that is not `vtee-golden N`, a missing required header key, a duplicated key (other than `ignore` and
  `x-`), an unknown key that does not begin with `x-`, an unknown section, a duplicated section;
- a `[text]` with the wrong number of rows, a row label that is not its index, a row with no `|`, more cells than `cols`,
  a malformed or unknown backslash sequence, a surrogate or out-of-range `\u{H}`, `\.` at the start of a row or after
  `\.`, `\+` at the start of a row or after `\.`;
- an `[attrs]` run that overlaps another, is outside the screen, has no tokens or only defaults, repeats a token, or
  uses an attribute not in the declared vocabulary; any comment in `[text]` or `[attrs]`;
- a region outside the golden's size, `N > M` in a range, a mask with neither `text=` nor `attrs=ignore`, `text="EXACT"`
  on a multi-row region or with `find=`, an assert with no constraint, a regex outside the dialect (6.7), an invalid
  JSON string, a repeated token in one `mask` or `assert` line, `ignore modes` with no token;
- a `modes` token that is not in `vocab modes`, or an `ignore` naming a mode or attribute that is not in the declared
  vocabulary.

A reader **accepts and normalises**: CRLF; a missing final newline; blank lines anywhere; trailing whitespace on
non-`[text]` lines; the sections or header lines in any order; `modes` tokens in any order or repeated (a set);
`wrapped` in any order; tokens of an `[attrs]` run in any order; region tokens of a mask or assert in any order; `\u{H}`
for a scalar outside the escape set, in either case with leading zeros; trailing literal spaces in a `[text]` row. Only
`golden update` and `golden write` normalise a file on disk; `golden check` never writes.

### 6.14 Portability across platforms

A golden is portable between Linux and Windows to the extent the program's output is. Measured differences (E3, E4,
E15 to E17), and how a golden handles each:

| Difference | Linux | Windows (ConPTY) | Handling |
| --- | --- | --- | --- |
| `?1`, `?1000`, `?1002`, `?7` | reported when set | never arrive (E15; `?1` and `?1006` from the project's earlier measurement; `?1003` and `?1005` are untested and assumed to behave the same) | declare only `?2004` in a portable golden, or `ignore modes ?1 ?1000 ?1002 ?1003 ?1005 ?1006` |
| `DECKPAM` (`ESC =`) | reported | never arrives (E15) | `ignore modes DECKPAM` |
| `?2004`, `?25` | reported | arrive normally | portable |
| `?1004`, `?9001` | not set unless the program does | injected by ConPTY at the start and removed at the end (E16) | `?1004` is not in the 0.1.0 vocabulary, and the portable vocabulary never declares it, so a portable golden never compares it |
| SGR 58 (underline colour) | colour kept | garbled into `dim`, or `blink` and `strike` (E17) | do not use SGR 58 in a portable golden; if a program emits it, goldens are per-platform |
| SGR 4:N (curly, dotted ...) | kept | kept | portable |
| `title` | `null` unless the program sets one | the host program's path unless set | `ignore title` |
| `byte_offset` | differs wildly (E3) | | never stored |
| Anything else | | | a real difference to investigate and report |

`golden write --portable` records the portable profile: `vocab modes ?2004`, the attribute vocabulary without `ulcolor`,
and `ignore title`. A golden recorded that way on one platform can pass on the other when the program's output is the
same. The profile is part of the contract: adding a token to it is a semantics-visible change.

## 7. Worked examples

The examples below were produced by running real snapshots through converters that implement this document's first
draft, then parsing the result back and comparing it to the snapshot (E14); the revision added the `vocab` header lines,
one whole-row assertion in 7.1, `cols=47` in 7.2 and a scalar-sequence assertion in 7.3, and kept the rest. The golden grammar here was independently implemented from the text by two reviewers, who
reproduced examples 7.1 and 7.2 byte for byte.

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
vocab modes ?1 ?1000 ?1002 ?1003 ?1005 ?1006 ?2004 DECKPAM
vocab attrs bold dim italic reverse strike conceal underline ulcolor link
x-producer vtee 0.1.0 alacritty_terminal 0.26.0

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
assert row=16 text="A Quiet Meadow"
```

The last assertion has no `cols=`: a whole-row region compares the row text with trailing blanks removed (6.5), so the
natural form passes on an 80-column screen. Assertions are checked against the actual screen, whatever is stored in
`[text]`, so they are written against the row numbers of the screen being tested.

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
vocab modes ?1 ?1000 ?1002 ?1003 ?1005 ?1006 ?2004 DECKPAM
vocab attrs bold dim italic reverse strike conceal underline ulcolor link

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
schema 2
semantics 1
size 40x6
cursor row=2 col=0 shape=block hidden=false
modes
alt_screen false
title null
vocab modes ?1 ?1000 ?1002 ?1003 ?1005 ?1006 ?2004 DECKPAM
vocab attrs bold dim italic reverse strike conceal underline ulcolor link

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

What `golden write --portable` records for a program that printed `ok` and turned on bracketed paste. The vocabulary
is the portable one (6.14): one mode token, no underline colour, and the title is ignored, so the same golden passes
on Linux and under ConPTY.

```text
vtee-golden 1
schema 2
semantics 1
size 40x3
cursor row=1 col=0 shape=block hidden=false
modes ?2004
alt_screen false
title null
vocab modes ?2004
vocab attrs bold dim italic reverse strike conceal underline link
ignore title
x-producer vtee 0.1.0 alacritty_terminal 0.26.0

[text]
0|ok
1|
2|
```

### 7.5 What a semantics difference looks like

A hypothetical future `semantics 2` that stops reporting tab-stop cells as spaces would show (a golden recorded under
semantics 1 whose screen contains a tab):

```text
REFUSED tf-edit.golden: semantics golden=1 snapshot=2, with 1 difference
differences (informational):
  text      row 4 col 8   expected ' '   actual '\u{9}'
see docs/guide/snapshot-semantics.md, entry 2
```

and if the same golden had no difference:

```text
PASS tf-startup.golden
WARN semantics golden=1 snapshot=2: no differences
```

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
| Versions of the golden grammar | `vtee-golden N`; a reader supports all N up to its own | Separates grammar from snapshot layout without retiring old files |
| Is "add a token" free? | Yes, because goldens declare their vocabulary (OD8) | Review R1: otherwise every addition breaks stored goldens |
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

### OWNER DECISIONS

The reviewers' verdicts are in parentheses (TF = TinyFugue, FA = the Fable reviewer).

| # | Decision | Recommendation | If you choose otherwise |
| --- | --- | --- | --- |
| OD1 | **Schema number.** Call the frozen shape `schema_version` 2, leaving 1 as the provisional shape on `main` (TF agree, FA agree) | 2 | Keep 1: simpler number, but every artefact already recorded against the provisional shape (TinyFugue's, ours) becomes indistinguishable from the frozen one |
| OD2 | **Sparse cells.** Omit defaults, as in 3.3, in the same change as the redesign (TF agree, FA agree) | Yes, now | Keep dense: simpler to read in JSON, but every added attribute grows every cell, the snapshot stays at 400 KB, and moving to sparse later is a breaking change |
| OD3 | **Deprecation window.** For a superseded schema number: at least 12 months and until the next major, with `golden migrate` kept forever; a reader supports all grammar numbers up to its own (TF: state it in time, FA: not for grammar) | As stated | Two minor releases (the first draft): in 0.x that can be two weeks; none at all (refuse immediately) makes every bump break every stored golden |
| OD4 | **Semantics mismatch in `golden check`.** Compare anyway: identical results pass with `WARN semantics`, different results are refused (exit 2) with the differences; `--strict-semantics` and `--accept-semantics` change this (TF and FA both proposed it) | As stated | Always refuse (the first draft): friendlier to describe, but one integer for all areas refuses every golden on every bump. Warn only: a golden can then fail for reasons that are not about the program |
| OD5 | **Regex dialect** pinned to the RE2 common subset with ASCII classes and a stated rejection list (TF agree, FA agree) | Yes | A richer dialect (Python `re`) makes goldens depend on one language |
| OD6 | **Known emulation deviations** (ED 1 on row 2, origin-mode reset, mode ?47): freeze them in semantics 1, list each precisely in the changelog, bump when upstream fixes them (TF agree: none touches tf; FA agree) | Freeze and document | Fix first by vendoring `alacritty_terminal`: a fork to maintain; goldens recorded before the fix would need `golden update` only if they exercised those sequences |
| OD7 | **What 0.1.0 contains.** The frozen formats and `vtee golden write`, `check` and `update` (with `--restamp`, `--refresh-vocab`, `--portable`, `--format json`), plus the test vectors; `migrate` follows no later than the first schema or grammar bump (TF and FA agree) | As stated | Formats only, with the commands in 0.2: the formats would be frozen before the reference implementation validated them. Everything including `migrate`: more to build before a tag, for goldens that TinyFugue re-records anyway |
| OD8 | **Vocabulary strategy.** Goldens declare the vocabulary they were written with, and the checker compares only that (4.1), against: every newly reported mode or attribute is a semantics bump | Declared vocabulary | Semantics bump for every addition: nothing to build in the header, but each new attribute refuses or warns on every stored golden |
| OD9 | **Mode vocabulary at 0.1.0.** Add `?1002`, `?1003`, `?1005`; reserve `?1004` (ConPTY injects it) and `?1007` (no consumer) | As stated | Add all five of the first draft: `?1004` would be set on Windows and not on Linux for the same program, and every portable golden would need to ignore it |
| OD10 | **A portable profile in 0.1.0** (`golden write --portable`, 6.14) | Yes | Leave it to authors: every cross-platform golden needs a hand-written `ignore` list that is easy to get wrong (the first draft's list was incomplete: E15 to E17) |

## 9. Required changes before 0.1.0

Sizes are rough: S under a day, M a day or two, L several days. Dependencies are marked.

1. **Versions and producer.** `SCHEMA_VERSION = 2`, a new `SEMANTICS_VERSION = 1`, both first in the snapshot; the `producer`
   object. **S** (do first; everything else assumes it)
2. **Sparse cells and the new cell shape** (OD1, OD2; sections 3.2 to 3.5): colours as strings, `underline` as a style
   string (removing `underline_style` and the old boolean), `conceal` added, `blink` and null `hyperlink` removed,
   defaults omitted, cursor shapes lower-case and `row` unsigned, `lines` documented, the JSON writer escaping U+0085,
   U+2028 and U+2029 (3.7). `src/snapshot.rs`, `redraw_sequence`, serde, the differential harness's reader,
   `docs/guide/protocol.md`, `examples/`, the text-mode printers. **L** (after 1)
3. **`wrapped_rows`** from `WRAPLINE` (including the wide-character placeholder case, E20), and an opt-in `wait_for` mode
   that matches against logical (joined) lines using it. **M** (after 2; parallel with 4)
4. **Modes:** a sorted set and the three new tokens (`?1002`, `?1003`, `?1005`), with the exclusivity rules of 3.6 and a
   test. **S** (parallel with 3)
5. **Conformance data** (section 5.4): `tests/fixtures/conformance/expected/`, the exact-comparison test, the width
   sample and the test that runs it through the emulator, the escape-set data, the CI guard, the `snapshot` commit
   scope in commitlint and the release-plz changelog groups. **M** (after 2 and 4)
6. **The golden library:** a module that parses, writes, compares and reports, with the region evaluation of 6.5, the
   regex dialect (an ASCII-rewriting layer over a regex engine plus the rejection parser of 6.7), the vocabulary
   handling of 4.1 and the version logic of 6.8. **L** (after 2; the largest item)
7. **`vtee golden write|check|update`** with `--restamp`, `--refresh-vocab`, `--portable`, `--format json`,
   `--max-mismatches`, `--strict-semantics`, `--accept-semantics`, the exit codes of 6.10 and a test that a bad flag
   exits 64 and a refusal exits 2. **M** (after 6)
8. **Test vectors** (appendix A): about 40 golden/snapshot/verdict triples, the 46 regex vectors, the round-trip files,
   and a runner test; confirm that a second implementation (TinyFugue's checker) passes them. **M** (written alongside
   6; the triples are the acceptance test for it)
9. **`golden migrate`** for schema 1 snapshots and grammar 0 goldens (5.3). **M** (no later than the first schema or
   grammar bump; may follow 0.1.0)
10. **Documentation:** the golden spec promoted to `docs/guide/golden-format.md`, `docs/guide/snapshot-semantics.md`
    created with the semantics 1 entry and the three known deviations, `protocol.md` and `versioning.md` updated, the
    platform table in `windows.md`, a testing page section. **M** (after 7)
11. **Platform CI:** a few repository goldens checked on both Linux and Windows jobs, recorded with `--portable`. **S**
    (after 7)
12. **Tell TinyFugue** the changes from their draft (below) before they record real goldens, and again when schema 2
    lands on `main`. **S**

The order is 1, 2, then 3 and 4 and 5 together, then 6, 7 and 8, then 10 and 11, with 9 free to follow. TinyFugue's
recordings made under schema 1 are re-recorded after step 2; they planned to re-record anyway.

### Changes from TinyFugue's draft

- Assertions moved to their own `[assert]` section.
- `[text]` is cell-per-token with escapes (`\.`, `\u{H}`, `\+{H}`, `\\`); the draft joined the cells' `ch` values and had no
  rule for wide characters.
- Header: `semantics`, `vocab`, `wrapped`, `ignore`, `x-producer` added; `title` is `null` or a JSON string, and the
  cursor line is `hidden=B`.
- `modes-ignore` and `title-ignore` became `ignore modes ...` and `ignore title`; `ignore attrs` and the cursor forms
  are new.
- Regex classes are ASCII-only, `$` anchors to the last visible character of the searched cells, and a rejection list
  and test vectors fix the dialect.
- A whole-row region compares the row text without trailing blanks; `rows=A-B` is one span per row; the mask and
  assertion rules are the pseudocode of 6.5.
- The grammar number is `vtee-golden 1`; the draft's `vtee-golden 0` is read by `golden migrate`.

## 10. Deferred, with the additive path reserved

Each path below was checked against the rule "an existing reader and an existing golden keep working unchanged".

| Item | Reserved name or path | Why it is additive | Why deferred |
| --- | --- | --- | --- |
| The inactive screen buffer | top-level `inactive` object with the same shape as the snapshot | A new optional field; goldens do not read it | The emulator cannot read it without destroying it |
| Cell-level scrollback | `scrollback_cells` | A new optional field | No consumer yet |
| Hyperlink ids | cell `hyperlink_id` | An optional cell field; a golden that does not declare `link` ids ignores it | Rare |
| Blink, overline | cell `blink`, `overline` | New names in the attribute vocabulary; goldens compare only declared attributes (4.1) | Emulator support |
| Focus reporting, alternate scroll, autowrap and other output-affecting modes, kitty keyboard flags | more tokens in `modes` (`?1004`, `?1007`, `?7`, ...) | New names in the mode vocabulary (4.1); an old golden passes with a `WARN vocab` | No consumer; `?1004` is also injected by ConPTY |
| `cursor.pending_wrap` | cursor field, written only when true | A new optional field; the golden *grammar* gains a header field behind a new grammar number, and old goldens keep being read as before | Invisible today; no consumer |
| A more compact row encoding | a `snapshot` request parameter, `"encoding":"runs"`, that returns `row_runs` **instead of** `cells` only when asked | The default reply is unchanged. (The first draft listed `row_runs` as a field; replacing `cells` by default would have been a schema bump, so the *request* is the reserved path) | Sparse cells already give 20 to 30 times; add only if measured to matter |
| `snapshot` command parameters (`scrollback`, `lines`, a cell region) | optional request fields | Absent means today's reply | Each is additive |
| Grapheme clustering | a different width model, behind `semantics_version` | **Not additive**: it changes meaning, so it is a semantics bump by the table in 4.1 | Changes meaning |
| Structural queries for tables (columns by header alignment, numeric ranges) | new assertion forms behind a new grammar number | Old goldens parse unchanged; only goldens using the new forms need a newer reader | TinyFugue's own recordings suggest the form; build after masks |
| Images (sixel, kitty graphics), selection, timestamps | none | | Not in the emulator |

## Appendix A. Test vectors

A second implementation of `golden check`, in any language, proves it agrees by passing these. They are frozen data
under the rules of section 5.4. Layout, in the repository when item 8 of section 9 is done:

```
tests/fixtures/golden-vectors/
  README.md                         how to run the vectors; the version of this document they belong to
  regions/<id>-<name>/
      golden.golden                 the golden under test
      snapshot.json                 the actual screen (schema 2)
      expected.txt                  the exact text report
      expected.json                 the exact JSON report
      exit                          the expected exit code, one number
  regex/vectors.tsv                 pattern <TAB> subject <TAB> operation <TAB> result   (A.3)
  roundtrip/*.golden                canonical files: write(parse(g)) must equal g byte for byte
  reject/*.golden                   one malformed file per rule of 6.13, expected exit 65 (name says which rule)
  width/width-sample.txt            appendix B
  escapes/escape-set.txt            appendix B
```

### A.1 The region, mask, assert and version vectors

About 40, each pinning one rule of sections 6.5 to 6.8. The `regions/` directories are named `<id>-<name>`:

| Id | Name | Pins |
| --- | --- | --- |
| R01 | mask-text-ignore-pass | `text=ignore` skips text, attributes still compared |
| R02 | mask-rows-range-per-row-regex | `rows=7-8` is two spans; each matches the pattern on its own |
| R03 | mask-rows-range-one-row-fails | a regex failure in the second row is a `mask` mismatch |
| R04 | mask-find-pass | a `find=` yields spans and the figures vary |
| R05 | mask-find-stale | a `find=` with no span at all is a failure |
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
| R27 | vocab-undeclared-token-warns | a mode the golden does not declare: PASS with `WARN vocab` |
| R28 | wrapped-mismatch-and-ignore | `wrapped` differs, then `ignore wrapped` |
| R29 | size-mismatch-skips-cells-masks-asserts | only the `size` block (and header blocks) |
| R30 | semantics-no-differences-warns | PASS, exit 0, `WARN semantics` |
| R31 | semantics-differences-refused | REFUSED, exit 2, differences listed |
| R32 | semantics-strict-refuses | `--strict-semantics`, nothing compared |
| R33 | semantics-accept-differences-fail | `--accept-semantics`, exit 1 |
| R34 | schema-higher-refused | exit 2 |
| R35 | vocab-unknown-token-refused | a declared token the reader does not know: exit 2 |
| R36 | crlf-accepted | the same golden with CRLF line endings passes |
| R37 | portable-golden-passes-both | a `--portable` golden against a Linux and a ConPTY-shaped snapshot |

The `reject/` files cover each rule of 6.13: BOM, lone CR, NUL, bad first line, missing and duplicated keys, unknown
section, wrong row count, bad label, `\.` at the start of a row and after `\.`, `\+` at the start of a row and after
`\.`, a bad `\u{}`, overlapping runs, an empty run, a repeated token, an undeclared attribute, a comment in `[text]`, a
region outside the size, a mask with nothing to do, `text="EXACT"` on several rows and with `find=`, a regex outside the
dialect, a bad JSON string, a repeated token in a `mask` line, an empty `ignore modes`.

### A.2 Two vectors in full

A 6 by 2 screen whose row 0 reads `ab 12 ` with the digits bold. The golden:

```text
vtee-golden 1
schema 2
semantics 1
size 6x2
cursor row=0 col=0 shape=block hidden=false
modes
alt_screen false
title null
vocab modes ?1 ?1000 ?1002 ?1003 ?1005 ?1006 ?2004 DECKPAM
vocab attrs bold dim italic reverse strike conceal underline ulcolor link

[text]
0|ab 12
1|

[attrs]
0: 3-4 bold

[masks]
mask row=0 cols=3-4 text=/[0-9]+/
```

**R-a (pass, exit 0).** The actual screen's row 0 is `ab 99 ` with the `99` bold. Cells 0, 1, 2 and 5 equal the golden's; cells
3 and 4 are masked for text, the span text `99` matches `^(?:[0-9]+)$`, and the bold attribute equals the stored one.
Report: `PASS g.golden`.

**R-b (fail, exit 1).** The actual row 0 is `ab x7 ` with the `x7` bold. The span text `x7` does not match. Report:

```text
FAIL g.golden: 1 mismatch
  mask       golden line 20   mask row=0 cols=3-4 text=/[0-9]+/   row 0 col 3-4   actual 'x7' does not match
```

and the JSON report:

```json
{"golden":"g.golden","result":"fail","grammar":1,"schema":2,"semantics":1,"snapshot":{"schema":2,"semantics":1},"total":1,"shown":1,"mismatches":[{"kind":"mask","line":20,"mask":"mask row=0 cols=3-4 text=/[0-9]+/","problem":"text","row":0,"col_start":3,"col_end":4,"actual":"x7"}],"warnings":[]}
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
`tests/fixtures/golden-vectors/` when the implementation lands.

- **`docs/design/golden-escape-set.txt`**: the escape set of section 6.3. 876 inclusive hexadecimal ranges, 966,490
  scalars, from Unicode 15.1.0 (Python 3.13.5's `unicodedata`), with the SHA-256 of the range lines in its header.
  Generated by a scratch script (`escset.py`) from the categories `Cc Cf Cs Co Cn Zs Zl Zp Mn Me`, minus U+0020, plus
  U+2800, U+115F, U+1160, U+3164, U+FFA0.
- **`docs/design/golden-width-sample.txt`**: the width class of 1,062 scalars (E19), as run-length ranges: `U+0020..U+0041 1`
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
| TF1 / FA-A1: additive vocabulary is not free for goldens | **Accepted; fixed differently from TF's proposal** | Both demonstrated the break. TF suggested treating newly reported things as semantics bumps; FA suggested self-describing goldens. FA's was chosen (OD8): additions stay additive, `WARN vocab` keeps the gap visible. 4.1, 6.2, 6.8 |
| TF2: report only specified for text | **Accepted** | An example and a JSON field table per kind; exit 65 for a malformed snapshot; NAME defined. 6.9, 6.10 |
| TF3: region and mask evaluation ambiguous (a to g) | **Accepted** | Pseudocode and fixed answers: (a) one span per row, (b) whole-row text trims, explicit `cols=` keeps, (c) `find=` with `cols=` searches only those cells, (d) stale = no span in the whole range, (e) every match, (f) forms and free token order, (g) a match inside appended scalars covers the whole cell. 6.5, 6.6; vectors R01 to R25 |
| TF4: JSON string literals have no canonical encoding | **Accepted** | 6.12 |
| TF5: canonical form implied, reader strictness unstated | **Accepted, two points changed** | Blank-line rule, optional `[attrs]`, hex rules, comment rules, `ignore` merging, update's treatment of header comments, a must-reject list. TF wanted `\u{41}` and an unsorted `modes` line rejected; they are **accepted** (FA wanted `\u{H}` accepted too) because the meaning is the same and it keeps extending the escape set a canonical-form change. 6.1, 6.2, 6.3, 6.13 |
| TF6: `x-producer` contradicts the empty-diff promise | **Accepted** | `update` rewrites it only when something else changes. 6.11 |
| TF7: CRLF on Windows | **Accepted** | Readers accept CRLF, reject a BOM (both reviewers wanted the BOM rejected); `.gitattributes` advice. 6.1 |
| TF8: migration mapping unwritten and lossy | **Accepted** | Table; migrate does not invent facts. 5.3 |
| TF9 / FA-A3: conformance set does not cover width tables | **Accepted** | A frozen width sample of 1,062 scalars and the rule for upgrades. E19, 5.4, appendix B |
| TF10: regex dialect is prose only | **Accepted** | Exact construct list, rejection list, ASCII rewriting, 46 vectors. 6.7, appendix A.3, E24 |
| TF11: exit code 2 may collide with clap | **Checked: no collision** | `src/main.rs` maps every clap error to 64. A test is required. E25, 6.10 |
| TF12: size mismatch and masks | **Accepted** | Masks and assertions are skipped too. 6.8 |
| TF13: `--accept-semantics` with no differences | **Accepted** | Prints `WARN semantics`. 6.8 |
| TF14: escape set gaps | **Accepted** | Defined by category rule, frozen from Unicode 15.1. 6.3, appendix B, E26 |
| TF15: minor items | **Accepted** | 0-based stated (6); "1 mismatch" wording (6.9); the header key is `schema`, the JSON field `schema_version` (4.3, 6.2); `ignore cursor` split into position, shape, hidden (6.2); `ulcolor` without `underline` is meaningful (3.2, 6.6) |
| TF: wrapped_rows is also set by a wide char pushed to the next row | **Accepted** | 3.2, E20 |
| TF: `\\.` and `\.` look alike | **No change** | Parsing is unambiguous (the grammar is by token). 6.3 |
| TF: `idx:1` and `rgb:#cd0000` are different values | **Stated as intentional** | 3.2 |
| TF OD3 (state the window in time) | **Accepted** | OD3 |
| TF OD4 (compare anyway) | **Accepted** | OD4, 6.8 |
| TF OD7 (formats, write, check, update in 0.1.0) | **Accepted** | OD7 |
| FA-A2: platform list in 6.12 is wrong | **Accepted and re-measured on Windows 11** | DECKPAM is also absorbed, ConPTY injects `?1004h`, SGR 58 is garbled. E15 to E17, 6.14, OD10 |
| FA-A4: `golden migrate` cannot produce a valid grammar-1 file | **Accepted** | `ignore wrapped`, `semantics unknown`, a shape rule. 5.3 |
| FA-A5: grammar bumps fall under the two-minor refusal | **Accepted** | Readers support all grammar numbers up to their own. 4.3, 5.3 |
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
| FA OD1 to OD7 | **Agreed**, with the changes to OD3, OD4, OD5 and OD7 above | Section 8 |
| FA-E: not verified (`wrapped_rows` live, Linux ConPTY equivalent, Windows vim and less) | **Open** | `wrapped_rows` is inferred from the emulator source (E20) and is verified when item 3 of section 9 is built |
