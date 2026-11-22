"""Stage 5 contract tests: speech, transcription, loops, songs and image-to-3D, run as real jobs against the fake Tiiny.
Checks the finished job, the saved files and the requests the server made to the device (multipart bodies as parsed
fields, file names and file checksums, since the boundaries differ)."""
import base64
import wave
import struct
import io
import json
import unittest

from harness import ContractCase, media, name_suffix, request  # first: puts testdata/ on the path
import fake_tiiny  # noqa: E402

TTS, ASR, LOOP, SONG, MESH = "Qwen/Qwen3-TTS-Base", "Fake/Whisper-ASR", "Fake/Foundation-1", "Fake/SongGeneration-v2", "Tencent/Hunyuan3D-2.1"
IMAGE = "Qwen/Qwen-Image-2.1"
TRELLIS = "microsoft/TRELLIS.2-4B"
VIDEO = fake_tiiny.VIDEO
PNG = "data:image/png;base64," + base64.b64encode(fake_tiiny.tiny_png()).decode()
WAV = "data:audio/wav;base64," + base64.b64encode(fake_tiiny.tiny_wav()).decode()
LONG_REF = "data:audio/wav;base64," + base64.b64encode(fake_tiiny.tiny_wav(12, 8000)).decode()  # a style reference the device takes
WAV_44K = "data:audio/x-wav;base64," + base64.b64encode(fake_tiiny.tiny_wav(0.05, 44100)).decode()
MP3 = "data:audio/mpeg;base64," + base64.b64encode(fake_tiiny.MP3).decode()
# ids, times and file names (they contain the job id) differ per run; the files are checked with file_media
IGNORE = ("id", "device_id", "created", "started", "finished", "seconds", "t", "outputs", "sources", "downloads", "from_entry")
LONG_TEXT = " ".join(f"Sentence number {i} is here, and it goes on a little." for i in range(14))
# what the fake hands out, as the studio saves it (without the tags it writes into files, #58)
WAV_FILE, PNG_FILE = fake_tiiny.tiny_wav(), fake_tiiny.tiny_png()
MP3_FILE, MP4_FILE, GLB_FILE = media("x.mp3", fake_tiiny.MP3), media("x.mp4", fake_tiiny.MP4), fake_tiiny.GLB
SYS, RUN = ("GET", "/api/v1/sys/status"), ("GET", "/api/v1/models/running")
SPEECH, TRANSCRIBE = ("POST", "/v1/audio/speech"), ("POST", "/v1/audio/transcriptions")
# a speech job asks for the voice model's own port first (where the link reaches it), then speaks there
OWN = [RUN, ("GET", "/v1/models")]
CAPS, TASKS = ("GET", "/v1/3d/capabilities"), ("POST", "/v1/3d/tasks")
VIDEO_ID = "/v1/videos/video_00000000000000000000000000000001"
VIDEO_CALLS = [SYS, RUN, ("POST", "/v1/videos"), ("GET", VIDEO_ID), ("GET", VIDEO_ID), ("GET", VIDEO_ID + "/content"), ("DELETE", VIDEO_ID)]
DOWNLOAD = ("GET", "/v1/music/sessions/sess-1/download")
PLAIN_DOWNLOAD = ("GET", "/v1/music/sessions/plain-1/download")  # a song on the plain gateway route (/v1/music/generate)


def png_pixels(data):
    """The pixels of an uncompressed-or-not 8-bit RGBA PNG whose rows all use filter 0 (what clean_alpha writes)."""
    import struct, zlib
    w, h = struct.unpack(">II", data[16:24])
    pos, idat = 8, b""
    while pos < len(data):
        n, kind = struct.unpack(">I4s", data[pos:pos + 8])
        idat += data[pos + 8:pos + 8 + n] if kind == b"IDAT" else b""
        pos += 12 + n
    raw = zlib.decompress(idat)
    assert all(raw[y * (w * 4 + 1)] == 0 for y in range(h))
    return [b for y in range(h) for b in raw[y * (w * 4 + 1) + 1:(y + 1) * (w * 4 + 1)]]


def speech_entry(model=TTS, text="RUST TEST hello there.", language=None, voice=None, ref_text="hi", speaker=None, design=None, instruct=None, parts=1, duration=0.25):
    return {"kind": "speech", "params": {"model": model, "text": text, "language": language, "parts": parts, "instruct": instruct, "voice": voice,
                                         "ref_text": ref_text, "speaker": speaker, "design": design}, "duration": duration}


def song_entry(route, model=SONG, device=True, lyrics="[verse]\nla la", session="sess-1", **params):
    """The entry of a finished song against the fake (tracks and the resume token are added by the test)."""
    e = {"kind": "song", "params": {"model": model, "route": route, "caption": "RUST TEST pop", **({"lyrics": lyrics} if lyrics else {}), **params},
         "tracks": {}, "duration": 0.25}
    if device:
        e["device"] = {"music_session": session}  # a plain-gateway song gets the fake's plain-N, as on the device
    return e


def mesh_entry(model=MESH, stages=("shape",), **params):
    return {"kind": "mesh", "params": {"model": model, "texture": False, "faces": None, "remove_bg": False, "cutout": None, **params},
            "stages": [{"stage": s, "metadata": {"stage": s}} for s in stages]}


def video_entry(**params):
    return {"kind": "video", "params": {"model": VIDEO, "prompt": "RUST TEST a red apple rolls", "size": "832x480", "steps": 4, **params}, "duration": 2.3}


def loop_entry(**params):
    return {"kind": "loop", "params": {"model": LOOP, "prompt": "RUST TEST piano", "bpm": None, "bars": None, "key": None, **params}, "duration": None}


def transcript_entry(text="fake transcript", **params):
    return {"kind": "transcript", "params": {"model": ASR, "language": None, "name": None, **params}, "text": text}


class Jobs(ContractCase):
    """Runs media jobs against the fake and checks them."""
    maxDiff = None

    def view(self, job):
        return strip({k: job.get(k) for k in ("status", "message", "error", "entry")})

    def comparable(self, calls):
        # the Rust server also asks the song model what a style reference may be before it sends anything, and lists the
        # sessions before a plain song so it can tell its own from older ones (by identity, not by the clocks)
        calls = [c for c in calls if c[:2] != ("GET", "/v1/music/info")]
        return [c for i, c in enumerate(calls)
                if not (c[:2] == ("GET", "/v1/music/sessions") and i + 1 < len(calls) and calls[i + 1][:2] == ("POST", "/v1/music/generate"))]

    def failed(self, req, text=None, **kw):
        job, _ = self.run_job(req, **kw)
        self.assertEqual(job["status"], "error")
        if text:
            self.assertIn(text, job["error"])
        return job

    def sent(self, path, method="POST", last=False):
        """The body of a request the server made to the fake (the first one to this path, or the last)."""
        found = [c[3] for c in self.calls() if c[:2] == (method, path)]
        return found[-1] if last else found[0]


class Audio(Jobs):
    # ---------- speech ----------
    def speech(self, **kw):
        return {"kind": "speech", "model": TTS, "text": "RUST TEST hello there.", "ref_audio": WAV, "ref_text": "hi", "load": TTS, **kw}

    def test_speech_without_a_voice_uses_the_models_default(self):
        # a model with its own default voice (Qwen3-TTS-Base clones, so it needs a reference, as on the device)
        self.check_job(self.speech(model="Fake/CustomVoice", load="Fake/CustomVoice", ref_audio=None, ref_text=None),
                       entry=speech_entry(model="Fake/CustomVoice", ref_text=None), calls=[SYS, RUN, *OWN, SPEECH])
        body = next(c[3] for c in self.calls() if c[1] == "/v1/audio/speech")
        for k in ("voice", "ref_audio", "instruct"):
            self.assertNotIn(k, body)

    def test_speech_short(self):
        job, _ = self.check_job(self.speech(language="en", instruct=" calm ", voice_name="RUST TEST voice"),
                                entry=speech_entry(language="en", instruct="calm", voice="RUST TEST voice"), calls=[SYS, RUN, *OWN, SPEECH])
        self.assertEqual(job["entry"]["params"]["parts"], 1)
        self.assertEqual(self.file_media(job), [("speech.wav", WAV_FILE)])
        self.assertEqual(self.file_media(job, "sources"), [("ref.wav", WAV_FILE)])

    def test_speech_uses_the_own_port_where_the_gateway_drops_sentences(self):
        # TiinyOS 0.1.39: through the gateway five voice models give a WAV without its last sentence (or with it moved),
        # which looks like any good answer; their own port is whole. Over Wi-Fi every part goes to the own port.
        job, _ = self.check_job(self.speech(text=LONG_TEXT, flags={"gateway_garbles": True}),
                                entry=speech_entry(text=LONG_TEXT, parts=3, duration=1.25), calls=[SYS, RUN, *OWN, SPEECH, SPEECH, SPEECH])
        self.assertEqual({c[3]["model"] for c in self.calls() if c[:2] == SPEECH}, {TTS.split("/")[-1]})
        # over USB only the gateway answers: the studio can't tell its short WAV from a whole one (the Speak panel warns)
        job, _ = self.check_job(self.speech(flags={"gateway_garbles": True, "closed_port": True}),
                                entry=speech_entry(duration=0.12), calls=[SYS, RUN, RUN, SPEECH])
        self.assertEqual([c[3]["model"] for c in self.calls() if c[:2] == SPEECH], [TTS])

    def test_speech_long_text_is_split_and_joined(self):
        job, _ = self.check_job(self.speech(text=LONG_TEXT), entry=speech_entry(text=LONG_TEXT, parts=3, duration=1.25), calls=[SYS, RUN, *OWN, SPEECH, SPEECH, SPEECH])
        self.assertGreater(job["entry"]["params"]["parts"], 1)
        (name, wav), = self.file_media(job)  # the three parts, joined into one WAV
        self.assertEqual((name, len(wav), wav[:4]), ("speech.wav", 60044, b"RIFF"))

    def test_speech_errors(self):
        self.failed(self.speech(text="  "), "Nothing to say")
        self.failed(self.speech(ref_audio=None, flags={"fail": {"/v1/audio/speech": [500, "boom"]}}), "No voice was chosen")
        self.failed(self.speech(ref_text=" "), "transcript")
        self.failed(self.speech(ref_audio="nope"), "base64 data URI")
        self.failed({**self.speech(), "load": None}, "isn't loaded")
        self.failed(self.speech(flags={"fail": {"/v1/audio/speech": [500, "boom"]}}), "boom")
        self.failed(self.speech(flags={"speech_text": True}), "not audio at all")

    # ---------- transcribe ----------
    def test_transcribe(self):
        base = {"kind": "transcribe", "model": ASR, "load": ASR}
        job, _ = self.check_job({**base, "audio": WAV, "language": " en ", "name": "RUST TEST clip"},  # 24 kHz: resampled
                                entry=transcript_entry(language="en", name="RUST TEST clip"), calls=[SYS, TRANSCRIBE])
        self.assertEqual(job["entry"]["text"], "fake transcript")
        self.assertEqual(self.file_media(job, "sources"), [("audio.wav", WAV_FILE)])
        self.check_job({**base, "audio": WAV_44K, "ephemeral": True}, entry=transcript_entry(), calls=[SYS, TRANSCRIBE])
        # not a WAV: refused up front (the device can hang on it) instead of forwarded
        self.failed({**base, "audio": MP3}, "needs a WAV file")
        self.failed({**base, "audio": "data:audio/webm;base64," + base64.b64encode(b"webm?").decode()}, "needs a WAV file")
        self.check_job({**base, "audio": "data:audio/wav;base64," + base64.b64encode(b"RIFFjunk").decode()}, entry=transcript_entry(), calls=[SYS, TRANSCRIBE])

    def test_transcribe_long_audio_in_parts(self):
        # the Tiiny's speech-to-text fails past ~90 s: a 185 s recording goes in ~60 s parts cut at quiet points, and the
        # parts' texts are joined. The fake refuses a WAV over 100 s like the real one.
        rate, n = 16000, 185 * 16000
        quiet = [(53 * rate, 54 * rate), (110 * rate, 111 * rate)]
        s = [0 if any(a <= i < b for a, b in quiet) else (i * 37 % 2001) - 1000 for i in range(n)]
        buf = io.BytesIO()
        with wave.open(buf, "wb") as o:
            o.setnchannels(1)
            o.setsampwidth(2)
            o.setframerate(rate)
            o.writeframes(struct.pack(f"<{n}h", *s))
        long_wav = "data:audio/wav;base64," + base64.b64encode(buf.getvalue()).decode()
        job, _ = self.check_job({"kind": "transcribe", "model": ASR, "load": ASR, "audio": long_wav, "ephemeral": True},
                                entry=transcript_entry(" ".join(["fake transcript"] * 3)), calls=[SYS, TRANSCRIBE, TRANSCRIBE, TRANSCRIBE])
        posts = [c for c in self.last_calls if c[1] == "/v1/audio/transcriptions"]
        self.assertEqual(len(posts), 3)  # cut at 53 s and 110 s (the quiet spots); the last 75 s stay whole
        self.assertEqual(job["entry"]["text"], " ".join(["fake transcript"] * 3))

    def test_transcribe_errors(self):
        self.failed({"kind": "transcribe", "model": ASR, "audio": "nope"}, "base64 data URI")
        self.failed({"kind": "transcribe", "model": ASR, "audio": WAV, "flags": {"fail": {"/v1/audio/transcriptions": [503, "asleep"]}}}, "asleep")

    # ---------- loops ----------
    def test_loop(self):
        sessions = ("GET", "/v1/music/sessions")
        job, _ = self.check_job({"kind": "loop", "model": LOOP, "prompt": " RUST TEST piano ", "seconds_total": 8, "seed": 3, "steps": "", "bpm": 120,
                                 "bars": 4, "key": "C minor", "load": LOOP},
                                entry={**loop_entry(seconds_total=8, seed=3, bpm=120, bars=4, key="C minor"), "duration": 8, "device": {"music_session": "sess-1"}},
                                calls=[SYS, RUN, sessions, ("POST", "/v1/music/generate/wav"), sessions])
        self.assertEqual(self.file_media(job), [("loop.wav", WAV_FILE)])

    def test_loop_falls_back_to_the_model_port(self):
        req = {"kind": "loop", "model": LOOP, "prompt": "RUST TEST piano", "load": LOOP, "flags": {"no_music_gateway": True}}
        sessions = ("GET", "/v1/music/sessions")
        job, _ = self.check_job(req, entry=loop_entry(), calls=[SYS, RUN, sessions, ("POST", "/v1/music/generate/wav"), RUN, ("POST", "/v1/generate/wav"), sessions])
        self.assertEqual(self.file_media(job), [("loop.wav", WAV_FILE)])
        self.failed({**req, "flags": {"no_music_gateway": True, "closed_port": True}}, "isn't reachable")

    def test_loop_errors(self):
        self.failed({"kind": "loop", "model": LOOP, "prompt": " ", "load": LOOP}, "Describe the loop")
        self.failed({"kind": "loop", "model": LOOP, "prompt": "x"}, "isn't loaded")
        self.failed({"kind": "loop", "model": LOOP, "prompt": "x", "load": LOOP, "flags": {"fail": {"/v1/music/generate/wav": [500, "boom"]}}}, "boom")

    # ---------- songs ----------
    def song(self, **kw):
        return {"kind": "song", "model": SONG, "caption": "RUST TEST pop", "lyrics": "[verse]\nla la", "seed": 5, "load": SONG, **kw}

    def test_queue_loads_the_jobs_model(self):
        """#78: a job queued for a model that isn't loaded loads it at its turn; with the switch off it doesn't."""
        req = {k: v for k, v in self.song().items() if k != "load"}
        base = self.h.rs_url
        request(self.h.fake_url, "POST", "/__reset")  # nothing loaded
        done, calls = self.finish(request(base, "POST", "/api/jobs", {**req, "load": True}).json)
        self.assertEqual(done["status"], "done", done.get("error"))
        self.assertEqual(self.view(done), self.view({"status": "done", "message": "Done", "error": None, "entry": song_entry("direct", device=False, seed=5)}))
        self.assertTrue(any(c[0] == "POST" and c[1].endswith("/launch") for c in calls))
        request(base, "POST", "/api/settings/swap", {"swap": {"queue": False}})
        try:
            request(self.h.fake_url, "POST", "/__reset")
            done, _ = self.finish(request(base, "POST", "/api/jobs", {**req, "load": True}).json)
            self.assertEqual(done["status"], "error")
            self.assertEqual(done["error"], "SongGeneration-v2 isn't loaded. Load it from the Models panel.")
        finally:
            request(base, "POST", "/api/settings/swap", {"swap": {"queue": True}})

    def test_song_direct(self):
        entry = {**song_entry("direct", device=False, seconds_total=30, seed=5), "tracks": {"a-vocals.wav": "Vocals", "a-bass_line.wav": "Bass Line"}}
        job, _ = self.check_job(self.song(seconds_total=30, flags={"song_tracks": True}), entry=entry,
                                calls=[SYS, RUN, ("POST", "/v1/generate"), ("DELETE", "/v1/sessions/sess-1")])
        self.assertEqual(job["entry"]["params"]["route"], "direct")
        files = self.file_media(job)  # song + Vocals + Bass Line
        self.assertEqual([(n, len(b)) for n, b in files], [("song.wav", len(WAV_FILE)), ("vocals.wav", 13044), ("bass_line.wav", 13044)])
        self.assertEqual(files[0][1], WAV_FILE)
        self.assertEqual(sorted(job["entry"]["tracks"].values()), ["Bass Line", "Vocals"])
        self.assertEqual([name_suffix(f) for f in job["entry"]["tracks"]], ["vocals.wav", "bass_line.wav"])

    def test_song_instrumental_with_style_reference(self):
        job, _ = self.check_job(self.song(generation_type="bgm", lyrics="", reference_audio=LONG_REF),
                                entry=song_entry("direct", device=False, lyrics=None, generation_type="bgm", seed=5),
                                calls=[SYS, RUN, ("POST", "/v1/generate"), ("DELETE", "/v1/sessions/sess-1")])
        self.assertEqual(self.file_media(job), [("song.wav", WAV_FILE)])
        self.assertEqual(self.file_media(job, "sources"), [("styleref.wav", base64.b64decode(LONG_REF.split(",", 1)[1]))])

    def test_song_style_reference_is_checked_before_the_device_is_asked(self):
        short = "data:audio/wav;base64," + base64.b64encode(fake_tiiny.tiny_wav(3, 8000)).decode()
        long = "data:audio/wav;base64," + base64.b64encode(fake_tiiny.tiny_wav(125, 4000)).decode()
        ace = "ACE-Step/AceStep-Fake"
        cases = [(self.song(reference_audio=short), "The style reference must be at least 10 seconds long for SongGeneration-v2 (this one is 3.0 s)"),
                 (self.song(reference_audio=long), "The style reference can be at most 120 seconds long (this one is 125 s)"),
                 (self.song(reference_audio="data:audio/mpeg;base64," + base64.b64encode(b"ID3junk").decode()), "The style reference must be a WAV audio clip"),
                 (self.song(model="Fake/YuE2", load="Fake/YuE2", reference_audio=LONG_REF), "YuE2 doesn't take a style reference")]
        for req, text in cases:
            job, calls = self.run_job(req)
            self.assertEqual(job["status"], "error", text)
            self.assertEqual(job["error"], text)
            self.assertEqual([c[:2] for c in self.comparable(calls)], [SYS], "nothing was sent to the song model")
        # AceStep takes 5 s and up; 3 s is refused, 12 s goes through
        job, _ = self.run_job(self.song(model=ace, load=ace, reference_audio=short))
        self.assertEqual(job["error"], "The style reference must be at least 5 seconds long for AceStep-Fake (this one is 3.0 s)")
        self.check_job(self.song(model=ace, load=ace, reference_audio=LONG_REF), entry=song_entry("gateway", model=ace, seed=5, session="plain-1"),
                       calls=[SYS, RUN, ("POST", "/v1/music/generate"), PLAIN_DOWNLOAD])

    def test_song_polls_progress_while_composing(self):
        job, calls = self.check_job(self.song(flags={"delay": {"/v1/generate": 3.4}}), entry=song_entry("direct", device=False, seed=5))
        self.assertEqual(job["status"], "done")
        self.assertIn(("GET", "/v1/sessions"), [(m, p) for m, p, *_ in self.calls()])  # polled while waiting (how often depends on timing)
        pairs = [c[:2] for c in self.comparable(calls)]
        self.assertEqual(pairs[:3] + pairs[-1:], [SYS, RUN, ("POST", "/v1/generate"), ("DELETE", "/v1/sessions/sess-1")])

    def test_song_through_the_gateway_only(self):
        gw = {"closed_port": True}
        preview, resume = ("POST", "/v1/music/generate/preview"), ("POST", "/v1/music/generate/resume")
        job, _ = self.check_job(self.song(seconds_total=45, flags=gw), entry=song_entry("gateway-preview", seed=5, seconds_total=45.0),
                                calls=[SYS, RUN, preview, DOWNLOAD])
        self.assertEqual(job["entry"]["params"]["route"], "gateway-preview")
        self.assertNotIn("resume", job["entry"])
        self.assertEqual(self.file_media(job), [("song.wav", WAV_FILE)])
        self.assertEqual(self.file_media(job, "downloads"), [("song.mp3", MP3_FILE)])
        self.assertEqual(len(job["entry"]["downloads"]), 1)
        job, _ = self.check_job(self.song(seconds_total=300, flags=gw), entry=song_entry("gateway-full", seed=5, seconds_total=300.0),
                                calls=[SYS, RUN, preview, resume, DOWNLOAD])  # preview, then resume to the full song
        self.assertEqual(job["entry"]["params"]["route"], "gateway-full")
        self.check_job(self.song(flags=gw), entry=song_entry("gateway-preview", seed=5, seconds_total=60.0), calls=[SYS, RUN, preview, DOWNLOAD])  # no length: 60 s

    def test_song_saved_from_its_session_when_the_answer_is_lost(self):
        """#91 (Rust): the device finishes a plain-gateway song but the long answer never arrives; the job sees the
        finished session on the Tiiny and saves the song from it (MP3) instead of composing forever. Takes ~2 minutes."""
        ace = "ACE-Step/AceStep-Fake"
        job, _ = self.run_job(self.song(model=ace, load=ace, seconds_total=30, flags={"song_answer_lost": 600}), timeout=240)
        try:
            self.assertEqual(job["status"], "done", job.get("error"))
            e = job["entry"]
            self.assertEqual(len(e["outputs"]), 1)
            self.assertTrue(e["outputs"][0].endswith("-song.mp3"))
            self.assertTrue(e["device"]["music_session"])
        finally:
            request(self.h.fake_url, "POST", "/__flags", {"song_answer_lost": 0})

    def test_song_ace_step(self):
        # AceStep: the gateway's plain /generate on every link (it has no preview/resume and no port API)
        ace = "ACE-Step/AceStep-Fake"
        for extra in ({"seconds_total": 30}, {"seconds_total": 30, "flags": {"closed_port": True}}, {"preview": True}):
            job, _ = self.check_job(self.song(model=ace, load=ace, **extra),
                                    entry=song_entry("gateway", model=ace, seed=5, seconds_total=8 if "preview" in extra else 30, session="plain-1"),
                                    calls=[SYS, RUN, ("POST", "/v1/music/generate"), PLAIN_DOWNLOAD])
            self.assertEqual(job["status"], "done", extra)
            self.assertEqual(job["entry"]["params"]["route"], "gateway")
            self.assertNotIn("resume", job["entry"])
            self.assertEqual(self.file_media(job), [("song.wav", WAV_FILE)])
            posts = [(p, b) for m, p, _, b, _ in self.calls() if m == "POST"]
            self.assertEqual([p for p, _ in posts], ["/v1/music/generate"], extra)
            self.assertEqual(posts[0][1]["config"]["seconds_total"], 8 if "preview" in extra else 30)

    def test_song_preview_then_resume(self):
        preview, c1 = self.check_job(self.song(preview=True, seconds_total=30),
                                     entry={**song_entry("preview", seed=5), "resume": {"session_id": "sess-1"}},
                                     calls=[SYS, RUN, ("POST", "/v1/music/generate/preview"), DOWNLOAD])
        self.assertEqual(preview["entry"]["params"]["route"], "preview")
        sid = preview["entry"]["resume"]["session_id"]
        full, c2 = self.check_job(self.song(resume_session=sid, from_entry=preview["entry"]["id"]), entry=song_entry("gateway-full", seed=5),
                                  calls=[SYS, RUN, ("POST", "/v1/music/generate/resume"), DOWNLOAD], reset=False)
        self.assertEqual(full["entry"]["params"]["route"], "gateway-full")
        self.assertEqual(len(preview["entry"]["downloads"]), 1)
        self.assertEqual(self.file_media(preview, "downloads"), [("song.mp3", MP3_FILE)])
        history = {e["id"]: e for e in self.get("/api/history").json}  # the preview's resume token is gone once the full song exists
        self.assertNotIn("resume", history[preview["entry"]["id"]])
        self.assertEqual(history[full["entry"]["id"]]["params"]["route"], "gateway-full")

    def test_song_errors(self):
        self.failed(self.song(lyrics=" "), "needs lyrics")
        self.failed({**self.song(), "load": None}, "isn't loaded")
        self.failed(self.song(flags={"fail": {"/v1/generate": [500, "boom"]}}), "boom")
        self.failed(self.song(reference_audio="nope"), "base64 data URI")

    def test_speech_voice_kinds(self):
        # what the loaded voice model offers decides the voice: a built-in speaker, a described voice, or a clone
        base = {"kind": "speech", "text": "RUST TEST hello there.", "language": "en"}
        for mid, extra, sent, not_sent, entry in [
                ("Fake/CustomVoice", {"speaker": "ryan"}, {"voice": "ryan"}, ("ref_audio", "instruct"),
                 speech_entry(model="Fake/CustomVoice", language="en", voice="ryan", ref_text=None, speaker="ryan")),
                ("Fake/VoiceDesign", {"design": "A deep, slow voice."}, {"instruct": "A deep, slow voice."}, ("ref_audio", "voice"),
                 speech_entry(model="Fake/VoiceDesign", language="en", ref_text=None, design="A deep, slow voice."))]:
            job, _ = self.check_job({**base, "model": mid, "load": mid, **extra}, entry=entry, calls=[SYS, RUN, *OWN, SPEECH])
            self.assertEqual(job["status"], "done", (mid, job.get("error")))
            self.assertEqual(job["entry"]["sources"], [])  # no reference clip to keep
            body = next(c[3] for c in self.calls() if c[1] == "/v1/audio/speech")
            for k, v in sent.items():
                self.assertEqual(body[k], v)
            for k in not_sent:
                self.assertNotIn(k, body)
        # the own port refuses it too, then the gateway: its error is the one shown
        self.failed({**base, "model": "Fake/CustomVoice", "load": "Fake/CustomVoice", "speaker": "nobody"}, "Unsupported speaker")
        self.assertEqual([c[3]["model"] for c in self.calls() if c[:2] == SPEECH], ["CustomVoice", "Fake/CustomVoice"])
        # the gateway fails it (Supertonic-3 on 0.1.38): over USB that's the end, over Wi-Fi its own port answers first
        broken = {**base, "model": "Fake/Broken-TTS", "load": "Fake/Broken-TTS", "speaker": "F1"}
        self.failed({**broken, "flags": {"closed_port": True}}, "Over USB only the gateway is reachable")
        self.check_job(broken, entry=speech_entry(model="Fake/Broken-TTS", language="en", voice="F1", ref_text=None, speaker="F1"),
                       calls=[SYS, RUN, *OWN, SPEECH])
        self.assertEqual([c[3]["model"] for c in self.calls() if c[:2] == SPEECH], ["Broken-TTS"])
        self.load_on_fake("Fake/CustomVoice", "Qwen/Qwen3-TTS-Base")  # what the panel reads
        info = request(self.h.rs_url, "GET", "/api/model/info?id=Fake%2FCustomVoice").json
        self.assertEqual((info["id"], info["type"]), ("Fake/CustomVoice", "Text-to-Speech"))
        self.assertEqual(info["info"]["supported_speakers"], ["ryan", "ono_anna"])

    def test_song_follows_the_model(self):
        # the music service (/v1/music/info) decides the route: YuE2 and AceStep take the plain /generate (no preview),
        # SongGeneration keeps its preview; AceStep's own settings go nested in config.ace_step
        base = {"kind": "song", "caption": "RUST TEST pop", "lyrics": "[verse]\nla la", "generation_type": "mixed", "seconds_total": 20}
        plain = [SYS, RUN, ("POST", "/v1/music/generate"), PLAIN_DOWNLOAD]
        job, _ = self.check_job({**base, "model": "Fake/YuE2", "load": "Fake/YuE2", "preview": True},
                                entry=song_entry("gateway", model="Fake/YuE2", generation_type="mixed", seconds_total=20, session="plain-1"), calls=plain)
        self.assertEqual((job["status"], job["entry"]["params"]["route"]), ("done", "gateway"), job.get("error"))
        self.assertEqual([c[1] for c in self.calls() if c[0] == "POST"], ["/v1/music/generate"])
        ace = {"bpm": 92, "key_scale": "C major", "time_signature": "4", "vocal_language": "en", "negative_prompt": "drums", "nope": 1}
        clean = {k: v for k, v in ace.items() if k != "nope"}
        job, _ = self.check_job({**base, "model": "ACE-Step/AceStep-Fake", "load": "ACE-Step/AceStep-Fake", "ace_step": ace},
                                entry=song_entry("gateway", model="ACE-Step/AceStep-Fake", generation_type="mixed", seconds_total=20, ace_step=clean, session="plain-1"), calls=plain)
        sent = next(c[3] for c in self.calls() if c[1] == "/v1/music/generate")
        self.assertEqual(sent["config"]["ace_step"], clean)
        self.failed({**base, "model": "ACE-Step/AceStep-Fake", "load": "ACE-Step/AceStep-Fake", "ace_step": {"bpm": "fast"}}, "must be number")
        job, _ = self.check_job({**base, "model": SONG, "load": SONG, "preview": True},
                                entry={**song_entry("preview", generation_type="mixed"), "resume": {"session_id": "sess-1"}},
                                calls=[SYS, RUN, ("POST", "/v1/music/generate/preview"), DOWNLOAD])
        self.assertEqual(job["entry"]["params"]["route"], "preview")
        info = request(self.h.rs_url, "GET", "/api/model/info?id=Fake%2FSongGeneration-v2").json
        self.assertEqual(info["info"]["service_name"], "SongGenerationTTMService")

    # ---------- 3D ----------
    def mesh(self, **kw):
        return {"kind": "mesh", "model": MESH, "image": PNG, "load": MESH, "flags": {"task_polls": 1}, **kw}

    def task(self, n):
        return [TASKS, ("GET", f"/v1/3d/tasks/task-{n}")]

    def test_mesh_shape_only(self):
        job, _ = self.check_job(self.mesh(output_precision=256, seed=9, asset_quality=""), entry=mesh_entry(output_precision=256, seed=9),
                                calls=[SYS, RUN, CAPS, *self.task(1)])
        self.assertEqual([s["stage"] for s in job["entry"]["stages"]], ["shape"])
        self.assertEqual(self.file_media(job), [("model.glb", GLB_FILE)])
        self.assertEqual(self.file_media(job, "sources"), [("src.png", PNG_FILE)])

    def test_mesh_full_chain(self):
        flags = {"task_polls": 1, "rich_texture": True}
        stages = ("background", "shape", "simplify", "texture", "export")
        job, _ = self.check_job(self.mesh(remove_bg=True, faces="5000", texture=True, inference_quality=30, seed=1, flags=flags),
                                entry=mesh_entry(stages=stages, inference_quality=30, seed=1, texture=True, faces="5000", remove_bg=True),
                                calls=[SYS, RUN, CAPS, *[c for n in range(1, 6) for c in self.task(n)]])
        self.assertEqual([s["stage"] for s in job["entry"]["stages"]], list(stages))
        self.assertEqual(job["entry"]["params"]["faces"], "5000")
        self.assertEqual(self.file_media(job), [("model.glb", GLB_FILE)])

    def set_swap(self, **sw):
        request(self.h.rs_url, "POST", "/api/settings/swap", {"swap": sw})

    def test_mesh_cut_out(self):
        # The fake's chat model and Hunyuan3D fill the NPU, so Qwen-Image has to make room, then both come back.
        img, mesh, chat = IMAGE.replace("/", "%2F"), MESH.replace("/", "%2F"), "Qwen%2FQwen3-4B-Instruct"
        unload = ("GET", f"/api/v1/models/{img}/unload_candidate")
        job, _ = self.check_job(self.mesh(cutout=True, cutout_model=IMAGE, cutout_size="512x512", flags={"task_polls": 1, "cutout_png": True}),
                                entry=mesh_entry(cutout=IMAGE),
                                calls=[SYS, RUN, unload, ("POST", f"/api/v1/models/{chat}/stop"), ("POST", f"/api/v1/models/{mesh}/stop"), unload,
                                       ("POST", f"/api/v1/models/{img}/launch"), RUN, RUN, ("POST", "/v1/images/generations"),
                                       ("POST", f"/api/v1/models/{img}/stop"), ("POST", f"/api/v1/models/{chat}/launch"), ("POST", f"/api/v1/models/{mesh}/launch"),
                                       RUN, RUN, RUN, RUN, CAPS, *self.task(1)])
        self.assertEqual(job["entry"]["params"]["cutout"], IMAGE)
        self.assertEqual(len(job["entry"]["sources"]), 2)  # the source and its cut-out
        self.assertEqual([n for n, _ in self.file_media(job, "sources")], ["src.png", "0.png"])
        posts = [c[1] for c in self.calls() if c[0] == "POST"]
        for expected in (f"/api/v1/models/{mesh}/stop", f"/api/v1/models/{img}/launch", "/v1/images/generations",
                         f"/api/v1/models/{img}/stop", f"/api/v1/models/{mesh}/launch", "/v1/3d/tasks"):
            self.assertIn(expected, posts)
        sent = next(c[3] for c in self.calls() if c[1] == "/v1/images/generations")
        self.assertIn("transparent background", sent["prompt"])
        self.assertEqual(sent["size"], "512x512")
        # the background noise became fully transparent white; the subject and its soft edge stayed, cropped and
        # centred on a square (1x2 subject -> 3x3, column 1)
        cut = request(self.h.rs_url, "GET", "/images/" + job["entry"]["sources"][1]).body
        clear = [255, 255, 255, 0]
        self.assertEqual(png_pixels(cut), [*clear, 200, 30, 30, 255, *clear,
                                           *clear, 200, 30, 30, 128, *clear,
                                           *clear, *clear, *clear])

    def test_mesh_cut_out_refused(self):
        job, _ = self.check_job(self.mesh(), entry=mesh_entry(), calls=[SYS, RUN, CAPS, *self.task(1)])  # not asked for: no cut-out
        self.assertEqual((job["entry"]["params"]["cutout"], len(job["entry"]["sources"])), (None, 1))
        self.assertNotIn("/v1/images/generations", [c[1] for c in self.calls()])
        job = self.failed(self.mesh(cutout=True), "needs an image model")
        self.assertEqual(job["error"], "Cutting out the subject needs an image model that makes transparent pictures (Qwen-Image).")
        try:
            self.set_swap(mesh=False)  # 3D swapping off: the image model has to be loaded already
            job = self.failed(self.mesh(cutout=True, cutout_model=IMAGE), "isn't loaded")
            self.assertEqual(job["error"], "Qwen-Image-2.1 isn't loaded. Load it from the Models panel.")
            self.assertFalse([c for c in self.calls() if c[0] == "POST" and ("/launch" in c[1] or "/stop" in c[1])])
        finally:
            self.set_swap(mesh=True)

    def test_mesh_trellis(self):
        # TRELLIS.2 names its inputs differently (shape_artifact, ...), lists each stage's params, and returns its
        # artifacts by id; the stages, inputs and params come from its /v1/3d/capabilities
        full = {"kind": "mesh", "model": TRELLIS, "image": PNG, "load": TRELLIS, "flags": {"task_polls": 1},
                "faces": "10000", "texture": True, "output_precision": "standard", "inference_quality": "balance", "seed": 4}
        params = {"output_precision": "standard", "inference_quality": "balance", "seed": 4}
        job, _ = self.check_job(full, entry=mesh_entry(TRELLIS, stages=("shape", "simplify", "texture", "export"), texture=True, faces="10000", **params),
                                calls=[SYS, RUN, CAPS, *[c for n in range(1, 5) for c in self.task(n)], ("GET", "/v1/3d/tasks/task-4/artifacts/task-4-0")])
        self.assertEqual([s["stage"] for s in job["entry"]["stages"]], ["shape", "simplify", "texture", "export"])
        self.assertEqual(self.file_media(job), [("model.glb", GLB_FILE + b"trellis")])
        self.assertEqual(request(self.h.rs_url, "GET", "/images/" + job["entry"]["outputs"][0]).body, fake_tiiny.GLB + b"trellis")
        fetched = [c[1] for c in self.calls() if "/artifacts/" in c[1]]
        self.assertEqual(len(fetched), 1)  # only the GLB: the rest stays on the device, passed on by reference
        export = [c for c in self.calls() if c[0] == "POST" and c[1] == "/v1/3d/tasks"][-1]
        self.assertEqual(sorted(json.loads(export[4]["fields"]["input_refs"])), ["geometry_artifact", "material_artifact", "shape_artifact"])
        self.assertFalse(export[4]["files"])  # nothing uploaded again
        self.assertTrue(all(c[2].get("model") == [TRELLIS] for c in self.calls() if c[1].startswith("/v1/3d/")))
        # shape only: the shape's preview GLB is the model
        job, _ = self.check_job({k: v for k, v in full.items() if k not in ("faces", "texture")}, entry=mesh_entry(TRELLIS, **params),
                                calls=[SYS, RUN, CAPS, *self.task(1), ("GET", "/v1/3d/tasks/task-1/artifacts/task-1-1")])
        self.assertEqual(request(self.h.rs_url, "GET", "/images/" + job["entry"]["outputs"][0]).body, fake_tiiny.GLB + b"shape")

    def test_mesh_rides_out_a_dropped_link(self):
        # 502s on the status polls (and on the artifact fetch) are "no answer", not a failed job
        flags = {"task_polls": 1, "task_flaky": 2}
        t1 = ("GET", "/v1/3d/tasks/task-1")
        job, _ = self.check_job(self.mesh(flags=flags), entry=mesh_entry(), calls=[SYS, RUN, CAPS, TASKS, t1, t1, t1])
        self.assertEqual([c[1] for c in self.calls() if c[0] == "GET" and c[1].startswith("/v1/3d/tasks/")].count("/v1/3d/tasks/task-1"), 3)
        self.assertEqual(self.file_media(job), [("model.glb", GLB_FILE)])
        art = ("GET", "/v1/3d/tasks/task-1/artifacts/task-1-1")
        job, _ = self.check_job({"kind": "mesh", "model": TRELLIS, "image": PNG, "load": TRELLIS, "flags": {"task_polls": 1, "task_flaky": 1, "artifact_flaky": 2}},
                                entry=mesh_entry(TRELLIS), calls=[SYS, RUN, CAPS, TASKS, t1, t1, art, art, art])
        self.assertEqual(len([c for c in self.calls() if "/artifacts/" in c[1]]), 3)  # two 502s, then the file
        self.assertEqual(request(self.h.rs_url, "GET", "/images/" + job["entry"]["outputs"][0]).body, fake_tiiny.GLB + b"shape")

    def test_mesh_two_models_loaded(self):
        # with TRELLIS loaded too, a Hunyuan3D job must still reach Hunyuan3D (the gateway routes by ?model=)
        job, _ = self.check_job({**self.mesh(faces="5000", texture=True), "load": [TRELLIS, MESH]},
                                entry=mesh_entry(stages=("shape", "simplify", "texture", "export"), texture=True, faces="5000"),
                                calls=[SYS, RUN, CAPS, *[c for n in range(1, 5) for c in self.task(n)]])
        self.assertEqual([s["stage"] for s in job["entry"]["stages"]], ["shape", "simplify", "texture", "export"])
        self.assertTrue(all(c[2].get("model") == [MESH] for c in self.calls() if c[1].startswith("/v1/3d/")))

    def test_mesh_failure_keeps_no_files(self):
        # a build that fails after saving its source copy and cut-out must not leave them behind (no entry to delete)
        d = self.h.data / "images"
        before = len(list(d.iterdir()))
        self.failed(self.mesh(cutout=True, cutout_model=IMAGE, flags={"task_polls": 1, "fail_stage": "shape"}), "shape: out of memory")
        self.assertEqual(len(list(d.iterdir())), before)

    def test_mesh_errors(self):
        self.failed(self.mesh(image="nope"), "base64 data URI")
        self.failed({**self.mesh(), "load": None}, "isn't loaded")
        self.failed(self.mesh(flags={"task_polls": 1, "fail": {"/v1/3d/tasks": [500, "boom"]}}), "shape: boom")
        self.failed(self.mesh(flags={"task_polls": 1, "fail_stage": "shape"}), "shape: out of memory")

    def test_mesh_cancel_stops_the_remote_task(self):
        base = self.h.rs_url
        request(self.h.fake_url, "POST", "/__reset")
        self.load_on_fake(MESH)
        request(self.h.fake_url, "POST", "/__reset_log")
        request(self.h.fake_url, "POST", "/__flags", {"task_hold": True})
        job = request(base, "POST", "/api/jobs", {"kind": "mesh", "model": MESH, "image": PNG}).json
        job = self.wait_job(job, lambda j: j.get("stage"), "the 3D job's first answered poll")
        self.assertEqual(job.get("stage"), "shape")
        self.assertEqual(request(base, "POST", f"/api/jobs/{job['id']}/cancel").status, 200)
        job, calls = self.finish(job)
        self.assertEqual(job["status"], "cancelled")
        self.assertEqual((job["error"], job["message"]), (None, "Cancelled"))  # the device's "cancelled" is just the cancel taking effect
        deduped = [c[:2] for i, c in enumerate(calls) if i == 0 or c != calls[i - 1]]  # the number of polls depends on timing
        t1 = ("GET", "/v1/3d/tasks/task-1")
        self.assertEqual(deduped, [SYS, RUN, CAPS, TASKS, t1, ("POST", "/v1/3d/tasks/task-1/cancel"), t1])

    def test_mesh_cancel_when_the_device_ignores_it(self):
        # Used to poll forever and hold the device; now Stop ends the job on our side either way.
        base = self.h.rs_url
        request(self.h.fake_url, "POST", "/__reset")
        self.load_on_fake(MESH)
        request(self.h.fake_url, "POST", "/__reset_log")
        request(self.h.fake_url, "POST", "/__flags", {"task_hold": True, "ignore_cancel": True})
        job = request(base, "POST", "/api/jobs", {"kind": "mesh", "model": MESH, "image": PNG}).json
        job = self.wait_job(job, lambda j: j.get("stage"), "the 3D job's first answered poll")
        request(base, "POST", f"/api/jobs/{job['id']}/cancel")
        job, _ = self.finish(job)
        self.assertEqual((job["status"], job["message"]), ("cancelled", "Cancelled"))


class Video(Jobs):
    """Video (0.4.0): the gateway's /v1/videos with MiniMax-H3-Turbo's own sizes and steps (/v1/video/info)."""

    def video(self, **kw):
        return {"kind": "video", "model": VIDEO, "load": VIDEO, "prompt": "RUST TEST a red apple rolls", "size": "832x480", "seconds": 2, **kw}

    def test_video_from_text(self):
        job, _ = self.check_job(self.video(steps=4, seed=7, bogus=1), entry=video_entry(seed=7), calls=VIDEO_CALLS)
        self.assertEqual(job["status"], "done", job.get("error"))
        self.assertEqual((job["entry"]["kind"], job["entry"]["duration"], job["entry"]["sources"]), ("video", 2.3, []))  # 56 frames at 24 fps, like the real device
        self.assertEqual(job["entry"]["params"], {"model": VIDEO, "prompt": "RUST TEST a red apple rolls", "size": "832x480", "seconds": 2, "steps": 4, "seed": 7})
        self.assertEqual(self.file_media(job), [("video.mp4", MP4_FILE)])
        calls = self.calls()
        sent = next(c[3] for c in calls if c[:2] == ("POST", "/v1/videos"))
        self.assertEqual(sent, {"model": VIDEO, "prompt": "RUST TEST a red apple rolls", "size": "832x480", "seconds": "2", "steps": 4, "seed": 7})
        self.assertTrue(any(c[0] == "GET" and c[1].endswith("/content") for c in calls))
        self.assertEqual(calls[-1][0], "DELETE")  # the device's copy goes once the studio has it
        got = request(self.h.rs_url, "GET", "/images/" + job["entry"]["outputs"][0])
        self.assertEqual((got.status, got.headers["Content-Type"], got.body[4:8]), (200, "video/mp4", b"ftyp"))

    def test_video_without_steps_defaults_to_four(self):
        # the device's own default is 7, which turns a 2 s clip into ~12 minutes instead of ~7
        job, _ = self.check_job(self.video(), entry=video_entry(), calls=VIDEO_CALLS)
        self.assertEqual(job["entry"]["params"]["steps"], 4)
        sent = next(c[3] for c in self.calls() if c[:2] == ("POST", "/v1/videos"))
        self.assertEqual(sent, {"model": VIDEO, "prompt": "RUST TEST a red apple rolls", "size": "832x480", "seconds": "2", "steps": 4})

    def test_video_from_frames(self):
        job, _ = self.check_job(self.video(prompt="", image=PNG, last_image=PNG), entry=video_entry(prompt=""), calls=VIDEO_CALLS)  # the device ignores an end frame: not sent
        self.assertEqual(len(job["entry"]["sources"]), 1)
        self.assertEqual(self.file_media(job, "sources"), [("frame0.png", PNG_FILE)])
        sent = next(c[3] for c in self.calls() if c[:2] == ("POST", "/v1/videos"))
        self.assertEqual(sent["prompt"], "Bring this picture to life")
        self.assertEqual(sent["image"].split(",")[0], "data:image/png;base64")
        self.assertNotIn("last_image", sent)

    def test_video_longer_than_the_device_renders_is_refused(self):
        self.failed(self.video(seconds=8), "1 to 7 seconds")
        self.failed(self.video(seconds=0), "1 to 7 seconds")
        job, _ = self.run_job(self.video(seconds=7, steps=4))
        self.assertEqual(job["status"], "done", job.get("error"))

    def test_transcribe_can_be_cancelled_while_the_device_holds_the_request(self):
        base = self.h.rs_url
        request(self.h.fake_url, "POST", "/__reset")
        self.load_on_fake(ASR)
        request(self.h.fake_url, "POST", "/__reset_log")
        request(self.h.fake_url, "POST", "/__flags", {"delay": {"/v1/audio/transcriptions": 20}})
        job = request(base, "POST", "/api/jobs", {"kind": "transcribe", "model": ASR, "audio": WAV, "ephemeral": True}).json
        self.wait_sent("POST", "/v1/audio/transcriptions")
        request(base, "POST", f"/api/jobs/{job['id']}/cancel", {})
        done, _ = self.finish(job, timeout=10)  # well before the fake answers
        self.assertEqual(done["status"], "cancelled")

    def test_video_errors(self):
        self.failed(self.video(prompt=" "), "Describe the clip")
        self.failed({**self.video(), "load": None}, "isn't loaded")
        self.failed(self.video(size="1x1"), "exported for [1344x768")
        d = self.h.data / "images"
        before = len(list(d.iterdir()))
        self.failed(self.video(size="1x1", image=PNG), "exported for")  # its saved first frame goes too
        self.assertEqual(len(list(d.iterdir())), before)

    def test_video_rides_out_a_dropped_link(self):
        # a render takes many minutes: polls and the download that get no answer are retried, not the end of the job
        for flags in ({"video_flaky": 3}, {"content_flaky": 2}):
            job, _ = self.check_job(self.video(flags=flags), entry=video_entry())  # how many polls it took is up to the flaky link
            self.assertEqual(job["status"], "done", (flags, job.get("error")))
            self.assertEqual(self.file_media(job), [("video.mp4", MP4_FILE)])

    def test_video_cancel(self):
        base = self.h.rs_url
        request(self.h.fake_url, "POST", "/__reset")
        self.load_on_fake(VIDEO)
        request(self.h.fake_url, "POST", "/__reset_log")
        request(self.h.fake_url, "POST", "/__flags", {"video_hold": True})
        job = request(base, "POST", "/api/jobs", {k: v for k, v in self.video().items() if k != "load"}).json
        self.wait_until(lambda: any(m == "GET" and p.startswith("/v1/videos/") for m, p, *_ in self.calls()), "the studio's first poll of the clip")
        self.assertEqual(request(base, "POST", f"/api/jobs/{job['id']}/cancel").status, 200)
        job, calls = self.finish(job)
        self.assertEqual(job["status"], "cancelled")
        deduped = [c[:2] for i, c in enumerate(calls) if i == 0 or c != calls[i - 1]]
        self.assertEqual(deduped, [SYS, RUN, ("POST", "/v1/videos"), ("GET", VIDEO_ID), ("DELETE", VIDEO_ID), ("GET", VIDEO_ID)])  # deleting the clip stops the render

    def test_video_info_and_workspace(self):
        request(self.h.fake_url, "POST", "/__reset")
        self.load_on_fake(VIDEO)
        info = request(self.h.rs_url, "GET", "/api/model/info?id=" + VIDEO.replace("/", "%2F")).json
        self.assertEqual(info["id"], VIDEO)
        self.assertEqual(info["info"]["step_options"], [4, 5, 6, 7, 8])
        row = next(m for m in request(self.h.rs_url, "GET", "/api/catalog").json["models"] if m["id"] == VIDEO)
        self.assertEqual(row["workspace"], "video")

    def test_media_ranges(self):
        # a <video> seeks with byte ranges
        job, _ = self.run_job(self.video())
        base = self.h.rs_url
        path = "/images/" + job["entry"]["outputs"][0]
        n = len(fake_tiiny.MP4)
        for rng, code, cr, body in [("bytes=4-11", 206, f"bytes 4-11/{n}", fake_tiiny.MP4[4:12]),
                                    ("bytes=-4", 206, f"bytes {n - 4}-{n - 1}/{n}", fake_tiiny.MP4[-4:]),
                                    ("bytes=10-", 206, f"bytes 10-{n - 1}/{n}", fake_tiiny.MP4[10:]),
                                    (f"bytes={n}-", 416, f"bytes */{n}", b"")]:
            got = request(base, "GET", path, headers={"Range": rng})
            self.assertEqual((got.status, got.headers.get("Content-Range"), got.body), (code, cr, body), rng)
        self.assertEqual(request(base, "GET", path).headers.get("Accept-Ranges"), "bytes")


def strip(obj):
    if isinstance(obj, dict):
        return {k: (sorted(v.values()) if k == "tracks" else strip(v)) for k, v in obj.items() if k not in IGNORE}
    if isinstance(obj, list):
        return [strip(v) for v in obj]
    return obj


if __name__ == "__main__":
    unittest.main()
