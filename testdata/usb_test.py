"""End-to-end check of a studio instance (default: native run on :7862 over the USB link).
Usage: python usb_test.py [STUDIO_URL]"""
import json
import sys
import time
import urllib.request

S = sys.argv[1] if len(sys.argv) > 1 else "http://localhost:7862"


def call(path, body=None):
    req = urllib.request.Request(S + path, json.dumps(body).encode() if body is not None else None,
                                 {"Content-Type": "application/json"})
    return json.loads(urllib.request.urlopen(req, timeout=60).read())


print("settings:", {k: v for k, v in call("/api/settings").items() if k != "key_hint"})
print("test:", call("/api/settings/test", {})["message"])
cat = call("/api/catalog")
running = [m["id"] for m in cat["models"] if m["status"] == "running"]
print("catalog:", len(cat["models"]), "installed; running:", running, "| npu", cat["npu"])
img = next((m for m in running if "Image" in m), None)
if img:
    t0 = time.time()
    job = call("/api/jobs", {"kind": "image", "model": img, "prompt": "a paper boat on a puddle, macro photo", "size": "512x512", "n": 1, "seed": 7})
    while True:
        j = next(x for x in call("/api/jobs") if x["id"] == job["id"])
        if j["status"] in ("done", "error", "cancelled"):
            break
        time.sleep(1)
    print(f"image job over this link: {j['status']} in {time.time() - t0:.1f}s ->", (j.get("entry") or {}).get("outputs"), j.get("error"))
