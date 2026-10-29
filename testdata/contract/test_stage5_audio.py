"""Stage 5 contract tests: speech, transcription, loops, songs and image-to-3D, run as real jobs on both servers
against the fake Tiiny. Compares the finished job, the saved files and the requests each server made (multipart
bodies compared as parsed fields, file names and file checksums, since the boundaries differ)."""
import base64
import json
import time
import unittest

from harness import ContractCase, request  # first: puts testdata/ on the path
import fake_tiiny  # noqa: E402

AUTH = {"Authorization": "Bearer fake-key"}
TTS, ASR, LOOP, SONG, MESH = "Qwen/Qwen3-TTS-Base", "Fake/Whisper-ASR", "Fake/Foundation-1", "Fake/SongGeneration-v2", "Tencent/Hunyuan3D-2.1"
IMAGE = "Qwen/Qwen-Image-2.1"
PNG = "data:image/png;base64," + base64.b64encode(fake_tiiny.tiny_png()).decode()
WAV = "data:audio/wav;base64," + base64.b64encode(fake_tiiny.tiny_wav()).decode()
WAV_44K = "data:audio/x-wav;base64," + base64.b64encode(fake_tiiny.tiny_wav(0.05, 44100)).decode()
MP3 = "data:audio/mpeg;base64," + base64.b64encode(fake_tiiny.MP3).decode()
# ids, times and file names (they contain the job id) differ per run; files are compared by files_equal
IGNORE = ("id", "created", "started", "finished", "seconds", "t", "outputs", "sources", "downloads", "from_entry")
LONG_TEXT = " ".join(f"Sentence number {i} is here, and it goes on a little." for i in range(14))


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


class Audio(ContractCase):
    maxDiff = None

    def run_job(self, base, req, reset=True):
        """Run one job to the end; returns (job, requests made to the fake). `load` loads a model first, `flags`
        sets fake behaviour (see fake_tiiny.py); reset=False continues with the same fake state (and a fresh log)."""
        req = dict(req)
        load, flags = req.pop("load", None), req.pop("flags", None)
        if reset:
            request(self.h.fake_url, "POST", "/__reset")
            if load:
                request(self.h.fake_url, "POST", "/api/v1/models/" + load.replace("/", "%2F") + "/launch", {"model_id": load}, headers=AUTH)
                time.sleep(0.4)
        request(self.h.fake_url, "POST", "/__reset_log")
        if flags:
            request(self.h.fake_url, "POST", "/__flags", flags)
        job = request(base, "POST", "/api/jobs", req).json
        return self.finish(base, job)

    def finish(self, base, job):
        for _ in range(1200):
            job = request(base, "GET", f"/api/jobs/{job['id']}").json
            if job.get("finished"):
                break
            time.sleep(0.05)
        return job, self.calls()

    def calls(self):
        log = json.loads(request(self.h.fake_url, "GET", "/__requests").body)
        return [(r["method"], r["path"], r["query"], r["body"], r.get("multipart")) for r in log if r["path"] != "/"]

    def same_job(self, req, **kw):
        py, py_calls = self.run_job(self.h.py_url, req, **kw)
        rs, rs_calls = self.run_job(self.h.rs_url, req, **kw)
        self.assertEqual(strip(py), strip(rs))
        self.assertEqual(py_calls, rs_calls)
        return py, rs

    def files_equal(self, py, rs, key="outputs"):
        p, r = py["entry"][key], rs["entry"][key]
        self.assertEqual(len(p), len(r))
        for a, b in zip(p, r):
            self.assertEqual(a.split("-", 1)[-1], b.split("-", 1)[-1])  # "-speech.wav", "-song.wav"
            self.assertEqual(request(self.h.py_url, "GET", "/images/" + a).body, request(self.h.rs_url, "GET", "/images/" + b).body)

    def failed(self, req, text=None, **kw):
        py, _ = self.same_job(req, **kw)
        self.assertEqual(py["status"], "error")
        if text:
            self.assertIn(text, py["error"])
        return py

    # ---------- speech ----------
    def speech(self, **kw):
        return {"kind": "speech", "model": TTS, "text": "RUST TEST hello there.", "ref_audio": WAV, "ref_text": "hi", "load": TTS, **kw}

    def test_speech_short(self):
        py, rs = self.same_job(self.speech(language="en", instruct=" calm ", voice_name="RUST TEST voice"))
        self.assertEqual(py["status"], "done")
        self.assertEqual(py["entry"]["params"]["parts"], 1)
        self.files_equal(py, rs)
        self.files_equal(py, rs, "sources")

    def test_speech_long_text_is_split_and_joined(self):
        py, rs = self.same_job(self.speech(text=LONG_TEXT))
        self.assertGreater(py["entry"]["params"]["parts"], 1)
        self.files_equal(py, rs)

    def test_speech_errors(self):
        self.failed(self.speech(text="  "), "Nothing to say")
        self.failed(self.speech(ref_audio=None), "cloned voice")
        self.failed(self.speech(ref_text=" "), "transcript")
        self.failed(self.speech(ref_audio="nope"), "base64 data URI")
        self.failed({**self.speech(), "load": None}, "isn't loaded")
        self.failed(self.speech(flags={"fail": {"/v1/audio/speech": [500, "boom"]}}), "boom")
        self.failed(self.speech(flags={"speech_text": True}), "not audio at all")

    # ---------- transcribe ----------
    def test_transcribe(self):
        base = {"kind": "transcribe", "model": ASR}
        py, rs = self.same_job({**base, "audio": WAV, "language": " en ", "name": "RUST TEST clip"})  # 24 kHz: resampled
        self.assertEqual(py["entry"]["text"], "fake transcript")
        self.files_equal(py, rs, "sources")
        self.same_job({**base, "audio": WAV_44K, "ephemeral": True})
        py, rs = self.same_job({**base, "audio": MP3})  # not a WAV: sent as it is
        self.assertTrue(py["entry"]["sources"][0].endswith("audio.mp3"))
        self.same_job({**base, "audio": "data:audio/webm;base64," + base64.b64encode(b"webm?").decode()})
        self.same_job({**base, "audio": "data:audio/wav;base64," + base64.b64encode(b"RIFFjunk").decode()})

    def test_transcribe_errors(self):
        self.failed({"kind": "transcribe", "model": ASR, "audio": "nope"}, "base64 data URI")
        self.failed({"kind": "transcribe", "model": ASR, "audio": WAV, "flags": {"fail": {"/v1/audio/transcriptions": [503, "asleep"]}}}, "asleep")

    # ---------- loops ----------
    def test_loop(self):
        py, rs = self.same_job({"kind": "loop", "model": LOOP, "prompt": " RUST TEST piano ", "seconds_total": 8, "seed": 3, "steps": "", "bpm": 120,
                                "bars": 4, "key": "C minor", "load": LOOP})
        self.assertEqual(py["status"], "done")
        self.files_equal(py, rs)

    def test_loop_falls_back_to_the_model_port(self):
        req = {"kind": "loop", "model": LOOP, "prompt": "RUST TEST piano", "load": LOOP, "flags": {"no_music_gateway": True}}
        py, rs = self.same_job(req)
        self.assertEqual(py["status"], "done")
        self.files_equal(py, rs)
        self.failed({**req, "flags": {"no_music_gateway": True, "closed_port": True}}, "isn't reachable")

    def test_loop_errors(self):
        self.failed({"kind": "loop", "model": LOOP, "prompt": " ", "load": LOOP}, "Describe the loop")
        self.failed({"kind": "loop", "model": LOOP, "prompt": "x"}, "isn't loaded")
        self.failed({"kind": "loop", "model": LOOP, "prompt": "x", "load": LOOP, "flags": {"fail": {"/v1/music/generate/wav": [500, "boom"]}}}, "boom")

    # ---------- songs ----------
    def song(self, **kw):
        return {"kind": "song", "model": SONG, "caption": "RUST TEST pop", "lyrics": "[verse]\nla la", "seed": 5, "load": SONG, **kw}

    def test_song_direct(self):
        py, rs = self.same_job(self.song(seconds_total=30, flags={"song_tracks": True}))
        self.assertEqual(py["entry"]["params"]["route"], "direct")
        self.assertEqual(len(py["entry"]["outputs"]), 3)  # song + Vocals + Bass Line
        self.files_equal(py, rs)
        self.assertEqual(sorted(py["entry"]["tracks"].values()), ["Bass Line", "Vocals"])
        self.assertEqual([f.split("-", 1)[1] for f in py["entry"]["tracks"]], [f.split("-", 1)[1] for f in rs["entry"]["tracks"]])

    def test_song_instrumental_with_style_reference(self):
        py, rs = self.same_job(self.song(generation_type="bgm", lyrics="", reference_audio=WAV))
        self.assertEqual(py["status"], "done")
        self.files_equal(py, rs)
        self.files_equal(py, rs, "sources")

    def test_song_polls_progress_while_composing(self):
        py, _ = self.same_job(self.song(flags={"delay": {"/v1/generate": 3.4}}))
        self.assertEqual(py["status"], "done")
        self.assertIn(("GET", "/v1/sessions"), [(m, p) for m, p, *_ in self.calls()])  # the Rust run's log: polled once while waiting

    def test_song_through_the_gateway_only(self):
        gw = {"closed_port": True}
        py, rs = self.same_job(self.song(seconds_total=45, flags=gw))
        self.assertEqual(py["entry"]["params"]["route"], "gateway-preview")
        self.assertNotIn("resume", py["entry"])
        self.files_equal(py, rs)
        self.files_equal(py, rs, "downloads")
        self.assertEqual(len(py["entry"]["downloads"]), 1)
        py, rs = self.same_job(self.song(seconds_total=300, flags=gw))  # preview, then resume to the full song
        self.assertEqual(py["entry"]["params"]["route"], "gateway-full")
        self.same_job(self.song(flags=gw))  # no length: 60 s

    def test_song_ace_step(self):
        # AceStep: the gateway's plain /generate on every link (it has no preview/resume and no port API)
        ace = "ACE-Step/AceStep-Fake"
        for extra in ({"seconds_total": 30}, {"seconds_total": 30, "flags": {"closed_port": True}}, {"preview": True}):
            py, rs = self.same_job(self.song(model=ace, load=ace, **extra))
            self.assertEqual(py["status"], "done", extra)
            self.assertEqual(py["entry"]["params"]["route"], "gateway")
            self.assertNotIn("resume", py["entry"])
            self.files_equal(py, rs)
            posts = [(p, b) for m, p, _, b, _ in self.calls() if m == "POST"]
            self.assertEqual([p for p, _ in posts], ["/v1/music/generate"], extra)
            self.assertEqual(posts[0][1]["config"]["seconds_total"], 8 if "preview" in extra else 30)

    def test_song_preview_then_resume(self):
        outs = []
        for base in (self.h.py_url, self.h.rs_url):
            preview, c1 = self.run_job(base, self.song(preview=True, seconds_total=30))
            self.assertEqual(preview["entry"]["params"]["route"], "preview")
            sid = preview["entry"]["resume"]["session_id"]
            full, c2 = self.run_job(base, self.song(resume_session=sid, from_entry=preview["entry"]["id"]), reset=False)
            self.assertEqual(full["entry"]["params"]["route"], "gateway-full")
            outs.append((preview, c1, full, c2))
        (p1, c1, f1, c2), (p2, c3, f2, c4) = outs
        self.assertEqual((strip(p1), c1, strip(f1), c2), (strip(p2), c3, strip(f2), c4))
        self.assertEqual(len(p1["entry"]["downloads"]), 1)
        self.files_equal(p1, p2, "downloads")
        self.same(self.get("/api/history"), IGNORE + ("tracks",))  # the preview's resume token is gone on both

    def test_song_errors(self):
        self.failed(self.song(lyrics=" "), "needs lyrics")
        self.failed({**self.song(), "load": None}, "isn't loaded")
        self.failed(self.song(flags={"fail": {"/v1/generate": [500, "boom"]}}), "boom")
        self.failed(self.song(reference_audio="nope"), "base64 data URI")

    # ---------- 3D ----------
    def mesh(self, **kw):
        return {"kind": "mesh", "model": MESH, "image": PNG, "load": MESH, "flags": {"task_polls": 1}, **kw}

    def test_mesh_shape_only(self):
        py, rs = self.same_job(self.mesh(output_precision=256, seed=9, asset_quality=""))
        self.assertEqual(py["status"], "done")
        self.assertEqual([s["stage"] for s in py["entry"]["stages"]], ["shape"])
        self.files_equal(py, rs)
        self.files_equal(py, rs, "sources")

    def test_mesh_full_chain(self):
        flags = {"task_polls": 1, "rich_texture": True}
        py, rs = self.same_job(self.mesh(remove_bg=True, faces="5000", texture=True, inference_quality=30, seed=1, flags=flags))
        self.assertEqual([s["stage"] for s in py["entry"]["stages"]], ["background", "shape", "simplify", "texture", "export"])
        self.assertEqual(py["entry"]["params"]["faces"], "5000")
        self.files_equal(py, rs)

    def set_swap(self, **sw):
        for base in (self.h.py_url, self.h.rs_url):
            request(base, "POST", "/api/settings/swap", {"swap": sw})

    def test_mesh_cut_out(self):
        # The fake's chat model and Hunyuan3D fill the NPU, so Qwen-Image has to make room, then both come back.
        py, rs = self.same_job(self.mesh(cutout=True, cutout_model=IMAGE, cutout_size="512x512", flags={"task_polls": 1, "cutout_png": True}))
        self.assertEqual(py["status"], "done")
        self.assertEqual(py["entry"]["params"]["cutout"], IMAGE)
        self.assertEqual(len(py["entry"]["sources"]), 2)  # the source and its cut-out
        self.files_equal(py, rs, "sources")
        posts = [c[1] for c in self.calls() if c[0] == "POST"]
        img, mesh = IMAGE.replace("/", "%2F"), MESH.replace("/", "%2F")
        for expected in (f"/api/v1/models/{mesh}/stop", f"/api/v1/models/{img}/launch", "/v1/images/generations",
                         f"/api/v1/models/{img}/stop", f"/api/v1/models/{mesh}/launch", "/v1/3d/tasks"):
            self.assertIn(expected, posts)
        sent = next(c[3] for c in self.calls() if c[1] == "/v1/images/generations")
        self.assertIn("transparent background", sent["prompt"])
        self.assertEqual(sent["size"], "512x512")
        # the background noise became fully transparent white; the subject and its soft edge stayed
        cut = request(self.h.py_url, "GET", "/images/" + py["entry"]["sources"][1]).body
        self.assertEqual(png_pixels(cut), [255, 255, 255, 0, 200, 30, 30, 255, 255, 255, 255, 0,
                                           255, 255, 255, 0, 200, 30, 30, 128, 255, 255, 255, 0])

    def test_mesh_cut_out_refused(self):
        py, _ = self.same_job(self.mesh())  # not asked for: no cut-out
        self.assertEqual((py["entry"]["params"]["cutout"], len(py["entry"]["sources"])), (None, 1))
        self.assertNotIn("/v1/images/generations", [c[1] for c in self.calls()])
        self.failed(self.mesh(cutout=True), "needs an image model")
        try:
            self.set_swap(mesh=False)  # 3D swapping off: the image model has to be loaded already
            self.failed(self.mesh(cutout=True, cutout_model=IMAGE), "isn't loaded")
            self.assertFalse([c for c in self.calls() if c[0] == "POST" and ("/launch" in c[1] or "/stop" in c[1])])
        finally:
            self.set_swap(mesh=True)

    def test_mesh_errors(self):
        self.failed(self.mesh(image="nope"), "base64 data URI")
        self.failed({**self.mesh(), "load": None}, "isn't loaded")
        self.failed(self.mesh(flags={"task_polls": 1, "fail": {"/v1/3d/tasks": [500, "boom"]}}), "shape: boom")
        self.failed(self.mesh(flags={"task_polls": 1, "fail_stage": "shape"}), "shape: out of memory")

    def test_mesh_cancel_stops_the_remote_task(self):
        results = []
        for base in (self.h.py_url, self.h.rs_url):
            request(self.h.fake_url, "POST", "/__reset")
            request(self.h.fake_url, "POST", "/api/v1/models/" + MESH.replace("/", "%2F") + "/launch", {"model_id": MESH}, headers=AUTH)
            time.sleep(0.4)
            request(self.h.fake_url, "POST", "/__reset_log")
            request(self.h.fake_url, "POST", "/__flags", {"task_hold": True})
            job = request(base, "POST", "/api/jobs", {"kind": "mesh", "model": MESH, "image": PNG}).json
            for _ in range(400):  # until the first poll of the task has been answered
                job = request(base, "GET", f"/api/jobs/{job['id']}").json
                if job.get("stage"):
                    break
                time.sleep(0.05)
            self.assertEqual(job.get("stage"), "shape")
            self.assertEqual(request(base, "POST", f"/api/jobs/{job['id']}/cancel").status, 200)
            job, calls = self.finish(base, job)
            self.assertEqual(job["status"], "cancelled")
            self.assertEqual((job["error"], job["message"]), (None, "Cancelled"))  # the device's "cancelled" is just the cancel taking effect
            deduped = [c for i, c in enumerate(calls) if i == 0 or c != calls[i - 1]]  # the number of polls depends on timing
            results.append((strip(job), deduped))
        self.assertEqual(results[0], results[1])
        self.assertIn("POST", [m for m, p, *_ in results[0][1] if p.endswith("/cancel")])

    def test_mesh_cancel_when_the_device_ignores_it(self):
        # Used to poll forever and hold the device; now Stop ends the job on our side either way.
        results = []
        for base in (self.h.py_url, self.h.rs_url):
            request(self.h.fake_url, "POST", "/__reset")
            request(self.h.fake_url, "POST", "/api/v1/models/" + MESH.replace("/", "%2F") + "/launch", {"model_id": MESH}, headers=AUTH)
            time.sleep(0.4)
            request(self.h.fake_url, "POST", "/__reset_log")
            request(self.h.fake_url, "POST", "/__flags", {"task_hold": True, "ignore_cancel": True})
            job = request(base, "POST", "/api/jobs", {"kind": "mesh", "model": MESH, "image": PNG}).json
            for _ in range(400):
                job = request(base, "GET", f"/api/jobs/{job['id']}").json
                if job.get("stage"):
                    break
                time.sleep(0.05)
            request(base, "POST", f"/api/jobs/{job['id']}/cancel")
            job, _ = self.finish(base, job)
            results.append((job["status"], job["message"]))
        self.assertEqual(results, [("cancelled", "Cancelled")] * 2)


def strip(obj):
    if isinstance(obj, dict):
        return {k: (sorted(v.values()) if k == "tracks" else strip(v)) for k, v in obj.items() if k not in IGNORE}
    if isinstance(obj, list):
        return [strip(v) for v in obj]
    return obj


if __name__ == "__main__":
    unittest.main()
