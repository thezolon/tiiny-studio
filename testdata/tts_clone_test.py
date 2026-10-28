"""One real voice-clone call against the Tiiny TTS container. Usage: python tts_clone_test.py HOST:PORT"""
import base64
import json
import sys
import time
import urllib.request
from pathlib import Path

here = Path(__file__).parent
ref = base64.b64encode((here / "ref-voice.wav").read_bytes()).decode()
body = {
    "text": "Tiiny Studio can now speak in a cloned voice, generated entirely on the device.",
    "config": {
        "ref_audio": ref,
        "ref_text": "Hello there. This is a short sample of my voice, recorded so the device can learn how I sound.",
        "language": "en",
    },
}
t0 = time.time()
req = urllib.request.Request(f"http://{sys.argv[1]}/v1/synthesize", json.dumps(body).encode(), {"Content-Type": "application/json"})
try:
    with urllib.request.urlopen(req, timeout=300) as r:
        d = json.loads(r.read())
except urllib.error.HTTPError as e:
    d = json.loads(e.read())
took = time.time() - t0
print("success:", d.get("success"), "| error:", d.get("error"), "| duration:", d.get("duration"), "| sr:", d.get("sample_rate"), f"| {took:.1f}s")
print("keys:", sorted(d))
if d.get("audio_data"):
    raw = base64.b64decode(d["audio_data"])
    out = here / "out-clone.wav"
    out.write_bytes(raw)
    print("saved", out, len(raw), "bytes, header", raw[:4])
