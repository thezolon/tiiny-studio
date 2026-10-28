"""Compare the fake Tiiny's response *shapes* (keys and value types) with the real device's, for the GET
endpoints the studio reads. Prints differences only; no values, no key. Read-only on the device.
Usage: python testdata/fake_vs_real.py   (device from TIINY_URL/TIINY_KEY or the studio's data/config.json)"""
import json
import os
import sys
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "testdata"))
import fake_tiiny  # noqa: E402

PATHS = ["/v1/models", "/api/v1/models/?including_downloading=false", "/api/v1/models/running", "/api/v1/models/online_models",
         "/api/v1/npu/status", "/api/v1/sys/status", "/api/v1/sys/device_info", "/api/v1/sys/ac_power/"]


def device():
    url, key = os.environ.get("TIINY_URL"), os.environ.get("TIINY_KEY")
    if not url:
        c = json.loads((ROOT / "data" / "config.json").read_text("utf-8"))
        d = next((d for d in c["devices"] if d["id"] == c.get("active")), c["devices"][0])
        url, key = d["url"], d.get("key")
    return url.rstrip("/"), key


def get(base, key, path):
    req = urllib.request.Request(base + path, headers={"Authorization": f"Bearer {key}"})
    try:
        with urllib.request.urlopen(req, timeout=15) as r:
            return r.status, json.loads(r.read() or b"{}")
    except urllib.error.HTTPError as e:
        return e.code, None


def shape(v):
    if isinstance(v, dict):
        return {k: shape(x) for k, x in v.items()}
    if isinstance(v, list):
        merged = {}
        for x in v:  # union of the items' keys, so one odd item doesn't hide fields
            s = shape(x)
            if isinstance(s, dict) and isinstance(merged, dict):
                for k, t in s.items():
                    merged.setdefault(k, t)
            else:
                merged = s
        return [merged] if v else []
    return type(v).__name__ if v is not None else "null"


def diff(real, fake, where=""):
    out = []
    if isinstance(real, dict) and isinstance(fake, dict):
        for k in sorted(set(real) | set(fake)):
            if k not in fake:
                out.append(f"  only real: {where}.{k} ({json.dumps(real[k])[:60]})")
            elif k not in real:
                out.append(f"  only fake: {where}.{k}")
            else:
                out += diff(real[k], fake[k], f"{where}.{k}")
    elif isinstance(real, list) and isinstance(fake, list):
        if real and fake:
            out += diff(real[0], fake[0], where + "[]")
    elif real != fake and not (isinstance(real, str) and isinstance(fake, str) and ("null" in (real, fake) or {real, fake} == {"int", "float"})):
        out.append(f"  type: {where} real={real} fake={fake}")
    return out


def main():
    url, key = device()
    server, fake_url = fake_tiiny.start(key="fake-key")
    try:
        for path in PATHS:
            rc, real = get(url, key, path)
            fc, fake = get(fake_url, "fake-key", path)
            lines = diff(shape(real), shape(fake)) if rc == 200 and fc == 200 else [f"  status real={rc} fake={fc}"]
            # extra fields on the real device are expected (the fake is minimal); what matters is fields the fake
            # invents and values of a different type
            extra = [line for line in lines if line.startswith("  only real")]
            bad = [line for line in lines if not line.startswith("  only real")]
            print(("OK  " if not bad else "DIFF") + f" {path}" + (f"  ({len(extra)} fields only on the real device)" if extra else ""))
            for line in bad[:40]:
                print(line)
    finally:
        server.shutdown()


if __name__ == "__main__":
    main()
