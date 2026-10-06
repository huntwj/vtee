"""Shared grading driver for Rust-crate probes.

Each Rust probe binary feeds docs/probe/T*.bin into one candidate emulator crate and writes a newline-delimited
JSON file, one object per test, to stdout or a file. This script reads that file, reconstructs a `Snap` (see
probe_eval.py) per test, and grades it with the same judge() used for pyte/tui-test/ht/pilotty, so results are
directly comparable.

Per-line JSON schema (any field may be null/omitted if the crate's API does not expose it):
{
  "test": "T1",                                  # one of probe_eval.TESTS
  "lines": ["...", ...] ,                         # ROWS plain-text rows (trailing padding is fine, judge() rstrips)
  "cell": [[{...}, ...COLS...], ...ROWS...] | null,   # per-cell dict: char, fg, bg, bold, dim, italic, underline,
                                                        # blink, reverse, strike. fg/bg: null, int (palette index),
                                                        # [r,g,b], or "rrggbb" hex string.
  "cursor": {"row": int, "col": int, "hidden": bool|null, "shape": str|null} | null,
  "title": str | null,
  "modes": [str, ...] | null,
  "links": any | null,
  "scrollback": [str, ...] | null
}

Usage: python run_rust_adapter.py <crate_name> <jsonl_path> [results_filename]
"""
import json
import sys

from probe_eval import Snap, judge, TESTS

# Windows consoles default to a legacy codepage (cp1252) that can't encode CJK/emoji evidence strings (T8).
sys.stdout.reconfigure(encoding="utf-8", errors="replace")


def make_cell_fn(grid):
    if grid is None:
        return None

    def cell(r, c):
        try:
            return grid[r][c]
        except (IndexError, TypeError):
            return None

    return cell


def snap_from_json(obj):
    return Snap(
        lines=obj.get("lines") or [],
        cell=make_cell_fn(obj.get("cell")),
        cursor=obj.get("cursor"),
        title=obj.get("title"),
        modes=obj.get("modes"),
        links=obj.get("links"),
        scrollback=obj.get("scrollback"),
    )


def main():
    crate = sys.argv[1]
    jsonl_path = sys.argv[2]
    out_path = sys.argv[3] if len(sys.argv) > 3 else "results_%s.json" % crate

    by_test = {}
    with open(jsonl_path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            obj = json.loads(line)
            by_test[obj["test"]] = obj

    out = {}
    for t in TESTS:
        obj = by_test.get(t)
        if obj is None:
            out[t] = ["MISSING", "no output for this test"]
            print("%-4s %-12s %s" % (t, "MISSING", "no output"))
            continue
        snap = snap_from_json(obj)
        v, ev = judge(t, snap)
        out[t] = [v, ev]
        print("%-4s %-12s %s" % (t, v, ev[:200]))

    json.dump(out, open(out_path, "w", encoding="utf-8"), indent=1, ensure_ascii=False)
    print("wrote", out_path)


if __name__ == "__main__":
    main()
