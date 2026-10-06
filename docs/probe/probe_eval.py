"""Shared evaluator for the terminal-emulator probe. An adapter turns a tool's state after consuming PROBE <name>.bin
into a Snap; judge() turns a Snap into (verdict, evidence). Verdicts: PASS / PARTIAL / FAIL / NOT EXPOSED."""
from dataclasses import dataclass, field

ROWS, COLS = 12, 40
ATTRS = ["bold", "dim", "italic", "underline", "blink", "reverse", "strike"]


@dataclass
class Snap:
    lines: list                      # ROWS strings (plain text of each row, trailing blanks trimmed or not)
    cell: object = None              # callable (row, col) -> dict(char, fg, bg, bold, dim, ...) or None if not exposed
    cursor: dict = None              # {"row","col","hidden"?,"shape"?}
    title: object = None             # str, or None if not exposed
    modes: object = None             # set/list of mode names or numbers, or None if not exposed
    links: object = None             # hyperlink info or None if not exposed
    scrollback: object = None        # list of str, or None if not exposed
    raw: object = None               # anything useful as evidence (e.g. raw sequences)


def row(snap, r):
    return snap.lines[r].rstrip() if r < len(snap.lines) else ""


def norm_color(v):
    """Return a comparable form of a color value from whichever tool: ('idx', n) / ('rgb', 'rrggbb') / ('default',) / ('other', repr)."""
    if v is None:
        return ("default",)
    if isinstance(v, int):
        return ("idx", v)
    if isinstance(v, (list, tuple)) and len(v) == 3:
        return ("rgb", "%02x%02x%02x" % tuple(v))
    if isinstance(v, dict):
        if "rgb" in v:
            return norm_color(v["rgb"])
        if "index" in v:
            return ("idx", v["index"])
    s = str(v).strip().lower().lstrip("#")
    if s in ("default", "", "none"):
        return ("default",)
    if len(s) == 6 and all(c in "0123456789abcdef" for c in s):
        return ("rgb", s)
    if s.isdigit():
        return ("idx", int(s))
    return ("other", s)


def color_is(v, idx, rgb):
    n = norm_color(v)
    return n == ("idx", idx) or n == ("rgb", rgb)


def judge(name, s):
    L = lambda r: row(s, r)
    cell = s.cell
    if name == "T1":
        ok = L(2)[4:9] == "HELLO" and L(5)[9:10] == "X"
        return ("PASS" if ok else "FAIL"), "row3=%r row6=%r" % (L(2), L(5))
    if name == "T2":
        if cell is None:
            return "NOT EXPOSED", "no per-cell attributes; text rows=%r" % [L(i) for i in range(7)]
        got = {}
        for i, a in enumerate(ATTRS):
            c = cell(i, 0)
            got[a] = bool(c and c.get(a))
        good = [a for a, v in got.items() if v]
        bad = [a for a, v in got.items() if not v]
        v = "PASS" if not bad else ("FAIL" if not good else "PARTIAL")
        return v, "reported=%s missing=%s" % (good, bad)
    if name == "T3":
        if cell is None:
            return "NOT EXPOSED", "no per-cell color"
        c = cell(0, 0) or {}
        fg, bg = c.get("fg"), c.get("bg")
        ok = color_is(fg, 196, "ff0000") and color_is(bg, 21, "0000ff")
        part = norm_color(fg) != ("default",) or norm_color(bg) != ("default",)
        return ("PASS" if ok else "PARTIAL" if part else "FAIL"), "fg=%r bg=%r" % (fg, bg)
    if name == "T4":
        if cell is None:
            return "NOT EXPOSED", "no per-cell color"
        c = cell(0, 0) or {}
        fg, bg = c.get("fg"), c.get("bg")
        ok = norm_color(fg) == ("rgb", "0ac81e") and norm_color(bg) == ("rgb", "010203")
        part = norm_color(fg) != ("default",) or norm_color(bg) != ("default",)
        return ("PASS" if ok else "PARTIAL" if part else "FAIL"), "fg=%r bg=%r" % (fg, bg)
    if name == "T5":
        exp = ["R01", "R02", "R03", "R06", "R07", "R08", "", "", "R09", "R10", "R11", "R12"]
        got = [L(i) for i in range(12)]
        return ("PASS" if got == exp else "FAIL"), "rows=%r" % got
    if name == "T6":
        exp = {0: "AB  CDEF", 1: "ABEF", 2: "ABXYCDEF", 3: "", 4: "L5", 5: "L6", 6: ""}
        bad = {r: L(r) for r, e in exp.items() if L(r) != e}
        names = {0: "ICH", 1: "DCH", 2: "IRM", 3: "IL/DL", 4: "IL/DL", 5: "IL/DL", 6: "IL/DL"}
        failed = sorted({names[r] for r in bad})
        v = "PASS" if not bad else ("FAIL" if len(failed) >= 3 else "PARTIAL")
        return v, ("all ok" if not bad else "wrong: %r (failed ops: %s)" % (bad, failed))
    if name == "T7a":
        txt = "\n".join(s.lines)
        ok = "MAIN" in txt and "ALT" not in txt
        return ("PASS" if ok else "FAIL"), "after leaving alt: MAIN %s, ALT %s (row1=%r)" % (
            "present" if "MAIN" in txt else "absent", "present" if "ALT" in txt else "absent", L(0))
    if name == "T7b":
        txt = "\n".join(s.lines)
        ok = "ALT" in txt and "MAIN" not in txt
        return ("PASS" if ok else "FAIL"), "while in alt: MAIN %s, ALT %s" % (
            "present" if "MAIN" in txt else "absent", "present" if "ALT" in txt else "absent")
    if name == "T8":
        parts = {}
        if cell:
            parts["cjk"] = (cell(0, 0) or {}).get("char") == "你" and (cell(0, 4) or {}).get("char") == "X"
            parts["emoji"] = (cell(1, 0) or {}).get("char") == "😀" and (cell(1, 2) or {}).get("char") == "Y"
            parts["combining"] = (cell(2, 0) or {}).get("char") in ("e" + chr(0x301), chr(0xe9)) and (cell(2, 1) or {}).get("char") == "Z"
        else:
            parts["cjk"] = "你好X" in L(0)
            parts["emoji"] = "😀Y" in L(1)
            parts["combining"] = L(2).startswith(("e" + chr(0x301) + "Z", chr(0xe9) + "Z"))
        ok = all(parts.values())
        return ("PASS" if ok else "PARTIAL" if any(parts.values()) else "FAIL"), "%s rows=%r" % (parts, [L(0), L(1), L(2)])
    if name == "T9":
        want = chr(0x250c) + chr(0x2500) * 2 + chr(0x2510)
        return ("PASS" if L(0)[:4] == want else "FAIL"), "row1=%r (want %r)" % (L(0), want)
    if name == "T10":
        if s.modes is None:
            return "NOT EXPOSED", "tool exposes no mode state"
        m = {str(x) for x in s.modes}
        want = {"mouse1000": ("1000", "?1000", "MOUSE_1000"), "mouse1006": ("1006", "?1006"), "bpaste2004": ("2004", "?2004"),
                "decckm1": ("1", "?1", "DECCKM"), "deckpam": ("DECKPAM", "=", "keypad")}
        found = {k: any(any(w == x or w in x for x in m) for w in ws) for k, ws in want.items()}
        n = sum(found.values())
        return ("PASS" if n == 5 else "PARTIAL" if n else "FAIL"), "modes=%r found=%s" % (sorted(m)[:20], found)
    if name == "T11":
        title_ok = s.title == "MYTITLE"
        link_ok = s.links is not None and "example.com" in str(s.links)
        parts = {"title": ("ok" if title_ok else ("NOT EXPOSED" if s.title is None else "wrong=%r" % s.title)),
                 "osc8": ("ok" if link_ok else ("NOT EXPOSED" if s.links is None else "exposed but empty/wrong=%r" % (s.links,)))}
        v = "PASS" if title_ok and link_ok else "PARTIAL" if title_ok or link_ok else (
            "NOT EXPOSED" if s.title is None and s.links is None else "FAIL")
        return v, "%s text=%r" % (parts, L(0))
    if name == "T12":
        c = s.cursor or {}
        hid, shape = c.get("hidden"), c.get("shape")
        parts = {"hidden": hid, "shape": shape}
        if hid is None and shape is None:
            return "NOT EXPOSED", "cursor=%r" % c
        ok_h, ok_s = hid is True, shape not in (None, "default", "block")   # block is the default shape
        return ("PASS" if ok_h and ok_s else "PARTIAL" if ok_h or ok_s else "FAIL"), "cursor=%r" % c
    if name == "T13":
        exp = {0: ("tab", lambda x: x[8:9] == "b" and x[16:17] == "c"), 1: ("bs", lambda x: x == "ac"),
               2: ("crlf", lambda x: x == "line1"), 3: ("cr", lambda x: x == "ZZne2")}
        res = {n: f(L(r)) for r, (n, f) in exp.items()}
        return ("PASS" if all(res.values()) else "PARTIAL" if any(res.values()) else "FAIL"), "%s rows=%r" % (res, [L(i) for i in range(4)])
    if name == "T14":
        vis = [L(i) for i in range(12)]
        vis_ok = vis == ["L%02d" % i for i in range(39, 51)]
        if s.scrollback is None:
            return "NOT EXPOSED", "visible_ok=%s, no scrollback API; visible=%r..%r" % (vis_ok, vis[0], vis[-1])
        sb = [x.rstrip() for x in s.scrollback]
        have = [x for x in sb if x.startswith("L")]
        full = have[:38] == ["L%02d" % i for i in range(1, 39)]
        return ("PASS" if vis_ok and full else "PARTIAL"), "visible_ok=%s scrollback_lines=%d first=%r last=%r" % (
            vis_ok, len(have), have[:1], have[-1:])
    return "FAIL", "unknown test"


TESTS = ["T1", "T2", "T3", "T4", "T5", "T6", "T7a", "T7b", "T8", "T9", "T10", "T11", "T12", "T13", "T14"]
