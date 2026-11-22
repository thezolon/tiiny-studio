"""What do the Tiiny's download controls really do? Pause, resume, cancel, and the shapes the studio reads.

    python testdata/download_probe.py --read-only            # GETs only: download rows, storage, store fields
    python testdata/download_probe.py --model <id>           # also start, pause, resume and cancel a real download

Talks to the device directly (TIINY_URL / TIINY_KEY, else the studio's data/config.json), not to a studio. The
download test needs a small model that is NOT installed (it refuses one over 2 GB unless --allow-big) and ALWAYS
cleans up: whatever ends up on the device for that model is deleted at the end. The Tiiny downloads over its own
Wi-Fi. Run it only when nothing else is using the Tiiny (one session at a time). Prints shapes, statuses and
messages, never the key; the full log goes to testdata/out-download-probe-<time>.json (git-ignored)."""
import argparse
import json
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from fake_vs_real import device, shape

LOG = []


def call(base, key, method, path, body=None, timeout=60):
    data = None if body is None else (body if isinstance(body, bytes) else json.dumps(body).encode())
    req = urllib.request.Request(base + path, data, method=method, headers={"Authorization": f"Bearer {key}"})
    if data:
        req.add_header("Content-Type", "application/json")
    t0 = time.time()
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            code, raw = r.status, r.read()
    except urllib.error.HTTPError as e:
        code, raw = e.code, e.read()
    except (urllib.error.URLError, OSError) as e:
        code, raw = 0, str(e).encode()
    try:
        res = json.loads(raw or b"{}")
    except ValueError:
        res = raw[:300].decode("utf-8", "replace")
    LOG.append({"t": round(t0, 1), "method": method, "path": path, "body": body if not isinstance(body, bytes) else None,
                "code": code, "answer": res if len(json.dumps(res)) < 4000 else shape(res)})
    return code, res


def rows(base, key):
    code, res = call(base, key, "GET", "/api/v1/models/?including_downloading=true", timeout=15)
    return res.get("data", []) if code == 200 and isinstance(res, dict) else []


def row(base, key, mid):
    r = next((m for m in rows(base, key) if m.get("id") == mid), None)
    return {k: r.get(k) for k in ("status", "progress", "need_update", "version", "size")} if r else None


def read_only(base, key):
    print("== read-only")
    all_rows = rows(base, key)
    statuses = sorted({str(m.get("status")) for m in all_rows})
    print(f"including_downloading=true: {len(all_rows)} rows, statuses {statuses}")
    print("  row fields:", json.dumps(shape(all_rows[:1])))
    for m in all_rows:
        if m.get("status") not in ("downloaded", "running", "stopped"):
            print("  not downloaded:", m.get("id"), {k: m.get(k) for k in ("status", "progress", "size")})
    code, false = call(base, key, "GET", "/api/v1/models/?including_downloading=false", timeout=15)
    if code == 200:
        print(f"including_downloading=false: {len(false.get('data', []))} rows, statuses "
              f"{sorted({str(m.get('status')) for m in false.get('data', [])})}")
    for p in ("/api/v1/models/storage", "/api/v1/sys/storage", "/api/v1/sys/storage/llm"):
        code, res = call(base, key, "GET", p, timeout=15)
        print(f"{p}: HTTP {code}", json.dumps(shape(res))[:600])
    code, store = call(base, key, "GET", "/api/v1/models/online_models", timeout=30)
    store = store if isinstance(store, list) else (store or {}).get("data", []) if isinstance(store, dict) else []
    print(f"online_models: {len(store)} models, statuses {sorted({str(m.get('status')) for m in store})}, "
          f"need_update {sum(bool(m.get('need_update')) for m in store)}")
    return {m.get("model_id"): m for m in store}


def watch(base, key, mid, secs, what):
    seen = []
    t0 = time.time()
    while time.time() - t0 < secs:
        r = row(base, key, mid)
        if not seen or seen[-1][1] != r:
            seen.append((round(time.time() - t0), r))
            print(f"  {what} +{seen[-1][0]:3d} s: {r}", flush=True)
        if r and r.get("status") == "downloaded":
            break
        time.sleep(3)
    return seen[-1][1] if seen else None


def try_post(base, key, path):
    """Empty body first (like download), then {"model_id"} if the device asks for a body."""
    code, res = call(base, key, "POST", path, b"")
    if code in (400, 422):
        code, res = call(base, key, "POST", path, {"model_id": path.split("/api/v1/models/", 1)[1].rsplit("/", 1)[0]})
    print(f"POST {path}: HTTP {code} {json.dumps(res)[:300]}", flush=True)
    return code, res


def download_test(base, key, mid, store, allow_big):
    s = store.get(mid)
    if not s:
        sys.exit(f"{mid} isn't in the store.")
    size = max((t.get("size") or 0 for t in s.get("tags") or []), default=0) / 1e9
    if row(base, key, mid):
        sys.exit(f"{mid} is already on the Tiiny ({row(base, key, mid)}); pick one that isn't installed.")
    if size > 2 and not allow_big:
        sys.exit(f"{mid} is {size:.1f} GB; pick a small model (or --allow-big).")
    print(f"== download test with {mid} ({size:.2f} GB)")
    try:
        code, res = call(base, key, "POST", f"/api/v1/models/{mid}/download", b"")
        print(f"start: HTTP {code} {json.dumps(res)[:200]}")
        if code >= 300:
            return
        # short watches: a small model finishes in under a minute on a good Wi-Fi (0.76 GB took ~25 s on 2026-11-21)
        watch(base, key, mid, 6, "downloading")
        try_post(base, key, f"/api/v1/models/{mid}/pause_download")
        paused = watch(base, key, mid, 6, "after pause")
        print("  PAUSE", "WORKS" if paused and paused.get("status") not in ("downloading", "downloaded") else "doesn't stop it")
        code, res = call(base, key, "POST", f"/api/v1/models/{mid}/download", b"")
        print(f"resume (download again): HTTP {code} {json.dumps(res)[:200]}")
        watch(base, key, mid, 6, "after resume")
        # cancel = pause, then DELETE the partial download (/interrupt only stops chats: 2026-11-21)
        before = row(base, key, mid)
        if not before or before.get("status") == "downloaded":
            print("  CANCEL not tested: the download finished first (use a bigger model)")
            return
        try_post(base, key, f"/api/v1/models/{mid}/pause_download")
        code, res = call(base, key, "DELETE", f"/api/v1/models/{mid}", timeout=120)
        print(f"DELETE while paused: HTTP {code} {json.dumps(res)[:200]}")
        after = watch(base, key, mid, 10, "after delete")
        print("  CANCEL (pause + DELETE)", "WORKS: nothing left" if not after else f"leaves {after}")
    finally:
        left = row(base, key, mid)
        if left:
            if left.get("status") == "downloading":
                call(base, key, "POST", f"/api/v1/models/{mid}/pause_download", b"")
            code, res = call(base, key, "DELETE", f"/api/v1/models/{mid}", timeout=120)
            print(f"cleanup: DELETE {mid}: HTTP {code} {json.dumps(res)[:200]}; now {row(base, key, mid)}")
        else:
            print(f"cleanup: nothing of {mid} left on the Tiiny")


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--read-only", action="store_true")
    ap.add_argument("--model")
    ap.add_argument("--allow-big", action="store_true")
    a = ap.parse_args()
    if not (a.read_only or a.model):
        ap.error("use --read-only, or --model <a small model that isn't installed>")
    base, key = device()
    try:
        store = read_only(base, key)
        if a.model:
            download_test(base, key, a.model, store, a.allow_big)
    finally:
        out = Path(__file__).with_name(f"out-download-probe-{time.strftime('%Y%m%d-%H%M%S')}.json")
        out.write_text(json.dumps(LOG, indent=1), "utf-8")
        print("log:", out)


if __name__ == "__main__":
    main()
