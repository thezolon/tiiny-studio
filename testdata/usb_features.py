"""Run one job of every kind through a studio instance and report, then delete the test results.
Usage: python usb_features.py [STUDIO_URL]   (labelled "USB TEST"; nothing of the user's is read)"""
import base64
import json
import sys
import time
import urllib.request
from pathlib import Path

S = (sys.argv[1] if len(sys.argv) > 1 else "http://localhost:7860").rstrip("/")
here = Path(__file__).parent


def call(path, body=None, method=None):
    req = urllib.request.Request(S + path, json.dumps(body).encode() if body is not None else None, {"Content-Type": "application/json"}, method=method)
    try:
        return json.loads(urllib.request.urlopen(req, timeout=60).read())
    except urllib.error.HTTPError as e:
        return json.loads(e.read())


uri = lambda p, mime: f"data:{mime};base64," + base64.b64encode(Path(p).read_bytes()).decode()
running = {m["id"]: m for m in call("/api/catalog")["models"] if m["status"] == "running"}
pick = lambda t: next((i for i, m in running.items() if m["type"] == t), None)
voice = uri(here / "ref-voice.wav", "audio/wav")
ref_text = "Hello there. This is a short sample of my voice, recorded so the device can learn how I sound."
jobs = [
    ("image", {"model": pick("Text-to-Image"), "prompt": "USB TEST: a red apple on a table", "size": "512x512", "n": 1, "seed": 3}),
    ("speech", {"model": pick("Text-to-Speech"), "text": "USB TEST. Speaking over the cable.", "ref_audio": voice, "ref_text": ref_text, "language": "en"}),
    ("transcribe", {"model": pick("ASR"), "audio": voice, "name": "USB TEST"}),
    ("loop", {"model": pick("Music Generation"), "prompt": "USB TEST, Marimba, soft, melody, 4 bars, 120 BPM, C major", "seconds_total": 8}),
    ("mesh", {"model": pick("Image-to-3D"), "image": uri(here / "mug.png", "image/png"), "output_precision": "draft", "inference_quality": "fast", "label": "USB TEST 3D"}),
]
print("studio device:", call("/api/settings")["name"])
ids = {}
for kind, body in jobs:
    if not body.get("model"):
        print(f"{kind:10} skipped (no model of that type loaded)")
        continue
    ids[call("/api/jobs", {"kind": kind, **body})["id"]] = kind
t0 = time.time()
done = {}
while len(done) < len(ids):
    for j in call("/api/jobs"):
        if j["id"] in ids and j["id"] not in done and j["status"] in ("done", "error", "cancelled"):
            done[j["id"]] = j
            e = j.get("entry") or {}
            info = e.get("outputs") or (e.get("text", "")[:50] if e.get("text") else "")
            print(f"{ids[j['id']]:10} {j['status']:5} after {time.time() - t0:5.0f}s (ran {e.get('seconds')}s) {info} {j.get('error') or ''}", flush=True)
    time.sleep(2)
for jid, j in done.items():
    if j["status"] == "done":
        call("/api/history/" + jid, method="DELETE")
print("test results deleted")
