"""Check the global device queue and server-side ASR resampling. Usage: python queue_test.py"""
import base64
import json
import time
import urllib.request
from pathlib import Path

S = "http://localhost:7860"
here = Path(__file__).parent


def post(path, body):
    r = urllib.request.Request(S + path, json.dumps(body).encode(), {"Content-Type": "application/json"})
    return json.loads(urllib.request.urlopen(r, timeout=30).read())


song = "data:audio/wav;base64," + base64.b64encode((here / "out-song-vocals.wav").read_bytes()).decode()
a = post("/api/jobs", {"kind": "loop", "model": "RoyalCities/Foundation-1", "prompt": "Rhodes, warm, chord progression, 4 bars, 100 BPM, F major", "seconds_total": 9.6, "seed": 5})
b = post("/api/jobs", {"kind": "transcribe", "model": "Qwen/Qwen3-ASR-1.7B", "audio": song, "language": "en", "name": "song check"})
seen = set()
t0 = time.time()
while True:
    jobs = {j["id"]: j for j in json.loads(urllib.request.urlopen(S + "/api/jobs").read())}
    for jid in (a["id"], b["id"]):
        j = jobs[jid]
        key = (jid, j["status"], j.get("queue_pos"))
        if key not in seen:
            seen.add(key)
            print(f"t={time.time() - t0:5.1f}s {j['kind']:10} {j['status']:8} pos={j.get('queue_pos')} {j.get('message')}")
    if all(jobs[x]["status"] in ("done", "error") for x in (a["id"], b["id"])):
        break
    time.sleep(0.5)
print("transcript:", (jobs[b["id"]].get("entry") or {}).get("text"), jobs[b["id"]].get("error"))
