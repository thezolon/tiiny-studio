"""App icons from the HiDream picture: crop B (dog and painting) for 64 px and up, crop C (dog's face) for the small
sizes, same rounded corners everywhere. Writes the Tauri icon set (icons/), the web favicon and the header logo.
To redo: copy crop_B.png to a temp master, run `cargo tauri icon <master> -o icons` in rust/studio-desktop (delete
icons/android and icons/ios), then `python icon-work/make_icons.py`."""
from pathlib import Path

from PIL import Image, ImageDraw

W = Path(__file__).resolve().parent
ICONS = W.parent / "icons"
STATIC = W.parents[2] / "static"
SMALL_MAX = 48


def rounded(im, size):
    im = im.resize((size, size), Image.LANCZOS).convert("RGBA")
    mask = Image.new("L", (size * 4, size * 4), 0)  # draw big, scale down: smooth edges
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, size * 4 - 1, size * 4 - 1], int(size * 4 * 0.2), fill=255)
    im.putalpha(mask.resize((size, size), Image.LANCZOS))
    return im


B = Image.open(W / "crop_B.png")
C = Image.open(W / "crop_C.png")
pick = lambda s: rounded(C if s <= SMALL_MAX else B, s)

# PNGs Tauri bundles (Linux/AppImage use these; Windows Store sizes too)
for f in ICONS.glob("*.png"):
    with Image.open(f) as im:
        s = im.size[0]
    pick(s).save(f)
    print("png", f.name, s, "C" if s <= SMALL_MAX else "B")

# Windows .ico: every common size, small ones from C
ico_sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
imgs = [pick(s) for s in ico_sizes]
imgs[-1].save(ICONS / "icon.ico", format="ICO", sizes=[(s, s) for s in ico_sizes], append_images=imgs[:-1])
print("ico", ico_sizes)

# macOS .icns: Pillow picks the entries from the images it's given
icns = [pick(s) for s in (16, 32, 64, 128, 256, 512, 1024)]
icns[-1].save(ICONS / "icon.icns", format="ICNS", append_images=icns[:-1])
print("icns written")

# Web UI: favicon (C) and the header logo (C at 2x for sharp edges)
pick(64).save(STATIC / "favicon.png")
pick(52).save(STATIC / "logo.png")
print("static favicon.png, logo.png")
