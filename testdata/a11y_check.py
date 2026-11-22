"""Accessibility check (WCAG 2.2 AA rules of axe-core) of the studio's web UI, driven by the fake Tiiny: every workspace,
the Models drawer and the dialogs, with an image in the gallery, in the dark and the light theme. Fails on serious or critical issues.
Needs: pip install playwright==1.64.1 && python -m playwright install chromium. Never contacts a real device.
Usage: python testdata/a11y_check.py [--all] [--server rust|python]   (--all also fails on moderate and minor issues; the
server defaults to the Rust one that ships, see ui_server.py). Also fails on JavaScript errors in the page."""
import base64
import hashlib
import io
import json
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import fake_tiiny  # noqa: E402
import ui_server  # noqa: E402

ROOT = Path(__file__).resolve().parents[1]
AXE = "4.14.0"  # pinned; the tarball must match npm's published integrity
AXE_SHA512 = "9WTZxEjsZ7b13TH8JPmbV2z8CHbl80/2hm3XPEG4JgNdQLK81IBRXmSxHfMAOkSqQeRxT/0dwNDz2GOm3zzpcQ=="
ANNOTATE = """fetch('/images/' + S.history[0].outputs[0]).then((r) => r.blob()).then((b) => new Promise((res) => { const f = new FileReader(); f.onload = () => res(f.result); f.readAsDataURL(b); }))
  .then((uri) => { S.sources = [{ uri }]; renderComposer(); openAnnot(0); })"""
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
    kind = ui_server.server_kind()
    strict = "--all" in sys.argv
    js_errors = []
    from playwright.sync_api import sync_playwright
    fake, device = fake_tiiny.start()
    # a newer release on the fake "GitHub", so the update pill and its dialog are scanned too
    fake.state.flags["release"] = {"tag_name": "v99.0.0", "body": "## What's new\n\n- A11Y TEST **notes** with a [link](https://example.com/x)",
                                   "html_url": "https://github.com/thezolon/tiiny-studio/releases/tag/v99.0.0"}
    data = tempfile.mkdtemp(prefix="a11y-")
    srv = ui_server.Server(kind, data, device, device + "/__release")
    base = srv.base
    found = {}
    try:
        post = srv.post
        post("/api/settings/updates", {"update_check": True})  # off until the user answers the first-run question
        post("/api/model/launch", {"id": "Qwen/Qwen-Image-2.1"})
        time.sleep(1)
        # the model store: two updates waiting and a download that stays under way, so the drawer shows all of it
        fake.state.flags.update(updates=["Qwen/Qwen3-4B-Instruct", "Fake/Whisper-ASR"], download_hold=True)
        post("/api/model/download", {"id": "Fake/Store-Small"})
        post("/api/model/queue", {"ids": ["Fake/Store-Image", "Fake/Store-Big"], "kind": "download"})  # the queue list too
        post("/api/jobs", {"kind": "image", "model": "Qwen/Qwen-Image-2.1", "prompt": "A11Y TEST apple", "size": "512x512"})
        time.sleep(2)
        axe = axe_source()
        with sync_playwright() as p:
            browser = p.chromium.launch()
            for scheme in ("dark", "light"):  # the theme tokens differ, so every state is scanned in both
                page = browser.new_page(viewport={"width": 1280, "height": 860}, color_scheme=scheme, bypass_csp=True)  # axe is injected as a script tag
                ui_server.watch(page, js_errors, f"{scheme}: ")

                def scan(state, page=page, scheme=scheme):
                    page.wait_for_timeout(600)
                    page.add_script_tag(content=axe) if not page.evaluate("!!window.axe") else None
                    res = page.evaluate("t => axe.run(document, {runOnly: {type: 'tag', values: t}})", TAGS)
                    for v in res["violations"]:
                        for n in v["nodes"]:
                            key = (v["id"], " ".join(n["target"]) if isinstance(n["target"][0], str) else json.dumps(n["target"]))
                            found.setdefault(key, {"impact": v["impact"], "help": v["help"], "states": []})["states"].append(f"{scheme} {state}")

                page.goto(base + "/")
                page.wait_for_load_state("networkidle")
                page.locator("#updatePill").wait_for(state="visible", timeout=5000)  # the notice is up in every scan
                for tab in ("Image", "Audio", "3D", "Video", "Chat"):
                    page.get_by_role("tab", name=tab, exact=True).first.click()
                    scan(tab)
                    if tab == "3D":  # the fake has two 3D models: the model picker's list too
                        page.locator("#wsMesh .pick-btn").click()
                        scan("3D model list")
                        page.keyboard.press("Escape")
                    for i in range(page.locator("#aTools button").count() if tab == "Audio" else 0):  # Audio: one panel per tool
                        page.locator("#aTools button").nth(i).click()
                        scan(f"Audio tool {i + 1}")
                page.get_by_role("tab", name="Image", exact=True).first.click()
                for state, js in (("models drawer", "openDrawer()"),
                                  ("model store", "openDrawer(); setMtab('store')"),
                                  ("store download confirm", "openDrawer(); setMtab('store'); setTimeout(() => confirmFor('download', 'Fake/Store-Big'), 300)"),
                                  ("model updates", "openDrawer(); setMtab('installed'); setTimeout(() => confirmFor('delete', 'Qwen/Qwen3-4B-Instruct'), 300)"),
                                  ("settings", "document.querySelector('#settingsBtn').click()"),
                                  ("add device", "openSettings(false); SET.editing = 'new'; refreshSettings()"),
                                  ("update dialog", "document.querySelector('#updateOpen').click()"),
                                  ("annotate", ANNOTATE),  # openAnnot needs a source picture: the gallery's image
                                  ("text dialog", "textDialog('Text in this image', {}).set([{label: 'Image', text: 'A11Y TEST text'}])")):
                    page.evaluate(js)
                    scan(state)
                    page.keyboard.press("Escape")
                    page.evaluate("closeDrawer(); setMtab('installed'); closeAnnot(); S.sources = []; renderComposer()")
                page.evaluate("galState('image').select = true; renderStrip()")  # select mode swaps the delete buttons for check marks
                scan("gallery select")
                page.evaluate("galState('image').select = false; renderStrip()")
                page.close()
            browser.close()
    finally:
        srv.stop()
        fake.shutdown()
        fake.server_close()
        subprocess.run([sys.executable, "-c", f"import shutil; shutil.rmtree({data!r}, ignore_errors=True)"])
    order = {"critical": 0, "serious": 1, "moderate": 2, "minor": 3}
    rows = sorted(found.items(), key=lambda kv: (order.get(kv[1]["impact"], 9), kv[0]))
    for (rule, target), v in rows:
        print(f"{v['impact']:9} {rule:28} {target[:70]:70} [{', '.join(dict.fromkeys(v['states']))}]  {v['help']}")
    bad = [r for r in rows if strict or r[1]["impact"] in ("serious", "critical")]
    for e in dict.fromkeys(js_errors):
        print("JS error: " + e)
    print(f"\n{len(rows)} issues, {len(bad)} failing ({'all' if strict else 'serious/critical'}), {len(js_errors)} JavaScript errors ({kind} server)")
    sys.exit(1 if bad or js_errors else 0)


if __name__ == "__main__":
    main()
