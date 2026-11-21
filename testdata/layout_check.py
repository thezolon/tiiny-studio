"""Layout check of the studio's web UI, driven by the fake Tiiny: at several window sizes (a TV at 200% scaling, a small
window, a wide and short one), in every workspace, Audio tool, the Models drawer and the dialogs:
- the page never scrolls sideways;
- every visible button, field and link can be reached: scrolled into view, it is the topmost thing at its own centre
  (not clipped by a box that doesn't scroll, not off-screen, not covered);
- a long gallery strip scrolls instead of widening the stage, and the picture stays centred (#125).
Guards the size bugs fixed before: #124 (Settings' OK off-screen at 200%), #125 (picture drifting right with a long strip),
and a tall picture's toolbar in a short window (the shape from #92; the queued card itself isn't covered).
Needs: pip install playwright==1.64.1 && python -m playwright install chromium. Never contacts a real device.
Usage: python testdata/layout_check.py"""
import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import fake_tiiny  # noqa: E402

ROOT = Path(__file__).resolve().parents[1]
SIZES = [(960, 540, "TV at 200%"), (800, 600, "small window"), (1600, 700, "wide and short"), (1280, 860, "laptop")]
ANNOTATE = """fetch('/images/' + S.history[0].outputs[0]).then((r) => r.blob()).then((b) => new Promise((res) => { const f = new FileReader(); f.onload = () => res(f.result); f.readAsDataURL(b); }))
  .then((uri) => { S.sources = [{ uri }]; renderComposer(); openAnnot(0); })"""

# Every visible control in the top layer (an open dialog or drawer, else the app): scroll it into view, then it must be
# the element hit at its centre. Returns the ones that fail, plus the page's sideways overflow.
PROBE = """() => {
  const top = [...document.querySelectorAll('.modal.show')].pop() || document.querySelector('.drawer.show') || document.querySelector('.app');
  const bad = [];
  const shown = (e) => { const r = e.getBoundingClientRect();  // checkVisibility: folded <details> content counts as hidden
    return e.checkVisibility({ contentVisibilityAuto: true, visibilityProperty: true, opacityProperty: true }) && r.width > 2 && r.height > 2 && !e.closest('[inert], [hidden], .sr'); };
  for (const e of top.querySelectorAll('button, input:not([type=hidden]):not([type=file]), select, textarea, a[href], [tabindex="0"]')) {
    if (!shown(e)) continue;
    e.scrollIntoView({ block: 'center', inline: 'nearest' });
    const r = e.getClientRects()[0];  // the first line box: a link that wraps has a gap in the middle of its outline
    const x = Math.min(Math.max(r.left + r.width / 2, 0), innerWidth - 1), y = Math.min(Math.max(r.top + r.height / 2, 0), innerHeight - 1);
    const hit = document.elementFromPoint(x, y);
    const inView = r.left + r.width / 2 >= 0 && r.left + r.width / 2 < innerWidth && r.top + r.height / 2 >= 0 && r.top + r.height / 2 < innerHeight;
    if (!inView || !hit || !(hit === e || e.contains(hit) || hit.contains(e) && hit.tagName === 'LABEL')) {
      const label = (e.getAttribute('aria-label') || e.textContent || e.placeholder || e.id || e.tagName).trim().replace(/\\s+/g, ' ').slice(0, 40);
      bad.push(`${e.tagName.toLowerCase()} "${label}" ${inView ? 'covered by ' + (hit ? hit.tagName.toLowerCase() + '.' + [...hit.classList].join('.') : 'nothing') : 'off-screen'}`);
    }
  }
  return { bad, sideways: document.documentElement.scrollWidth - innerWidth };
}"""

# #125: 40 more thumbnails in the visible strip; the stage keeps its column's width and the picture stays centred
STRIP = """() => {
  const col = [...document.querySelectorAll('.stage-col')].find((c) => c.offsetParent); if (!col) return null;
  const strip = col.querySelector('.strip'), stage = col.querySelector('.stage'); if (!strip || !stage) return null;
  const one = strip.querySelector('.thumb-cell, .thumb') || Object.assign(document.createElement('button'), { className: 'thumb' });
  for (let i = 0; i < 40; i++) strip.appendChild(one.cloneNode(true));
  const c = col.getBoundingClientRect(), s = stage.getBoundingClientRect();
  const pic = stage.querySelector('.canvas img, .canvas, model-viewer, .result-wrap'); const p = pic && pic.getBoundingClientRect();
  const out = { wider: Math.round(s.width - c.width), off: p ? Math.round(Math.abs((p.left + p.width / 2) - (s.left + s.width / 2))) : 0 };
  strip.replaceChildren(...[...strip.children].slice(0, strip.children.length - 40));
  return out;
}"""


def main():
    from playwright.sync_api import sync_playwright
    fake, device = fake_tiiny.start()
    fake.state.flags["release"] = {"tag_name": "v99.0.0", "body": "## What's new\n\n" + "- LAYOUT TEST line\n" * 80,
                                   "html_url": "https://github.com/thezolon/tiiny-studio/releases/tag/v99.0.0"}
    data = tempfile.mkdtemp(prefix="layout-")
    port = 7961
    env = {**os.environ, "DATA_DIR": data, "TIINY_URL": device, "TIINY_KEY": "fake-key", "PORT": str(port), "TIINY_NO_BROWSER": "1",
           "STUDIO_UPDATE_URL": device + "/__release"}
    env.pop("STUDIO_DOCKER", None)
    server = subprocess.Popen([sys.executable, str(ROOT / "server.py")], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    base = f"http://127.0.0.1:{port}"
    fails = []
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
        # the model store: two updates waiting and a download that stays under way, so the drawer shows all of it
        fake.state.flags.update(updates=["Qwen/Qwen3-4B-Instruct", "Fake/Whisper-ASR"], download_hold=True)
        post("/api/model/download", {"id": "Fake/Store-Small"})
        # a tall picture: the shape that pushed the preview off to the right in #92
        post("/api/jobs", {"kind": "image", "model": "Qwen/Qwen-Image-2.1", "prompt": "LAYOUT TEST tall apple", "size": "576x1024"})
        time.sleep(2)
        with sync_playwright() as p:
            browser = p.chromium.launch()
            for w, h, what in SIZES:
                page = browser.new_page(viewport={"width": w, "height": h})

                def check(state, page=page, size=f"{w}x{h} ({what})"):
                    page.wait_for_timeout(400)
                    if os.environ.get("LAYOUT_SHOTS"):  # a folder: a screenshot of every state, before the probe scrolls anything
                        page.screenshot(path=str(Path(os.environ["LAYOUT_SHOTS"]) / f"{size.split()[0]}-{state.replace(' ', '_')}.png"))
                    r = page.evaluate(PROBE)
                    if r["sideways"] > 0:
                        fails.append(f"{size} {state}: the page scrolls sideways by {r['sideways']} px")
                    fails.extend(f"{size} {state}: {b}" for b in r["bad"])

                def strip(state, page=page, size=f"{w}x{h} ({what})"):
                    r = page.evaluate(STRIP)
                    if r and r["wider"] > 1:
                        fails.append(f"{size} {state}: a long gallery strip widens the stage by {r['wider']} px")
                    if r and r["off"] > 2:
                        fails.append(f"{size} {state}: with a long gallery strip the picture is {r['off']} px off centre")

                print(f"{w}x{h} ({what})", flush=True)
                page.goto(base + "/")
                page.wait_for_load_state("networkidle")
                page.evaluate("document.querySelectorAll('.toast').forEach((t) => t.remove())")
                for tab in ("Image", "Audio", "3D", "Video", "Chat"):
                    page.evaluate("n => [...document.querySelectorAll('[role=tab]')].find((b) => b.textContent.trim() === n).click()", tab)
                    check(tab)
                    if tab != "Chat":
                        strip(tab)
                    for i in range(page.locator("#aTools button").count() if tab == "Audio" else 0):
                        page.locator("#aTools button").nth(i).click()
                        check(f"Audio tool {i + 1}")
                page.evaluate("[...document.querySelectorAll('[role=tab]')].find((b) => b.textContent.trim() === 'Image').click()")
                for state, js in (("models drawer", "openDrawer()"),
                                  ("model store", "openDrawer(); setMtab('store')"),
                                  ("store download confirm", "openDrawer(); setMtab('store'); setTimeout(() => confirmFor('download', 'Fake/Store-Big'), 300)"),
                                  ("model updates", "openDrawer(); setMtab('installed'); setTimeout(() => confirmFor('delete', 'Qwen/Qwen3-4B-Instruct'), 300)"),
                                  ("settings", "document.querySelector('#settingsBtn').click()"),
                                  ("add device", "openSettings(false); SET.editing = 'new'; refreshSettings()"),
                                  ("update dialog", "document.querySelector('#updateOpen').click()"),
                                  ("annotate", ANNOTATE),
                                  ("text dialog", "textDialog('Text in this image', {}).set([{label: 'Image', text: 'LAYOUT TEST text\\n'.repeat(200)}])")):
                    page.evaluate(js)
                    check(state)
                    page.keyboard.press("Escape")
                    page.evaluate("closeDrawer(); setMtab('installed'); closeAnnot(); S.sources = []; renderComposer()")
                page.close()
            browser.close()
    finally:
        server.terminate()
        server.wait(10)
        fake.shutdown()
        fake.server_close()
        subprocess.run([sys.executable, "-c", f"import shutil; shutil.rmtree({data!r}, ignore_errors=True)"])
    for f in fails:
        print(f)
    print(f"\n{len(fails)} layout problems")
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
