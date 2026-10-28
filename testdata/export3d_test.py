"""Run the Hunyuan3D export stage from saved texture + mesh artifacts. Usage: python export3d_test.py HOST:PORT"""
import base64
import json
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from pipeline3d_test import call, multipart, summarize  # noqa: E402  (reuses helpers; HOST from argv)

here = Path(__file__).parent
files = {"material": ("hunyuan3d_material.zip", (here / "out-texture-artifact.bin").read_bytes(), "application/zip"),
         "mesh": ("simplified.glb", (here / "out-simplified.glb").read_bytes(), "model/gltf-binary")}
body, ctype = multipart({"stage": "export", "params": "{}"}, files)
code, res = call("POST", "/v1/tasks", body, ctype)
print("submit", code, json.dumps(summarize(res))[:400], flush=True)
tid = res["data"]["task_id"]
t0 = time.time()
while True:
    _, st = call("GET", f"/v1/tasks/{tid}")
    d = st["data"]
    if d["status"] in ("completed", "failed", "cancelled", "error"):
        break
    time.sleep(3)
print(f"export {d['status']} in {time.time() - t0:.0f}s", flush=True)
out = (d.get("result") or {}).get("output") or {}
print(json.dumps(summarize(out), indent=1)[:1800])
print("error:", d.get("error"))
for k in ("glb", "artifact"):
    if isinstance(out.get(k), str):
        (here / "out-textured.glb").write_bytes(base64.b64decode(out[k]))
        print("saved out-textured.glb from", k)
