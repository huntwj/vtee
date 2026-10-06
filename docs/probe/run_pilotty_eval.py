import json
from probe_eval import Snap, judge, TESTS, ROWS
out = {}
for t in TESTS:
    d = json.load(open("pt_%s.json" % t, encoding="utf-8"))
    lines = d["text"].split(chr(10)); lines += [""] * (ROWS - len(lines))
    vis = d["cursor"].get("visible", True)
    ro = open("pt_out_%s.txt" % t, encoding="utf-8", errors="replace").read().split(chr(10))
    ro = [x.rstrip() for x in ro]
    while ro and not ro[-1]: ro.pop()
    sb = ro[:max(0, len(ro) - ROWS)] if t == "T14" else None
    sn = Snap(lines=lines, cell=None, cursor={"row": d["cursor"]["row"], "col": d["cursor"]["col"], "hidden": (not vis) or None, "shape": None},
              title=None, modes=None, links=None, scrollback=sb)
    v, ev = judge(t, sn)
    out[t] = [v, ev]
    print("%-4s %-12s %s" % (t, v, ev[:200]))
json.dump(out, open("results_pilotty.json", "w", encoding="utf-8"), indent=1, ensure_ascii=False)
