"""Voice clone + 3D (simplify-only) + music reachability through a studio instance.
Usage: python link_test.py STUDIO_URL"""
import base64
import json
import sys
import time
import urllib.request
from pathlib import Path

S = sys.argv[1].rstrip("/")
here = Path(__file__).parent


def call(path, body=None):
    req = urllib.request.Request(S + path, json.dumps(body).encode() if body is not None else None, {"Content-Type": "application/json"})
    try:
        return json.loads(urllib.request.urlopen(req, timeout=60).read())
    except urllib.error.HTTPError as e:
        return json.loads(e.read())


def run(kind, body):
    t0 = time.time()
    job = call("/api/jobs", {"kind": kind, **body})
    if "id" not in job:
        return f"submit failed: {job}"
    while True:
        j = next(x for x in call("/api/jobs") if x["id"] == job["id"])
        if j["status"] in ("done", "error", "cancelled"):
            e = j.get("entry") or {}
            return f"{j['status']} in {time.time() - t0:.1f}s | {e.get('outputs')} {e.get('duration') or ''} | {j.get('error') or ''}"
        time.sleep(1)


cat = call("/api/catalog")
print(S, "| host", call("/api/settings")["host"], "| direct_access:", cat.get("direct_access"))
running = {m["id"] for m in cat["models"] if m["status"] == "running"}
ref = "data:audio/wav;base64," + base64.b64encode((here / "ref-voice.wav").read_bytes()).decode()
if "Qwen/Qwen3-TTS-12Hz-1.7B-Base" in running:
    print("speech:", run("speech", {"model": "Qwen/Qwen3-TTS-12Hz-1.7B-Base", "text": "Testing the voice route on this link.", "ref_audio": ref, "language": "en",
                                    "ref_text": "Hello there. This is a short sample of my voice, recorded so the device can learn how I sound."}))
    print("speech w/o transcript:", run("speech", {"model": "Qwen/Qwen3-TTS-12Hz-1.7B-Base", "text": "x", "ref_audio": ref}))
if "Tencent/Hunyuan3d-2.1" in running:
    img = "data:image/png;base64," + base64.b64encode((here / "mug.png").read_bytes()).decode()
    print("mesh (draft shape + simplify, no texture):", run("mesh", {"model": "Tencent/Hunyuan3d-2.1", "image": img, "output_precision": "draft", "inference_quality": "fast", "faces": 10000}))
music = next((m["id"] for m in cat["models"] if m["type"] == "Music Generation" and m["status"] == "running"), None)
if music:
    print("loop:", run("loop", {"model": music, "prompt": "Marimba, soft, melody, 4 bars, 120 BPM, C major", "seconds_total": 8}))
