"""Run the Hunyuan3D stage chain once: simplify -> texture -> export, starting from out-shape.glb.
Usage: python pipeline3d_test.py HOST:PORT   (logs every stage's request/response shape)"""
import base64
import json
import sys
import time
import urllib.request
import uuid
from pathlib import Path

HOST = sys.argv[1] if len(sys.argv) > 1 else "192.168.1.50:9091"  # device IP:Hunyuan3D port
here = Path(__file__).parent


def multipart(fields, files):
    b = uuid.uuid4().hex
    out = []
    for k, v in fields.items():
        out += [f"--{b}", f'Content-Disposition: form-data; name="{k}"', "", v]
    body = "\r\n".join(out).encode() + b"\r\n"
    for k, (name, data, ctype) in files.items():
        body += f'--{b}\r\nContent-Disposition: form-data; name="{k}"; filename="{name}"\r\nContent-Type: {ctype}\r\n\r\n'.encode() + data + b"\r\n"
    body += f"--{b}--\r\n".encode()
    return body, f"multipart/form-data; boundary={b}"


def call(method, path, body=None, ctype=None):
    req = urllib.request.Request(f"http://{HOST}{path}", body, method=method)
    if ctype:
        req.add_header("Content-Type", ctype)
    try:
        with urllib.request.urlopen(req, timeout=600) as r:
            return r.status, json.loads(r.read())
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read() or b"{}")


def summarize(o, depth=0):
    if isinstance(o, dict):
        return {k: (f"<{len(v)} chars>" if isinstance(v, str) and len(v) > 200 else summarize(v, depth + 1)) for k, v in o.items()}
    if isinstance(o, list):
        return [summarize(x, depth + 1) for x in o[:5]]
    return o


def run(stage, params, files):
    body, ctype = multipart({"stage": stage, "params": json.dumps(params)}, files)
    code, res = call("POST", "/v1/tasks", body, ctype)
    print(f"\n### {stage}: submit {code} {json.dumps(summarize(res))[:400]}", flush=True)
    if code >= 300:
        return None
    tid = res["data"]["task_id"]
    t0 = time.time()
    last = ""
    while True:
        _, st = call("GET", f"/v1/tasks/{tid}")
        d = st["data"]
        msg = f"{d['status']} {d.get('progress')} {d.get('message') or ''}"
        if msg != last and int(time.time() - t0) % 30 < 5:
            print(f"   t={time.time() - t0:.0f}s {msg}", flush=True)
            last = msg
        if d["status"] in ("completed", "failed", "cancelled", "error"):
            break
        time.sleep(5)
    print(f"   {stage} done in {time.time() - t0:.0f}s status={d['status']}", flush=True)
    print("   result:", json.dumps(summarize(d.get("result") or d.get("error")), indent=1)[:2500], flush=True)
    return d


if __name__ == "__main__":
    glb = (here / "out-shape.glb").read_bytes()
    img = (here / "mug.png").read_bytes()

    s = run("simplify", {"target_faces": 40000}, {"mesh": ("shape.glb", glb, "model/gltf-binary")})
    if s and s["status"] == "completed":
        out = s["result"]["output"]
        glb = base64.b64decode(out.get("glb") or out.get("mesh") or "") or glb
        (here / "out-simplified.glb").write_bytes(glb)
        print("   saved out-simplified.glb", len(glb), flush=True)

    t = run("texture", {"inference_quality": "fast"}, {"image": ("mug.png", img, "image/png"), "mesh": ("mesh.glb", glb, "model/gltf-binary")})
    if t and t["status"] == "completed":
        out = t["result"]["output"]
        for k, v in out.items():
            if isinstance(v, str) and len(v) > 1000:
                ext = "glb" if "glb" in k else "bin"
                (here / f"out-texture-{k}.{ext}").write_bytes(base64.b64decode(v))
                print(f"   saved out-texture-{k}.{ext}", flush=True)
    print("\nALL DONE", flush=True)
