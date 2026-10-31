"""A fake Tiiny device for contract tests: answers what Tiiny Studio asks the real one, with canned data.
Usage: python fake_tiiny.py [--port N] [--key KEY] [--launch-delay SECONDS]
Or import it: `server, url = fake_tiiny.start()`; `server.state` holds the models, tasks and request log.
Every request is recorded (minus the Authorization header): GET /__requests, POST /__reset (no auth).
POST /__flags {..} switches on test behaviour until the next reset: "fail" {path: [status, message]}, "delay" {path: seconds},
"html" [paths] (answer 200 with a web page instead of JSON, like a login portal or proxy in the way),
"closed_port", "no_music_gateway", "speech_text", "task_polls" N, "task_hold", "fail_stage" name, "rich_texture", "song_tracks",
"disk_free_gb" N (free storage in /api/v1/sys/status; plenty by default), "no_disk" (sys/status without disk keys).
"task_flaky" N / "artifact_flaky" N (the next N 3D task polls / artifact fetches answer 502), "ignore_cancel" (a 3D task that
never stops), "image_warmup" N (the next N image requests fail like Qwen-Image just after loading), "cutout_png" (images
come back as RGBA cut-outs), "video_flaky" N / "content_flaky" N (the next N video polls answer 502 / downloads 503),
"video_hold" (a clip renders until it's cancelled).
POST /__seed {"videos": [{"id", "status", "age", "seconds"}], "sessions": [{"session_id", "model_id", "route", "status", "age"}]} plants results the studio never
saw (a finished clip, music sessions made `age` seconds ago), like ones left on the device by a restart or another studio.
Everything except GET / and /__* needs `Authorization: Bearer <key>`, like the device. stdlib only."""
import argparse
import base64
import hashlib
import io
import json
import re
import struct
import sys
import threading
import time
import wave
import zlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, unquote, urlparse


def tiny_png():
    raw = b"\x00\xff\x80\x20"  # one RGB pixel, filter byte 0
    def chunk(t, d):
        c = struct.pack(">I", len(d)) + t + d
        return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))


def tiny_wav(seconds=0.25, rate=24000):
    buf = io.BytesIO()
    with wave.open(buf, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(b"\x00\x00" * int(seconds * rate))
    return buf.getvalue()


def rgba_png(w, h, pixels, filt=4):
    """An 8-bit RGBA PNG, rows filtered with `filt` (Paeth by default, as image models write them)."""
    stride, rows, prev = w * 4, b"", bytes(w * 4)
    for y in range(h):
        line = pixels[y * stride:(y + 1) * stride]
        out = bytearray()
        for i in range(stride):
            a = line[i - 4] if i >= 4 else 0
            b, c = prev[i], (prev[i - 4] if i >= 4 else 0)
            p = a + b - c
            pred = a if abs(p - a) <= abs(p - b) and abs(p - a) <= abs(p - c) else b if abs(p - b) <= abs(p - c) else c
            out.append((line[i] - {0: 0, 1: a, 2: b, 3: (a + b) // 2, 4: pred}[filt]) & 255)
        rows += bytes([filt]) + bytes(out)
        prev = line

    def chunk(t, d):
        c = struct.pack(">I", len(d)) + t + d
        return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))


PNG, WAV = tiny_png(), tiny_wav()
# 3x2: a red subject (opaque, half-transparent edge) on purple background noise with alpha 1-2
CUTOUT_PNG = rgba_png(3, 2, bytes([169, 57, 212, 1, 200, 30, 30, 255, 125, 78, 201, 2,
                                   10, 20, 30, 0, 200, 30, 30, 128, 90, 90, 90, 31]))
GLB = b"glTF" + struct.pack("<II", 2, 12)  # header only: enough to be recognisable, not to be loaded
MP3 = b"ID3" + bytes(2000)
MP4 = struct.pack(">I", 24) + b"ftypisom" + bytes(12) + bytes(3000)  # an ftyp box and padding: recognisable, not playable
VIDEO = "lightx2v/MiniMax-H3-Turbo"
# /v1/video/info as probed on 0.1.38 (trimmed)
VIDEO_MAX_SECONDS = 7  # 0.1.38: 8 s and longer fail with "no exported bucket fits"


def rendered_length(secs):
    """A finished clip reports the length it really has: 0.1.38 rendered 2 s as 56 frames, "2.29167" seconds."""
    frames = secs * 24 + 8
    return {"seconds": str(round((frames - 1) / 24, 5)), "frames": frames}


VIDEO_INFO = {"family": "minimax_h3", "id": "MiniMax-H3-Turbo", "fps": 24, "default_steps": 7, "step_options": [4, 5, 6, 7, 8], "max_keyframes": 2,
              "audio": {"channels": 2, "sample_rate": 32000},
              "canvases": [{"height": 768, "width": 1344}, {"height": 1344, "width": 768}, {"height": 480, "width": 832}, {"height": 832, "width": 480}]}
CHAT_MODEL = "Qwen/Qwen3-4B-Instruct"


# the catalog's "thinking" object (TiinyOS 0.1.36+), one per kind of chat model seen on 0.1.38
NO_THINKING = {"supported": False, "toggleable": False, "enabled_by_default": False, "levels": [], "default_level": None}
SWITCH = {"supported": True, "toggleable": True, "enabled_by_default": True, "levels": [], "default_level": None}  # Qwen3.5/3.6
ALWAYS = {"supported": True, "toggleable": False, "enabled_by_default": True, "levels": ["low", "medium", "high"], "default_level": "medium"}  # gpt-oss
CAPS = {"Text Generation": ["main"], "Text-to-Image": ["image"], "Text-to-Speech": ["voice"], "ASR": ["audio"], "Music Generation": ["music"],
        "Image-to-3D": ["Image", "3d"], "OCR": ["ocr"], "Embedding": ["embedding"]}


# /v1/music/info as probed on 0.1.38 (trimmed): the service tells the families apart; Foundation-1 has no config schema
COMMON_SONG_KEYS = ["seconds_total", "steps", "llm_cfg_scale", "seed", "lyrics", "caption", "generation_type", "reference_audio_base64",
                    "temperature", "top_k", "top_p", "record_tokens", "record_window", "preview_seconds"]


def song_info(service, steps, temperature, top_k, top_p, duration, max_steps):
    return {"service_name": service, "config_schema": {"common": COMMON_SONG_KEYS},
            "default_config": {"generation_type": "mixed", "steps": steps, "temperature": temperature, "top_k": top_k, "top_p": top_p},
            "model_info": {"model": {"codec_llm_duration": duration, "max_decoder_steps": max_steps}}}


MUSIC_INFO = {"Fake/SongGeneration-v2": song_info("SongGenerationTTMService", 10, 1.0, 250, 0.0, 270, 512),
              "ACE-Step/AceStep-Fake": song_info("AceStepTTMService", 8, 0.85, 0, 0.9, 480, 20),
              "Fake/YuE2": song_info("YuE2TTMService", 32, 1.0, 100, 0.95, 0, 512),
              "Fake/Foundation-1": {"service_name": "FoundationTTMService", "model_info": {"audio": {"sample_rate": 44100}}}}
PLAIN_SONG = ("ACE-Step/AceStep-Fake", "Fake/YuE2")  # only the gateway's plain /generate: no preview or resume


# /v1/capabilities of the voice models as probed on 0.1.38 (trimmed): clone, built-in speakers, a voice from words
def tts_caps(clone=False, speakers=(), design=False, instruct=False):
    return {"support_voice_clone": clone, "support_custom_voice": bool(speakers), "support_voice_design": design,
            "support_instruct": instruct, "supported_speakers": list(speakers) or ["aiden", "vivian"], "sample_rate": 24000,
            "supported_languages": ["en", "zh", "german", "beijing_dialect"], "service": "tts"}


TTS_CAPS = {"Qwen/Qwen3-TTS-Base": tts_caps(clone=True, instruct=True), "Fake/CustomVoice": tts_caps(speakers=("ryan", "ono_anna"), instruct=True),
            "Fake/VoiceDesign": tts_caps(design=True, instruct=True), "Fake/Broken-TTS": tts_caps(speakers=("F1", "M1"))}


def model(mid, name, mtype, npu, supported=(), inp="text", out="text", thinking=NO_THINKING):
    return {"id": mid, "display_name": name, "params": "4B", "type": mtype, "input": inp, "output": out, "supported": list(supported),
            "desc": f"Fake {name}", "npu_usage": npu, "load_time": 20, "costMemory": npu * 100, "size": 1 << 30, "version": 1,  # the device sends numbers
            "thinking": thinking, "reasoning_levels": thinking["levels"], "capabilities": CAPS.get(mtype, [])}


CATALOG = [
    model(CHAT_MODEL, "Qwen3 4B", "Text Generation", 30, ["Tool Use"], thinking=SWITCH),
    model("Fake/Always-Thinks", "Always Thinks", "Text Generation", 25, ["Reasoning", "Tool Use"], thinking=ALWAYS),
    model("Fake/No-Thinking", "No Thinking", "Text Generation", 20, ["Tool Use"]),
    model("Qwen/Qwen-Image-2.1", "Qwen Image", "Text-to-Image", 60, ["Image Editing"], "text", "image"),
    model("Qwen/Qwen3-TTS-Base", "Qwen3 TTS", "Text-to-Speech", 15, [], "text", "audio"),
    model("Fake/CustomVoice", "CustomVoice", "Text-to-Speech", 7, [], "text", "audio"),
    model("Fake/VoiceDesign", "VoiceDesign", "Text-to-Speech", 5, [], "text", "audio"),
    model("Fake/Broken-TTS", "Broken TTS", "Text-to-Speech", 0, [], "text", "audio"),
    model("Fake/Whisper-ASR", "Whisper ASR", "ASR", 10, [], "audio", "text"),
    model("Fake/Foundation-1", "Foundation-1", "Music Generation", 25, [], "text", "audio"),
    model("Fake/SongGeneration-v2", "SongGeneration", "Music Generation", 45, [], "text", "audio"),
    model("ACE-Step/AceStep-Fake", "AceStep", "Music Generation", 40, [], "text", "audio"),
    model("Fake/YuE2", "YuE2 Song", "Music Generation", 14, [], "text,audio", "audio"),
    model("Tencent/Hunyuan3D-2.1", "Hunyuan3D", "Image-to-3D", 50, [], "image", "3d"),
    model("microsoft/TRELLIS.2-4B", "TRELLIS.2", "Image-to-3D", 6, [], "image", "3d"),
    model(VIDEO, "MiniMax-H3-Turbo", "Text-to-Video", 49, [], "text,image", "video"),
    model("Fake/PP-OCRv6", "PP-OCRv6", "OCR", 5, [], "image", "text"),
    model("Qwen/Qwen3-Embedding-0.6B", "Qwen3 Embedding", "Embedding", 1),
]


class State:
    def __init__(self, key, launch_delay, running):
        self.key, self.launch_delay, self.initial = key, launch_delay, tuple(running)
        self.lock = threading.Lock()
        self.port = 0
        self.reset()

    def reset(self):
        with self.lock:
            self.requests, self.shutdowns = [], 0
            self.running = {mid: self._instance(mid) for mid in self.initial}
            self.pending, self.tasks, self.sessions, self.videos = set(), {}, {}, {}
            self.session_meta = {}  # session id -> model_id, route, created_at (what GET /v1/music/sessions adds)
            self.flags = {}

    def _instance(self, mid):
        m = next(m for m in CATALOG if m["id"] == mid)
        return {"model_id": mid, "port": self.port, "npu_usage": m["npu_usage"], "active_request_count": 0}

    def used(self):
        return sum(r["npu_usage"] for r in self.running.values())

    def launch(self, mid):
        with self.lock:
            if mid in self.running or mid in self.pending:
                return
            self.pending.add(mid)

        def done():
            with self.lock:
                if mid in self.pending:
                    self.pending.discard(mid)
                    self.running[mid] = self._instance(mid)
        threading.Timer(self.launch_delay, done).start()

    def stop(self, mid):
        with self.lock:
            self.running.pop(mid, None)
            self.pending.discard(mid)


def parse_multipart(ctype, body):
    """{'fields': {name: text}, 'files': {name: filename}, 'sums': {name: sha1 of the file}} of a multipart body."""
    m = re.search(r"boundary=([^;]+)", ctype or "")
    out = {"fields": {}, "files": {}, "sums": {}}
    if not m:
        return out
    for part in body.split(b"--" + m.group(1).strip('"').encode())[1:-1]:
        head, _, data = part.partition(b"\r\n\r\n")
        h = head.decode("utf-8", "replace")
        name = re.search(r'name="([^"]*)"', h)
        fn = re.search(r'filename="([^"]*)"', h)
        if name and fn:
            out["files"][name.group(1)] = fn.group(1)
            out["sums"][name.group(1)] = hashlib.sha1(data[:-2]).hexdigest()
        elif name:
            out["fields"][name.group(1)] = data.rstrip(b"\r\n").decode("utf-8", "replace")
    return out


def last_user_text(body):
    msgs = body.get("messages") or []
    if not msgs or msgs[-1].get("role") != "user":
        return ""
    c = msgs[-1].get("content")
    if isinstance(c, list):
        c = " ".join(p.get("text", "") for p in c if isinstance(p, dict))
    return c or ""


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    # ---------- plumbing ----------
    def disk(self):
        flags = self.st.flags
        if flags.get("no_disk"):
            return {}
        total = 921123848192
        return {"total_bytes": total, "used_bytes": total - int(flags.get("disk_free_gb", 400) * 1e9)}

    @property
    def st(self):
        return self.server.state

    def send_bytes(self, code, data, ctype):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def send_json(self, code, obj):
        self.send_bytes(code, json.dumps(obj).encode(), "application/json")

    def err(self, code, msg):
        self.send_json(code, {"error": {"message": msg}})

    def handle_any(self, method):
        u = urlparse(self.path)
        path = u.path
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n) if n else b""
        ctype = self.headers.get("Content-Type") or ""
        body, extra = None, {}
        if "json" in ctype and raw:
            try:
                body = json.loads(raw)
            except ValueError:
                pass
        elif ctype.startswith("multipart/"):
            extra = {"multipart": parse_multipart(ctype, raw)}
        if path.startswith("/__"):
            return self.control(method, path, body)
        with self.st.lock:
            self.st.requests.append({"method": method, "path": path, "query": parse_qs(u.query), "body": body, **extra})
        open_paths = method == "GET" and (path == "/" or path.endswith("/icon"))  # the device serves model icons without a key
        # a model container's own port (what Studio reaches directly over Wi-Fi/LAN) takes no key
        open_paths = open_paths or path in ("/v1/generate", "/v1/generate/wav") or path.startswith("/v1/sessions")
        # a voice model's own port (no key): only while Broken-TTS is loaded, so the gateway still refuses keyless calls
        tts_port = path in ("/v1/models", "/v1/audio/speech") and not self.headers.get("Authorization") and "Fake/Broken-TTS" in self.st.running
        open_paths = open_paths or tts_port
        if not open_paths:
            if self.headers.get("Authorization") != f"Bearer {self.st.key}":
                return self.err(401, "Unauthorized")
        self.body, self.multi, self.query = body if isinstance(body, dict) else {}, extra.get("multipart"), parse_qs(u.query)
        flags = self.st.flags
        if path in flags.get("delay", {}):  # a slow model; the live session lets progress polling see something
            self.st.sessions["live"] = "running"
            time.sleep(flags["delay"][path])
            self.st.sessions.pop("live", None)
        if path in flags.get("fail", {}):
            return self.err(*flags["fail"][path])
        if path in flags.get("html", ()):
            return self.send_bytes(200, b"<html><body>Please log in</body></html>", "text/html")
        if tts_port:  # Broken-TTS's container, like Supertonic-3's on 0.1.38: it works under its own name only
            if method == "GET":
                return self.send_json(200, {"model_config": {"id": "Broken-TTS"}})
            if self.body.get("model") != "Broken-TTS":
                return self.send_json(400, {"error": {"message": f"Model '{self.body.get('model')}' not found"}})
            return self.send_bytes(200, WAV, "audio/wav")
        if not self.route(method, unquote(path)):
            self.err(404, "endpoint not found")

    def control(self, method, path, body=None):
        if method == "GET" and path == "/__requests":
            with self.st.lock:
                return self.send_json(200, list(self.st.requests))
        if method == "POST" and path == "/__reset":
            self.st.reset()
            return self.send_json(200, {"ok": True})
        if method == "POST" and path == "/__reset_log":  # clear the request log only (models stay loaded)
            with self.st.lock:
                self.st.requests = []
            return self.send_json(200, {"ok": True})
        if method == "POST" and path == "/__seed":
            return self.seed(body or {})
        if method == "POST" and path == "/__flags":
            self.st.flags.update(body or {})
            return self.send_json(200, {"ok": True})
        self.err(404, "not found")

    def seed(self, b):
        st, now = self.st, time.time()
        for v in b.get("videos", []):
            vid = v.get("id") or f"video_{len(st.videos) + 1:032x}"
            secs = int(v.get("seconds") or 2)
            st.videos[vid] = {"id": vid, "object": "video", "model": v.get("model") or VIDEO, "status": v.get("status") or "completed", "progress": 100,
                              "size": "832x480", **rendered_length(secs), "fps": 24, "error": None, "looks": 0, "created_at": int(now - v.get("age", 60))}
        for s in b.get("sessions", []):
            sid = s.get("session_id") or f"sess-{len(st.sessions) + 1}"
            st.sessions[sid] = s.get("status") or "completed"
            st.session_meta[sid] = {"model_id": s.get("model_id") or "Fake/SongGeneration-v2", "route": s.get("route") or "preview", "created_at": now - s.get("age", 60)}
        self.send_json(200, {"ok": True})

    do_GET = lambda self: self.handle_any("GET")
    do_POST = lambda self: self.handle_any("POST")
    do_DELETE = lambda self: self.handle_any("DELETE")
    do_PATCH = lambda self: self.handle_any("PATCH")
    do_PUT = lambda self: self.handle_any("PUT")

    # ---------- routes ----------
    def route(self, method, p):
        st = self.st
        if method == "GET" and p == "/":
            self.send_bytes(200, b"<html><body>Fake Tiiny status page</body></html>", "text/html; charset=utf-8")
        elif method == "GET" and p == "/api/v1/models/":
            self.send_json(200, {"data": [{**m, "status": "running" if m["id"] in st.running else "stopped"} for m in CATALOG]})
        elif method == "GET" and p == "/api/v1/models/online_models":
            # the device answers a bare list here, keyed by model_id
            self.send_json(200, [{**{k: v for k, v in m.items() if k not in ("id", "display_name", "costMemory", "size", "version")}, "model_id": m["id"], "name": m["display_name"]} for m in CATALOG])
        elif method == "GET" and re.fullmatch(r"/api/v1/models/online_models/.+/icon", p):
            self.send_bytes(200, PNG, "image/png")
        elif method == "GET" and p == "/api/v1/models/running":
            with st.lock:
                running = [{**r, "port": 1} if st.flags.get("closed_port") else r for r in st.running.values()]  # a link that only exposes the gateway
                self.send_json(200, {"instances": {"running": running, "pending": [{"model_id": m} for m in st.pending]},
                                     "pending": []})
        elif method == "GET" and p == "/api/v1/models/types":
            self.send_json(200, {"data": sorted({m["type"] for m in CATALOG})})
        elif method == "GET" and (m := re.fullmatch(r"/api/v1/models/(.+)/unload_candidate", p)):
            self.unload_candidate(m.group(1))
        elif method == "POST" and p == "/api/v1/models/unload_all":
            with st.lock:
                st.running.clear()
                st.pending.clear()
            self.send_json(200, {"status": "ok"})
        elif method == "POST" and (m := re.fullmatch(r"/api/v1/models/(.+)/(launch|stop)", p)):
            if not any(c["id"] == m.group(1) for c in CATALOG):
                return self.err(404, f"model {m.group(1)} not found") or True
            (st.launch if m.group(2) == "launch" else st.stop)(m.group(1))
            self.send_json(200, {"status": "ok", "message": m.group(2) + " accepted"})
        elif method == "POST" and (m := re.fullmatch(r"/api/v1/models/(.+)/interrupt", p)):
            # like 0.1.36: only stops an in-flight chat completion
            self.send_json(200, {"success": False, "interrupted": 0, "models": [],
                                 "error_reason": f"No in-flight chat completion found for '{m.group(1)}'."})
        elif method == "GET" and p == "/v1/models":
            self.send_json(200, {"object": "list", "data": [
                {"id": mid, "object": "model", "aliases": ["default"] if mid == CHAT_MODEL else [], "supports_vision": False,
                 "supports_chat": mid == CHAT_MODEL, "context_window": 32768,
                 # an object since the 2026-10 TiinyOS update (was a bool)
                 "thinking": next(m for m in CATALOG if m["id"] == mid)["thinking"]} for mid in list(st.running)]})
        elif method == "GET" and p == "/api/tags":
            self.send_json(200, {"models": [{"name": m["id"]} for m in CATALOG]})
        elif method == "GET" and p == "/v1/capabilities":
            named = (self.query.get("model") or [None])[0]
            if not named:
                return self.send_json(200, {"capabilities": ["chat"]}) or True
            row = next((m for m in CATALOG if m["id"] == named), None)
            if not row or named not in st.running:  # like the device: only a loaded model answers
                return self.send_json(503, {"error": {"message": "No suitable model is currently running."}}) or True
            if row["type"] == "Text Generation":
                self.send_json(200, {"object": "capabilities", "service": "chat", "model": named, "supports_chat": True,
                                     "supported": row["supported"], "thinking": row["thinking"]})
            elif row["type"] == "Text-to-Speech":
                self.send_json(200, {**TTS_CAPS[named], "model": named})
            else:
                self.send_json(400, {"error": {"message": f"Model '{named}' does not support tts.", "type": "invalid_request_error"}})
        elif method == "POST" and p == "/api/v1/sys/shutdown/":
            with st.lock:
                st.shutdowns += 1
            self.send_json(200, {"status": "ok", "message": "System shutdown will be initiated in seconds via D-Bus"})  # 0.1.37+
        elif method == "GET" and p == "/api/v1/sys/status":
            self.send_json(200, {"cpu": {"total_percent": 12.5}, "memory": {"usage_percent": 40.0}, "gpu": {}, "npus": [], "disk": self.disk()})
        elif method == "GET" and p == "/api/v1/sys/device_info":
            self.send_json(200, {"device_name": "Fake Tiiny"})
        elif method == "GET" and p == "/api/v1/sys/ac_power/":
            self.send_json(200, {"status": "ok", "ac_power": False})
        elif method == "GET" and p == "/api/v1/npu/status":
            self.send_json(200, {"devices": [{"util_percent": 7, "mem_used_mb": st.used() * 100, "mem_total_mb": 10000, "temp_c": 41.5,
                                              "power_w": 9.5}], "cpu": {"total_percent": 12.5}, "memory": {"usage_percent": 40.0},
                                 "npu": {"status": "ok"}})
        elif method == "POST" and p == "/v1/chat/completions":
            self.chat()
        elif method == "POST" and p in ("/v1/images/generations", "/v1/images/edits"):
            if st.flags.get("image_warmup", 0) > 0:  # like Qwen-Image just after it reports "running"
                st.flags["image_warmup"] -= 1
                return self.send_json(500, {"error": {"message": "xh2a backend fail"}})
            img = CUTOUT_PNG if st.flags.get("cutout_png") else PNG  # an image model's cut-out: RGBA with faint noise
            self.send_json(200, {"created": int(time.time()), "data": [{"b64_json": base64.b64encode(img).decode()}]})
        elif method == "POST" and p == "/v1/audio/speech" and self.body.get("model") in ("Fake/CustomVoice", "Fake/VoiceDesign", "Fake/Broken-TTS"):
            # how the real ones answered on 0.1.38: CustomVoice wants a known "voice" and refuses "instruct"; VoiceDesign
            # needs "instruct" (the description); Supertonic-3 failed whatever it was sent
            b, mid = self.body, self.body.get("model")
            if mid == "Fake/Broken-TTS":
                self.send_json(500, {"error": {"message": "The upstream TTS model failed.", "type": "model_error"}})
            elif mid == "Fake/CustomVoice" and b.get("instruct"):
                self.send_json(500, {"error": {"message": "voice_design is not supported by this model", "type": "unsupported_capability"}})
            elif mid == "Fake/CustomVoice" and b.get("voice") not in (None, "ryan", "ono_anna"):
                self.send_json(500, {"error": {"message": f"Unsupported speaker: {b.get('voice')}", "type": "invalid_request"}})
            elif mid == "Fake/VoiceDesign" and not b.get("instruct"):
                self.send_json(500, {"error": {"message": "custom_voice is not supported by this model", "type": "unsupported_capability"}})
            else:
                self.send_bytes(200, WAV, "audio/wav")
        elif method == "POST" and p == "/v1/audio/speech":
            if st.flags.get("speech_text"):
                self.send_bytes(200, b"not audio at all", "text/plain")
            else:
                self.send_bytes(200, WAV, "audio/wav")
        elif method == "POST" and p == "/v1/audio/transcriptions":
            self.send_json(200, {"text": "fake transcript"})
        elif method == "POST" and p == "/v1/ocr":
            self.send_json(200, {"data": [{"text": "FAKE OCR", "lines": [{"text": "FAKE OCR", "score": 0.99}]}]})
        elif method == "POST" and p == "/v1/embeddings":
            inputs = self.body.get("input")
            inputs = inputs if isinstance(inputs, list) else [inputs]
            self.send_json(200, {"object": "list", "model": self.body.get("model"), "usage": {"prompt_tokens": 1, "total_tokens": 1},
                                 "data": [{"object": "embedding", "index": i, "embedding": [round((len(str(t)) + j) % 7 / 7, 4) for j in range(8)]}
                                          for i, t in enumerate(inputs)]})
        elif p.startswith(("/v1/music/", "/v1/generate", "/v1/sessions")):
            self.music(method, p)
        elif p.startswith(("/v1/video", "/v1/videos")):
            self.videos(method, p)
        elif p.startswith(("/v1/3d/", "/v1/tasks")):
            self.tasks3d(method, p)
        else:
            return False
        return True

    def unload_candidate(self, mid):
        m = next((c for c in CATALOG if c["id"] == mid), None)
        if not m:
            return self.err(404, f"model {mid} not found")
        with self.st.lock:
            victims, free = [], 100 - self.st.used()
            for rid, r in self.st.running.items():
                if free >= m["npu_usage"] or rid == mid:
                    break
                victims.append(rid)
                free += r["npu_usage"]
            self.send_json(200, {"unload_required": bool(victims), "model_id": victims, "npu_required": m["npu_usage"],
                                 "npu_available": 100 - self.st.used()})

    def chat(self):
        b = self.body
        text = last_user_text(b)
        tools = b.get("tools") or []
        if not b.get("stream"):
            msg = {"role": "assistant", "content": "Fake reply."}
            return self.send_json(200, {"id": "chatcmpl-fake", "object": "chat.completion", "created": int(time.time()), "model": b.get("model"),
                                        "choices": [{"index": 0, "message": msg, "finish_reason": "stop"}],
                                        "usage": {"prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8}})
        chunks = []
        if (b.get("chat_template_kwargs") or {}).get("enable_thinking", True) is not False and b.get("reasoning_effort") != "none":
            chunks += [{"reasoning_content": "Thinking"}, {"reasoning_content": " it over."}]
        finish = "stop"
        if "FAKE_TOOL" in text and tools:
            name = tools[0]["function"]["name"]
            chunks += [{"tool_calls": [{"index": 0, "id": "call_fake", "type": "function", "function": {"name": name, "arguments": ""}}]},
                       {"tool_calls": [{"index": 0, "function": {"arguments": "{}"}}]}]
            finish = "tool_calls"
        elif "FAKE_NO_ANSWER" in text:  # reasoned until max_tokens, never answered (gpt-oss-20b once on 0.1.38)
            chunks += [{"reasoning_content": " And on and on."}]
            finish = "length"
        else:
            chunks += [{"content": "Fake "}, {"content": "reply."}]
        out = b""
        for d in chunks:
            out += b"data: " + json.dumps({"id": "chatcmpl-fake", "object": "chat.completion.chunk", "choices": [{"index": 0, "delta": d, "finish_reason": None}]}).encode() + b"\n\n"
        out += b"data: " + json.dumps({"id": "chatcmpl-fake", "object": "chat.completion.chunk", "choices": [{"index": 0, "delta": {}, "finish_reason": finish}]}).encode() + b"\n\n"
        self.send_bytes(200, out + b"data: [DONE]\n\n", "text/event-stream")

    def song(self, sid):
        out = {"success": True, "audio_data": base64.b64encode(WAV).decode(), "session_id": sid, "duration": 0.25}
        if self.st.flags.get("song_tracks"):  # stems: "Mixed" duplicates the song, short ones are ignored
            stem = base64.b64encode(WAV + bytes(1000)).decode()
            out["audio_outputs"] = {"Vocals": stem, "Mixed": stem, "Bass Line": stem, "Tiny": "abc"}
        return out

    def videos(self, method, p):
        """The gateway's OpenAI-style video API (0.1.38): POST /v1/videos queues a clip, GET /v1/videos/{id} reports on it
        (done on the second look), /content is the MP4; a size the model wasn't exported for fails the clip, not the POST."""
        st, parts = self.st, p.strip("/").split("/")
        if p == "/v1/video/info":
            if (self.query.get("model") or [None])[0] != VIDEO or VIDEO not in st.running:
                return self.send_json(503, {"error": {"message": "No suitable model is currently running."}})
            return self.send_json(200, VIDEO_INFO)
        if method == "GET" and p == "/v1/videos":
            return self.send_json(200, {"object": "list", "data": [{k: x for k, x in v.items() if k != "looks"} for v in st.videos.values()]})
        if method == "POST" and p == "/v1/videos":
            b = self.body
            if not str(b.get("prompt") or "").strip():
                return self.send_json(400, {"error": {"message": "prompt must be a non-empty string", "type": "invalid_request_error", "param": "prompt"}})
            if b.get("model") not in st.running:
                return self.send_json(503, {"error": {"message": "No suitable model is currently running."}})
            vid = f"video_{len(st.videos) + 1:032x}"
            sizes = [f"{c['width']}x{c['height']}" for c in VIDEO_INFO["canvases"]]
            err = None if b.get("size") in sizes else {"code": "invalid_request", "message": f"this model was exported for [{', '.join(sizes)}], got {b.get('size')}"}
            secs = int(b.get("seconds") or 4)
            if not err and secs > VIDEO_MAX_SECONDS:  # accepted, then failed within seconds
                err = {"code": "invalid_request", "message": f"no exported bucket fits a request needing [audio={secs * 81}, cond_video=0, target_video={secs * 2808}]"}
            st.videos[vid] = {"id": vid, "object": "video", "model": b["model"], "status": "queued", "progress": 0, "size": b.get("size"),
                              "seconds": str(secs), "frames": secs * 24 + 1, "fps": 24, "error": err, "looks": 0, "created_at": int(time.time())}
            return self.send_json(200, {k: v for k, v in st.videos[vid].items() if k != "looks"})
        v = st.videos.get(parts[2]) if len(parts) > 2 else None
        if not v:
            return self.send_json(404, {"error": {"message": "video not found", "type": "not_found"}})
        if method == "DELETE" and len(parts) == 3:
            del st.videos[v["id"]]
            return self.send_json(200, {"id": v["id"], "object": "video.deleted", "deleted": True})
        if method == "GET" and st.flags.get("video_flaky", 0) > 0:  # the link drops for a few requests
            st.flags["video_flaky"] -= 1
            return self.send_json(502, {"error": {"message": "Bad gateway"}})
        if method == "GET" and len(parts) == 3:
            v["looks"] += 1
            if v["status"] != "cancelled" and not st.flags.get("video_hold"):  # video_hold: renders until cancelled
                v["status"], v["progress"] = ("failed", 0) if v["error"] else ("completed", 100) if v["looks"] > 1 else ("in_progress", 50)
                if v["status"] == "completed":
                    v.update(rendered_length(int(v["seconds"])))
            return self.send_json(200, {k: x for k, x in v.items() if k != "looks"})
        if method == "GET" and parts[3:] == ["content"]:
            if st.flags.get("content_flaky", 0) > 0:
                st.flags["content_flaky"] -= 1
                return self.send_json(503, {"error": {"message": "Service unavailable"}})
            if v["status"] != "completed":
                return self.send_json(409, {"error": {"message": "video is not ready"}})
            return self.send_bytes(200, MP4, "video/mp4")
        return self.send_json(404, {"error": {"message": f"Unknown video path: /{'/'.join(parts[2:])}", "type": "not_found"}})

    def music(self, method, p):
        st = self.st
        if method == "GET" and p in ("/v1/music/info", "/v1/music/sessions", "/v1/sessions"):
            if p == "/v1/music/info":
                named = (self.query.get("model") or [None])[0]
                if not named:
                    return self.send_json(200, {"models": [m["id"] for m in CATALOG if m["type"] == "Music Generation"]})
                if named not in st.running:
                    return self.send_json(503, {"error": {"message": "No suitable model is currently running."}})
                if named not in MUSIC_INFO:
                    return self.send_json(400, {"error": {"message": f"Model '{named}' does not support music."}})
                return self.send_json(200, {**MUSIC_INFO[named], "model": named})
            return self.send_json(200, {"sessions": [{"session_id": s, "status": v, "progress": 1.0, "metadata": {}, **{"model_id": None, "route": None, "created_at": time.time()}, **st.session_meta.get(s, {})}
                                                     for s, v in st.sessions.items()]})
        if method == "GET" and (m := re.fullmatch(r"/v1/music/sessions/([\w-]+)/download", p)):
            return self.send_bytes(200, MP3, "audio/mpeg")
        if method == "DELETE" and (m := re.fullmatch(r"/v1/music/sessions/([\w-]+)", p)):
            st.sessions.pop(m.group(1), None)
            return self.send_json(200, {"message": f"Session {m.group(1)} deleted"})
        if method == "DELETE" and (m := re.fullmatch(r"/v1/sessions/([\w-]+)", p)):
            st.sessions.pop(m.group(1), None)
            return self.send_json(200, {"success": True})
        if method == "POST" and p == "/v1/music/generate/wav" and st.flags.get("no_music_gateway"):
            return self.send_json(404, {"detail": "Not Found"})
        if method == "POST" and p in ("/v1/music/generate/wav", "/v1/generate/wav"):
            if p == "/v1/music/generate/wav":  # the gateway keeps a session for each loop
                sid = self.new_session("completed")
                st.session_meta[sid] = {"model_id": self.body.get("model"), "route": "generate_wav", "created_at": time.time()}
            return self.send_bytes(200, WAV, "audio/wav")
        if method == "POST" and p.startswith("/v1/music/generate/") and self.body.get("model") == "Fake/YuE2":
            return self.send_json(400, {"error": {"code": "unsupported_feature", "message": "YuE2 preview generation is not implemented"}})  # 0.1.38
        if method == "POST" and p.startswith("/v1/music/generate/") and str(self.body.get("model", "")).startswith("ACE-Step/"):
            return self.send_json(500, {"success": False, "audio_data": None, "error": "AceStep preview generation is not implemented"})  # TiinyOS 0.1.36
        if method == "POST" and p == "/v1/music/generate" and self.body.get("model") in PLAIN_SONG:
            # like the real ones: only the common keys at the top of config; AceStep's own go nested in config.ace_step
            config = self.body.get("config") or {}
            extra = [k for k in config if k not in COMMON_SONG_KEYS and not (k == "ace_step" and self.body["model"].startswith("ACE-Step/"))]
            if extra:
                return self.send_json(400, {"success": False, "audio_data": None, "error": f"Extra inputs are not permitted in config: {extra[0]}", "error_code": "INVALID_REQUEST"})
            if not isinstance((config.get("ace_step") or {}).get("bpm", 0), (int, float)):
                return self.send_json(400, {"success": False, "audio_data": None, "error": "type must be number, but is string", "error_code": "INVALID_REQUEST"})
        if method == "POST" and p in ("/v1/music/generate", "/v1/music/generate/preview", "/v1/music/generate/resume", "/v1/generate"):
            sid = self.body.get("session_id") or self.new_session("completed")
            st.sessions[sid] = "completed"
            st.session_meta.setdefault(sid, {"model_id": self.body.get("model"), "route": p.rsplit("/", 1)[-1] if p != "/v1/music/generate" else "generate", "created_at": time.time()})
            return self.send_json(200, self.song(sid))
        self.err(404, "endpoint not found")

    def new_session(self, status):
        n = len(self.st.sessions) + 1
        while f"sess-{n}" in self.st.sessions:
            n += 1
        self.st.sessions[f"sess-{n}"] = status
        return f"sess-{n}"

    def tasks3d(self, method, p):
        st = self.st
        named = (self.query.get("model") or [None])[0]  # the gateway picks the 3D model by ?model=
        trellis = named == TRELLIS if named else TRELLIS in st.running
        if method == "GET" and p == "/v1/3d/capabilities":
            return self.send_json(200, {"status": "success", "data": TRELLIS_CAPS if trellis else HUNYUAN_CAPS})
        if method == "POST" and p in ("/v1/3d/tasks", "/v1/tasks"):
            multi = self.multi or {}
            stage = multi.get("fields", {}).get("stage") or "shape"
            if trellis:  # like the real one: refuse a missing input, and params the stage doesn't list
                spec = TRELLIS_CAPS["stage_specs"].get(stage, {})
                refs = json.loads(multi.get("fields", {}).get("input_refs") or "{}")
                for name, r in refs.items():
                    if not (isinstance(r, dict) and r.get("task_id") in st.tasks and r.get("artifact_id")):
                        return self.send_json(400, {"error": f"Input reference {name} requires task_id and artifact_id", "error_code": "INVALID_REQUEST"})
                for name in spec.get("required_inputs", []):
                    if name not in refs and FORM_FIELD.get(name, name) not in multi.get("files", {}):
                        return self.send_json(400, {"error": f"Missing stage input '{name}'", "error_code": "INVALID_REQUEST"})
                extra = set(json.loads(multi.get("fields", {}).get("params") or "{}")) - set(spec.get("params", {})) - set(spec.get("controls", []))
                if extra:
                    return self.send_json(400, {"error": f"Unknown params for {stage}: {sorted(extra)}", "error_code": "INVALID_REQUEST"})
            tid = f"task-{len(st.tasks) + 1}"
            st.tasks[tid] = {"stage": stage, "polls": 0, "cancelled": False, "trellis": trellis}
            return self.send_json(200, {"data": {"task_id": tid, "status": "queued"}})
        m = re.fullmatch(r"/v1/(?:3d/)?tasks/([\w-]+)/artifacts/([\w:.-]+)", p)
        if method == "GET" and m:
            if st.flags.get("artifact_flaky", 0) > 0:  # the next n artifact fetches get a 502
                st.flags["artifact_flaky"] -= 1
                return self.err(502, "bad gateway")
            t = st.tasks.get(m.group(1))
            art = next((a for i, a in enumerate(TRELLIS_OUT.get(t["stage"], []) if t else []) if f"{m.group(1)}-{i}" == unquote(m.group(2))), None)
            return self.send_bytes(200, art[2], "application/octet-stream") if art else self.err(404, "artifact not found")
        m = re.fullmatch(r"/v1/(?:3d/)?tasks/([\w-]+)(/cancel)?", p)
        t = st.tasks.get(m.group(1)) if m else None
        if not t:
            return self.err(404, "task not found")
        if method == "POST" and m.group(2):
            t["cancelled"] = not st.flags.get("ignore_cancel")  # ignore_cancel: a device that never stops the task
            return self.send_json(200, {"data": {"task_id": m.group(1), "status": "cancelled"}})
        if method == "GET" and not m.group(2):
            if st.flags.get("task_flaky", 0) > 0:  # a dropped link: the next n polls get a 502 and don't count
                st.flags["task_flaky"] -= 1
                return self.err(502, "bad gateway")
            t["polls"] += 1  # queued -> running -> completed on successive polls
            status = "cancelled" if t["cancelled"] else "running" if t["polls"] < st.flags.get("task_polls", 2) or st.flags.get("task_hold") else "completed"
            if status == "completed" and st.flags.get("fail_stage") == t["stage"]:
                return self.send_json(200, {"data": {"task_id": m.group(1), "status": "failed", "error": "out of memory"}})
            d = {"task_id": m.group(1), "status": status, "progress": 100 if status == "completed" else 40, "message": status}
            if status == "completed" and t.get("trellis"):  # artifacts listed by id, fetched separately
                d["result"] = {"output": {"artifacts": [{"artifact_id": f"{m.group(1)}-{i}", "artifact_type": kind, "filename": name,
                                                         "mime_type": "model/gltf-binary" if name.endswith(".glb") else "application/zip",
                                                         "payload_key": "glb" if kind == "mesh_glb" else "preview_glb" if kind.endswith("_preview") else "artifact"}
                                                        for i, (kind, name, _) in enumerate(TRELLIS_OUT[t["stage"]])],
                                          "metadata": {"stage": t["stage"]}}}
            elif status == "completed" and t["stage"] == "texture" and st.flags.get("rich_texture"):
                d["result"] = {"output": {"artifacts": [{"artifact_type": "material_pbr", "filename": "mat.png", "mime_type": "image/png", "payload_key": "material"},
                                                         {"artifact_type": "mesh_glb", "filename": "textured.glb", "mime_type": "model/gltf-binary", "payload_key": "glb"}],
                                          "material": base64.b64encode(PNG).decode(), "glb": base64.b64encode(GLB + b"tex").decode(), "metadata": {"stage": "texture"}}}
            elif status == "completed":
                d["result"] = {"output": {"artifact_type": "mesh_glb", "filename": f"{t['stage']}.glb", "mime_type": "model/gltf-binary",
                                          "glb": base64.b64encode(GLB).decode(), "metadata": {"stage": t["stage"]}}}
            return self.send_json(200, {"data": d})
        self.err(404, "endpoint not found")


# /v1/3d/capabilities as the real models report them (TiinyOS 0.1.37), trimmed to what the studio reads.
HUNYUAN_CAPS = {"model": "hunyuan3d", "stages": ["background", "shape", "simplify", "texture", "export"],
                "inputs": {"background": ["image"], "shape": ["image"], "simplify": ["mesh"], "texture": ["image", "mesh"],
                           "export": ["material_artifact", "mesh_glb"]},
                "api_schema": "image-to-3d/v2"}
TRELLIS = "microsoft/TRELLIS.2-4B"
TRELLIS_CAPS = {"model": "trellis2", "stages": ["background", "shape", "simplify", "texture", "export"],
                "inputs": {"background": ["image"], "shape": ["image"], "simplify": ["shape_artifact"], "texture": ["shape_artifact"],
                           "export": ["geometry_artifact", "material_artifact", "mesh_glb", "shape_artifact"]},
                "stage_specs": {"background": {"required_inputs": ["image"], "params": {"return_mask": {}}, "controls": []},
                                "shape": {"required_inputs": ["image"], "params": {"pipeline_type": {}}, "controls": ["seed", "output_precision", "inference_quality"]},
                                "simplify": {"required_inputs": ["shape_artifact"], "params": {"simplify": {}, "target_faces": {}}, "controls": ["asset_quality"]},
                                "texture": {"required_inputs": ["shape_artifact"], "params": {"seed": {}, "inference_quality": {}}, "controls": ["seed", "inference_quality"]},
                                "export": {"required_inputs": ["material_artifact", "shape_artifact"], "params": {"output_name": {}, "texture_size": {}},
                                           "controls": ["asset_quality"]}},
                "input_refs": True, "api_schema": "image-to-3d/v2"}
# What each TRELLIS stage returns (artifact type, file, bytes), fetched by id: never inline.
TRELLIS_OUT = {"shape": [("shape", "trellis2_shape.zip", b"PK\x03\x04shape"), ("shape_preview", "trellis2_shape_preview.glb", GLB + b"shape")],
               "simplify": [("geometry", "trellis2_geometry.zip", b"PK\x03\x04geometry"), ("geometry_preview", "trellis2_geometry_preview.glb", GLB + b"geometry")],
               "texture": [("material", "trellis2_texture.zip", b"PK\x03\x04material")],
               "export": [("mesh_glb", "trellis2.glb", GLB + b"trellis")]}
FORM_FIELD = {"material_artifact": "material", "mesh_glb": "mesh"}  # the task form's own names for these inputs


def make_server(port=0, key="fake-key", launch_delay=0.2, running=(CHAT_MODEL,), host="127.0.0.1"):
    srv = ThreadingHTTPServer((host, port), Handler)
    srv.daemon_threads = True
    srv.state = State(key, launch_delay, running)
    srv.state.port = srv.server_address[1]  # model "containers" answer on the fake's own port
    srv.state.reset()
    return srv


def start(port=0, **kw):
    """Run a fake Tiiny in a thread. Returns (server, base_url); stop it with server.shutdown(); server.server_close()."""
    srv = make_server(port, **kw)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv, f"http://127.0.0.1:{srv.server_address[1]}"  # (a Docker container reaches it as host.docker.internal)


if __name__ == "__main__":
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--port", type=int, default=0)
    ap.add_argument("--key", default="fake-key")
    ap.add_argument("--launch-delay", type=float, default=0.2)
    a = ap.parse_args()
    srv = make_server(a.port, a.key, a.launch_delay)
    print(f"fake Tiiny on http://127.0.0.1:{srv.server_address[1]} (key {a.key!r})", flush=True)
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        sys.exit(0)
