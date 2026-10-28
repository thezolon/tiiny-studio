"""Smoke-test the studio's /api/jobs for speech, transcription and a music loop. Usage: python jobs_test.py"""
import base64
import json
import time
import urllib.request
from pathlib import Path

S = "http://localhost:7860"
here = Path(__file__).parent


def post(path, body):
    r = urllib.request.Request(S + path, json.dumps(body).encode(), {"Content-Type": "application/json"})
    try:
        return json.loads(urllib.request.urlopen(r, timeout=30).read())
    except urllib.error.HTTPError as e:
        return json.loads(e.read())


ref = "data:audio/wav;base64," + base64.b64encode((here / "ref-voice.wav").read_bytes()).decode()
jobs = [
    post("/api/jobs", {"kind": "speech", "model": "Qwen/Qwen3-TTS-12Hz-1.7B-Base", "text": "Job system check, one two three.",
                       "ref_audio": ref, "language": "en",
                       "ref_text": "Hello there. This is a short sample of my voice, recorded so the device can learn how I sound."}),
    post("/api/jobs", {"kind": "transcribe", "model": "Qwen/Qwen3-ASR-1.7B", "audio": ref}),
    post("/api/jobs", {"kind": "loop", "model": "RoyalCities/Foundation-1", "prompt": "Vibraphone, soft, airy, melody, 4 bars, 120 BPM, C major",
                       "seconds_total": 8, "seed": 3}),
]
print("submitted:", [(j.get("id"), j.get("status"), j.get("error")) for j in jobs])
for _ in range(90):
    jobs_now = json.loads(urllib.request.urlopen(S + "/api/jobs").read())
    if all(j["status"] in ("done", "error", "cancelled") for j in jobs_now):
        break
    time.sleep(2)
for j in jobs_now:
    e = j.get("entry") or {}
    print(f"{j['kind']:10} {j['status']:6} err={j.get('error')} | {e.get('seconds')}s | {e.get('outputs')} | {(e.get('text') or '')[:90]}")
