import subprocess, json, time, os, select
os.environ["TERM"]="xterm-256color"
open("/tmp/r.sh","w").write("cat /p/T2.bin; sleep 20" + chr(10))
p=subprocess.Popen(["ht","--size","40x12","--subscribe","output,snapshot","--","sh","/tmp/r.sh"],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,text=True)
chunks=[]; end=time.time()+2.0
while time.time()<end:
    r,_,_=select.select([p.stdout],[],[],0.2)
    if r:
        l=p.stdout.readline()
        if not l: break
        try: ev=json.loads(l)
        except Exception: continue
        if ev.get("type")=="output": chunks.append(ev["data"]["seq"])
p.kill()
got="".join(chunks).encode("utf-8", "surrogateescape")
raw=open("/p/T2.bin","rb").read()
print("output events:", len(chunks), "bytes:", len(got), "| source file bytes:", len(raw))
print("equal to file:", got==raw, "| equal after CR/LF normalisation:", got.replace(b"\r\r\n", b"\r\n")==raw.replace(b"\n", b"\r\n") or got.replace(b"\r\n", b"\n")==raw)
print("got head:", got[:80]); print("raw head:", raw[:80])
