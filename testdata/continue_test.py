"""Preview a song, then continue it to the full song through the studio; deletes both afterwards.
Usage: python continue_test.py [STUDIO_URL]"""
import json
import sys
import time
import urllib.request

S = (sys.argv[1] if len(sys.argv) > 1 else "http://localhost:7860").rstrip("/")


def call(path, body=None, method=None):
    req = urllib.request.Request(S + path, json.dumps(body).encode() if body is not None else None, {"Content-Type": "application/json"}, method=method)
    with urllib.request.urlopen(req, timeout=120) as r:
        return json.loads(r.read())


def wait(jid):
    while True:
        j = next(x for x in call("/api/jobs") if x["id"] == jid)
        if j["status"] in ("done", "error", "cancelled"):
            return j
        time.sleep(3)


cfg = {"model": "tencent/SongGeneration-v2-large", "caption": "F2 TEST: calm lofi beat", "generation_type": "bgm", "seed": 7}
t0 = time.time()
p = wait(call("/api/jobs", {"kind": "song", **cfg, "preview": True})["id"])
pe = p["entry"]
print(f"preview: {p['status']} {time.time() - t0:.0f}s resume={pe.get('resume')}", flush=True)
t1 = time.time()
c = wait(call("/api/jobs", {"kind": "song", **cfg, "resume_session": pe["resume"]["session_id"], "from_entry": pe["id"]})["id"])
ce = c.get("entry") or {}
src_after = next((x for x in call("/api/history") if x["id"] == pe["id"]), {})
print(f"continue: {c['status']} {time.time() - t1:.0f}s duration={ce.get('duration')} route={ce.get('params', {}).get('route')} mp3={ce.get('downloads')} "
      f"| preview's resume token cleared: {'resume' not in src_after} {c.get('error') or ''}", flush=True)
call("/api/history/delete", {"ids": [pe["id"], c["id"]]})
print("test songs deleted")
