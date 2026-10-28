"""Run the vault (knowledge base) flow once with a labelled test file and delete it afterwards:
upload, index, search, rename, download, delete. Only the test file's entries and results are printed
(the user's own vault is never listed), and searches run with include_memory_context off.
Loads the embedding model if needed and unloads it again if it did.
Usage: python kb_test.py [DEVICE_URL]"""
import json
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

cfg = json.load(open(os.path.expanduser("~/.local/tiiny/config.json")))
B = (sys.argv[1] if len(sys.argv) > 1 else "http://" + cfg["deviceAddress"]).rstrip("/")
KEY = cfg["account"]["authKey"]
NAME = "KB TEST tiiny studio.txt"
FACT = "KB TEST: the lighthouse keeper's cat is named Biscuit-7731 and sleeps on the third step."
DATA = (FACT + "\nThis file was uploaded by Tiiny Studio's vault test and is deleted right after.\n").encode()


def call(method, path, body=None, host=True, raw=False, ctype="application/json", timeout=120):
    h = {"Authorization": f"Bearer {KEY}", "Accept": "application/json"}
    if host:
        h["Host"] = "kb.tiiny.local"
    if body is not None and not isinstance(body, bytes):
        body = json.dumps(body).encode()
    if body is not None:
        h["Content-Type"] = ctype
    req = urllib.request.Request(B + path, body, h, method=method)
    t0 = time.time()
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            data = r.read()
            return r.status, (data if raw else json.loads(data or b"{}")), time.time() - t0
    except urllib.error.HTTPError as e:
        return e.code, e.read()[:300], time.time() - t0


def multipart(fields, fname, data):
    b = uuid.uuid4().hex
    out = b""
    for k, v in fields.items():
        out += f'--{b}\r\nContent-Disposition: form-data; name="{k}"\r\n\r\n{v}\r\n'.encode()
    out += f'--{b}\r\nContent-Disposition: form-data; name="file"; filename="{fname}"\r\nContent-Type: text/plain\r\n\r\n'.encode() + data + b"\r\n"
    return out + f"--{b}--\r\n".encode(), f"multipart/form-data; boundary={b}"


def mine(name):
    code, res, _ = call("GET", "/kb/sources?limit=500")
    items = res.get("items", []) if isinstance(res, dict) else []
    return next((i for i in items if i.get("filename") == name or i.get("name") == name), None), (res.get("total") if isinstance(res, dict) else None)


def model_call(method, path, body=None):
    req = urllib.request.Request(B + path, json.dumps(body).encode() if body is not None else None,
                                 {"Authorization": f"Bearer {KEY}", "Content-Type": "application/json"}, method=method)
    with urllib.request.urlopen(req, timeout=120) as r:
        return json.loads(r.read() or b"{}")


# 0. embedding model
models = model_call("GET", "/api/v1/models/?including_downloading=false")
models = models.get("models") or models.get("data") or models
emb = next((m for m in models if "Embedding" in (m.get("type") or "") or "embedding" in (m.get("id") or m.get("model_id") or "").lower()), None)
emb_id = emb and (emb.get("id") or emb.get("model_id"))
loaded_here = False
if emb_id and emb.get("status") != "running":
    print("loading", emb_id)
    model_call("POST", f"/api/v1/models/{urllib.parse.quote(emb_id, safe='')}/launch", {"model_id": emb_id})
    loaded_here = True
    for _ in range(60):
        run = model_call("GET", "/api/v1/models/running")
        if any(r.get("model_id") == emb_id for r in (run.get("instances") or {}).get("running", [])):
            break
        time.sleep(3)
print("embedding model:", emb_id, "| loaded for this test:", loaded_here)

try:
    # 1. upload (does the Host header matter? try without first)
    body, ctype = multipart({}, NAME, DATA)
    code, res, dt = call("POST", "/kb/upload", body, host=False, ctype=ctype)
    print(f"\n1 upload without Host header: {code} {dt:.1f}s {json.dumps(res)[:300] if isinstance(res, dict) else res}")
    if code != 200:
        code, res, dt = call("POST", "/kb/upload", body, ctype=ctype)
        print(f"  upload with Host: kb.tiiny.local: {code} {dt:.1f}s {json.dumps(res)[:300] if isinstance(res, dict) else res}")
    item, total = mine(NAME)
    print(f"2 sources: found test file: {bool(item)} | fields: {sorted(item.keys()) if item else None} | is_indexed: {item and item.get('is_indexed')}")
    sid = item and (item.get("source_id") or item.get("id"))
    print("  source_id:", sid)
    # 3. index
    code, res, dt = call("POST", "/kb/finalize", {"source_id": sid}, timeout=600)
    print(f"3 finalize: {code} {dt:.1f}s {json.dumps(res)[:300] if isinstance(res, dict) else res}")
    for _ in range(40):
        item, _ = mine(NAME)
        if item and item.get("is_indexed"):
            break
        time.sleep(3)
    print("  is_indexed now:", item and item.get("is_indexed"))
    # 4. search (memory context off so the user's own notes stay out of it)
    q = {"question": "What is the lighthouse keeper's cat called?", "include_memory_context": False, "return_memory_context": False}
    code, res, dt = call("POST", "/kb/retrieve", q, timeout=300)
    text = json.dumps(res) if isinstance(res, dict) else str(res)
    print(f"4 retrieve: {code} {dt:.1f}s | response keys: {sorted(res.keys()) if isinstance(res, dict) else None} | finds Biscuit-7731: {'Biscuit-7731' in text}")
    if isinstance(res, dict):  # show only parts that come from the test file
        for k, v in res.items():
            if isinstance(v, str) and "Biscuit" in v:
                print(f"  {k}: {v[:300]!r}")
            elif isinstance(v, list):
                ours = [x for x in v if "Biscuit" in json.dumps(x)]
                if ours:
                    print(f"  {k}[] item from the test file: {json.dumps(ours[0])[:400]}")
    # 5. rename
    new = "KB TEST renamed.txt"
    code, res, dt = call("PATCH", f"/kb/sources/{sid}", {"filename": new})
    item2, _ = mine(new)
    print(f"5 rename: {code} {json.dumps(res)[:200] if isinstance(res, dict) else res} | listed under new name: {bool(item2)}")
    # 6. download
    code, res, dt = call("GET", f"/kb/files/{sid}", raw=True)
    print(f"6 download: {code} {len(res) if isinstance(res, bytes) else res} bytes | identical to upload: {res == DATA}")
finally:
    # 7. delete
    item, _ = mine("KB TEST renamed.txt")
    item = item or mine(NAME)[0]
    if item:
        sid = item.get("source_id") or item.get("id")
        code, res, dt = call("POST", "/kb/delete-memory", {"source_id": sid})
        print(f"7 delete-memory: {code} {json.dumps(res)[:200] if isinstance(res, dict) else res}")
    gone = not mine("KB TEST renamed.txt")[0] and not mine(NAME)[0]
    code, res, _ = call("POST", "/kb/retrieve", {"question": "What is the lighthouse keeper's cat called?", "include_memory_context": False}, timeout=300)
    print(f"  gone from sources: {gone} | still found by search: {'Biscuit-7731' in json.dumps(res) if isinstance(res, dict) else '?'}")
    if loaded_here:
        model_call("POST", f"/api/v1/models/{urllib.parse.quote(emb_id, safe='')}/stop")
        print("unloaded", emb_id)
