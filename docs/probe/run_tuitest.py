"""tui-test adapter: for each emulator backend and each probe, run `sh -c 'cat X.bin; sleep 20'` at 40x12, TERM=xterm-256color."""
import asyncio, json, sys, time
from tui_test import TuiTest
from probe_eval import Snap, judge, TESTS, COLS, ROWS

BACKENDS = sys.argv[1:] or ["xtermjs", "alacritty", "ghostty", "rio"]
DETAIL = {}  # raw things worth showing in the report


async def harvest(name, backend):
    async with TuiTest.ephemeral() as t:
        await t.run("sh", "-c", "cat %s.bin; sleep 20" % name, backend=backend, cols=COLS, rows=ROWS,
                    env={"TERM": "xterm-256color"}, wait_ready=False)
        last, since = None, time.time()
        for _ in range(80):
            txt = await t.text()
            if txt != last:
                last, since = txt, time.time()
            elif time.time() - since > 0.8:
                break
            await asyncio.sleep(0.1)
        st = await t.state()
        cells = await t.cells(0, 0, COLS, ROWS)
        full = await t.text(full=True)
        grid = {(c.y, c.x): c for c in cells}
        lines = ["".join(grid[(r, c)].char for c in range(COLS) if (r, c) in grid) for r in range(ROWS)]

        def cell(r, c):
            x = grid.get((r, c))
            if x is None:
                return None
            return {"char": x.char, "fg": x.fg, "bg": x.bg, "bold": x.bold, "dim": x.dim, "italic": x.italic,
                    "underline": x.underline, "blink": x.blink, "reverse": x.inverse, "strike": x.strike}
        modes = {k for k, v in st.modes.items() if v}
        modes |= {"mouse:%s" % st.mouse_mode} if st.mouse_mode != "none" else set()
        # translate this tool's mode names to the probe's vocabulary
        tr = {"bracketed_paste": "2004", "application_cursor_keys": "1", "application_keypad": "keypad"}
        modes |= {tr[m] for m in list(modes) if m in tr}
        if st.mouse_mode != "none":
            modes.add("1000")
        links = sorted({c.link for c in cells if c.link})
        sb_lines = [l.rstrip() for l in full.split("\n")]
        while sb_lines and not sb_lines[-1]:
            sb_lines.pop()
        scrollback = sb_lines[:max(0, len(sb_lines) - ROWS)]
        DETAIL[(backend, name)] = {"modes": st.modes, "mouse_mode": st.mouse_mode, "cursor": st.cursor, "title": st.title,
                                   "links": links, "full_len": len(sb_lines)}
        return Snap(lines=lines, cell=cell, cursor={"row": st.cursor["y"], "col": st.cursor["x"],
                    "hidden": not st.cursor["visible"], "shape": st.cursor["shape"]},
                    title=st.title, modes=modes, links=links, scrollback=scrollback)


async def main():
    out = {}
    for b in BACKENDS:
        for t in TESTS:
            try:
                snap = await harvest(t, b)
                v, ev = judge(t, snap)
            except Exception as e:
                v, ev = "ERROR", "%s: %s" % (type(e).__name__, str(e)[:200])
            out.setdefault(t, {})[b] = [v, ev]
            print("%-4s %-9s %-12s %s" % (t, b, v, ev[:190]), flush=True)
    json.dump(out, open("results_tuitest.json", "w"), indent=1, ensure_ascii=False)
    for k in (("xtermjs", "T10"), ("xtermjs", "T12"), ("alacritty", "T10"), ("ghostty", "T10")):
        print("DETAIL", k, DETAIL.get(k))

asyncio.run(main())
