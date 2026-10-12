"""ht adapter. ht gives `text` (plain rows) and `seq` (escape sequences that redraw the screen). Per-cell attributes can only be
recovered by interpreting `seq` ourselves; this does that minimally (text runs, REP, CR/LF, SGR, CSI/C1-CSI cursor moves)."""
import json, re, sys
from ht_snap import snap
from probe_eval import Snap, judge, TESTS, COLS, ROWS

CSI = re.compile(r"(?:\x1b\[|\x9b)([0-9:;?<=> ]*)([@-~])")


def sgr_apply(state, params):
    toks = params.split(";") if params else ["0"]
    i = 0
    while i < len(toks):
        t = toks[i]
        parts = t.split(":")
        n = int(parts[0] or 0)
        if n == 0:
            state.clear()
        elif n in (1, 2, 3, 4, 5, 7, 9):
            state[{1: "bold", 2: "dim", 3: "italic", 4: "underline", 5: "blink", 7: "reverse", 9: "strike"}[n]] = True
        elif n in (38, 48):
            key = "fg" if n == 38 else "bg"
            if len(parts) > 1:                       # colon form: 38:5:N or 38:2::r:g:b
                if parts[1] == "5":
                    state[key] = int(parts[2])
                elif parts[1] == "2":
                    rgb = [int(x) for x in parts[-3:]]
                    state[key] = tuple(rgb)
            else:                                    # semicolon form
                if toks[i + 1] == "5":
                    state[key] = int(toks[i + 2]); i += 2
                elif toks[i + 1] == "2":
                    state[key] = (int(toks[i + 2]), int(toks[i + 3]), int(toks[i + 4])); i += 4
        elif n == 39:
            state.pop("fg", None)
        elif n == 49:
            state.pop("bg", None)
        i += 1


def parse_seq(seq):
    grid, r, c, last, st = {}, 0, 0, " ", {}
    pos, modes, shape = 0, set(), None
    while pos < len(seq):
        m = CSI.match(seq, pos)
        if m:
            p, f = m.group(1), m.group(2)
            if f == "m":
                sgr_apply(st, p)
            elif f == "b":
                for _ in range(int(p or 1)):
                    grid[(r, c)] = (last, dict(st)); c += 1
            elif f in ("h", "l") and p.startswith("?"):
                for x in p[1:].split(";"):
                    (modes.add if f == "h" else modes.discard)(x)
                    if x == "25":
                        shape = shape  # visibility handled via modes
            elif f == "q" and p.endswith(" "):
                shape = p.strip()
            elif f == "H":
                a = p.split(";"); r = int(a[0] or 1) - 1; c = int((a[1] if len(a) > 1 else "1") or 1) - 1
            pos = m.end()
            continue
        ch = seq[pos]
        pos += 1
        if ch == "\r":
            c = 0
        elif ch == "\n":
            r += 1
        elif ch == "\x1b" or ch == "\x9b" or ord(ch) < 32:
            continue
        else:
            grid[(r, c)] = (ch, dict(st)); last = ch; c += 1
    return grid, modes, shape


def build(name):
    ev, err = snap(name)
    if not ev:
        raise RuntimeError("no snapshot: %r" % err)
    d = ev["data"]
    seq = d["seq"]
    grid, modes, shape = parse_seq(seq)
    lines = d["text"].split("\n")
    lines += [""] * (ROWS - len(lines))

    def cell(r, c):
        ch, st = grid.get((r, c), (" ", {}))
        return {"char": ch, "fg": st.get("fg"), "bg": st.get("bg"), "bold": st.get("bold", False), "dim": st.get("dim", False),
                "italic": st.get("italic", False), "underline": st.get("underline", False), "blink": st.get("blink", False),
                "reverse": st.get("reverse", False), "strike": st.get("strike", False)}
    hidden = "25" not in modes and "?25l" in seq.replace("\x9b", "\x1b[") or None
    hid = True if "?25l" in seq.replace("\x9b", "\x1b[") else None
    return Snap(lines=lines, cell=cell, cursor={"row": None, "col": None, "hidden": hid, "shape": shape},
                title=("MYTITLE" if "MYTITLE" in seq else None) if False else None,   # no title field in a snapshot
                modes=modes if modes else None, links=None, scrollback=None, raw=seq)


out = {}
for t in TESTS:
    try:
        sn = build(t)
        if t == 'T8':
            sn.cell = None   # adapter does not model double-width cells; judge on ht's own text rows
        v, ev = judge(t, sn)
        if t == "T11":
            ev += " | seq contains title? %s, OSC8? %s" % ("MYTITLE" in sn.raw, "example.com" in sn.raw)
    except Exception as e:
        v, ev = "ERROR", "%s: %s" % (type(e).__name__, str(e)[:200])
    out[t] = [v, ev]
    print("%-4s %-12s %s" % (t, v, ev[:230]), flush=True)
json.dump(out, open("results_ht.json", "w"), indent=1, ensure_ascii=False)
