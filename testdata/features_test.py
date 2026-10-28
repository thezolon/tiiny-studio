"""Queue the less-tested features through a studio instance and report results.
Usage: python features_test.py STUDIO_URL DATA_IMAGES_DIR multiref|mesh-high|song-ref [...]"""
import base64
import json
import sys
import time
import urllib.request
from pathlib import Path

S, IMG = sys.argv[1].rstrip("/"), Path(sys.argv[2])
here = Path(__file__).parent


def call(path, body=None):
    req = urllib.request.Request(S + path, json.dumps(body).encode() if body is not None else None, {"Content-Type": "application/json"})
    try:
        return json.loads(urllib.request.urlopen(req, timeout=60).read())
    except urllib.error.HTTPError as e:
        return json.loads(e.read())


def uri(path, mime):
    return f"data:{mime};base64," + base64.b64encode(Path(path).read_bytes()).decode()


history = call("/api/history")
pick = lambda words: next(IMG / e["outputs"][0] for e in history if e.get("kind", "image") == "image" and all(w in e["params"].get("prompt", "") for w in words))
jobs = {}
for what in sys.argv[3:]:
    if what == "multiref":
        robot, lighthouse = pick(["brass robot", "cozy"]), pick(["lighthouse"])
        jobs[what] = call("/api/jobs", {"kind": "image", "model": "Qwen/Qwen-Image-2.1", "size": "768x768", "n": 1, "seed": 21,
                                        "prompt": "Place the brass robot from the first image on the cliff next to the lighthouse in the second image, in the same oil-painting style",
                                        "images": [uri(robot, "image/png"), uri(lighthouse, "image/png")]})
    elif what == "mesh-high":
        jobs[what] = call("/api/jobs", {"kind": "mesh", "model": "Tencent/Hunyuan3d-2.1", "image": uri(here / "mug.png", "image/png"),
                                        "output_precision": "high", "inference_quality": "balance", "faces": 40000})
    elif what == "song-ref":
        ref = next(IMG / e["outputs"][0] for e in history if e.get("kind") in ("song", "loop") and (e.get("duration") or 0) >= 10)  # needs >= 10 s
        jobs[what] = call("/api/jobs", {"kind": "song", "model": "tencent/SongGeneration-v2-large", "caption": "dreamy instrumental, soft synths",
                                        "generation_type": "bgm", "seconds_total": 20, "seed": 9, "reference_audio": uri(ref, "audio/wav")})
for what, j in jobs.items():
    print("submitted", what, j.get("id"), j.get("error"))
t0 = time.time()
pending = {j["id"]: w for w, j in jobs.items() if "id" in j}
while pending:
    for j in call("/api/jobs"):
        if j["id"] in pending and j["status"] in ("done", "error", "cancelled"):
            e = j.get("entry") or {}
            print(f"{pending.pop(j['id'])}: {j['status']} after {time.time() - t0:.0f}s (ran {e.get('seconds')}s) -> {e.get('outputs')} {j.get('error') or ''}", flush=True)
    time.sleep(3)
