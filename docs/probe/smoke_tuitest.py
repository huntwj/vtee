import asyncio, sys, time, traceback
from tui_test import TuiTest

async def one(backend, name):
    async with TuiTest.ephemeral() as t:
        await t.run("sh", "-c", "cat %s.bin; sleep 20" % name, backend=backend, cols=40, rows=12,
                    env={"TERM": "xterm-256color"}, wait_ready=False)
        last, since = None, time.time()
        for _ in range(60):
            txt = await t.text()
            if txt != last:
                last, since = txt, time.time()
            elif time.time() - since > 0.8:
                break
            await asyncio.sleep(0.1)
        st = await t.state()
        cells = await t.cells(0, 0, 40, 12)
        return t, txt, st, cells

async def main():
    for backend in ("xtermjs", "alacritty", "ghostty", "rio"):
        try:
            async with TuiTest.ephemeral() as t:
                await t.run("sh", "-c", "cat T3.bin; sleep 20", backend=backend, cols=40, rows=12,
                            env={"TERM": "xterm-256color"}, wait_ready=False)
                await asyncio.sleep(1.5)
                txt = await t.text()
                st = await t.state()
                cells = await t.cells(0, 0, 4, 1)
                print("BACKEND", backend, "OK text=%r" % txt[:60])
                print("  cell0:", cells[0])
                print("  state.modes:", st.modes, "mouse_mode:", st.mouse_mode, "cursor:", st.cursor, "title:", st.title)
        except Exception as e:
            print("BACKEND", backend, "FAILED:", type(e).__name__, str(e)[:300])

asyncio.run(main())
