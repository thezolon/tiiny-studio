"""Retake the README's Models screenshot (docs/screenshots/models.webp): the Models drawer on the Model Store tab.

    TIINY_URL=http://192.168.1.50 TIINY_KEY=... python tools/screenshot_models.py [--out docs/screenshots/models.webp]

Starts a scratch studio (Python server, empty temporary data folder, so no gallery, chats or voices can show) against
the Tiiny, names the device "Wi-Fi" so no address appears in the picture, opens the drawer on the Model Store ("Hide
installed" ticked unless --all; with --details, the first model's details open), and saves a 1670x993 WebP in the dark theme. Read-only on the Tiiny: nothing is
downloaded, loaded or deleted. The scratch studio and its folder are removed afterwards. Look at the picture before
committing it. Needs: pip install playwright==1.64.1 pillow && python -m playwright install chromium."""
import argparse
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--out", default=str(ROOT / "docs" / "screenshots" / "models.webp"))
    ap.add_argument("--port", type=int, default=7971)
    ap.add_argument("--details", action="store_true", help="open the first model's Details (long descriptions fill the drawer)")
    ap.add_argument("--all", action="store_true", help="list installed models too (by default only what can be downloaded)")
    a = ap.parse_args()
    url, key = os.environ.get("TIINY_URL"), os.environ.get("TIINY_KEY")
    if not url or not key:
        sys.exit("Set TIINY_URL and TIINY_KEY.")
    from PIL import Image
    from playwright.sync_api import sync_playwright
    data = tempfile.mkdtemp(prefix="shot-")
    env = {**os.environ, "DATA_DIR": data, "TIINY_URL": url, "TIINY_KEY": key, "PORT": str(a.port), "TIINY_NO_BROWSER": "1",
           "STUDIO_UPDATE_URL": "http://127.0.0.1:9/none"}  # no version check from a scratch studio
    env.pop("STUDIO_DOCKER", None)
    server = subprocess.Popen([sys.executable, str(ROOT / "server.py")], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    base = f"http://127.0.0.1:{a.port}"
    try:
        for _ in range(50):
            try:
                urllib.request.urlopen(base + "/api/settings", timeout=2)
                break
            except OSError:
                time.sleep(0.2)
        req = urllib.request.Request(base + "/api/settings", json.dumps({"name": "Wi-Fi", "url": url}).encode(), {"Content-Type": "application/json"})
        urllib.request.urlopen(req, timeout=30).read()
        with sync_playwright() as p:
            browser = p.chromium.launch()
            page = browser.new_page(viewport={"width": 1670, "height": 993}, color_scheme="dark")
            page.goto(base + "/")
            page.wait_for_load_state("networkidle")
            # by default "Hide installed" is ticked, so the picture shows what the store offers and its Download buttons
            page.evaluate(f"S.hideInstalled = {'false' if a.all else 'true'}; openDrawer(); setMtab('store')")
            page.wait_for_function("() => S.store && S.store.models.length", timeout=60000)
            if a.details:  # the first store model's details open, to show what the store tells about each model
                page.evaluate("""() => { const m = S.store.models.filter((x) => x.in_store)
                    .sort((a, b) => (a.recommend_rank ?? 1e9) - (b.recommend_rank ?? 1e9))[0];
                    (S.openDesc ||= {})['store:' + m.id] = true; renderDrawer(); }""")
            page.wait_for_timeout(2500)  # icons
            page.evaluate("document.querySelectorAll('.toast').forEach((t) => t.remove()); document.activeElement?.blur()")
            png = page.screenshot()
            browser.close()
        Image.open(io.BytesIO(png)).save(a.out, "WEBP", quality=82, method=6)
        print("saved", a.out, Path(a.out).stat().st_size, "bytes")
    finally:
        server.terminate()
        server.wait(timeout=10)
        shutil.rmtree(data, ignore_errors=True)


if __name__ == "__main__":
    main()
