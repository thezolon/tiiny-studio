"""Accessibility check (WCAG 2.2 AA rules of axe-core) of the studio's web UI, driven by the fake Tiiny: every workspace,
the Models drawer and the dialogs, with an image in the gallery. Fails on serious or critical issues.
Needs: pip install playwright==1.64.1 && python -m playwright install chromium. Never contacts a real device.
Usage: python testdata/a11y_check.py [--all]   (--all also fails on moderate and minor issues)"""
import base64
import hashlib
import io
import json
import os
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import fake_tiiny  # noqa: E402

ROOT = Path(__file__).resolve().parents[1]
AXE = "4.14.0"  # pinned; the tarball must match npm's published integrity
AXE_SHA512 = "9WTZxEjsZ7b13TH8JPmbV2z8CHbl80/2hm3XPEG4JgNdQLK81IBRXmSxHfMAOkSqQeRxT/0dwNDz2GOm3zzpcQ=="
TAGS = ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa"]


def axe_source():
    cache = Path(tempfile.gettempdir()) / f"axe-core-{AXE}.min.js"
    if not cache.is_file():
        with urllib.request.urlopen(f"https://registry.npmjs.org/axe-core/-/axe-core-{AXE}.tgz", timeout=60) as r:
            tgz = r.read()
        if base64.b64encode(hashlib.sha512(tgz).digest()).decode() != AXE_SHA512:
            sys.exit(f"axe-core {AXE} download doesn't match its published hash")
        with tarfile.open(fileobj=io.BytesIO(tgz)) as t:
            cache.write_bytes(t.extractfile("package/axe.min.js").read())
    return cache.read_text("utf-8")


def main():
    strict = "--all" in sys.argv
    from playwright.sync_api import sync_playwright
    fake, device = fake_tiiny.start()
    data = tempfile.mkdtemp(prefix="a11y-")
    port = 7960
    env = {**os.environ, "DATA_DIR": data, "TIINY_URL": device, "TIINY_KEY": "fake-key", "PORT": str(port), "TIINY_NO_BROWSER": "1"}
    server = subprocess.Popen([sys.executable, str(ROOT / "server.py")], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    base = f"http://127.0.0.1:{port}"
    found = {}
    try:
        for _ in range(50):
            try:
                urllib.request.urlopen(base + "/api/settings", timeout=2)
                break
            except OSError:
                time.sleep(0.2)
        post = lambda path, body: urllib.request.urlopen(urllib.request.Request(base + path, json.dumps(body).encode(), {"Content-Type": "application/json"}), timeout=30).read()
        post("/api/model/launch", {"id": "Qwen/Qwen-Image-2.1"})
        time.sleep(1)
        post("/api/jobs", {"kind": "image", "model": "Qwen/Qwen-Image-2.1", "prompt": "A11Y TEST apple", "size": "512x512"})
        time.sleep(2)
        axe = axe_source()
        with sync_playwright() as p:
            browser = p.chromium.launch()
            page = browser.new_page(viewport={"width": 1280, "height": 860})

            def scan(state):
                page.wait_for_timeout(600)
                page.add_script_tag(content=axe) if not page.evaluate("!!window.axe") else None
                res = page.evaluate("t => axe.run(document, {runOnly: {type: 'tag', values: t}})", TAGS)
                for v in res["violations"]:
                    for n in v["nodes"]:
                        key = (v["id"], " ".join(n["target"]) if isinstance(n["target"][0], str) else json.dumps(n["target"]))
                        found.setdefault(key, {"impact": v["impact"], "help": v["help"], "states": []})["states"].append(state)

            page.goto(base + "/")
            page.wait_for_load_state("networkidle")
            for tab in ("Image", "Audio", "3D", "Video", "Chat"):
                page.get_by_role("button", name=tab, exact=True).first.click() if page.get_by_role("button", name=tab, exact=True).count() \
                    else page.get_by_role("tab", name=tab, exact=True).first.click()
                scan(tab)
            page.get_by_role("button", name="Image", exact=True).first.click() if page.get_by_role("button", name="Image", exact=True).count() \
                else page.get_by_role("tab", name="Image", exact=True).first.click()
            for state, js in (("models drawer", "openDrawer()"), ("settings", "document.querySelector('#settingsBtn').click()"),
                              ("annotate", "openAnnot(0)")):
                page.evaluate(js)
                scan(state)
                page.keyboard.press("Escape")
                page.evaluate("typeof closeDrawer === 'function' && closeDrawer(); typeof closeAnnot === 'function' && closeAnnot()")
            browser.close()
    finally:
        server.terminate()
        server.wait(10)
        fake.shutdown()
        fake.server_close()
        subprocess.run([sys.executable, "-c", f"import shutil; shutil.rmtree({data!r}, ignore_errors=True)"])
    order = {"critical": 0, "serious": 1, "moderate": 2, "minor": 3}
    rows = sorted(found.items(), key=lambda kv: (order.get(kv[1]["impact"], 9), kv[0]))
    for (rule, target), v in rows:
        print(f"{v['impact']:9} {rule:28} {target[:70]:70} [{', '.join(dict.fromkeys(v['states']))}]  {v['help']}")
    bad = [r for r in rows if strict or r[1]["impact"] in ("serious", "critical")]
    print(f"\n{len(rows)} issues, {len(bad)} failing ({'all' if strict else 'serious/critical'})")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
