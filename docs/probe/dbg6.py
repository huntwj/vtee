import pyte
E=bytes([27])
def cup(r,c): return E+b"[%d;%dH"%(r,c)
sc=pyte.Screen(40,12); st=pyte.ByteStream(sc)
def show(tag): print(tag, [l.rstrip() for l in sc.display[:8]], "cursor", (sc.cursor.y, sc.cursor.x))
st.feed(E+b"[2J"+E+b"[H"+cup(4,1)+b"L4"+cup(5,1)+b"L5"+cup(6,1)+b"L6"); show("init  ")
st.feed(cup(5,1)+E+b"[L"); show("after IL at row5")
st.feed(cup(4,1)); print("cursor after CUP(4,1):", (sc.cursor.y, sc.cursor.x))
st.feed(E+b"[M"); show("after DL at row4")
# DL alone on fresh screen
sc2=pyte.Screen(40,12); st2=pyte.ByteStream(sc2)
st2.feed(E+b"[2J"+E+b"[H"+cup(4,1)+b"L4"+cup(5,1)+b"L5"+cup(6,1)+b"L6"+cup(4,1)+E+b"[M")
print("DL alone:", [l.rstrip() for l in sc2.display[:8]])
