import pyte, re
data=open("T6.bin","rb").read()
parts=re.split(rb"(?=\x1b\[\d+;\d+H)", data)
def run(cls, chunked):
    sc=cls(40,12) if cls is pyte.Screen else cls(40,12,history=1000,ratio=0.5)
    st=pyte.ByteStream(sc)
    if chunked:
        for p in parts: st.feed(p)
    else: st.feed(data)
    return [l.rstrip() for l in sc.display[:7]]
for cls in (pyte.Screen, pyte.HistoryScreen):
    for chunked in (True, False):
        print(cls.__name__.ljust(13), "chunked" if chunked else "one-feed", run(cls, chunked))
