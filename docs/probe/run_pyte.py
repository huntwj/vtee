"""pyte adapter: (a) feed the .bin bytes straight in, (b) run `sh -c 'cat X.bin; sleep 20'` under a stdlib pty and feed what comes out."""
import fcntl, json, os, pty, select, struct, sys, termios, time
import pyte
from probe_eval import Snap, judge, TESTS, COLS, ROWS


def snap_from_screen(sc):
    def cell(r, c):
        ch = sc.buffer[r][c]
        return {"char": ch.data, "fg": ch.fg, "bg": ch.bg, "bold": ch.bold, "dim": False, "italic": ch.italics,
                "underline": ch.underscore, "blink": ch.blink, "reverse": ch.reverse, "strike": ch.strikethrough}
    modes = set()
    for m in sc.mode:
        modes.add("?%d" % (m >> 5) if m >= 32 else str(m))
    hist = None
    if hasattr(sc, "history"):
        hist = ["".join(l[c].data for c in range(COLS) if c in l) if hasattr(l, "keys") else str(l) for l in sc.history.top]
    return Snap(lines=list(sc.display), cell=cell,
                cursor={"row": sc.cursor.y, "col": sc.cursor.x, "hidden": sc.cursor.hidden, "shape": None},
                title=sc.title or None, modes=modes, links=None, scrollback=hist)


def via_direct(data):
    sc = pyte.HistoryScreen(COLS, ROWS, history=1000, ratio=0.5)
    pyte.ByteStream(sc).feed(data)
    return snap_from_screen(sc), data


def via_pty(path):
    pid, fd = pty.fork()
    if pid == 0:
        os.environ["TERM"] = "xterm-256color"
        os.execvp("sh", ["sh", "-c", "cat %s; sleep 20" % path])
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
    buf, last = b"", time.time()
    while time.time() - last < 0.6:
        r, _, _ = select.select([fd], [], [], 0.2)
        if r:
            try:
                d = os.read(fd, 65536)
            except OSError:
                break
            if not d:
                break
            buf += d
            last = time.time()
    os.kill(pid, 9)
    os.waitpid(pid, 0)
    sc = pyte.HistoryScreen(COLS, ROWS, history=1000, ratio=0.5)
    pyte.ByteStream(sc).feed(buf)
    return snap_from_screen(sc), buf


out = {}
for mode in ("direct", "pty"):
    for t in TESTS:
        data = open("%s.bin" % t, "rb").read()
        snap, raw = via_direct(data) if mode == "direct" else via_pty("%s.bin" % t)
        v, ev = judge(t, snap)
        out.setdefault(t, {})[mode] = [v, ev]
        print("%-4s %-6s %-12s %s" % (t, mode, v, ev[:200]))
json.dump(out, open("results_pyte.json", "w"), indent=1, ensure_ascii=False)
