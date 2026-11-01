"""Watchlist: Tiiny issues the studio works around. Run before a release (and after a TiinyOS update) to see whether
Tiiny fixed any of them, so the workaround can go. Only GET requests are sent; nothing on the device changes.

Each entry says what the studio does about it and where. "auto" entries are checked here; "manual" ones need a model
loaded (or a restart), so the steps to check by hand are printed instead.

Usage: python tools/device_watch.py     (device as in api_snapshot.py: TIINY_URL / TIINY_KEY, else the studio's
data/config.json or the desktop app's config, else TiinyOS's own config). Exit code 1 if an auto check sees a fix."""
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from api_snapshot import device, get  # noqa: E402

VIDEO = "lightx2v/MiniMax-H3-Turbo"


def minimax_icon(base, key):
    """MiniMax-H3-Turbo has no icon: empty icon_url in the catalog, 404 from its icon route (feedback sent 2026-10-31)."""
    code, body = get(base, key, "/api/v1/models/online_models")
    if code != 200:
        return None, f"catalog answered {code}"
    rows = json.loads(body)
    row = next((m for m in (rows if isinstance(rows, list) else rows.get("data", [])) if (m.get("model_id") or m.get("id")) == VIDEO), None)
    icon, _ = get(base, key, f"/api/v1/models/online_models/{VIDEO}/icon")
    if row is None:
        return None, "the model isn't in the catalog"
    if not row.get("icon_url") and icon == 404:
        return True, "icon_url empty, icon route 404"
    return False, f"icon_url {row.get('icon_url')!r}, icon route {icon}"


def layout_route(base, key):
    """PP-DocLayoutV3 has no gateway route (/v1/layout); it only answers on its own port."""
    code, _ = get(base, key, "/v1/layout")  # a POST-only route that exists answers 405 to a GET; a missing one 404
    if code is None:
        return None, "couldn't reach the device"
    if code == 404:
        return True, "GET /v1/layout -> 404 (no route)"
    return False, f"GET /v1/layout -> {code} (a route is there now)"


WATCH = [
    ("auto", "MiniMax-H3-Turbo has no model icon", minimax_icon,
     "Nothing to undo: the studio shows the device's icon as soon as there is one (after a restart); the chip tile "
     "(modelImg/NO_ICON in static/studio.js) only covers a missing icon."),
    ("auto", "PP-DocLayoutV3 has no gateway route", layout_route,
     "Text mode leaves layout models out (layoutOnly() in static/shared.js). Its reading order was wrong too: test "
     "that before using it."),
    ("manual", "Supertonic-3 fails through the gateway",
     "Load Supertonic-3; POST /v1/audio/speech through port 80 with a short text. Still broken if it answers "
     "'The upstream TTS model failed'.",
     "The studio calls its own port (Wi-Fi/LAN only): needsWifi()/wifiNote() in static/shared.js, the direct route in "
     "media.py and rust/studio-core/src/media/speech.rs."),
    ("manual", "GLM-OCR isn't reachable through the gateway",
     "Load GLM-OCR; read a picture with it over the USB link in the studio (Text mode). Still broken if it fails.",
     "OCR uses GLM-OCR's own port (Wi-Fi/LAN only); PP-OCRv6 works on every link."),
    ("manual", "3D background stage fails (BriefNet missing)",
     "Load Hunyuan3D or TRELLIS.2; GET /v1/3d/capabilities and run the background stage on a photo.",
     "The studio cuts the subject out with Qwen-Image first and doesn't offer the stage (static/mesh.js)."),
    ("manual", "SongGeneration's style reference fails",
     "Load SongGeneration; make a song with reference_audio_base64 set. Still broken if it answers 'xh2a backend fail'.",
     "The control is hidden: SONG_STYLE_REF in static/audio.js."),
    ("manual", "Video: no end frame, 7 s maximum",
     "Load MiniMax-H3-Turbo; GET /v1/video/info (max_keyframes, buckets) and try a 2 s clip with last_image, and 8 s.",
     "The studio offers a start frame only and lengths up to 7 s (LENGTHS in static/video.js; the fake refuses >7 s)."),
    ("manual", "3D tasks can't be listed",
     "Load a 3D model; GET /v1/3d/tasks. Still missing if it answers 405.",
     "A 3D build cut short can't be recovered (media/recover.rs, media.py recover)."),
    ("manual", "API answers 502 after a restart until TiinyOS unlocks it",
     "Power-cycle the Tiiny; call /v1/models before opening the TiinyOS app or running `tiiny login`. Still there if 502.",
     "Issue #46: the studio should say so instead of 'Can't reach the Tiiny'."),
]


def main():
    base, key = device()
    fixed = 0
    print("Auto checks (read-only):")
    for _, title, check, undo in (w for w in WATCH if w[0] == "auto"):
        try:
            present, detail = check(base, key)
        except OSError as e:
            present, detail = None, f"couldn't check: {e}"
        state = "still there" if present else "unknown" if present is None else "FIXED?"
        fixed += present is False
        print(f"  [{state:11}] {title}: {detail}")
        if present is False:
            print(f"                {undo}")
    print("\nCheck by hand (needs a model loaded or a restart):")
    for _, title, how, undo in (w for w in WATCH if w[0] == "manual"):
        print(f"  - {title}\n      how: {how}\n      we:  {undo}")
    if fixed:
        print(f"\n{fixed} looks fixed: check it and remove the workaround.")
    sys.exit(1 if fixed else 0)


if __name__ == "__main__":
    main()
