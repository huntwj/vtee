import pyte, re
data=open("T6.bin","rb").read()
print(repr(data))
sc=pyte.Screen(40,12); st=pyte.ByteStream(sc)
parts=re.split(rb"(?=\x1b\[\d+;\d+H)", data)
for p in parts:
    st.feed(p)
    print(repr(p)[:60].ljust(62), [l.rstrip() for l in sc.display[:7]], (sc.cursor.y,sc.cursor.x))
