"""Chat through the studio: a plain reply, an image tool call with a model that fits next to the chat model,
and (with --swap) one that needs the chat model unloaded and reloaded. Test images are deleted afterwards.
Usage: python chat_test.py [--swap] [STUDIO_URL]"""
import json
import sys
import time
import urllib.request

args = [a for a in sys.argv[1:] if not a.startswith("--")]
S = (args[0] if args else "http://localhost:7860").rstrip("/")


def call(path, body=None):
    req = urllib.request.Request(S + path, json.dumps(body).encode() if body is not None else None, {"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return json.loads(r.read())
    except urllib.error.HTTPError as e:
        return json.loads(e.read())


def run(label, req):
    t0 = time.time()
    job = call("/api/jobs", {"kind": "chat", "label": "Chat", **req})
    if "id" not in job:
        print(label, "submit failed:", job); return None
    seen, last = [], None
    while True:
        j = call(f"/api/jobs/{job['id']}")
        msg = j.get("tool_message") or j.get("message")
        if msg != last:
            seen.append(f"{time.time() - t0:.0f}s {msg}"); last = msg
        if j["status"] in ("done", "error", "cancelled"):
            break
        time.sleep(0.5)
    e = j.get("entry") or {}
    print(f"\n=== {label}: {j['status']} in {time.time() - t0:.0f}s {j.get('error') or ''}")
    print("  steps:", " | ".join(seen))
    print("  reply:", (e.get("text") or "")[:300].replace("\n", " "))
    print("  artifacts:", [(a["tool"], a["outputs"], a["prompt"][:60]) for a in e.get("artifacts", [])])
    return e


tools = ["create_image", "edit_image", "speak", "make_loop", "make_song"]
models = {"image": "Tongyi-MAI/Z-Image-Turbo", "edit": "Qwen/Qwen-Image-2.1", "loop": "RoyalCities/Foundation-1"}
made = []
e = run("plain reply", {"model": "default", "messages": [{"role": "user", "content": "In one sentence: what is a lighthouse for?"}], "tools": tools, "tool_models": models})
e = run("image, fits beside chat", {"model": "default", "tools": tools, "tool_models": models, "prefs": {"size": "512x512"},
        "messages": [{"role": "user", "content": "CHAT TEST: make a small picture of a red lighthouse on a cliff at dusk."}]})
made += [a["id"] for a in (e or {}).get("artifacts", [])]
if "--swap" in sys.argv:
    e = run("image with swap (Qwen-Image)", {"model": "default", "tools": tools, "tool_models": {**models, "image": "Qwen/Qwen-Image-2.1"}, "prefs": {"size": "512x512"},
            "messages": [{"role": "user", "content": "CHAT TEST: make a picture of a blue teapot on a wooden table."}]})
    made += [a["id"] for a in (e or {}).get("artifacts", [])]
running = [m["id"] for m in call("/api/catalog")["models"] if m["status"] == "running"]
print("\nloaded now:", running)
if made:
    print("deleted test images:", call("/api/history/delete", {"ids": made}))
