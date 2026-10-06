import pyte, re, inspect
data=open("T6.bin","rb").read()
parts=re.split(rb"(?=\x1b\[\d+;\d+H)", data)
def go(read_between, prefill):
    sc=pyte.Screen(40,12); st=pyte.ByteStream(sc)
    if prefill:
        for y in range(12): sc.buffer[y]          # materialize every row up front
    for p in parts:
        st.feed(p)
        if read_between: sc.display
    return [l.rstrip() for l in sc.display[:7]]
for rb in (False, True):
    for pf in (False, True):
        print("read .display between feeds=%-5s  prefill all rows=%-5s -> %s" % (rb, pf, go(rb, pf)))
print(inspect.getsource(pyte.Screen.delete_lines))
