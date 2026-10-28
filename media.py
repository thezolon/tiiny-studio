"""Media jobs for Tiiny Studio: voice-clone speech, transcription, music loops, songs, image-to-3D,
and chat-model helpers (prompt rewrite, image description, lyrics, loop tags).

Each job runs in a background thread so long work (songs, 3D) survives page reloads; the UI polls
/api/jobs. Model containers are reached on the port the device reports for their running instance
(found by probing TiinyOS 0.1.35, see README).
"""
import base64
import json
import os
import re
import socket
import threading
import time
import urllib.error
import urllib.request
import uuid
from urllib.parse import quote

JOBS = {}
JOBS_LOCK = threading.Lock()


class JobError(Exception):
    pass


class Media:
    def __init__(self, settings, media_dir, add_history, device_call, device_lock):
        self.settings = settings  # live device address + key (can change from the Settings screen)
        self.dir = media_dir
        self.add_history = add_history
        self.device = device_call  # (method, path, body, timeout) -> (code, obj)
        # The Tiiny serves one request at a time; parallel requests to different models just slow
        # each other down. One lock, shared with image generation, makes every job wait its turn.
        self.lock = device_lock

    # ---------- plumbing ----------
    def running_instance(self, model_id):
        code, run = self.device("GET", "/api/v1/models/running", None, 15)
        for r in (run.get("instances") or {}).get("running", []):
            if r.get("model_id") == model_id:
                return r
        raise JobError(f"{model_id.split('/')[-1]} isn't loaded. Load it from the Models panel.")

    def ensure_running(self, model_id):
        """For work routed through the gateway (port 80): just check the model is loaded."""
        self.running_instance(model_id)

    def instance_url(self, model_id):
        """Direct URL of a model's own container. Only the music models need this: the gateway has no
        route for them. Some links (the USB cable) only expose the gateway, so check the port first."""
        r = self.running_instance(model_id)
        if not self.port_open(self.settings.host, r.get("port")):
            raise JobError(f"{model_id.split('/')[-1]} is loaded, but its port isn't reachable at {self.settings.host}. "
                           "The USB link only exposes the device's gateway, which has no route for music models — "
                           "switch Settings to the device's Wi-Fi/LAN address to use them.")
        return f"http://{self.settings.host}:{r['port']}"

    @staticmethod
    def port_open(host, port, timeout=2):
        try:
            socket.create_connection((host, int(port)), timeout=timeout).close()
            return True
        except (OSError, TypeError, ValueError):
            return False

    def direct_access(self):
        """Can this connection reach model containers directly? None when nothing is loaded. Cached 30 s."""
        now = time.time()
        if now - getattr(self, "_da_t", 0) < 30 and getattr(self, "_da_host", None) == self.settings.host:
            return self._da
        code, run = self.device("GET", "/api/v1/models/running", None, 10)
        ports = [r.get("port") for r in (run.get("instances") or {}).get("running", []) if r.get("port")] if code == 200 else []
        self._da = self.port_open(self.settings.host, ports[0], timeout=1.5) if ports else None
        self._da_t, self._da_host = now, self.settings.host
        return self._da

    def gateway_headers(self):
        return {"Authorization": f"Bearer {self.settings.key}"}

    @staticmethod
    def http(method, url, body=None, ctype="application/json", timeout=3600, headers=None, raw=False):
        if isinstance(body, (dict, list)):
            body = json.dumps(body).encode()
        req = urllib.request.Request(url, body, method=method)
        if body is not None:
            req.add_header("Content-Type", ctype)
        for k, v in (headers or {}).items():
            req.add_header(k, v)
        try:
            with urllib.request.urlopen(req, timeout=timeout) as r:
                data = r.read()
                return r.status, (data if raw else json.loads(data or b"{}"))
        except urllib.error.HTTPError as e:
            data = e.read()
            try:
                return e.code, json.loads(data)
            except ValueError:
                return e.code, {"error": data.decode("utf-8", "replace")[:500]}
        except (urllib.error.URLError, TimeoutError, OSError) as e:
            raise JobError(f"Device unreachable: {e}")

    @staticmethod
    def multipart(fields, files):
        b = uuid.uuid4().hex
        body = b""
        for k, v in fields.items():
            body += f'--{b}\r\nContent-Disposition: form-data; name="{k}"\r\n\r\n{v}\r\n'.encode()
        for k, (name, data, ctype) in files.items():
            body += (f'--{b}\r\nContent-Disposition: form-data; name="{k}"; filename="{name}"\r\n'
                     f"Content-Type: {ctype}\r\n\r\n").encode() + data + b"\r\n"
        return body + f"--{b}--\r\n".encode(), f"multipart/form-data; boundary={b}"

    @staticmethod
    def decode_uri(uri):
        m = re.match(r"data:([\w/+.-]+)?(?:;[\w=-]+)*;base64,(.+)", uri or "", re.S)
        if not m:
            raise JobError("Expected a base64 data URI")
        return m.group(1) or "application/octet-stream", base64.b64decode(m.group(2))

    def save(self, name, data):
        (self.dir / name).write_bytes(data)
        return name

    @staticmethod
    def err(res):
        if isinstance(res, dict):
            e = res.get("error") or res.get("detail") or res
            if isinstance(e, dict):
                e = e.get("message") or json.dumps(e)
            if isinstance(e, list):
                e = "; ".join(f"{'.'.join(map(str, x.get('loc', [])))}: {x.get('msg')}" for x in e)
            return str(e)[:400]
        return str(res)[:400]

    # ---------- job lifecycle ----------
    def submit(self, kind, req):
        if kind not in KINDS:
            raise JobError(f"unknown job kind {kind}")
        model = req.get("model") or ""
        job = {"id": uuid.uuid4().hex[:12], "kind": kind, "model": model, "status": "queued", "progress": 0,
               "message": "Waiting for the model", "created": time.time(), "started": None, "label": req.get("label") or kind,
               "entry": None, "error": None, "cancel": False, "remote_task": None, "est": req.get("est"),
               # small, display-only copy of the request (prompt, size, …) so the UI can show queued jobs
               "params": {k: v for k, v in req.items() if isinstance(v, (int, float)) or (isinstance(v, str) and len(v) < 2000)}}
        with JOBS_LOCK:
            JOBS[job["id"]] = job
        threading.Thread(target=self._run, args=(job, req), daemon=True).start()
        return self.view(job)

    def _run(self, job, req):
        try:
            self._work(job, req)
        finally:
            job["finished"] = time.time()

    def _work(self, job, req):
        job["message"] = "Waiting in the device queue"
        with self.lock:
            if job["cancel"]:
                job.update(status="cancelled", message="Cancelled")
                return
            job.update(status="running", started=time.time(), message="Working…")
            try:
                entry = KINDS[job["kind"]](self, job, req)
                entry.update(id=job["id"], created=int(time.time()), kind=entry.get("kind", job["kind"]),
                             seconds=round(time.time() - job["started"], 1))
                if job["cancel"]:  # stopped while the device was still working: throw the result away
                    self.discard(entry)
                    job.update(status="cancelled", message="Cancelled")
                    return
                if entry["kind"] not in ("assist", "chat"):  # helper text and chat replies live with the job, not in the gallery
                    if not req.get("ephemeral"):  # throwaway jobs (e.g. transcribing a voice clip) never touch disk
                        self.add_history(entry)
                job.update(status="done", progress=100, message="Done", entry=entry)
            except JobError as e:
                if job["cancel"]:  # the device's error is just the cancel taking effect
                    job.update(status="cancelled", message="Cancelled")
                else:
                    job.update(status="error", error=str(e), message=str(e))
            except Exception as e:  # keep the worker alive and surface the problem
                job.update(status="error", error=f"{type(e).__name__}: {e}", message=str(e))

    def discard(self, entry):
        """Shred the files a cancelled job's result left behind (zeros first, then delete)."""
        for name in entry.get("outputs", []) + entry.get("sources", []) + entry.get("downloads", []):
            path = self.dir / name
            try:
                with open(path, "r+b") as f:
                    f.write(b"\0" * path.stat().st_size)
                path.unlink()
            except OSError:
                pass

    @staticmethod
    def view(job):
        return {k: v for k, v in job.items() if k not in ("cancel", "remote_task")}

    def list(self):
        cutoff = time.time() - 600
        with JOBS_LOCK:
            for k in [k for k, j in JOBS.items() if j["status"] in ("done", "error", "cancelled") and (j.get("finished") or j["created"]) < cutoff]:
                del JOBS[k]
            jobs = sorted(JOBS.values(), key=lambda j: j["created"])
            waiting = [j for j in jobs if j["status"] == "queued"]
            out = []
            for j in jobs:
                v = self.view(j)
                if j["status"] == "queued":
                    v["queue_pos"] = waiting.index(j) + 1
                out.append(v)
            return out

    def cancel(self, job_id):
        job = JOBS.get(job_id)
        if not job:
            return False
        job["cancel"] = True
        if job.get("remote_task"):
            try:
                self.http("POST", job["remote_task"], b"", timeout=15, headers=self.gateway_headers())
            except JobError:
                pass
        if job["status"] == "queued":
            job.update(status="cancelled", message="Cancelled")
        elif job["status"] == "running":
            job["stopping"] = True  # the result is thrown away when the device answers
            threading.Thread(target=self.halt_device, args=(job["kind"], job["model"]), daemon=True).start()
        return True

    def halt_device(self, kind, model):
        """Make the Tiiny stop a request now instead of finishing it: chat replies can be interrupted; an image only
        stops when its model is unloaded (~10 s), so it's unloaded and loaded straight back (~15 s, tested 0.1.36)."""
        if not model:
            return
        q = quote(model, safe="")
        if kind in ("chat", "assist"):
            self.device("POST", f"/api/v1/models/{q}/interrupt", {}, 15)
        elif kind == "image":
            self.device("POST", f"/api/v1/models/{q}/stop", None, 120)
            self.device("POST", f"/api/v1/models/{q}/launch", {"model_id": model}, 120)


# ---------- kinds ----------
def run_speech(m, job, req):
    text = (req.get("text") or "").strip()
    if not text:
        raise JobError("Nothing to say")
    if not req.get("ref_audio"):
        raise JobError("This model only speaks in a cloned voice: add a reference clip")
    if not (req.get("ref_text") or "").strip():
        raise JobError("Add what's said in the reference clip (its transcript) — voice cloning needs both")
    _, ref = m.decode_uri(req["ref_audio"])
    # The gateway's OpenAI-style /v1/audio/speech takes the clone fields at the top level and works on
    # every link (the USB cable blocks the TTS container's own port).
    body = {"model": job["model"], "input": text, "ref_audio": base64.b64encode(ref).decode(), "response_format": "wav"}
    config = {}
    for k in ("ref_text", "language", "instruct"):
        if (req.get(k) or "").strip():
            body[k] = config[k] = req[k].strip()
    m.ensure_running(job["model"])
    # Long text is voiced a few sentences at a time (the model has a context limit) and stitched together.
    chunks = split_for_speech(text)
    parts = []
    for i, chunk in enumerate(chunks):
        if job["cancel"]:
            raise JobError("Cancelled")
        job["message"] = "Synthesising speech" if len(chunks) == 1 else f"Speaking part {i + 1} of {len(chunks)}"
        job["progress"] = round(i / len(chunks) * 100) if len(chunks) > 1 else 0
        code, res = m.http("POST", f"{m.settings.url}/v1/audio/speech", {**body, "input": chunk}, headers=m.gateway_headers(), raw=True)
        if code != 200 or not isinstance(res, bytes) or res[:4] != b"RIFF":
            raise JobError(m.err(res if not isinstance(res, bytes) else res.decode("utf-8", "replace")))
        parts.append(res)
    res = join_wavs(parts)
    out = m.save(f"{job['id']}-speech.wav", res)
    src = m.save(f"{job['id']}-ref.wav", ref)
    res = {"duration": wav_seconds(res)}
    return {"kind": "speech", "params": {"model": job["model"], "text": text, "language": config.get("language"), "parts": len(chunks),
                                         "instruct": config.get("instruct"), "voice": req.get("voice_name"),
                                         "ref_text": config.get("ref_text")},
            "sources": [src], "outputs": [out], "duration": res.get("duration")}


def split_for_speech(text, limit=320):
    """Split text into chunks of whole sentences, each at most ~limit characters."""
    sentences = [s for s in re.split(r"(?<=[.!?…。！？])\s+|\n{2,}", text.strip()) if s.strip()]
    chunks, cur = [], ""
    for s in sentences:
        while len(s) > limit:  # one very long sentence: split at the last comma/space before the limit
            cut = max(s.rfind(", ", 0, limit), s.rfind(" ", 0, limit))
            cut = cut if cut > limit // 3 else limit
            piece, s = s[:cut + 1].strip(), s[cut + 1:].strip()
            if cur:
                chunks.append(cur)
                cur = ""
            chunks.append(piece)
        if cur and len(cur) + 1 + len(s) > limit:
            chunks.append(cur)
            cur = s
        else:
            cur = f"{cur} {s}".strip()
    if cur:
        chunks.append(cur)
    return chunks or [text]


def join_wavs(parts, gap=0.25):
    """Concatenate WAVs with the same format, with a short pause between parts."""
    import io, wave
    if len(parts) == 1:
        return parts[0]
    out, params, frames = io.BytesIO(), None, []
    for p in parts:
        w = wave.open(io.BytesIO(p))
        params = params or w.getparams()
        frames.append(w.readframes(w.getnframes()))
    silence = b"\0" * int(params.framerate * gap) * params.sampwidth * params.nchannels
    with wave.open(out, "wb") as o:
        o.setparams(params)
        o.writeframes(silence.join(frames))
    return out.getvalue()


def wav_seconds(data):
    import io, wave
    try:
        w = wave.open(io.BytesIO(data))
        return round(w.getnframes() / w.getframerate(), 2)
    except (wave.Error, EOFError, ZeroDivisionError):
        return None


def to_asr_wav(audio):
    """Downmix/resample WAV input to 16-bit 16 kHz mono; other formats pass through unchanged.
    Pure Python (audioop is gone from Python 3.13), fast enough for a few minutes of audio."""
    import array, io, sys, wave
    try:
        w = wave.open(io.BytesIO(audio))
    except (wave.Error, EOFError):
        return None
    channels, sw, rate = w.getnchannels(), w.getsampwidth(), w.getframerate()
    if channels == 1 and rate == 16000 and sw == 2:
        return audio
    raw = w.readframes(w.getnframes())
    if sw == 2:
        a = array.array("h", raw)
    elif sw == 4:
        a = array.array("i", raw)
    elif sw == 1:  # 8-bit WAV is unsigned
        a = array.array("h", ((b - 128) << 8 for b in raw))
    elif sw == 3:
        a = array.array("h", (int.from_bytes(raw[i + 1:i + 3], "little", signed=True) for i in range(0, len(raw) - 2, 3)))
    else:
        return audio
    if sys.byteorder == "big" and sw in (2, 4):  # WAV samples are little-endian
        a.byteswap()
    if sw == 4:
        a = array.array("h", (v >> 16 for v in a))
    if channels > 1:
        a = array.array("h", (sum(s) // channels for s in zip(*(a[c::channels] for c in range(channels)))))
    if rate != 16000:
        step = rate / 16000
        if step.is_integer():  # 32/48 kHz: average each group of samples (a simple low-pass)
            k = int(step)
            a = array.array("h", (sum(g) // k for g in zip(*(a[i::k] for i in range(k)))))
        else:  # linear interpolation
            n, last = int(len(a) / step), len(a) - 1
            a = array.array("h", (int(a[int(x)] + (a[min(int(x) + 1, last)] - a[int(x)]) * (x - int(x))) for x in (i * step for i in range(n))))
    if sys.byteorder == "big":
        a.byteswap()
    buf = io.BytesIO()
    with wave.open(buf, "wb") as o:
        o.setnchannels(1); o.setsampwidth(2); o.setframerate(16000); o.writeframes(a.tobytes())
    return buf.getvalue()


def run_transcribe(m, job, req):
    ctype, audio = m.decode_uri(req.get("audio"))
    asr_audio = to_asr_wav(audio) if "wav" in ctype else None
    ext = {"audio/wav": "wav", "audio/x-wav": "wav", "audio/mpeg": "mp3", "audio/mp4": "m4a", "audio/webm": "webm",
           "audio/ogg": "ogg", "audio/flac": "flac"}.get(ctype, "wav")
    fields = {"model": job["model"], "response_format": "json"}
    if (req.get("language") or "").strip():
        fields["language"] = req["language"].strip()
    body, mp = m.multipart(fields, {"file": ("audio.wav", asr_audio, "audio/wav") if asr_audio else (f"audio.{ext}", audio, ctype)})
    job["message"] = "Transcribing"
    code, res = m.http("POST", f"{m.settings.url}/v1/audio/transcriptions", body, mp, headers={"Authorization": f"Bearer {m.settings.key}"})
    if code != 200 or "text" not in res:
        raise JobError(m.err(res))
    src = None if req.get("ephemeral") else m.save(f"{job['id']}-audio.{ext}", audio)
    return {"kind": "transcript", "params": {"model": job["model"], "language": fields.get("language"), "name": req.get("name")},
            "sources": [src] if src else [], "outputs": [], "text": res["text"]}


# Fields the device's /v1/images/generations validates (found by probing TiinyOS 0.1.35).
IMAGE_FIELDS = ("model", "prompt", "negative_prompt", "n", "size", "seed", "steps", "guidance_scale")
WARMUP_WAIT = float(os.environ.get("STUDIO_WARMUP_WAIT", "10"))  # seconds between warm-up retries (tests shorten it)


def run_image(m, job, req):
    """Create (or, with source images, edit) through the gateway's /v1/images/generations."""
    body = {k: req[k] for k in IMAGE_FIELDS if req.get(k) not in (None, "")}
    body["response_format"] = "b64_json"
    sources = [s for s in (req.get("images") or []) if isinstance(s, str)]
    if len(sources) == 1:
        body["image"] = sources[0]
    elif sources:
        body["images"] = sources  # several references (Qwen-Image takes up to 10) go in "images", an array
    # The device only makes one image per request ("batch_size other than 1 is not supported"), so several
    # images are separate requests with consecutive seeds, grouped into one result.
    count = max(1, min(4, int(body.pop("n", 1) or 1)))
    body["n"] = 1
    seed0 = body.get("seed")
    res = {"data": []}
    for i in range(count):
        if job["cancel"]:
            raise JobError("Cancelled")
        verb = "Editing" if sources else "Generating"
        job["message"] = verb if count == 1 else f"{verb} image {i + 1} of {count}"
        job["progress"] = round(i / count * 100) if count > 1 else 0
        if seed0 is not None:
            body["seed"] = seed0 + i
        code, one = m.device("POST", "/v1/images/generations", body, 3600)
        # A model that just reported "running" may not be ready yet: its first request answers "xh2a backend
        # fail" (seen with Qwen-Image right after a load). Give it up to a minute before giving up.
        for _ in range(6):
            if code == 200 or res["data"] or "backend fail" not in m.err(one) or job["cancel"]:
                break
            job["message"] = "Waiting for the model to warm up"
            time.sleep(WARMUP_WAIT)
            code, one = m.device("POST", "/v1/images/generations", body, 3600)
        if code != 200 or not one.get("data"):
            if res["data"]:
                break  # keep the images that did come out
            raise JobError(m.err(one))
        res["data"] += one["data"]
    body["n"], body["seed"] = count, seed0
    src_names = []
    for i, uri in enumerate(sources):
        ctype, data = m.decode_uri(uri)
        src_names.append(m.save(f"{job['id']}-src{i}.{ {'image/jpeg': 'jpg', 'image/webp': 'webp'}.get(ctype, 'png') }", data))
    outputs = [m.save(f"{job['id']}-{i}.png", base64.b64decode(d["b64_json"])) for i, d in enumerate(res["data"]) if d.get("b64_json")]
    return {"kind": "image", "mode": "edit" if sources else "create",
            "params": {k: v for k, v in body.items() if k not in ("image", "response_format")},
            "sources": src_names, "outputs": outputs}


LOOP_CONFIG = ("seconds_total", "steps", "cfg_scale", "seed")


def run_loop(m, job, req):
    prompt = (req.get("prompt") or "").strip()
    if not prompt:
        raise JobError("Describe the loop")
    config = {k: req[k] for k in LOOP_CONFIG if req.get(k) not in (None, "")}
    m.ensure_running(job["model"])
    job["message"] = "Generating loop"
    # Gateway music route (Tiiny SDK, Oct 2026) works on every link, the USB cable included. Foundation-1
    # still takes its native body ({prompt, config}); older firmware without the route falls back to the port.
    code, res = m.http("POST", f"{m.settings.url}/v1/music/generate/wav", {"model": job["model"], "prompt": prompt, "config": config},
                       headers=m.gateway_headers(), raw=True)
    if code == 404 and not isinstance(res, (bytes, bytearray)) and "detail" in res:
        url = m.instance_url(job["model"])
        code, res = m.http("POST", f"{url}/v1/generate/wav", {"prompt": prompt, "config": config}, raw=True)
    if code != 200 or not isinstance(res, (bytes, bytearray)):
        raise JobError(m.err(res))
    out = m.save(f"{job['id']}-loop.wav", res)
    return {"kind": "loop", "params": {"model": job["model"], "prompt": prompt, **config, "bpm": req.get("bpm"),
                                       "bars": req.get("bars"), "key": req.get("key")},
            "sources": [], "outputs": [out], "duration": config.get("seconds_total")}


SONG_CONFIG = ("caption", "lyrics", "generation_type", "seconds_total", "seed", "steps", "temperature", "top_k", "top_p")
FULL_SONG_SECONDS = 240  # at or above this, the gateway path renders the model's full-length arrangement


def run_song(m, job, req):
    config = {k: req[k] for k in SONG_CONFIG if req.get(k) not in (None, "")}
    if config.get("generation_type", "mixed") != "bgm" and not (config.get("lyrics") or "").strip():
        raise JobError("A song with vocals needs lyrics (or switch to Instrumental)")
    sources = []
    if req.get("reference_audio"):
        _, ref = m.decode_uri(req["reference_audio"])
        config["reference_audio_base64"] = base64.b64encode(ref).decode()
        sources.append(m.save(f"{job['id']}-styleref.wav", ref))
    inst = m.running_instance(job["model"])
    gw, auth = f"{m.settings.url}/v1/music/generate", m.gateway_headers()
    stop = threading.Event()
    direct = m.port_open(m.settings.host, inst.get("port")) and not req.get("preview") and not req.get("resume_session")
    threading.Thread(target=song_progress, args=(m, job, f"http://{m.settings.host}:{inst.get('port')}" if direct else None, stop), daemon=True).start()
    try:
        code, res, route, url = _compose(m, job, req, config, inst, direct, gw, auth)
    finally:
        stop.set()
    if code != 200 or not res.get("success") or not res.get("audio_data"):
        raise JobError(m.err(res))
    return _song_entry(m, job, req, res, config, sources, route, url)


def _compose(m, job, req, config, inst, direct, gw, auth):
    route, url = "direct", None
    body = {"model": job["model"], "config": config}
    if req.get("resume_session"):
        # Continue a saved preview to the full-length song (the gateway renders the whole arrangement).
        route = "gateway-full"
        job["message"] = "Composing the full song (this takes a while)"
        config.pop("seconds_total", None)
        code, res = m.http("POST", gw + "/resume", {**body, "session_id": req["resume_session"]}, headers=auth)
        return code, res, route, url
    if req.get("preview"):
        route = "preview"
        job["message"] = "Composing an 8-second preview"
        config.pop("seconds_total", None)
        code, res = m.http("POST", gw + "/preview", {**body, "config": {**config, "preview_seconds": 8}}, headers=auth)
        return code, res, route, url
    if direct:
        # Direct to the model's port (Wi-Fi/LAN): composes a complete song at exactly seconds_total.
        url = f"http://{m.settings.host}:{inst['port']}"
        job["message"] = "Composing"
        code, res = m.http("POST", f"{url}/v1/generate", {"config": config})
        return code, res, route, url
    # Over links that only expose the gateway (the USB cable). Its plain /generate fails ("Session not
    # found", TiinyOS 0.1.35) and /resume ignores seconds_total, so: a preview of the requested length (the
    # opening of a full-length arrangement), or preview + resume for a full-length song.
    secs = float(config.pop("seconds_total", 60) or 60)
    if secs >= FULL_SONG_SECONDS:
        route = "gateway-full"
        job["message"] = "Composing the opening"
        code, res = m.http("POST", gw + "/preview", {**body, "config": {**config, "preview_seconds": 8}}, headers=auth)
        if code == 200 and res.get("session_id"):
            job["message"] = "Composing the full song (this takes a while)"
            code, res = m.http("POST", gw + "/resume", {**body, "session_id": res["session_id"]}, headers=auth)
    else:
        route = "gateway-preview"
        job["message"] = "Composing"
        code, res = m.http("POST", gw + "/preview", {**body, "config": {**config, "preview_seconds": secs}}, headers=auth)
    config["seconds_total"] = secs
    return code, res, route, url


def song_progress(m, job, direct_url, stop):
    """Poll the session list for the running song's progress (null while the model can't measure it)."""
    auth = m.gateway_headers()
    while not stop.wait(3):
        try:
            if direct_url:
                _, r = m.http("GET", f"{direct_url}/v1/sessions", timeout=5)
            else:
                _, r = m.http("GET", f"{m.settings.url}/v1/music/sessions?model={quote(job['model'], safe='')}", timeout=5, headers=auth)
            live = [s for s in (r.get("sessions") or []) if s.get("status") not in ("completed", "failed", "cancelled")]
            prog = [s.get("progress") for s in live if isinstance(s.get("progress"), (int, float))]
            if prog and max(prog) > 0:
                job["progress"] = min(99, round(max(prog) * 100))
        except (JobError, ValueError, AttributeError, TypeError):
            pass


def _song_entry(m, job, req, res, config, sources, route, url):
    outputs = [m.save(f"{job['id']}-song.wav", base64.b64decode(res["audio_data"]))]
    tracks = {}
    for name, data in (res.get("audio_outputs") or {}).items():
        if isinstance(data, str) and len(data) > 1000 and name.lower() != "mixed":  # "mixed" duplicates the main file
            fn = m.save(f"{job['id']}-{re.sub(r'[^a-z0-9]+', '_', name.lower())}.wav", base64.b64decode(data))
            outputs.append(fn)
            tracks[fn] = name
    downloads, resume = [], None
    sid = res.get("session_id")
    if url and sid:
        m.http("DELETE", f"{url}/v1/sessions/{sid}", timeout=15)
    elif sid:
        # Gateway songs can also be fetched as MP3 from their session.
        try:
            code, mp3 = m.http("GET", f"{m.settings.url}/v1/music/sessions/{sid}/download?format=mp3", timeout=120, headers=m.gateway_headers(), raw=True)
            if code == 200 and isinstance(mp3, (bytes, bytearray)) and len(mp3) > 1000:
                downloads.append(m.save(f"{job['id']}-song.mp3", mp3))
        except JobError:
            pass
        if route == "preview":
            resume = {"session_id": sid}  # a preview can be continued once
    if req.get("resume_session") and req.get("from_entry") and getattr(m, "edit_history", None):
        m.edit_history(req["from_entry"], lambda e: e.pop("resume", None))
    entry = {"kind": "song", "params": {"model": job["model"], "route": route, **{k: v for k, v in config.items() if k != "reference_audio_base64"}},
             "sources": sources, "outputs": outputs, "tracks": tracks, "duration": res.get("duration"), "downloads": downloads}
    if resume:
        entry["resume"] = resume
    return entry


def run_mesh(m, job, req):
    """Hunyuan3D: [background] -> shape -> [simplify] -> [texture -> export], chaining artifacts."""
    _, image = m.decode_uri(req.get("image"))
    m.ensure_running(job["model"])
    url, auth = f"{m.settings.url}/v1/3d", m.gateway_headers()  # gateway proxy: /v1/3d/tasks -> container /v1/tasks
    plan = (["background"] if req.get("remove_bg") else []) + ["shape"] + (["simplify"] if req.get("faces") else []) \
        + (["texture", "export"] if req.get("texture") else [])
    weights = {"background": 5, "shape": 45, "simplify": 3, "texture": 42, "export": 5}
    total = sum(weights[s] for s in plan)
    done = 0
    quality = {k: req[k] for k in ("output_precision", "inference_quality", "asset_quality", "seed") if req.get(k) not in (None, "")}
    have = {"image": ("input.png", image, "image/png")}
    stage_log = []
    src = m.save(f"{job['id']}-src.png", image)

    for stage in plan:
        if job["cancel"]:
            raise JobError("Cancelled")
        params = dict(quality)
        if stage == "simplify":
            params = {"target_faces": int(req["faces"])}
        if stage == "background":
            params = {}
        files = {k: v for k, v in have.items() if k in STAGE_INPUTS[stage]}
        body, mp = m.multipart({"stage": stage, "params": json.dumps(params)}, files)
        code, res = m.http("POST", f"{url}/tasks", body, mp, headers=auth)
        if code >= 300:
            raise JobError(f"{stage}: {m.err(res)}")
        tid = res["data"]["task_id"]
        job["remote_task"] = f"{url}/tasks/{tid}/cancel"
        silent = 0  # polls in a row without a status: the task is gone, don't hold the device forever
        while True:
            time.sleep(2)
            _, st = m.http("GET", f"{url}/tasks/{tid}", timeout=30, headers=auth)
            d = st.get("data") or {}
            p = d.get("progress") or 0
            job["progress"] = round((done + weights[stage] * p / 100) / total * 100)
            job["message"] = f"{stage.capitalize()}: {d.get('message') or d.get('status')}"
            job["stage"] = stage
            if d.get("status") in ("completed", "failed", "cancelled", "error"):
                break
            if job["cancel"]:
                raise JobError("Cancelled")
            silent = 0 if d.get("status") else silent + 1
            if silent >= 30:
                raise JobError(f"{stage}: the device stopped reporting on this task")
        if d.get("status") != "completed":
            raise JobError(f"{stage}: {m.err(d.get('error') or d.get('message') or d.get('status'))}")
        out = (d.get("result") or {}).get("output") or {}
        stage_log.append({"stage": stage, "metadata": out.get("metadata")})
        # Stages report artifacts either as a list or (texture) as one top-level payload.
        single = {"artifact_type": out.get("artifact_type") or "mesh_glb", "filename": out.get("filename"),
                  "mime_type": out.get("mime_type"),
                  "payload_key": next((k for k in ("glb", "artifact", "image", "mesh") if isinstance(out.get(k), str)), "")}
        for art in out.get("artifacts") or [single]:
            payload = out.get(art.get("payload_key") or "")
            if not isinstance(payload, str):
                continue
            data = base64.b64decode(payload)
            kind = art.get("artifact_type") or ""
            fname = art.get("filename") or f"{kind}.bin"
            mime = art.get("mime_type") or "application/octet-stream"
            if kind == "mesh_glb":
                have["mesh"] = (fname, data, mime)
                have["mesh_glb"] = (fname, data, mime)
            elif kind.startswith("material"):
                have["material"] = (fname, data, mime)
                have["material_artifact"] = (fname, data, mime)
            elif kind.startswith("image") or mime.startswith("image/"):
                have["image"] = (fname, data, mime)
        done += weights[stage]
        job["remote_task"] = None

    if "mesh" not in have:
        raise JobError("The device returned no mesh")
    out = m.save(f"{job['id']}-model.glb", have["mesh"][1])
    return {"kind": "mesh", "params": {"model": job["model"], **quality, "texture": bool(req.get("texture")),
                                       "faces": req.get("faces"), "remove_bg": bool(req.get("remove_bg"))},
            "sources": [src], "outputs": [out], "stages": stage_log}


def run_ocr(m, job, req):
    """Text from an image via the gateway's /v1/ocr (Tiiny SDK docs). Sent as a throwaway job by the UI."""
    m.decode_uri(req.get("image"))  # validates the data URI
    inst = m.running_instance(job["model"])
    job["message"] = "Reading text"
    code, res = m.http("POST", f"{m.settings.url}/v1/ocr?model={quote(job['model'], safe='')}",
                       {"input": req["image"], "response_format": "json"}, headers=m.gateway_headers(), timeout=300)
    if code != 200 and "endpoint not found" in m.err(res).lower():
        # GLM-OCR only serves OpenAI-style chat on its own port; the gateway's /v1/ocr has no route for it and
        # its chat route refuses it ("does not support chat", TiinyOS 0.1.35). Reach it directly when possible.
        if not m.port_open(m.settings.host, inst.get("port")):
            raise JobError(f"{job['model'].split('/')[-1]} only works over Wi-Fi/LAN (the gateway can't route it). Use PP-OCRv6 on this connection.")
        _, info = m.http("GET", f"http://{m.settings.host}:{inst['port']}/v1/models", timeout=10)
        name = ((info.get("data") or [{}])[0]).get("id") or job["model"].split("/")[-1]
        code, chat = m.http("POST", f"http://{m.settings.host}:{inst['port']}/v1/chat/completions", {"model": name, "temperature": 0, "max_tokens": 4096,
                            "messages": [{"role": "user", "content": [{"type": "image_url", "image_url": {"url": req["image"]}}, {"type": "text", "text": "Text Recognition:"}]}]}, timeout=300)
        if code != 200 or not chat.get("choices"):
            raise JobError(m.err(chat))
        text = chat["choices"][0]["message"]["content"].strip()
        res = {"data": [{"text": text, "lines": [{"text": l} for l in text.splitlines() if l.strip()]}]}
    elif code != 200 or not res.get("data"):
        raise JobError(m.err(res))
    item = res["data"][0]
    return {"kind": "ocr", "params": {"model": job["model"]}, "sources": [], "outputs": [], "text": item.get("text", ""),
            "lines": [{"text": l.get("text"), "score": l.get("score")} for l in item.get("lines") or []]}


# Which multipart fields each Hunyuan3D stage takes (from /v1/capabilities).
STAGE_INPUTS = {"background": {"image"}, "shape": {"image"}, "simplify": {"mesh"}, "texture": {"image", "mesh"},
                "export": {"material", "mesh"}}

ASSIST_SYSTEM = {
    "prompt": "You improve prompts for a text-to-image model. Rewrite the user's idea as one vivid, concrete prompt: "
              "subject, setting, composition, lighting, style, mood. Keep everything the user asked for and invent nothing "
              "that contradicts it. Under 80 words. Reply with the prompt only: no preamble, no quotes, no lists.",
    "edit": "You improve instructions for an image-editing model that changes an existing picture. Rewrite the user's "
            "request as one clear, specific edit instruction: say exactly what to change and what must stay the same "
            "(subject, pose, framing, style). Under 60 words. Reply with the instruction only: no preamble, no quotes.",
    "describe": "You describe images for an artist who wants to recreate or edit them. Write one paragraph (60-100 words) "
                "covering the subject, setting, composition, colours, lighting and style, phrased so it works as a "
                "text-to-image prompt. Reply with the description only.",
    "lyrics": "You write song lyrics for a singing model. Use only these section tags, each alone on its own line: "
              "[verse], [chorus], [bridge]. Under each tag write 2-4 short singable lines, one per line, then a blank line "
              "before the next tag. Repeat the chorus word for word. No title, no notes, no other markup, no blank lines "
              "inside a section. Reply with the lyrics only.",
    "tags": "You write prompts for Foundation-1, a model that makes instrument loops from comma-separated tags. Turn the "
            "user's description into tags: first exactly one instrument, then 1-3 character tags, 1-2 phrase tags and 0-2 "
            "effect tags. Prefer tags from the vocabulary given; never mention drums, tempo, bars or key. Reply with one "
            "line of comma-separated tags only.",
}
ASSIST_TOKENS = {"prompt": 200, "edit": 160, "describe": 260, "lyrics": 500, "tags": 60}


def clean_reply(text):
    text = re.sub(r"<think>.*?</think>", "", text or "", flags=re.S).strip()
    text = re.sub(r"^```\w*\n?|\n?```$", "", text).strip()
    return text.strip('"“”').strip()


def clean_lyrics(text):
    out = []
    for line in clean_reply(text).splitlines():
        line = line.strip().strip("*_#").strip()
        m = re.fullmatch(r"\[?\s*(verse|chorus|bridge|intro|outro|pre-chorus|hook)(?:\s*\d+)?\s*\]?\s*:?", line, re.I)
        if m:
            tag = {"pre-chorus": "verse", "hook": "chorus"}.get(m.group(1).lower(), m.group(1).lower())
            out += ([""] if out else []) + [f"[{tag}]"]
        elif line and out:
            out.append(line)
    text = "\n".join(out)
    if not re.search(r"\[(verse|chorus)\]", text):
        raise JobError("The model didn't return lyrics with [verse]/[chorus] sections. Try again.")
    return re.sub(r"\[(intro|outro)\]\n(?=\n|$)", "", text).strip()


def run_assist(m, job, req):
    """One chat-completions call through the gateway; thinking is off so short tasks stay quick."""
    task = req.get("task")
    if task not in ASSIST_SYSTEM:
        raise JobError(f"unknown helper {task}")
    text = (req.get("text") or "").strip()
    if task == "describe":
        if req.get("file"):
            name = req["file"]
            if not re.fullmatch(r"[\w.-]+", name) or not (m.dir / name).is_file():
                raise JobError("That image isn't in the gallery")
            ext = name.rsplit(".", 1)[-1].lower()
            uri = f"data:image/{'jpeg' if ext == 'jpg' else ext};base64," + base64.b64encode((m.dir / name).read_bytes()).decode()
        else:
            m.decode_uri(req.get("image"))
            uri = req["image"]
        user = [{"type": "text", "text": text or "Describe this image."}, {"type": "image_url", "image_url": {"url": uri}}]
    elif not text:
        raise JobError("Nothing to work from: type something first")
    else:
        user = text
        if req.get("context"):
            user = f"{req['context'].strip()}\n\n{text}"
    m.ensure_running(job["model"])  # clear "isn't loaded" error instead of a gateway 4xx
    body = {"model": job["model"], "messages": [{"role": "system", "content": ASSIST_SYSTEM[task]}, {"role": "user", "content": user}],
            "max_tokens": ASSIST_TOKENS[task], "temperature": 0.8 if task == "lyrics" else 0.6, "stream": False}
    if "gpt-oss" in job["model"]:  # always reasons and rejects enable_thinking; give it room to think
        body.update(reasoning_effort="low", max_tokens=body["max_tokens"] + 1500)
    else:
        body["chat_template_kwargs"] = {"enable_thinking": False}
    job["message"] = {"prompt": "Rewriting the prompt", "edit": "Rewriting the instruction", "describe": "Looking at the image",
                      "lyrics": "Writing lyrics", "tags": "Picking tags"}[task]
    code, res = m.device("POST", "/v1/chat/completions", body, 600)
    if code != 200 or not res.get("choices"):
        raise JobError(m.err(res))
    reply = (res["choices"][0].get("message") or {}).get("content") or ""
    out = clean_lyrics(reply) if task == "lyrics" else clean_reply(reply)
    if task == "tags":
        out = re.sub(r"\s*\n\s*", ", ", out).strip(", ")
    if not out:
        raise JobError("The model returned nothing. Try again.")
    return {"kind": "assist", "params": {"model": job["model"], "task": task}, "sources": [], "outputs": [], "text": out,
            "usage": res.get("usage")}


KINDS = {"image": run_image, "ocr": run_ocr, "speech": run_speech, "transcribe": run_transcribe, "loop": run_loop, "song": run_song, "mesh": run_mesh,
         "assist": run_assist}
