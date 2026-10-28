"""Exercise variations, song preview (+MP3, progress), long narration, favorites, smart search,
zip export and bulk delete through the studio. Everything is labelled "F2 TEST" and deleted at the end.
Usage: python features2_test.py [STUDIO_URL]"""
import base64
import io
import json
import sys
import time
import urllib.request
import zipfile
from pathlib import Path

S = (sys.argv[1] if len(sys.argv) > 1 else "http://localhost:7860").rstrip("/")
here = Path(__file__).parent


def call(path, body=None, method=None, raw=False):
    req = urllib.request.Request(S + path, json.dumps(body).encode() if body is not None else None, {"Content-Type": "application/json"}, method=method)
    try:
        with urllib.request.urlopen(req, timeout=120) as r:
            d = r.read()
            return d if raw else json.loads(d)
    except urllib.error.HTTPError as e:
        return json.loads(e.read())


def wait(jid, watch_progress=False):
    seen = set()
    while True:
        j = next(x for x in call("/api/jobs") if x["id"] == jid)
        if watch_progress and j.get("progress"):
            seen.add(j["progress"])
        if j["status"] in ("done", "error", "cancelled"):
            return j, sorted(seen)
        time.sleep(1)


created = []
voice = "data:audio/wav;base64," + base64.b64encode((here / "ref-voice.wav").read_bytes()).decode()

j, _ = wait(call("/api/jobs", {"kind": "image", "model": "Tongyi-MAI/Z-Image-Turbo", "prompt": "F2 TEST: a red fox sleeping in snow", "size": "512x512", "n": 4, "seed": 11})["id"])
print("variations n=4:", j["status"], "->", len((j.get("entry") or {}).get("outputs", [])), "images", j.get("error") or "")
created.append(j["id"])
j2, _ = wait(call("/api/jobs", {"kind": "image", "model": "Tongyi-MAI/Z-Image-Turbo", "prompt": "F2 TEST: a lighthouse on a cliff at night", "size": "512x512", "n": 1, "seed": 12})["id"])
created.append(j2["id"])

j, prog = wait(call("/api/jobs", {"kind": "song", "model": "tencent/SongGeneration-v2-large", "caption": "F2 TEST: calm lofi beat", "generation_type": "bgm", "preview": True, "seed": 5})["id"], True)
e = j.get("entry") or {}
print("song preview:", j["status"], f"duration={e.get('duration')} route={e.get('params', {}).get('route')} resume={'yes' if e.get('resume') else 'no'} mp3={e.get('downloads')} progress seen={prog}", j.get("error") or "")
created.append(j["id"])

text = " ".join(f"F2 TEST sentence number {i}, which is long enough to need several parts when voiced." for i in range(1, 13))
j, prog = wait(call("/api/jobs", {"kind": "speech", "model": "Qwen/Qwen3-TTS-12Hz-1.7B-Base", "text": text, "ref_audio": voice, "language": "en",
                                  "ref_text": "Hello there. This is a short sample of my voice, recorded so the device can learn how I sound."})["id"], True)
e = j.get("entry") or {}
print("long narration:", j["status"], f"{len(text)} chars -> parts={e.get('params', {}).get('parts')} duration={e.get('duration')}s progress seen={prog}", j.get("error") or "")
created.append(j["id"])

print("favorite:", call(f"/api/history/{created[0]}/fav", {"fav": True}), "| saved fav flag:", next(x for x in call("/api/history") if x["id"] == created[0]).get("fav"))
r = call("/api/search", {"query": "lighthouse by the sea"})
ranked = [x["id"] for x in r.get("results", [])]
print("smart search 'lighthouse by the sea': lighthouse ranked", ranked.index(created[1]) + 1 if created[1] in ranked else "missing", "of", len(ranked), "| fox ranked", ranked.index(created[0]) + 1 if created[0] in ranked else "missing")
z = zipfile.ZipFile(io.BytesIO(call("/api/export", {"ids": created[:2]}, raw=True)))
print("zip export:", sorted(n.split("_", 2)[-1] if n != "info.json" else n for n in z.namelist()))
print("bulk delete:", call("/api/history/delete", {"ids": created}), "| left:", sum(1 for x in call("/api/history") if x["id"] in created))
