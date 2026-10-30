"""Tiiny Studio — a local media studio for the Tiiny Pocket: images, voice, music, transcription, 3D.

Serves the single-page UI, proxies requests to the device (injecting the API key so it
never reaches the browser), and keeps every result in a persistent on-disk gallery.
Stdlib only.
"""
import base64
import hmac
import json
import os
import re
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, quote, unquote, urlparse

import chat  # noqa: F401  (registers the "chat" job kind)
import extract
from media import JOBS, JobError, Media

PORT = int(os.environ.get("PORT", "7860"))
PASSWORD = os.environ.get("STUDIO_PASSWORD", "")
HOST = os.environ.get("HOST", "127.0.0.1")  # native runs stay on this computer unless HOST=0.0.0.0
# Who can reach the studio: Docker publishes on STUDIO_BIND; a native run listens on HOST.
LAN_ACCESS = os.environ.get("STUDIO_BIND", HOST) not in ("127.0.0.1", "localhost", "::1")  # optional: ask browsers for a password (any username)
NOT_A_TIINY = ("This address answers chat requests but has no Tiiny model controls, so it looks like a relay or proxy "
               "(for example an OpenAI-compatible gateway). Use your Tiiny's own address and API key from the TiinyOS app.")
STUDIO_DOCKER = os.environ.get("STUDIO_DOCKER") == "1"  # the Docker image (its Dockerfile sets it)
# The USB cable's address is picked from the device's serial in 172.16-172.31; 172.17 is also Docker's default
# network, so a studio in Docker can't reach it (docs/CONNECTING.md). Said plainly instead of "unreachable".
DOCKER_USB = ("This is a USB address in Docker's own network (172.17.x.x), so the studio in Docker can't reach it. "
              "Use the Tiiny's Wi-Fi/LAN address, or the desktop app, or move Docker's network (see the connection guide).")
FROZEN = getattr(sys, "frozen", False)  # the standalone download (PyInstaller): code and static files unpack to a temp folder


def user_data_dir():
    """Where the standalone app keeps results and settings: the usual per-user place on each OS."""
    if sys.platform == "win32":
        return Path(os.environ.get("APPDATA") or Path.home() / "AppData" / "Roaming") / "Tiiny Studio"
    if sys.platform == "darwin":
        return Path.home() / "Library" / "Application Support" / "Tiiny Studio"
    return Path(os.environ.get("XDG_DATA_HOME") or Path.home() / ".local" / "share") / "tiiny-studio"


# Docker sets DATA_DIR=/data; a source checkout keeps ./data next to the code
DATA = Path(os.environ.get("DATA_DIR") or (user_data_dir() if FROZEN else Path(__file__).with_name("data")))


def run_as_data_owner():
    """Docker on Linux runs us as root, so results in the mounted data folder would belong to root and
    the user couldn't delete them without sudo. Take on the folder's owner instead, and hand back
    anything an earlier root run left behind. (Docker Desktop on Windows shows the folder as root's: no change.)"""
    if os.name != "posix" or os.geteuid() != 0 or not DATA.is_dir():
        return
    st = DATA.stat()
    if st.st_uid == 0:
        return
    for root, dirs, files in os.walk(DATA):
        for path in [root] + [os.path.join(root, n) for n in dirs + files]:
            try:
                if os.lstat(path).st_uid == 0:
                    os.lchown(path, st.st_uid, st.st_gid)
            except OSError:
                pass
    os.setgid(st.st_gid)
    os.setuid(st.st_uid)


run_as_data_owner()
STATIC = Path(__file__).with_name("static")
VERSION = (Path(__file__).with_name("VERSION").read_text("utf-8").strip() if Path(__file__).with_name("VERSION").exists() else "dev")
IMAGES = DATA / "images"
IMAGES.mkdir(parents=True, exist_ok=True)
ICONS = DATA / "icons"
ICONS.mkdir(parents=True, exist_ok=True)
HISTORY = DATA / "history.json"
CONFIG = DATA / "config.json"
VOICES = DATA / "voices.json"
CHATS = DATA / "chats"  # one JSON file per saved conversation
CHATS.mkdir(parents=True, exist_ok=True)
LOCK = threading.Lock()
DEVICE_LOCK = threading.Lock()  # the device runs one request at a time; every job (image, audio, 3D) takes this

SWAP_KEYS = ("on", "chat", "image", "audio", "mesh")


class Settings:
    """Saved Tiiny devices (name, address and that device's own API key) in data/config.json, plus which
    one is active. TIINY_URL/TIINY_KEY seed a first device when nothing is saved.

    With auto-switch on, a monitor probes every saved device (the unauthenticated status page at "/",
    so no key is sent) and keeps the studio on the first one in the list that answers: it moves away
    when the active device stops responding and back to a higher-priority one when that returns.
    A key is only ever sent to the device it was saved with."""

    def __init__(self):
        self.lock = threading.Lock()
        try:
            saved = json.loads(CONFIG.read_text("utf-8"))
        except (OSError, ValueError):
            saved = {}
        if "devices" in saved:
            self.devices, self.active, self.auto = saved["devices"], saved.get("active"), saved.get("auto", True)
            self.source = "file"
        else:  # older single-device config, or env vars
            url = normalize_url(saved.get("url") or os.environ.get("TIINY_URL", ""))
            key = saved.get("key") or os.environ.get("TIINY_KEY", "")
            self.devices = [{"id": uuid.uuid4().hex[:8], "name": default_device_name(url), "url": url, "key": key}] if url else []
            self.active = self.devices[0]["id"] if self.devices else None
            self.auto = True
            self.source = "file" if saved else "env" if url else "none"
        # Model swapping: may helpers and chat tools load what they need (and put the user's models back)?
        # "on" is the global switch; each section's switch only counts while it's on. All on by default.
        self.swap = {k: bool((saved.get("swap") or {}).get(k, True)) for k in SWAP_KEYS}
        self.helper_model = str(saved.get("helper_model") or "")  # the text model helpers use ("" = automatic)
        self.status = {}        # device id -> {"ok": bool, "ms": int}
        self.pinned = None      # a device picked by hand stays in use while it answers
        self.last_switch = None  # {"to": name, "from": name, "t": epoch}

    def current(self):
        return next((d for d in self.devices if d["id"] == self.active), self.devices[0] if self.devices else None)

    @property
    def url(self):
        return (self.current() or {}).get("url", "")

    @property
    def key(self):
        return (self.current() or {}).get("key", "")

    @property
    def host(self):
        return urlparse(self.url).hostname or ""

    @property
    def configured(self):
        return bool(self.url and self.key)

    def public(self):
        cur = self.current() or {}
        hint = lambda k: f"…{k[-4:]}" if len(k or "") >= 8 else ""
        return {"devices": [{"id": d["id"], "name": d["name"], "url": d["url"], "host": urlparse(d["url"]).hostname or "",
                             "key_set": bool(d.get("key")), "key_hint": hint(d.get("key")), "status": self.status.get(d["id"])}
                            for d in self.devices],
                "active": cur.get("id"), "name": cur.get("name", ""), "url": self.url, "host": self.host, "auto": self.auto,
                "key_set": bool(self.key), "key_hint": hint(self.key), "configured": self.configured, "source": self.source,
                "lan_access": LAN_ACCESS, "password": bool(PASSWORD), "last_switch": self.last_switch, "version": VERSION,
                "swap": self.swap, "helper_model": self.helper_model, "docker": STUDIO_DOCKER,
                "app": FROZEN}  # the standalone app: no .env to edit, so the UI words the network notes differently

    def _save(self):
        tmp = CONFIG.with_suffix(".tmp")
        tmp.write_text(json.dumps({"devices": self.devices, "active": self.active, "auto": self.auto, "swap": self.swap,
                                   "helper_model": self.helper_model}, indent=1), "utf-8")
        try:
            os.chmod(tmp, 0o600)
        except OSError:
            pass
        tmp.replace(CONFIG)
        self.source = "file"

    def upsert(self, did, name, url, key):
        url = normalize_url(url)
        if not url:
            raise ValueError("Enter the device address, e.g. http://192.168.1.50")
        with self.lock:
            d = next((x for x in self.devices if x["id"] == did), None) if did else None
            if d:
                if key:
                    d["key"] = key
                elif urlparse(url).hostname != urlparse(d["url"]).hostname:
                    d["key"] = ""  # a key is never sent to a different host: a new address needs it re-entered
                d["url"], d["name"] = url, (name or "").strip()[:40] or d["name"]
            else:
                d = {"id": uuid.uuid4().hex[:8], "name": (name or "").strip()[:40] or default_device_name(url), "url": url, "key": key or ""}
                self.devices.append(d)
            if not self.active or not self.current():
                self.active = d["id"]
            self.status.pop(d["id"], None)
            self._save()
            return d["id"]

    def remove(self, did):
        with self.lock:
            self.devices = [d for d in self.devices if d["id"] != did]
            if self.active == did:
                self.active = self.devices[0]["id"] if self.devices else None
            self.status.pop(did, None)
            self._save()

    def move(self, did, step):
        with self.lock:
            i = next((n for n, d in enumerate(self.devices) if d["id"] == did), None)
            j = None if i is None else i + step
            if i is not None and 0 <= j < len(self.devices):
                self.devices[i], self.devices[j] = self.devices[j], self.devices[i]
                self._save()

    def activate(self, did, reason=None):
        if not reason:  # chosen by hand: keep it while it answers
            self.pinned = did
        with self.lock:
            target = next((d for d in self.devices if d["id"] == did), None)
            if not target or did == self.active:
                return
            before = self.current()
            self.active = did
            if reason:
                self.last_switch = {"to": target["name"], "from": (before or {}).get("name"), "t": int(time.time()), "reason": reason}
            self._save()
        print(f"Device: {target['name']} ({target['url']}){' — ' + reason if reason else ''}", flush=True)

    def set_swap(self, req):
        with self.lock:
            for k, v in (req.get("swap") or {}).items():
                if k in SWAP_KEYS:
                    self.swap[k] = bool(v)
            if isinstance(req.get("helper_model"), str):
                self.helper_model = req["helper_model"].strip()[:200]
            self._save()

    def swap_allowed(self, section):
        return self.swap["on"] and self.swap.get(section, False)

    def set_auto(self, on):
        with self.lock:
            self.auto = bool(on)
            if on:
                self.pinned = None  # turning auto-switch (back) on releases a hand-picked device
            self._save()

    @staticmethod
    def probe(url, timeout=2.0):
        t0 = time.time()
        try:
            with urllib.request.urlopen(url + "/", timeout=timeout) as r:
                r.read(200)
            return {"ok": True, "ms": round((time.time() - t0) * 1000)}
        except (urllib.error.URLError, TimeoutError, OSError, ValueError):
            return {"ok": False, "ms": None}

    def check(self):
        """Probe every saved device in parallel; with auto-switch on, use the first one that answers."""
        devices = list(self.devices)
        results = {}
        threads = [threading.Thread(target=lambda d=d: results.__setitem__(d["id"], self.probe(d["url"]))) for d in devices]
        for t in threads:
            t.start()
        for t in threads:
            t.join(4)
        self.status.update(results)
        if self.auto and len(devices) > 1:
            cur = self.current()
            if self.pinned and cur and self.pinned == cur["id"] and results.get(cur["id"], {}).get("ok"):
                return self.status  # the hand-picked device still answers: leave it
            self.pinned = None
            best = next((d for d in devices if results.get(d["id"], {}).get("ok") and d.get("key")), None)
            if best and cur and best["id"] != cur["id"]:
                gone = not results.get(cur["id"], {}).get("ok")
                self.activate(best["id"], reason=f"{cur['name']} isn't answering" if gone else f"{best['name']} is back")
        return self.status

    def monitor(self, every=20):
        while True:
            try:
                self.check()
            except Exception as e:  # keep monitoring whatever happens
                print(f"device check failed: {e}", flush=True)
            time.sleep(every)


def default_device_name(url):
    host = urlparse(url).hostname or ""
    if host.startswith("172.17."):
        return "USB cable"
    return f"Tiiny ({host})" if host else "Tiiny"


def normalize_url(url):
    url = (url or "").strip().rstrip("/")
    if url and not re.match(r"https?://", url):
        url = "http://" + url
    return url


SETTINGS = Settings()


def load_history():
    try:
        return json.loads(HISTORY.read_text("utf-8"))
    except (OSError, ValueError):
        return []


def save_history(items):
    save_json(HISTORY, items)


def shred(path):
    """Overwrite a file with zeros, then delete it. Best effort: SSDs and copy-on-write filesystems
    may still keep old blocks, but the data is no longer reachable through the file system."""
    try:
        size = path.stat().st_size
        with open(path, "r+b") as f:
            while size > 0:
                n = min(size, 1 << 20)
                f.write(b"\0" * n)
                size -= n
            f.flush()
            os.fsync(f.fileno())
    except OSError:
        pass
    path.unlink(missing_ok=True)


def save_json(path, obj):
    tmp = path.with_suffix(".tmp")
    tmp.write_text(json.dumps(obj, indent=1), "utf-8")
    tmp.replace(path)


def add_history(entry):
    with LOCK:
        items = load_history()
        items.insert(0, entry)
        save_history(items)


def edit_history(entry_id, fn):
    """Apply fn to one saved entry (e.g. clear a preview's resume token) and save."""
    with LOCK:
        items = load_history()
        for e in items:
            if e.get("id") == entry_id:
                fn(e)
        save_history(items)


def device(method, path, body=None, timeout=30, url=None, key=None):
    if not (url or SETTINGS.url):
        return 503, {"error": {"message": "No device configured yet. Open Settings and enter your Tiiny's address and API key."}}
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request((url or SETTINGS.url) + path, data=data, method=method)
    req.add_header("Authorization", f"Bearer {key if key is not None else SETTINGS.key}")
    if data is not None:
        req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.status, json.loads(r.read() or b"{}")
    except urllib.error.HTTPError as e:
        raw = e.read()
        try:
            return e.code, json.loads(raw)
        except ValueError:
            return e.code, {"error": {"message": raw.decode("utf-8", "replace")[:500]}}
    except (urllib.error.URLError, TimeoutError, OSError) as e:
        return 502, {"error": {"message": f"Device unreachable: {e}"}}


WORKSPACE = {"Text-to-Image": "image", "Image-to-Image": "image", "Text-to-Speech": "audio", "ASR": "audio",
             "Music Generation": "audio", "Image-to-3D": "3d", "Text-to-Video": "video", "Image-Text-to-Text": "text", "Text Generation": "text"}


# Where a loaded model reports its own options (#23), by catalog type. Image, OCR and transcription models have no
# such route: their options come from the catalog row (supported, capabilities, desc).
INFO_ROUTES = {"Text Generation": "/v1/capabilities", "Image-Text-to-Text": "/v1/capabilities", "Text-to-Speech": "/v1/capabilities",
               "Music Generation": "/v1/music/info", "Image-to-3D": "/v1/3d/capabilities", "Text-to-Video": "/v1/video/info"}


def model_info(mid):
    """What a model reports about its options: chat (thinking, vision), voice (clone, speakers, languages), music
    (config schema and defaults), 3D (stages, params). Only a loaded model answers; info is None otherwise."""
    code, inst = device("GET", "/api/v1/models/?including_downloading=false", timeout=15)
    if code != 200:
        return code, inst
    row = next((m for m in inst.get("data", []) if m.get("id") == mid), None)
    if not row:
        return 404, {"error": {"message": f"{mid} isn't installed on the device"}}
    route, info = INFO_ROUTES.get(row.get("type")), None
    if route:
        code, res = device("GET", f"{route}?model={quote(mid, safe='')}", timeout=15)
        if code == 200 and isinstance(res, dict):
            info = res["data"] if isinstance(res.get("data"), dict) else res
    return 200, {"id": mid, "type": row.get("type"), "info": info}


def catalog():
    """Every installed model with live status, tagged with the studio workspace that uses it."""
    code, inst = device("GET", "/api/v1/models/?including_downloading=false", timeout=15)
    if code != 200:
        return code, inst
    _, run = device("GET", "/api/v1/models/running", timeout=15)
    running = {r["model_id"]: r for r in (run.get("instances") or {}).get("running", [])}
    pending = {r.get("model_id") for r in (run.get("instances") or {}).get("pending", [])} | set(run.get("pending") or [])
    keep = ("id", "display_name", "params", "type", "input", "output", "supported", "desc",
            "npu_usage", "load_time", "costMemory", "size", "version", "thinking", "reasoning_levels", "capabilities")
    models = []
    for m in inst.get("data", []):
        mid = m["id"]
        row = {k: m.get(k) for k in keep}
        row["status"] = "running" if mid in running else "loading" if mid in pending else "stopped"
        row["busy"] = bool(running.get(mid, {}).get("active_request_count")) or any(
            j["model"] == mid and j["status"] == "running" for j in list(JOBS.values()))
        row["workspace"] = WORKSPACE.get(m.get("type"), "other")
        models.append(row)
    used = sum((running[k].get("npu_usage") or 0) for k in running)
    return 200, {"models": models, "npu": {"used": used, "total": 100}, "direct_access": MEDIA.direct_access(),
                 "device": {"id": SETTINGS.active, "name": (SETTINGS.current() or {}).get("name"), "host": SETTINGS.host},
                 "last_switch": SETTINGS.last_switch}


MEDIA = Media(SETTINGS, IMAGES, add_history, lambda m, p, b=None, t=30: device(m, p, b, timeout=t), DEVICE_LOCK)
MEDIA.edit_history = edit_history
SEARCH_CACHE = {}  # entry id -> (text, vector); memory only, dropped on delete/wipe
STATS_CACHE = {"t": 0, "data": None}


def entry_text(e):
    p = e.get("params") or {}
    return " ".join(str(x) for x in (p.get("prompt"), p.get("text"), p.get("caption"), p.get("lyrics"), e.get("text")) if x)[:2000]


def embed(model, texts):
    vecs = []
    for i in range(0, len(texts), 32):
        code, res = device("POST", "/v1/embeddings", {"model": model, "input": texts[i:i + 32]}, timeout=120)
        if code != 200:
            raise JobError((res.get("error") or {}).get("message") if isinstance(res.get("error"), dict) else str(res)[:200])
        vecs += [d["embedding"] for d in sorted(res["data"], key=lambda d: d["index"])]
    return vecs


def device_stats():
    """NPU/CPU stats: the gateway's /api/v1/npu/status (every link) plus the optional on-device
    dashboard (:9109/api/stats) for power and temperatures when it's running. Cached 3 s."""
    if time.time() - STATS_CACHE["t"] < 3 and STATS_CACHE["data"]:
        return STATS_CACHE["data"]
    out = {"source": []}
    code, st = device("GET", "/api/v1/npu/status", timeout=5)
    if code == 200:
        out["source"].append("gateway")
        dev = (st.get("devices") or [{}])[0]
        out.update(npu_util=dev.get("util_percent"), npu_mem_used_mb=dev.get("mem_used_mb"), npu_mem_total_mb=dev.get("mem_total_mb"),
                   npu_temp_c=dev.get("temp_c"), npu_power_w=dev.get("power_w"), cpu_pct=(st.get("cpu") or {}).get("total_percent"),
                   mem_pct=(st.get("memory") or {}).get("usage_percent"), npu_status=(st.get("npu") or {}).get("status"))
    if SETTINGS.host:
        try:
            with urllib.request.urlopen(f"http://{SETTINGS.host}:9109/api/stats", timeout=1.2) as r:
                d = json.loads(r.read())
            out["source"].append("dashboard")
            npu = d.get("npu") or {}
            temps = [v for v in (npu.get("temps") or {}).values() if isinstance(v, (int, float))]
            out.update(npu_power_w=npu.get("board_power_w", out.get("npu_power_w")), npu_util=npu.get("avg_util", out.get("npu_util")),
                       npu_temp_c=max(temps) if temps else out.get("npu_temp_c"))
        except (urllib.error.URLError, OSError, ValueError, AttributeError):
            pass
    STATS_CACHE.update(t=time.time(), data=out)
    return out
MIME = {"png": "image/png", "jpg": "image/jpeg", "webp": "image/webp", "wav": "audio/wav", "mp3": "audio/mpeg",
        "m4a": "audio/mp4", "webm": "audio/webm", "ogg": "audio/ogg", "flac": "audio/flac", "glb": "model/gltf-binary", "mp4": "video/mp4",
        "js": "text/javascript; charset=utf-8", "mjs": "text/javascript; charset=utf-8", "css": "text/css; charset=utf-8", "mp3": "audio/mpeg"}


IN_DOCKER = Path("/.dockerenv").exists()


def can_open_folder(client_ip):
    """Opening the results folder only makes sense on the computer running the studio, with a desktop:
    not in Docker, not for a phone or another PC on the LAN."""
    if IN_DOCKER or client_ip not in ("127.0.0.1", "::1", "::ffff:127.0.0.1"):
        return False
    return sys.platform in ("win32", "darwin") or bool(os.environ.get("DISPLAY") or os.environ.get("WAYLAND_DISPLAY"))


def open_folder(path):
    """Show a folder in the system file manager (Explorer, Finder, or the Linux desktop's)."""
    if sys.platform == "win32":
        os.startfile(path)  # noqa: S606 (a fixed folder of ours, never a user-given path)
    else:
        subprocess.Popen(["open" if sys.platform == "darwin" else "xdg-open", str(path)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def list_chats():
    out = []
    for path in CHATS.glob("*.json"):
        try:
            c = json.loads(path.read_text("utf-8"))
        except (OSError, ValueError):
            continue
        out.append({"id": path.stem, "title": c.get("title") or "Chat", "updated": c.get("updated", 0), "count": len(c.get("messages") or [])})
    return sorted(out, key=lambda c: -c["updated"])


def voice_for(vid):
    v = next((v for v in load_voices() if v.get("id") == vid), None)
    if not v or not (IMAGES / v["file"]).is_file():
        return None
    return {"name": v["name"], "text": v.get("text", ""), "uri": "data:audio/wav;base64," + base64.b64encode((IMAGES / v["file"]).read_bytes()).decode()}


def load_voices():
    try:
        return json.loads(VOICES.read_text("utf-8"))
    except (OSError, ValueError):
        return []


class Handler(BaseHTTPRequestHandler):
    server_version = "TiinyStudio/1.0"

    def log_message(self, fmt, *args):
        print(f"[{time.strftime('%H:%M:%S')}] {self.command} {self.path.split('?')[0]} {fmt % args}", flush=True)

    def send_json(self, code, obj):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def send_file(self, path, ctype, cache=False):
        try:
            body = path.read_bytes()
        except OSError:
            return self.send_json(404, {"error": {"message": "not found"}})
        # one byte range ("bytes=a-b", "bytes=a-", "bytes=-n"): a <video> asks for them to seek
        rng = re.fullmatch(r"bytes=(\d*)-(\d*)", self.headers.get("Range") or "")
        start, end = 0, len(body) - 1
        if rng and (rng.group(1) or rng.group(2)):
            if rng.group(1):
                start, end = int(rng.group(1)), min(int(rng.group(2) or end), end)
            else:
                start = max(0, len(body) - int(rng.group(2)))
            if start > end:
                self.send_response(416)
                self.send_header("Content-Range", f"bytes */{len(body)}")
                self.send_header("Content-Length", "0")
                self.end_headers()
                return
        part = rng and (rng.group(1) or rng.group(2))
        self.send_response(206 if part else 200)
        self.send_header("Content-Type", ctype)
        self.send_header("Accept-Ranges", "bytes")
        if part:
            self.send_header("Content-Range", f"bytes {start}-{end}/{len(body)}")
        self.send_header("Content-Length", str(end - start + 1))
        self.send_header("Cache-Control", {True: "public, max-age=31536000, immutable", "private": "no-store"}.get(cache, "no-cache"))
        self.end_headers()
        self.wfile.write(body[start:end + 1])

    def foreign(self):
        """Other websites are refused (same rule as the Rust server): any page open in the browser could otherwise
        POST here, or read the gallery by pointing its own domain at 127.0.0.1 (DNS rebinding)."""
        host = (self.headers.get("Host") or "").lower()
        if host and not LAN_ACCESS:
            name = host[1:].split("]")[0] if host.startswith("[") else host.rsplit(":", 1)[0] if ":" in host else host
            if name not in ("localhost", "127.0.0.1", "::1"):
                return "This studio only answers at localhost."
        if self.command in ("GET", "HEAD"):
            return None
        origin = (self.headers.get("Origin") or "").lower()
        if not origin:
            return None
        authority = origin.removeprefix("http://") if origin.startswith("http://") else origin.removeprefix("https://") if origin.startswith("https://") else None
        return None if authority is not None and authority == host else "Requests from other websites aren't allowed."

    def authorized(self):
        why = self.foreign()
        if why:
            self.send_json(403, {"error": {"message": why}})
            return False
        if not PASSWORD:
            return True
        auth = self.headers.get("Authorization") or ""
        if auth.startswith("Basic "):
            try:
                _, _, given = base64.b64decode(auth[6:]).decode("utf-8").partition(":")
                if hmac.compare_digest(given.encode(), PASSWORD.encode()):
                    return True
            except (ValueError, UnicodeDecodeError):
                pass
        self.send_response(401)
        self.send_header("WWW-Authenticate", 'Basic realm="Tiiny Studio", charset="UTF-8"')
        self.send_header("Content-Length", "0")
        self.end_headers()
        return False

    def extract_document(self):
        """Text and pictures out of a document sent as the raw body; all in memory, nothing kept."""
        n = int(self.headers.get("Content-Length") or 0)
        if n > 100 << 20:
            return self.send_json(413, {"error": {"message": "That file is over 100 MB."}})
        data = self.rfile.read(n)
        try:
            parts = extract.extract(unquote(self.headers.get("X-Filename") or "file"), data)
        except extract.ExtractError as e:
            return self.send_json(400, {"error": {"message": str(e)}})
        except Exception as e:  # a malformed file shouldn't take the request down with a traceback
            return self.send_json(400, {"error": {"message": f"Couldn't read this file ({type(e).__name__})."}})
        return self.send_json(200, {"parts": parts})

    def read_json(self):
        n = int(self.headers.get("Content-Length") or 0)
        return json.loads(self.rfile.read(n) or b"{}")

    def do_GET(self):
        if not self.authorized():
            return
        p = self.path.split("?")[0]
        if p in ("/", "/index.html"):
            return self.send_file(STATIC / "index.html", "text/html; charset=utf-8")
        if p == "/api/models":
            return self.send_json(*device("GET", "/v1/models", timeout=10))
        if p == "/api/history":
            with LOCK:
                return self.send_json(200, load_history())
        if p == "/api/settings":
            return self.send_json(200, {**SETTINGS.public(), "can_open_folder": can_open_folder(self.client_address[0])})
        if p == "/api/stats":
            return self.send_json(200, device_stats())
        if p == "/api/busy":
            return self.send_json(200, {"busy": DEVICE_LOCK.locked()})
        if p == "/api/catalog":
            return self.send_json(*catalog())
        if p == "/api/jobs":
            return self.send_json(200, MEDIA.list())
        m = re.fullmatch(r"/api/jobs/(\w+)", p)
        if m:  # one job, polled fast while a chat reply streams in
            job = JOBS.get(m.group(1))
            return self.send_json(200, MEDIA.view(job)) if job else self.send_json(404, {"error": {"message": "no such job"}})
        if p == "/api/chats":
            return self.send_json(200, list_chats())
        m = re.fullmatch(r"/api/chats/(\w+)", p)
        if m:
            path = CHATS / f"{m.group(1)}.json"
            return self.send_file(path, "application/json") if path.is_file() else self.send_json(404, {"error": {"message": "no such chat"}})
        if p == "/api/voices":
            with LOCK:
                return self.send_json(200, load_voices())
        m = re.fullmatch(r"/docs/([\w-]+\.md)", p)
        if m:
            return self.send_file(Path(__file__).with_name("docs") / m.group(1), "text/plain; charset=utf-8")
        m = re.fullmatch(r"/static/((?:vendor/(?:pdfjs/(?:cmaps/)?)?)?[\w.-]+)", p)
        if m:
            return self.send_file(STATIC / m.group(1), MIME.get(m.group(1).rsplit(".", 1)[-1], "application/octet-stream"))
        if p == "/api/model/info":
            mid = self.query_id()
            if not mid:
                return self.send_json(400, {"error": {"message": "id required"}})
            return self.send_json(*model_info(mid))
        if p == "/api/model/plan":
            mid = self.query_id()
            if not mid:
                return self.send_json(400, {"error": {"message": "id required"}})
            return self.send_json(*device("GET", f"/api/v1/models/{quote(mid, safe='')}/unload_candidate", timeout=30))
        m = re.fullmatch(r"/api/icon/([\w.-]+/[\w.-]+)", p)
        if m:
            return self.proxy_icon(m.group(1))
        m = re.fullmatch(r"/images/([\w.-]+)", p)
        if m:
            ctype = MIME.get(m.group(1).rsplit(".", 1)[-1].lower(), "application/octet-stream")
            return self.send_file(IMAGES / m.group(1), ctype, cache="private")
        self.send_json(404, {"error": {"message": "not found"}})

    def query_id(self):
        return parse_qs(urlparse(self.path).query).get("id", [""])[0]

    def proxy_icon(self, mid):
        cached = ICONS / (mid.replace("/", "__") + ".png")
        if not cached.exists():
            try:
                with urllib.request.urlopen(f"{SETTINGS.url}/api/v1/models/online_models/{mid}/icon", timeout=10) as r:
                    cached.write_bytes(r.read())
            except (urllib.error.URLError, OSError):
                return self.send_json(404, {"error": {"message": "no icon"}})
        return self.send_file(cached, "image/png", cache=True)

    def model_action(self, action):
        try:
            mid = self.read_json().get("id") or ""
        except ValueError:
            mid = ""
        if not mid:
            return self.send_json(400, {"error": {"message": "id required"}})
        path = f"/api/v1/models/{quote(mid, safe='')}/{action}"
        if action == "stop":
            if any(j["model"] == mid and j["status"] in ("queued", "running") for j in list(JOBS.values())):
                return self.send_json(409, {"error": {"message": "That model has a job running; unload after it finishes."}})
            return self.send_json(*device("POST", path, timeout=120))
        # Same call the TiinyOS app makes; it returns once the load is accepted.
        return self.send_json(*device("POST", path, {"model_id": mid}, timeout=120))

    def do_POST(self):
        if not self.authorized():
            return
        if self.path == "/api/settings":
            return self.save_settings()
        if self.path == "/api/open-folder":
            if not can_open_folder(self.client_address[0]):
                return self.send_json(403, {"error": {"message": "The folder can only be opened on the computer running the studio."}})
            open_folder(IMAGES)
            return self.send_json(200, {"ok": True})
        if self.path == "/api/settings/test":
            return self.test_settings()
        m = re.fullmatch(r"/api/settings/(delete|move|activate|auto|swap|check)", self.path)
        if m:
            return self.device_action(m.group(1))
        if self.path == "/api/model/launch":
            return self.model_action("launch")
        if self.path == "/api/model/stop":
            return self.model_action("stop")
        if self.path == "/api/device/shutdown":
            # Same call the TiinyOS app's "Shut down" makes; the device answers and powers off seconds later.
            if any(j["status"] in ("queued", "running") for j in list(JOBS.values())):
                return self.send_json(409, {"error": {"message": "Jobs are still running or queued; power off after they finish."}})
            return self.send_json(*device("POST", "/api/v1/sys/shutdown/", timeout=20))
        if self.path == "/api/model/unload_all":
            if any(j["status"] in ("queued", "running") for j in list(JOBS.values())):
                return self.send_json(409, {"error": {"message": "Jobs are still running or queued; unload after they finish."}})
            # Same call the TiinyOS app's "unload all" makes.
            return self.send_json(*device("POST", "/api/v1/models/unload_all", timeout=180))
        if self.path == "/api/jobs":
            try:
                req = self.read_json()
                if req.get("kind") == "chat" and req.get("voice_id"):  # the chat's speak tool uses a saved voice
                    req["voice"] = voice_for(req.pop("voice_id"))
                return self.send_json(200, MEDIA.submit(req.pop("kind", ""), req))
            except (ValueError, JobError) as e:
                return self.send_json(400, {"error": {"message": str(e)}})
        m = re.fullmatch(r"/api/jobs/(\w+)/cancel", self.path)
        if m:
            return self.send_json(200 if MEDIA.cancel(m.group(1)) else 404, {"ok": True})
        if self.path == "/api/voices":
            return self.save_voice()
        m = re.fullmatch(r"/api/chats/(\w+)(/delete)?", self.path)
        if m:
            path = CHATS / f"{m.group(1)}.json"
            if m.group(2):  # gone completely: the file is overwritten before removal
                if path.is_file():
                    shred(path)
                return self.send_json(200, {"ok": True})
            try:
                convo = self.read_json()
            except ValueError:
                return self.send_json(400, {"error": {"message": "bad chat"}})
            convo.update(id=m.group(1), updated=int(time.time()))
            save_json(path, convo)
            return self.send_json(200, {"ok": True})
        if self.path == "/api/extract":
            return self.extract_document()
        if self.path == "/api/privacy/wipe":
            return self.wipe_everything()
        if self.path == "/api/history/delete":
            return self.delete_entries((self.read_json().get("ids") or []))
        m = re.fullmatch(r"/api/history/(\w+)/fav", self.path)
        if m:
            fav = bool(self.read_json().get("fav", True))
            edit_history(m.group(1), lambda e: e.update(fav=fav) if fav else e.pop("fav", None))
            return self.send_json(200, {"ok": True, "fav": fav})
        if self.path == "/api/export":
            return self.export_zip(self.read_json().get("ids") or [])
        if self.path == "/api/search":
            return self.search(self.read_json())
        self.send_json(404, {"error": {"message": "not found"}})

    def save_settings(self):
        """Add or update a saved device ({id?, name, url, key}); without an id on first run, the active one."""
        try:
            req = self.read_json()
            did = req.get("id") or (None if req.get("new") else SETTINGS.active)
            SETTINGS.upsert(did, req.get("name"), req.get("url"), (req.get("key") or "").strip())
        except ValueError as e:
            return self.send_json(400, {"error": {"message": str(e)}})
        threading.Thread(target=SETTINGS.check, daemon=True).start()
        self.send_json(200, SETTINGS.public())

    def device_action(self, action):
        try:
            req = self.read_json()
        except ValueError:
            req = {}
        did = req.get("id")
        if action == "delete":
            SETTINGS.remove(did)
        elif action == "move":
            SETTINGS.move(did, -1 if req.get("dir", -1) < 0 else 1)
        elif action == "activate":
            SETTINGS.activate(did)
        elif action == "auto":
            SETTINGS.set_auto(req.get("auto", True))
        elif action == "swap":
            SETTINGS.set_swap(req)
        elif action == "check":
            SETTINGS.check()
        self.send_json(200, SETTINGS.public())

    def test_settings(self):
        """Try a device address/key (typed or saved) without saving it."""
        try:
            req = self.read_json()
        except ValueError:
            req = {}
        saved = next((d for d in SETTINGS.devices if d["id"] == req.get("id")), None) or SETTINGS.current() or {}
        url = normalize_url(req.get("url")) or saved.get("url", "")
        key = (req.get("key") or "").strip()
        if not key and urlparse(url).hostname == urlparse(saved.get("url", "")).hostname:
            key = saved.get("key", "")  # a saved key is only ever tried against its own host
        if not url:
            return self.send_json(200, {"ok": False, "message": "Enter the device address."})
        code, res = device("GET", "/v1/models", timeout=8, url=url, key=key)
        if code == 200:
            # any OpenAI-style relay answers /v1/models; only a Tiiny has its model controls
            if device("GET", "/api/v1/models/running", timeout=8, url=url, key=key)[0] != 200:
                return self.send_json(200, {"ok": False, "message": NOT_A_TIINY})
            names = [m.get("id", "").split("/")[-1] for m in res.get("data", [])]
            return self.send_json(200, {"ok": True, "message": f"Connected. {len(names)} model(s) loaded" + (f": {', '.join(names[:4])}" if names else "."), "loaded": names})
        if code in (401, 403):
            return self.send_json(200, {"ok": False, "message": ("The device rejected the API key. Copy the API key (not the device ID) from TiinyOS: avatar → Settings → API Key." if key else "The device needs an API key.")})
        if STUDIO_DOCKER and (urlparse(url).hostname or "").startswith("172.17."):
            return self.send_json(200, {"ok": False, "message": DOCKER_USB})
        msg = (res.get("error") or {}).get("message") if isinstance(res.get("error"), dict) else res.get("error")
        return self.send_json(200, {"ok": False, "message": msg or f"Unexpected response (HTTP {code})."})

    def delete_entries(self, ids):
        ids = set(ids)
        with LOCK:
            items = load_history()
            for e in items:
                if e["id"] in ids:
                    for f in e.get("outputs", []) + e.get("sources", []) + e.get("downloads", []):
                        shred(IMAGES / f)
                    JOBS.pop(e["id"], None)
                    SEARCH_CACHE.pop(e["id"], None)
            save_history([e for e in items if e["id"] not in ids])
        self.send_json(200, {"ok": True, "deleted": len(ids)})

    def export_zip(self, ids):
        """Zip the chosen results (files + an info.json of their settings), built in memory, never stored."""
        import io, zipfile
        with LOCK:
            items = [e for e in load_history() if e["id"] in set(ids)]
        buf = io.BytesIO()
        with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as z:
            for e in items:
                stamp = time.strftime("%Y%m%d-%H%M%S", time.localtime(e.get("created", 0)))
                for n, f in enumerate(e.get("outputs", []) + e.get("downloads", [])):
                    path = IMAGES / f
                    if path.exists():
                        z.write(path, f"{stamp}_{e.get('kind', 'image')}_{e['id']}_{n}{path.suffix}")
            z.writestr("info.json", json.dumps([{k: e.get(k) for k in ("id", "kind", "mode", "created", "params", "text", "duration", "seconds")} for e in items], indent=1))
        data = buf.getvalue()
        self.send_response(200)
        self.send_header("Content-Type", "application/zip")
        self.send_header("Content-Disposition", f'attachment; filename="tiiny-studio-{time.strftime("%Y%m%d-%H%M%S")}.zip"')
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(data)

    def search(self, req):
        """Rank saved results by meaning with the device's embedding model (vectors kept in memory only)."""
        query = (req.get("query") or "").strip()
        code, cat = catalog()
        emb = [mm for mm in cat.get("models", []) if mm.get("type") == "Text Embedding"]
        model = next((mm["id"] for mm in emb if mm["status"] == "running"), None)
        if not model:
            return self.send_json(409, {"error": {"message": "Smart search needs the embedding model loaded."}, "need_model": emb[0]["id"] if emb else None})
        with LOCK:
            items = [e for e in load_history() if entry_text(e)]
        try:
            todo = [e for e in items if SEARCH_CACHE.get(e["id"], (None,))[0] != entry_text(e)]
            for e, v in zip(todo, embed(model, [entry_text(e) for e in todo])):
                SEARCH_CACHE[e["id"]] = (entry_text(e), v)
            q = embed(model, [query])[0]
        except JobError as e:
            return self.send_json(502, {"error": {"message": str(e)}})
        norm = lambda v: sum(x * x for x in v) ** 0.5 or 1
        qn = norm(q)
        scored = sorted(((sum(a * b for a, b in zip(q, SEARCH_CACHE[e["id"]][1])) / (qn * norm(SEARCH_CACHE[e["id"]][1])), e["id"]) for e in items), reverse=True)
        self.send_json(200, {"results": [{"id": i, "score": round(s, 4)} for s, i in scored]})

    def wipe_everything(self):
        """Delete every result, source and saved voice (settings are kept)."""
        if any(j["status"] in ("queued", "running") for j in list(JOBS.values())):
            return self.send_json(409, {"error": {"message": "Jobs are still running or queued; wipe after they finish."}})
        with LOCK:
            files = [p for p in IMAGES.iterdir() if p.is_file()]
            for p in files:
                shred(p)
            entries = len(load_history())
            voices = len(load_voices())
            chats = [p for p in CHATS.glob("*.json")]
            for p in chats:
                shred(p)
            save_history([])
            save_json(VOICES, [])
            JOBS.clear()
            SEARCH_CACHE.clear()
        self.send_json(200, {"ok": True, "entries": entries, "voices": voices, "files": len(files)})

    def save_voice(self):
        try:
            req = self.read_json()
            _, audio = Media.decode_uri(req.get("audio"))
        except (ValueError, JobError) as e:
            return self.send_json(400, {"error": {"message": str(e)}})
        vid = uuid.uuid4().hex[:10]
        (IMAGES / f"voice-{vid}.wav").write_bytes(audio)
        voice = {"id": vid, "name": (req.get("name") or "Voice").strip()[:60], "file": f"voice-{vid}.wav",
                 "text": (req.get("text") or "").strip(), "created": int(time.time())}
        with LOCK:
            voices = load_voices()
            voices.insert(0, voice)
            VOICES.write_text(json.dumps(voices, indent=1), "utf-8")
        self.send_json(200, voice)

    def do_DELETE(self):
        if not self.authorized():
            return
        m = re.fullmatch(r"/api/voices/(\w+)", self.path)
        if m:
            with LOCK:
                voices = load_voices()
                for v in voices:
                    if v["id"] == m.group(1):
                        shred(IMAGES / v["file"])
                save_json(VOICES, [v for v in voices if v["id"] != m.group(1)])
            return self.send_json(200, {"ok": True})
        m = re.fullmatch(r"/api/history/(\w+)", self.path)
        if not m:
            return self.send_json(404, {"error": {"message": "not found"}})
        with LOCK:
            items = load_history()
            keep = [e for e in items if e["id"] != m.group(1)]
            for e in items:
                if e["id"] == m.group(1):
                    for f in e.get("outputs", []) + e.get("sources", []) + e.get("downloads", []):
                        shred(IMAGES / f)
            save_history(keep)
            SEARCH_CACHE.pop(m.group(1), None)
            JOBS.pop(m.group(1), None)  # the finished job's in-memory copy (prompt, label) goes too
        self.send_json(200, {"ok": True})


if __name__ == "__main__":
    if not SETTINGS.configured:
        print("No device configured yet: open the studio and fill in Settings.", flush=True)
    print(f"Tiiny Studio {VERSION} on :{PORT} ->{SETTINGS.url or '(not configured)'}"
          f"{' (password protected)' if PASSWORD else ''}", flush=True)
    if LAN_ACCESS and not PASSWORD:
        print("WARNING: reachable from your network without a password (set STUDIO_PASSWORD).", flush=True)
    print(f"Access: {'your network (LAN)' if LAN_ACCESS else 'this computer only'}", flush=True)
    url = f"http://localhost:{PORT}/"
    try:
        # On Windows SO_REUSEADDR lets a second process bind the same port, hiding "already running"
        httpd = type("Server", (ThreadingHTTPServer,), {"allow_reuse_address": os.name != "nt", "daemon_threads": True})((HOST, PORT), Handler)
    except OSError:
        # Usually the studio is already running (started twice): just show it.
        print(f"Port {PORT} is already in use. If Tiiny Studio is already running, it's at {url}", flush=True)
        if FROZEN and not os.environ.get("TIINY_NO_BROWSER"):
            import webbrowser
            webbrowser.open(url)
            time.sleep(5)  # leave the message on screen briefly
        sys.exit(1)
    threading.Thread(target=SETTINGS.monitor, daemon=True).start()
    if FROZEN:
        print(f"Data folder: {DATA}\nOpen {url} in your browser. Close this window to stop the studio.", flush=True)
        if not os.environ.get("TIINY_NO_BROWSER"):
            import webbrowser
            threading.Timer(1.0, webbrowser.open, (url,)).start()
    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        # stop what we started on the Tiiny: it keeps rendering if we just disappear
        for job in [j for j in list(JOBS.values()) if j["status"] == "running"]:
            job["cancel"] = True
            MEDIA.halt_device(job["kind"], job["model"])
