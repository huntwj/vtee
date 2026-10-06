# Terminal-emulator capability probe: results

Date 2026-10-06. Everything ran inside throwaway `docker run --rm` containers (python:3.12-slim, node:22-slim); nothing was installed on the host.
Each probe is a byte file (`T1.bin` ... `T14.bin`, committed as-is in `probe/`; the throwaway generator was not saved); the program under test is `sh -c 'cat Tn.bin; sleep 20'`
at 40 cols x 12 rows, `TERM=xterm-256color`. All probes use explicit CR LF, so the pty's LF-to-CRLF translation does not change results.
Verdicts come from `probe_eval.py` judging the state each tool exposes. **Measured** = I ran it and saw the output. **Read** = from
source/README/help text only. Anything not marked measured is not a result.

## Matrix (measured)

| Test | pyte 0.8.2 | tui-test xterm.js | tui-test alacritty | tui-test ghostty | tui-test rio | ht 0.4.0 | pilotty 0.0.11 |
|---|---|---|---|---|---|---|---|
| T1 CUP + text | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| T2 SGR 1,2,3,4,5,7,9 | PARTIAL (no dim) | PASS | PARTIAL (no blink) | PASS | PARTIAL (no blink) | PASS (via `seq`) | NOT EXPOSED |
| T3 256-colour fg/bg | PASS | PASS | PASS | PASS | PASS | PASS (via `seq`) | NOT EXPOSED |
| T4 truecolour | PASS | PASS | PASS | PASS | PASS | PASS (via `seq`) | NOT EXPOSED |
| T5 DECSTBM scroll region | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| T6 IL/DL, ICH/DCH, IRM | PARTIAL (DL bug) | PASS | PASS | PASS | PASS | PASS | PARTIAL (IRM ignored) |
| T7 alternate screen (a: leave, b: stay) | FAIL / FAIL | PASS / PASS | PASS / PASS | PASS / PASS | PASS / PASS | PASS / PASS | PASS / PASS |
| T8 wide CJK, emoji, combining | PASS | PASS | PASS | PASS | PASS | PASS (text rows only) | PASS (text rows only) |
| T9 DEC special graphics | FAIL (default) | PASS | PASS | PASS | PASS | PASS | FAIL |
| T10 mode state (?1000 ?1006 ?2004 ?1, ESC=) | PARTIAL (4/5; no keypad) | PARTIAL (4/5; no ?1006) | PARTIAL | PARTIAL | PARTIAL | PARTIAL (only ?1) | NOT EXPOSED |
| T11 OSC 0 title, OSC 8 link | PARTIAL (title only) | PASS | PASS | PASS | PASS | NOT EXPOSED | NOT EXPOSED |
| T12 DECTCEM, DECSCUSR | PARTIAL (hidden, no shape) | PASS | PASS | PASS | PASS | PARTIAL (hidden, no shape) | PARTIAL (visible flag, no shape) |
| T13 tab, BS, CR, LF | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| T14 scrollback (50 lines, 12 rows) | PASS (HistoryScreen) | PASS (`text(full=True)`) | PASS | PASS | PASS | NOT EXPOSED | PASS (via `output`) |

pyte gave identical results fed directly and via a stdlib `pty` running `cat`. tui-test: 4 of 4 backends started in a plain container.

## Evidence worth keeping

- **pyte DL bug (measured + source):** `Screen.delete_lines` only moves a row up `if y + count in self.buffer`. If the row being moved
  up is blank and never written (absent from pyte's sparse buffer), the destination keeps its stale text. Reading `screen.display`
  between feeds materialises the rows and hides it (4-way experiment in `dbg6d.py`: wrong without the read, right with it).
- **pyte DEC graphics (measured + source):** `Stream` ignores `ESC ( 0` when `use_utf8` is True (the default; `ByteStream` too).
  With `use_utf8=False` the same bytes give the box characters, but then the stream must be fed already-decoded text.
- **pyte alternate screen:** no handling of modes 47/1047/1049 in `screens.py`; T7a/T7b fail as expected.
- **tui-test colours:** cells report `fg=196 bg=21` for 256-colour and `'#0ac81e'` for truecolour. Per its own `Cell` docs, alacritty and
  rio "cannot report blink" (matches T2). Alacritty backend puts a literal `\t` character in cells for tab padding (cosmetic).
- **tui-test state:** `State.modes` is a dict (`alternate_screen`, `application_cursor_keys`, `application_keypad`, `bracketed_paste`,
  `cursor_visible`, `focus_events`, `insert`, `origin`, `wraparound`) plus `mouse_mode` (`click` after `?1000h`); the SGR mouse
  encoding (`?1006`) is not reported. Cursor has `visible` and `shape` (`bar` after `ESC[5 q`).
- **tui-test recording (measured, head only):** `format="cast"` gives asciicast v2, but it opens with a reset and a palette dump the
  program never sent, so it is a re-rendered screen history, not raw program bytes. No replay-into-emulator API found (Read).
- **ht raw capture (measured):** with `--subscribe output`, the `output` event data is byte-identical to the 145-byte probe file.
  Snapshots give `text` plus `seq`; attributes exist only as escape-sequence text in `seq` (my adapter parses it). No title,
  OSC 8, scrollback, or most mode state in a snapshot. No wait primitive exists (Read: `--help`, README).
- **ht quirks (measured):** needs `--` before the command; it re-joins argv with spaces without quoting, so `sh -c 'a; b'` breaks
  (use a script file). The Linux binary would not execute from the Windows bind mount; copy it into the container first.
- **pilotty (measured):** full snapshot = `{size, cursor{row,col,visible}, text, content_hash}`. Nothing else. `spawn` has no size
  option (`resize` afterwards). Back-to-back `stop`/`spawn` races (SESSION_NOT_FOUND) unless it waits ~2 s. IRM ignored, DEC
  graphics not translated. `output` returned the retained text of all 50 lines (rendered text, not escape sequences).

## Per-candidate facts

| | pyte | tui-test | ht | pilotty |
|---|---|---|---|---|
| Form | Python library | Python (native ext.), Rust, Node, CLI (README) | single Rust binary, JSON over stdin/stdout | daemon + CLI (Unix socket) |
| Install (measured) | `pip install pyte` (+`wcwidth`), seconds | `pip install tui-test` 3 s, 16 MB | 3.0 MB release binary | `npm i -g pilotty` 20 MB |
| Version | 0.8.2 (Nov 2023, PyPI) | 0.1.0 | 0.4.0 (2025-07-25) | 0.0.11 |
| License (Read) | LGPL-3.0 | MIT (pip metadata, measured) | Apache-2.0 | MIT |
| Input (Read from stubs/help; only `write`/`type` exercised in tui-test) | n/a: we write to our own pty | write, type, press, key_down/up, mouse click/drag/scroll, resize, signal | `sendKeys`, `input`, `resize`, `takeSnapshot` | type, key, click, scroll, resize |
| Wait primitive (Read, not exercised) | none | `wait_idle`, `wait_locator`/`expect_*` (retry-until-match), wait_title/exit/bell | none | `wait-for`, `--settle`, `--await-change` |
| Raw byte capture | yes, we own the pty | no (re-rendered cast only) | yes (`output` events, measured) | no (rendered text only) |
| Platforms (Read) | pty module is Unix-only; pyte itself portable | Windows, Linux, macOS (README) | Linux, macOS | Linux, macOS |

## Rust emulator crates, probed directly (2026-10-06)

This matrix only covers the four candidates above, which were exercised through wrappers (tui-test's Python
bindings, ht's JSON protocol), not through their own Rust API. A follow-up probe fed the same `T1`-`T14` files
straight into four Rust emulator/parser crates (`avt`, `vte`, `alacritty_terminal`, `termwiz`) and graded them with
the same `probe_eval.py` judge. Full matrix, evidence and the decision made from it are in `docs/decisions.md`;
raw results are `docs/probe/results_{avt,vte,alacritty,termwiz}.json`, source in `docs/probe/rust/`. Short version:
`alacritty_terminal` won (14/15 PASS, only gap is a structurally absent blink flag), confirmed robust to chunked
feeds and malformed input. `avt` was close behind but missing mode state/title/OSC8/cursor shape; `vte` and
`termwiz` are parsers, not emulators, and needed a hand-rolled state layer to grade at all.

## Not run / not measured

- **agent-tui and npcterm:** not run (time box; the four above were the priority).
- Repeated-run determinism, Windows/ConPTY, resize reflow, mouse input, and the wait primitives' real behaviour were not exercised.
- tui-test is 0.1.0; maturity under a real long-running program (TinyFugue) is unknown.

## Reproduce

Run from the repository root; `probe/` holds the byte files, the drivers and the raw per-candidate result JSON.

```sh
D="$PWD/docs/probe"
MSYS_NO_PATHCONV=1 docker run --rm -v "$D:/p" -w /p python:3.12-slim sh -c "pip install -q pyte && python run_pyte.py"
MSYS_NO_PATHCONV=1 docker run --rm -v "$D:/p" -w /p python:3.12-slim sh -c "pip install -q tui-test && python run_tuitest.py"
# ht: download v0.4.0 x86_64-unknown-linux-gnu from github.com/andyk/ht/releases into $D/ht first (do not commit it)
MSYS_NO_PATHCONV=1 docker run --rm -v "$D:/p" -w /p python:3.12-slim sh -c "cp /p/ht /usr/local/bin/ht && chmod 755 /usr/local/bin/ht && python run_ht.py"
MSYS_NO_PATHCONV=1 docker run --rm -v "$D:/p" -w /p node:22-slim sh -c "npm i -g pilotty && sh /p/pt_all.sh"  # then: PYTHONIOENCODING=utf-8 python run_pilotty_eval.py
```

Files not copied from the original run: the raw pilotty output captures (`pt_*.json`, `pt_out_*.txt`), run logs, and the `ht`
binary. The drivers may still name those paths. The summaries are in `probe/results_*.json`.

The Rust crate probes run directly with `cargo`, no Docker needed:

```sh
cd docs/probe/rust/<crate>   # avt, vte, alacritty_terminal, termwiz
cargo run --quiet > ../../<crate>_raw.jsonl
cd ../..
python3 run_rust_adapter.py <crate> <crate>_raw.jsonl results_<crate>.json
```

(`alacritty_terminal`'s probe dir also has `cargo run --bin stress` for the chunked-feed and malformed-input
checks in `docs/decisions.md`; those print straight to stdout, no grading step.) The `*_raw.jsonl` files are not
committed, regenerate them if needed.
