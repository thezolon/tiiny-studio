"""Check Hunyuan3D through the gateway (/v1/3d/*), e.g. over the USB link where model ports are blocked.
Usage: python gateway3d_test.py DEVICE_URL KEY GLB_PATH"""
import json
import sys
import time
import urllib.request
import uuid

base, key, glb = sys.argv[1].rstrip("/") + "/v1/3d", sys.argv[2], open(sys.argv[3], "rb").read()


def call(method, path, body=None, ctype=None):
    req = urllib.request.Request(base + path, body, method=method)
    req.add_header("Authorization", f"Bearer {key}")
    if ctype:
        req.add_header("Content-Type", ctype)
    try:
        with urllib.request.urlopen(req, timeout=120) as r:
            return r.status, json.loads(r.read())
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read() or b"{}")


code, caps = call("GET", "/capabilities")
print("capabilities:", code, caps.get("data", {}).get("stages") if code == 200 else caps)
b = uuid.uuid4().hex
body = (f'--{b}\r\nContent-Disposition: form-data; name="stage"\r\n\r\nsimplify\r\n'
        f'--{b}\r\nContent-Disposition: form-data; name="params"\r\n\r\n{{"target_faces": 5000}}\r\n'
        f'--{b}\r\nContent-Disposition: form-data; name="mesh"; filename="m.glb"\r\nContent-Type: model/gltf-binary\r\n\r\n').encode() + glb + f"\r\n--{b}--\r\n".encode()
code, res = call("POST", "/tasks", body, f"multipart/form-data; boundary={b}")
print("submit:", code, json.dumps(res)[:200])
if code < 300:
    tid = res["data"]["task_id"]
    for _ in range(60):
        code, st = call("GET", f"/tasks/{tid}")
        d = st.get("data") or {}
        if d.get("status") in ("completed", "failed", "error", "cancelled") or code >= 400:
            break
        time.sleep(1)
    out = ((d.get("result") or {}).get("output") or {})
    print("poll:", code, d.get("status"), "| output keys:", sorted(out), "| faces:", (out.get("metadata") or {}).get("output_faces"))
    print("cancel route:", call("POST", f"/tasks/{tid}/cancel", b"")[0])
