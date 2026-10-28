"""Chat tools other than images: speak (with the synthetic test voice, not the user's voice library),
make_loop and make_song. Everything is labelled CHAT TEST and deleted afterwards.
Usage: python chat_tools_test.py [STUDIO_URL]"""
import base64
import json
import sys
import time
import urllib.request
from pathlib import Path

S = (sys.argv[1] if len(sys.argv) > 1 else "http://localhost:7860").rstrip("/")
here = Path(__file__).parent


def call(path, body=None):
    req = urllib.request.Request(S + path, json.dumps(body).encode() if body is not None else None, {"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.loads(r.read())


catalog = call("/api/catalog")["models"]
pick = lambda t, rx="": next((m["id"] for m in catalog if m["type"] == t and rx.lower() in m["id"].lower()), None)
models = {"speech": pick("Text-to-Speech"), "loop": pick("Music Generation", "foundation"), "song": pick("Music Generation", "song")}
voice = {"name": "test voice", "uri": "data:audio/wav;base64," + base64.b64encode((here / "ref-voice.wav").read_bytes()).decode(),
         "text": "Hello there. This is a short sample of my voice, recorded so the device can learn how I sound."}
before = [m["id"] for m in catalog if m["status"] == "running"]
print("tool models:", models, "| loaded before:", before)
made = []
for label, prompt in [("speak", "CHAT TEST: read this aloud: Good morning, the studio is ready."),
                      ("loop", "CHAT TEST: make a mellow electric piano loop at 110 BPM."),
                      ("song", "CHAT TEST: make a short instrumental lo-fi song preview, no vocals.")]:
    t0 = time.time()
    job = call("/api/jobs", {"kind": "chat", "label": "Chat", "model": "default", "tools": ["speak", "make_loop", "make_song"], "tool_models": models,
                             "voice": voice, "messages": [{"role": "user", "content": prompt}]})
    steps, last = [], None
    while True:
        j = call(f"/api/jobs/{job['id']}")
        msg = j.get("tool_message") or j.get("message")
        if msg != last:
            steps.append(f"{time.time() - t0:.0f}s {msg}"); last = msg
        if j["status"] in ("done", "error", "cancelled"):
            break
        time.sleep(0.7)
    e = j.get("entry") or {}
    arts = e.get("artifacts", [])
    made += [a["id"] for a in arts]
    print(f"\n=== {label}: {j['status']} in {time.time() - t0:.0f}s {j.get('error') or ''}")
    print("  steps:", " | ".join(steps))
    print("  reply:", (e.get("text") or "")[:200].replace("\n", " "))
    print("  artifacts:", [(a["tool"], a["outputs"], a.get("duration"), a["prompt"][:50]) for a in arts])
    for a in arts:
        for o in a["outputs"]:
            r = urllib.request.urlopen(S + "/images/" + o, timeout=30).read()
            print(f"  file {o}: {len(r) // 1024} KB, starts {r[:4]!r}")
after = [m["id"] for m in call("/api/catalog")["models"] if m["status"] == "running"]
print("\nloaded after:", after)
if made:
    print("deleted test results:", call("/api/history/delete", {"ids": made}))
