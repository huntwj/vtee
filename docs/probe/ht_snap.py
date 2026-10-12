import subprocess, json, time, sys, select, os
def snap(name, size="40x12", wait=1.2, extra=()):
    env=dict(os.environ, TERM="xterm-256color")
    open("/tmp/r_%s.sh"%name,"w").write("cat /p/%s.bin; sleep 20" % name + chr(10))
    p=subprocess.Popen(["ht","--size",size,"--subscribe","snapshot",*extra,"--","sh","/tmp/r_%s.sh"%name],
                       stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env, text=True)
    time.sleep(wait)
    p.stdin.write(json.dumps({"type":"takeSnapshot"})+"\n"); p.stdin.flush()
    out=None; end=time.time()+5
    while time.time()<end:
        r,_,_=select.select([p.stdout],[],[],0.3)
        if r:
            line=p.stdout.readline()
            if not line: break
            try: ev=json.loads(line)
            except Exception: continue
            if ev.get("type")=="event" and ev.get("data",{}).get("type")=="snapshot" or ev.get("type")=="snapshot":
                out=ev; break
    p.kill(); err=p.stderr.read()[:300]
    return out, err
if __name__=="__main__":
    for n in sys.argv[1:]:
        o,e=snap(n)
        print("=====",n); print(json.dumps(o)[:900] if o else ("NO SNAPSHOT; stderr=%r"%e))
