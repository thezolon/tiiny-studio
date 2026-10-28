"""Music through the studio on a chosen saved device; test results are deleted afterwards.
Usage: python music_links_test.py DEVICE_NAME [STUDIO_URL]"""
import json
import sys
import time
import urllib.request

NAME = sys.argv[1]
S = (sys.argv[2] if len(sys.argv) > 2 else "http://localhost:7860").rstrip("/")


def call(path, body=None, method=None):
    req = urllib.request.Request(S + path, json.dumps(body).encode() if body is not None else None, {"Content-Type": "application/json"}, method=method)
    try:
        return json.loads(urllib.request.urlopen(req, timeout=60).read())
    except urllib.error.HTTPError as e:
        return json.loads(e.read())


info = call("/api/settings")
dev = next(d for d in info["devices"] if d["name"] == NAME)
call("/api/settings/activate", {"id": dev["id"]})
time.sleep(1)
cat = call("/api/catalog")
print(f"device: {NAME} | direct model ports: {cat['direct_access']}")
lyrics = "[verse]\nCity lights are fading slow\nI keep walking where the rivers go\n\n[chorus]\nHold on, hold on to the night"
jobs = {
    "loop": {"kind": "loop", "model": "RoyalCities/Foundation-1", "prompt": "MUSIC TEST, Rhodes, warm, chord progression, 4 bars, 100 BPM, F major", "seconds_total": 9.6, "seed": 2},
}
if len(sys.argv) > 3 and sys.argv[3] == "vocal":
    jobs["song vocal 15s"] = {"kind": "song", "model": "tencent/SongGeneration-v2-large", "caption": "MUSIC TEST: soft pop, female vocals", "lyrics": lyrics, "generation_type": "vocal", "seconds_total": 15, "seed": 3}
else:
    jobs["song separate 20s"] = {"kind": "song", "model": "tencent/SongGeneration-v2-large", "caption": "MUSIC TEST: upbeat indie pop, female vocals", "lyrics": lyrics, "generation_type": "separate", "seconds_total": 20, "seed": 3}
ids = {call("/api/jobs", body)["id"]: name for name, body in jobs.items()}
t0, done = time.time(), {}
while len(done) < len(ids):
    for j in call("/api/jobs"):
        if j["id"] in ids and j["id"] not in done and j["status"] in ("done", "error", "cancelled"):
            done[j["id"]] = j
            e = j.get("entry") or {}
            print(f"  {ids[j['id']]:18} {j['status']:5} {time.time() - t0:5.0f}s | route={e.get('params', {}).get('route')} duration={e.get('duration')} "
                  f"files={e.get('outputs')} tracks={list((e.get('tracks') or {}).values())} {j.get('error') or ''}", flush=True)
    time.sleep(2)
for jid, j in done.items():
    if j["status"] == "done":
        call("/api/history/" + jid, method="DELETE")
call("/api/settings/auto", {"auto": True})
print("deleted test results; auto-switch back on")
