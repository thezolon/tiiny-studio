"""The Model Store end to end through a running studio and a real Tiiny: download, pause, resume, cancel, download
again, delete, and the refusals in between. Uses one small model that is NOT installed (you pick it) and always
removes it again at the end; with --load-check it also loads it once to see that a loaded model can't be deleted.
The Tiiny downloads over its own Wi-Fi. Run it only when nothing else is using the Tiiny.

    python testdata/model_store_test.py --model tencent/R3-embedding-0.6b [--load-check] [STUDIO_URL]

Prints PASS/FAIL per step; exit code 1 if any step failed. Never touches any other model."""
import argparse
import json
import sys
import time
import urllib.error
import urllib.request

RESULTS = []


def call(base, path, body=None, timeout=180):
    req = urllib.request.Request(base + path, json.dumps(body).encode() if body is not None else None, {"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.status, json.loads(r.read() or b"{}")
    except urllib.error.HTTPError as e:
        try:
            return e.code, json.loads(e.read() or b"{}")
        except ValueError:
            return e.code, {}


def check(what, ok, detail=""):
    RESULTS.append(bool(ok))
    print(f"{'PASS' if ok else 'FAIL'}  {what}{'' if ok else f'  ({detail})'}", flush=True)
    return ok


def entry(base, mid):
    code, j = call(base, "/api/store")
    return next((m for m in j.get("models", []) if m["id"] == mid), None) if code == 200 else None


def in_catalog(base, mid):
    return any(m["id"] == mid for m in call(base, "/api/catalog")[1].get("models", []))


def wait(base, mid, cond, what, timeout):
    t0 = time.time()
    while time.time() - t0 < timeout:
        m = entry(base, mid)
        if m and cond(m):
            return m
        time.sleep(2)
    return None


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("studio", nargs="?", default="http://localhost:7860")
    ap.add_argument("--model", required=True, help="a small model that isn't installed")
    ap.add_argument("--allow-big", action="store_true", help="allow a model over 2 GB")
    ap.add_argument("--load-check", action="store_true", help="also load it once: delete must be refused while loaded")
    a = ap.parse_args()
    base, mid = a.studio.rstrip("/"), a.model
    m = entry(base, mid)
    if not m:
        sys.exit(f"{mid} isn't in the store (or the studio can't reach the Tiiny).")
    if m["status"] != "not_downloaded":
        sys.exit(f"{mid} is already on the Tiiny ({m['status']}); pick one that isn't installed.")
    size = (m.get("size") or 0) / 1e9
    if size > 2 and not a.allow_big:
        sys.exit(f"{mid} is {size:.1f} GB; pick a small model (or --allow-big).")
    store = call(base, "/api/store")[1]
    other = next((x["id"] for x in store["models"] if x["status"] == "not_downloaded" and x["id"] != mid), None)
    print(f"model {mid} ({size:.2f} GB); free {store.get('disk')}", flush=True)
    loaded = False
    try:
        # refusals that never reach the device's delete
        check("delete without confirm is refused (400)", call(base, "/api/model/delete", {"id": mid})[0] == 400)
        check("delete of a model that isn't there is refused (404)", call(base, "/api/model/delete", {"id": mid, "confirm": mid})[0] == 404)
        check("a bad id is refused (400)", call(base, "/api/model/download", {"id": "../x/y"})[0] == 400)

        code, j = call(base, "/api/model/download", {"id": mid})
        check("download starts", code == 200, j)
        if other:
            code, j = call(base, "/api/model/download", {"id": other})
            check("a second download is refused (one at a time)", code == 409, j)
        got = wait(base, mid, lambda x: x["status"] == "downloading" and (x.get("progress") or 0) > 0, "progress", 60)
        check("progress shows", got, entry(base, mid))
        check("a model mid-download isn't loadable (not in the catalog)", not in_catalog(base, mid))
        check("delete while downloading is refused (409)", call(base, "/api/model/delete", {"id": mid, "confirm": mid})[0] == 409)

        code, j = call(base, "/api/model/pause", {"id": mid})
        check("pause answers", code == 200, j)
        paused = wait(base, mid, lambda x: x["status"] == "paused", "paused", 30)
        check("status: paused", paused, entry(base, mid))
        p1 = (paused or {}).get("progress") or 0
        time.sleep(6)
        p2 = (entry(base, mid) or {}).get("progress") or 0
        check("a paused download doesn't move", abs(p2 - p1) < 0.5, f"{p1} -> {p2}")
        check("paused: still not in the catalog", not in_catalog(base, mid))

        code, j = call(base, "/api/model/download", {"id": mid})
        check("resume answers", code == 200, j)
        resumed = wait(base, mid, lambda x: x["status"] in ("downloading", "downloaded"), "resumed", 30)
        check("resumed from where it paused (not from 0)", resumed and ((resumed.get("progress") or 100) >= p1 - 0.5), resumed)

        if (entry(base, mid) or {}).get("status") == "downloading":
            code, j = call(base, "/api/model/cancel", {"id": mid})
            check("cancel answers", code == 200, j)
            gone = wait(base, mid, lambda x: x["status"] == "not_downloaded", "gone", 30)
            check("cancelled: nothing left", gone, entry(base, mid))
        else:
            print("SKIP  cancel: the download finished first (use a bigger model)", flush=True)
            call(base, "/api/model/delete", {"id": mid, "confirm": mid})

        code, j = call(base, "/api/model/download", {"id": mid})
        check("download again", code == 200, j)
        done = wait(base, mid, lambda x: x["installed"], "installed", max(300, int(size * 1000 / 5)))  # 5 MB/s at worst
        check("downloaded and installed", done, entry(base, mid))
        check("now in the catalog", in_catalog(base, mid))
        check("installed: download again is refused (409)", call(base, "/api/model/download", {"id": mid})[0] == 409)
        check("no update waiting: update is refused (409)", call(base, "/api/model/update", {"id": mid, "confirm": mid})[0] == 409)

        if a.load_check:
            call(base, "/api/model/launch", {"id": mid})
            loaded = True
            t0 = time.time()
            while time.time() - t0 < 180 and not any(x["id"] == mid and x["status"] == "running" for x in call(base, "/api/catalog")[1].get("models", [])):
                time.sleep(3)
            code, j = call(base, "/api/model/delete", {"id": mid, "confirm": mid})
            check("a loaded model can't be deleted (409)", code == 409, j)
            call(base, "/api/model/stop", {"id": mid})
            loaded = False
            time.sleep(3)

        code, j = call(base, "/api/model/delete", {"id": mid, "confirm": mid})
        check("delete", code == 200, j)
        check("deleted: gone from the store and the catalog", (entry(base, mid) or {}).get("status") == "not_downloaded" and not in_catalog(base, mid))
    finally:
        if loaded:
            call(base, "/api/model/stop", {"id": mid})
            time.sleep(3)
        left = entry(base, mid)
        if left and left["status"] in ("downloading", "paused", "error"):
            call(base, "/api/model/cancel", {"id": mid})
        elif left and left["installed"]:
            call(base, "/api/model/delete", {"id": mid, "confirm": mid})
        left = entry(base, mid)
        print(f"cleanup: {mid} is {left and left['status']}", flush=True)
    print(f"\n{sum(RESULTS)} of {len(RESULTS)} passed")
    sys.exit(0 if all(RESULTS) else 1)


if __name__ == "__main__":
    main()
