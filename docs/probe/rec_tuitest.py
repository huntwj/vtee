import asyncio
from tui_test import TuiTest
async def main():
    async with TuiTest.ephemeral() as t:
        await t.run("sh","-c","cat T1.bin; sleep 3", backend="xtermjs", cols=40, rows=12, env={"TERM":"xterm-256color"}, wait_ready=False)
        await t.start_recording("/p/rec_test.cast", format="cast")
        await t.write("")
        await asyncio.sleep(1.0)
        await t.type("zz")
        await asyncio.sleep(1.0)
        r = await t.stop_recording()
        print("stop_recording returned:", r)
asyncio.run(main())
