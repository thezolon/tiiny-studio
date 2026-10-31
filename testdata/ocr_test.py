"""Load every installed OCR (Image-to-Text) model and read ocr-test.png with each through the studio. Reads are
ephemeral (nothing is saved to the gallery); models this script loaded are unloaded at the end.
Usage: python ocr_test.py [STUDIO_URL]"""
import base64
import difflib
import json
import sys
import time
import urllib.request
from pathlib import Path

S = (sys.argv[1] if len(sys.argv) > 1 else "http://localhost:7860").rstrip("/")
here = Path(__file__).parent
EXPECTED = """Tiiny Studio OCR Test
The quick brown fox jumps over the lazy dog.
Invoice #4821 - Total: $1,234.56 - Due 2026-11-15
Phone: (555) 010-7788 Email: hello@example.com
def add(a, b): return a + b # Python code line
Line six: 3 apples, 12 oranges, 0.5 kg grapes"""


def call(path, body=None):
    req = urllib.request.Request(S + path, json.dumps(body).encode() if body is not None else None, {"Content-Type": "application/json"})
    try:
        return json.loads(urllib.request.urlopen(req, timeout=120).read())
    except urllib.error.HTTPError as e:
        return json.loads(e.read())


ocr = [m for m in call("/api/catalog")["models"] if m["type"] == "Image-to-Text" and "DocLayout" not in m["id"]]  # layout only, no text
print("OCR models:", [(m["id"], m["status"], f"{m.get('npu_usage')}% NPU") for m in ocr])
loaded = [m["id"] for m in ocr if m["status"] != "running"]
for mid in loaded:
    call("/api/model/launch", {"id": mid})
for _ in range(60):
    st = {m["id"]: m["status"] for m in call("/api/catalog")["models"] if m["id"] in {o["id"] for o in ocr}}
    if all(v == "running" for v in st.values()):
        break
    time.sleep(3)
print("status:", st)
img = "data:image/png;base64," + base64.b64encode((here / "ocr-test.png").read_bytes()).decode()
norm = lambda t: " ".join(t.split())
for m in ocr:
    t0 = time.time()
    job = call("/api/jobs", {"kind": "ocr", "model": m["id"], "image": img, "ephemeral": True})
    while True:
        j = next(x for x in call("/api/jobs") if x["id"] == job["id"])
        if j["status"] in ("done", "error"):
            break
        time.sleep(0.5)
    e = j.get("entry") or {}
    text = e.get("text", "")
    score = difflib.SequenceMatcher(None, norm(EXPECTED), norm(text)).ratio()
    print(f"\n=== {m['id']}: {j['status']} in {time.time() - t0:.1f}s | {len(e.get('lines', []))} lines | match {score:.1%} {j.get('error') or ''}")
    print(text)
for mid in loaded:
    call("/api/model/stop", {"id": mid})
print("unloaded:", loaded)
