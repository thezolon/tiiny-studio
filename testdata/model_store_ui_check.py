"""The Model Store in the web UI, clicked through against the fake Tiiny (Python server): download with its confirm and
progress, delete, Update all one after another, cancel (never announced as finished), cancel inside Update all (it
stops, the model shows as missing, the next one is left alone), and the update confirm in the updates list.
Needs: pip install playwright==1.64.1 && python -m playwright install chromium. Never contacts a real device.
Usage: python testdata/model_store_ui_check.py"""
import os
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "testdata"))
import fake_tiiny  # noqa: E402

fake, device = fake_tiiny.start()
fake.state.flags.update(download_rate=20, updates=["Fake/Whisper-ASR", "Fake/PP-OCRv6"])
data = tempfile.mkdtemp(prefix="uiflow-")
port = 7962  # its own port: runs next to layout_check (7961) and a11y_check (7960)
env = {**os.environ, "DATA_DIR": data, "TIINY_URL": device, "TIINY_KEY": "fake-key", "PORT": str(port), "TIINY_NO_BROWSER": "1"}
env.pop("STUDIO_DOCKER", None)
server = subprocess.Popen([sys.executable, str(ROOT / "server.py")], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
base = f"http://127.0.0.1:{port}"
ok = []


def check(what, cond):
    ok.append((what, bool(cond)))
    print(("PASS " if cond else "FAIL ") + what, flush=True)


try:
    from playwright.sync_api import sync_playwright
    for _ in range(50):
        try:
            urllib.request.urlopen(base + "/api/settings", timeout=2)
            break
        except OSError:
            time.sleep(0.2)
    with sync_playwright() as p:
        page = p.chromium.launch().new_page(viewport={"width": 1280, "height": 860})
        page.goto(base + "/")
        page.wait_for_load_state("networkidle")
        page.evaluate("openDrawer()")
        page.get_by_role("tab", name="Model Store").click()
        # every model's Details list the store's facts as text (not "[object HTMLElement]")
        page.locator(".mm-store details").first.evaluate("(d) => { d.open = true; }")
        facts = page.locator(".mm-store .mm-facts").first.inner_text()
        check("Details list the store's facts", "Model id" in facts and "object HTMLElement" not in facts)
        page.get_by_role("button", name="Download · 0.30 GB, Store Small").click()
        check("confirm focuses Cancel", page.evaluate("document.activeElement.textContent") == "Cancel")
        page.locator(".mm-confirm").get_by_role("button", name="Download", exact=True).click()
        page.wait_for_timeout(1500)
        check("progress bar shows", page.locator("[role=progressbar]").count() >= 1)
        page.wait_for_function("() => (S.store?.models || []).find((m) => m.id === 'Fake/Store-Small')?.installed", timeout=20000)
        check("Store Small installed", True)
        check("toast says it's on the Tiiny", "is on your Tiiny" in page.locator("#toasts").inner_text())
        page.get_by_role("tab", name="Installed").first.click()
        page.wait_for_timeout(500)
        page.get_by_role("button", name="Delete Store Small").click()
        page.locator(".mm-confirm").get_by_role("button", name="Delete Store Small").click()
        page.wait_for_function("() => !(S.store?.models || []).find((m) => m.id === 'Fake/Store-Small')?.installed", timeout=10000)
        check("Store Small deleted", True)
        check("2 updates waiting shown", "2 updates waiting" in page.locator("#drawerBody").inner_text())
        page.get_by_role("button", name="Update all (2)").click()
        page.locator(".mm-confirm").get_by_role("button", name="Update all").click()
        page.wait_for_function("() => S.store && S.store.updates === 0 && !S.updating && !S.updateQueue.length", timeout=40000)
        check("Update all finished both", True)
        reqs = [(r["method"], r["path"]) for r in fake.state.requests if r["method"] in ("DELETE", "POST") and "/api/v1/models/" in r["path"]]
        order = [r for r in reqs if r[0] == "DELETE" or r[1].endswith("/download")]
        check("updates ran one after another", order[-4:] == [("DELETE", "/api/v1/models/Fake/Whisper-ASR"), ("POST", "/api/v1/models/Fake/Whisper-ASR/download"),
                                                         ("DELETE", "/api/v1/models/Fake/PP-OCRv6"), ("POST", "/api/v1/models/Fake/PP-OCRv6/download")])
        check("no update left", "No model updates waiting" in page.locator("#drawerBody").inner_text())
        # --- from the review: a cancel is not a finish; cancelling inside Update all stops it ---
        fake.state.flags.update(download_rate=2, updates=["Fake/Foundation-1", "Fake/YuE2"])
        page.evaluate("document.querySelectorAll('.toast').forEach((t) => t.remove())")
        page.get_by_role("tab", name="Model Store").click()
        page.get_by_role("button", name="Download · 6.2 GB, Store Image").click()
        page.locator(".mm-confirm").get_by_role("button", name="Download", exact=True).click()
        page.wait_for_function("() => S.downloads.some((d) => d.id === 'Fake/Store-Image' && d.status === 'downloading')", timeout=10000)
        page.locator(".mm-downloads").get_by_role("button", name="Cancel").click()
        page.locator(".mm-confirm").get_by_role("button", name="Stop the download").click()
        page.wait_for_function("() => (S.store?.models || []).find((m) => m.id === 'Fake/Store-Image')?.status === 'not_downloaded' && !S.checking.size", timeout=15000)
        page.wait_for_timeout(3000)
        toasts = page.locator("#toasts").inner_text()
        check("cancel isn't announced as finished", "is on your Tiiny" not in toasts and "stopped downloading" not in toasts)
        page.get_by_role("tab", name="Installed").first.click()
        page.wait_for_function("() => S.store && S.store.updates === 2", timeout=10000)
        page.get_by_role("button", name="Update all (2)").click()
        page.locator(".mm-confirm").get_by_role("button", name="Update all").click()
        page.wait_for_function("() => S.updating && S.downloads.some((d) => d.status === 'downloading')", timeout=15000)
        first = page.evaluate("S.updating.id")
        page.wait_for_timeout(5000)  # past the 4 s fallback: must not count as finished
        check("an update still downloading isn't taken as finished", page.evaluate("S.updating && S.updating.id") == first and page.evaluate("S.updateQueue.length") == 1)
        page.locator(".mm-downloads").get_by_role("button", name="Cancel").click()
        check("cancel confirm warns the old build is gone", "old build was already deleted" in page.locator(".mm-confirm").inner_text())
        page.locator(".mm-confirm").get_by_role("button", name="Stop the download").click()
        page.wait_for_function("() => !S.updating && !S.updateQueue.length && !S.checking.size", timeout=15000)
        page.wait_for_timeout(4000)
        dels = [r["path"] for r in fake.state.requests if r["method"] == "DELETE"]
        second = "Fake/YuE2" if first == "Fake/Foundation-1" else "Fake/Foundation-1"
        check("Update all stopped: the second model was left alone", f"/api/v1/models/{second}" not in dels)
        check("the cancelled update shows as missing", first in page.evaluate("S.missing") and "is missing" in page.locator("#drawerBody").inner_text())
        check("no false 'on your Tiiny' toast", "is on your Tiiny" not in page.locator("#toasts").inner_text())
        # the update confirm opens where it was asked for, in the updates list (an "Other models" row has no card)
        fake.state.flags.update(updates=["Qwen/Qwen3-Embedding-0.6B"], download_rate=20)
        page.wait_for_function("() => S.store && S.store.models.some((m) => m.id === 'Qwen/Qwen3-Embedding-0.6B' && m.need_update)", timeout=10000)
        page.locator(".mm-updates").get_by_role("button", name="Update", exact=True).click()
        check("update confirm opens in the updates list", page.locator(".mm-updates .mm-confirm").count() == 1 and page.evaluate("document.activeElement.textContent") == "Cancel")
        # a tab with no model of its kind points to the store, filtered to it
        page.keyboard.press("Escape")
        page.evaluate("closeDrawer(); S.allModels = S.allModels.filter((m) => m.type !== 'Text-to-Speech'); document.body.append(modelGate('speech', 'voice'))")
        page.get_by_role("button", name="Open the Model Store").click()
        page.wait_for_timeout(500)
        check("'no model installed' opens the store on that workspace",
              page.evaluate("S.drawer && S.mtab === 'store' && S.storeWs === 'audio'") and "Get one from the Model Store" in page.locator(".gate").last.inner_text())
finally:
    server.terminate()
    server.wait(timeout=10)
    fake.shutdown()
    shutil.rmtree(data, ignore_errors=True)
print("ALL PASS" if all(c for _, c in ok) else "SOME FAILED")
sys.exit(0 if all(c for _, c in ok) else 1)
