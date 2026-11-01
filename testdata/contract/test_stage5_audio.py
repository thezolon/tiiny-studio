"""Stage 5 contract tests: speech, transcription, loops, songs and image-to-3D, run as real jobs on both servers
against the fake Tiiny. Compares the finished job, the saved files and the requests each server made (multipart
bodies compared as parsed fields, file names and file checksums, since the boundaries differ)."""
import base64
import json
import unittest

from harness import ContractCase, request  # first: puts testdata/ on the path
import fake_tiiny  # noqa: E402

TTS, ASR, LOOP, SONG, MESH = "Qwen/Qwen3-TTS-Base", "Fake/Whisper-ASR", "Fake/Foundation-1", "Fake/SongGeneration-v2", "Tencent/Hunyuan3D-2.1"
IMAGE = "Qwen/Qwen-Image-2.1"
TRELLIS = "microsoft/TRELLIS.2-4B"
VIDEO = fake_tiiny.VIDEO
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


class Jobs(ContractCase):
    """Runs media jobs on both servers against the fake and compares them."""
    maxDiff = None

    def view(self, job):
        return strip(job)

    def failed(self, req, text=None, **kw):
        py, _ = self.same_job(req, **kw)
        self.assertEqual(py["status"], "error")
        if text:
            self.assertIn(text, py["error"])
        return py


class Audio(Jobs):
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

    def test_speech_voice_kinds(self):
        # what the loaded voice model offers decides the voice: a built-in speaker, a described voice, or a clone
        base = {"kind": "speech", "text": "RUST TEST hello there.", "language": "en"}
        for mid, extra, sent, not_sent in [("Fake/CustomVoice", {"speaker": "ryan"}, {"voice": "ryan"}, ("ref_audio", "instruct")),
                                           ("Fake/VoiceDesign", {"design": "A deep, slow voice."}, {"instruct": "A deep, slow voice."}, ("ref_audio", "voice"))]:
            py, rs = self.same_job({**base, "model": mid, "load": mid, **extra})
            self.assertEqual(py["status"], "done", (mid, py.get("error")))
            self.assertEqual(py["entry"]["sources"], [])  # no reference clip to keep
            body = next(c[3] for c in self.calls() if c[1] == "/v1/audio/speech")
            for k, v in sent.items():
                self.assertEqual(body[k], v)
            for k in not_sent:
                self.assertNotIn(k, body)
        self.failed({**base, "model": "Fake/CustomVoice", "load": "Fake/CustomVoice", "speaker": "nobody"}, "Unsupported speaker")
        # the gateway fails it (Supertonic-3 on 0.1.38); over USB that's the end, over Wi-Fi its own port answers
        broken = {**base, "model": "Fake/Broken-TTS", "load": "Fake/Broken-TTS", "speaker": "F1"}
        self.failed({**broken, "flags": {"closed_port": True}}, "don't work through TiinyOS's gateway")
        py, _ = self.same_job(broken)
        self.assertEqual(py["status"], "done", py.get("error"))
        self.assertEqual([c[3]["model"] for c in self.calls() if c[:2] == ("POST", "/v1/audio/speech")], ["Fake/Broken-TTS", "Broken-TTS"])
        self.load_on_fake("Fake/CustomVoice", "Qwen/Qwen3-TTS-Base")  # what the panel reads
        infos = [request(b, "GET", "/api/model/info?id=Fake%2FCustomVoice").json for b in (self.h.py_url, self.h.rs_url)]
        self.assertEqual(infos[0], infos[1])
        self.assertEqual(infos[0]["info"]["supported_speakers"], ["ryan", "ono_anna"])

    def test_song_follows_the_model(self):
        # the music service (/v1/music/info) decides the route: YuE2 and AceStep take the plain /generate (no preview),
        # SongGeneration keeps its preview; AceStep's own settings go nested in config.ace_step
        base = {"kind": "song", "caption": "RUST TEST pop", "lyrics": "[verse]\nla la", "generation_type": "mixed", "seconds_total": 20}
        py, _ = self.same_job({**base, "model": "Fake/YuE2", "load": "Fake/YuE2", "preview": True})
        self.assertEqual((py["status"], py["entry"]["params"]["route"]), ("done", "gateway"), py.get("error"))
        self.assertEqual([c[1] for c in self.calls() if c[0] == "POST"], ["/v1/music/generate"])
        ace = {"bpm": 92, "key_scale": "C major", "time_signature": "4", "vocal_language": "en", "negative_prompt": "drums", "nope": 1}
        py, _ = self.same_job({**base, "model": "ACE-Step/AceStep-Fake", "load": "ACE-Step/AceStep-Fake", "ace_step": ace})
        self.assertEqual(py["status"], "done", py.get("error"))
        sent = next(c[3] for c in self.calls() if c[1] == "/v1/music/generate")
        self.assertEqual(sent["config"]["ace_step"], {k: v for k, v in ace.items() if k != "nope"})
        self.failed({**base, "model": "ACE-Step/AceStep-Fake", "load": "ACE-Step/AceStep-Fake", "ace_step": {"bpm": "fast"}}, "must be number")
        py, _ = self.same_job({**base, "model": SONG, "load": SONG, "preview": True})
        self.assertEqual(py["entry"]["params"]["route"], "preview")
        infos = [request(b, "GET", "/api/model/info?id=Fake%2FSongGeneration-v2").json for b in (self.h.py_url, self.h.rs_url)]
        self.assertEqual(infos[0], infos[1])
        self.assertEqual(infos[0]["info"]["service_name"], "SongGenerationTTMService")

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

    def test_mesh_trellis(self):
        # TRELLIS.2 names its inputs differently (shape_artifact, ...), lists each stage's params, and returns its
        # artifacts by id; the stages, inputs and params come from its /v1/3d/capabilities
        full = {"kind": "mesh", "model": TRELLIS, "image": PNG, "load": TRELLIS, "flags": {"task_polls": 1},
                "faces": "10000", "texture": True, "output_precision": "standard", "inference_quality": "balance", "seed": 4}
        py, rs = self.same_job(full)
        self.assertEqual(py["status"], "done", py.get("error"))
        self.assertEqual([s["stage"] for s in py["entry"]["stages"]], ["shape", "simplify", "texture", "export"])
        self.files_equal(py, rs)
        self.assertEqual(request(self.h.py_url, "GET", "/images/" + py["entry"]["outputs"][0]).body, fake_tiiny.GLB + b"trellis")
        fetched = [c[1] for c in self.calls() if "/artifacts/" in c[1]]
        self.assertEqual(len(fetched), 1)  # only the GLB: the rest stays on the device, passed on by reference
        export = [c for c in self.calls() if c[0] == "POST" and c[1] == "/v1/3d/tasks"][-1]
        self.assertEqual(sorted(json.loads(export[4]["fields"]["input_refs"])), ["geometry_artifact", "material_artifact", "shape_artifact"])
        self.assertFalse(export[4]["files"])  # nothing uploaded again
        self.assertTrue(all(c[2].get("model") == [TRELLIS] for c in self.calls() if c[1].startswith("/v1/3d/")))
        # shape only: the shape's preview GLB is the model
        py, _ = self.same_job({k: v for k, v in full.items() if k not in ("faces", "texture")})
        self.assertEqual(request(self.h.py_url, "GET", "/images/" + py["entry"]["outputs"][0]).body, fake_tiiny.GLB + b"shape")

    def test_mesh_rides_out_a_dropped_link(self):
        # 502s on the status polls (and on the artifact fetch) are "no answer", not a failed job
        flags = {"task_polls": 1, "task_flaky": 2}
        py, rs = self.same_job(self.mesh(flags=flags))
        self.assertEqual(py["status"], "done", py.get("error"))
        self.assertEqual([c[1] for c in self.calls() if c[0] == "GET" and c[1].startswith("/v1/3d/tasks/")].count("/v1/3d/tasks/task-1"), 3)
        self.files_equal(py, rs)
        py, rs = self.same_job({"kind": "mesh", "model": TRELLIS, "image": PNG, "load": TRELLIS,
                                "flags": {"task_polls": 1, "task_flaky": 1, "artifact_flaky": 2}})
        self.assertEqual(py["status"], "done", py.get("error"))
        self.assertEqual(len([c for c in self.calls() if "/artifacts/" in c[1]]), 3)  # two 502s, then the file
        self.files_equal(py, rs)
        self.assertEqual(request(self.h.py_url, "GET", "/images/" + py["entry"]["outputs"][0]).body, fake_tiiny.GLB + b"shape")

    def test_mesh_two_models_loaded(self):
        # with TRELLIS loaded too, a Hunyuan3D job must still reach Hunyuan3D (the gateway routes by ?model=)
        py, rs = self.same_job({**self.mesh(faces="5000", texture=True), "load": [TRELLIS, MESH]})
        self.assertEqual(py["status"], "done", py.get("error"))
        self.assertEqual([s["stage"] for s in py["entry"]["stages"]], ["shape", "simplify", "texture", "export"])
        self.assertTrue(all(c[2].get("model") == [MESH] for c in self.calls() if c[1].startswith("/v1/3d/")))

    def test_mesh_failure_keeps_no_files(self):
        # a build that fails after saving its source copy and cut-out must not leave them behind (no entry to delete)
        dirs = [d / "images" for d in self.h.tmp[-2:]]
        before = [len(list(d.iterdir())) for d in dirs]
        self.failed(self.mesh(cutout=True, cutout_model=IMAGE, flags={"task_polls": 1, "fail_stage": "shape"}), "shape: out of memory")
        self.assertEqual([len(list(d.iterdir())) for d in dirs], before)

    def test_mesh_errors(self):
        self.failed(self.mesh(image="nope"), "base64 data URI")
        self.failed({**self.mesh(), "load": None}, "isn't loaded")
        self.failed(self.mesh(flags={"task_polls": 1, "fail": {"/v1/3d/tasks": [500, "boom"]}}), "shape: boom")
        self.failed(self.mesh(flags={"task_polls": 1, "fail_stage": "shape"}), "shape: out of memory")

    def test_mesh_cancel_stops_the_remote_task(self):
        results = []
        for base in (self.h.py_url, self.h.rs_url):
            request(self.h.fake_url, "POST", "/__reset")
            self.load_on_fake(MESH)
            request(self.h.fake_url, "POST", "/__reset_log")
            request(self.h.fake_url, "POST", "/__flags", {"task_hold": True})
            job = request(base, "POST", "/api/jobs", {"kind": "mesh", "model": MESH, "image": PNG}).json
            job = self.wait_job(base, job, lambda j: j.get("stage"), "the 3D job's first answered poll")
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
            self.load_on_fake(MESH)
            request(self.h.fake_url, "POST", "/__reset_log")
            request(self.h.fake_url, "POST", "/__flags", {"task_hold": True, "ignore_cancel": True})
            job = request(base, "POST", "/api/jobs", {"kind": "mesh", "model": MESH, "image": PNG}).json
            job = self.wait_job(base, job, lambda j: j.get("stage"), "the 3D job's first answered poll")
            request(base, "POST", f"/api/jobs/{job['id']}/cancel")
            job, _ = self.finish(base, job)
            results.append((job["status"], job["message"]))
        self.assertEqual(results, [("cancelled", "Cancelled")] * 2)


class Video(Jobs):
    """Video (0.4.0): the gateway's /v1/videos with MiniMax-H3-Turbo's own sizes and steps (/v1/video/info)."""

    def video(self, **kw):
        return {"kind": "video", "model": VIDEO, "load": VIDEO, "prompt": "RUST TEST a red apple rolls", "size": "832x480", "seconds": 2, **kw}

    def test_video_from_text(self):
        py, rs = self.same_job(self.video(steps=4, seed=7, bogus=1))
        self.assertEqual(py["status"], "done", py.get("error"))
        self.assertEqual((py["entry"]["kind"], py["entry"]["duration"], py["entry"]["sources"]), ("video", 2.3, []))  # 56 frames at 24 fps, like the real device
        self.assertEqual(py["entry"]["params"], {"model": VIDEO, "prompt": "RUST TEST a red apple rolls", "size": "832x480", "seconds": 2, "steps": 4, "seed": 7})
        self.files_equal(py, rs)
        calls = self.calls()
        sent = next(c[3] for c in calls if c[:2] == ("POST", "/v1/videos"))
        self.assertEqual(sent, {"model": VIDEO, "prompt": "RUST TEST a red apple rolls", "size": "832x480", "seconds": "2", "steps": 4, "seed": 7})
        self.assertTrue(any(c[0] == "GET" and c[1].endswith("/content") for c in calls))
        self.assertEqual(calls[-1][0], "DELETE")  # the device's copy goes once the studio has it
        for base, job in ((self.h.py_url, py), (self.h.rs_url, rs)):
            got = request(base, "GET", "/images/" + job["entry"]["outputs"][0])
            self.assertEqual((got.status, got.headers["Content-Type"], got.body[4:8]), (200, "video/mp4", b"ftyp"))

    def test_video_without_steps_defaults_to_four(self):
        # the device's own default is 7, which turns a 2 s clip into ~12 minutes instead of ~7
        py, rs = self.same_job(self.video())
        self.assertEqual((py["status"], rs["status"]), ("done", "done"), py.get("error"))
        self.assertEqual(py["entry"]["params"]["steps"], 4)
        sent = next(c[3] for c in self.calls() if c[:2] == ("POST", "/v1/videos"))
        self.assertEqual(sent, {"model": VIDEO, "prompt": "RUST TEST a red apple rolls", "size": "832x480", "seconds": "2", "steps": 4})

    def test_video_from_frames(self):
        py, rs = self.same_job(self.video(prompt="", image=PNG, last_image=PNG))
        self.assertEqual(py["status"], "done", py.get("error"))
        self.assertEqual(len(py["entry"]["sources"]), 2)
        self.files_equal(py, rs, "sources")
        sent = next(c[3] for c in self.calls() if c[:2] == ("POST", "/v1/videos"))
        self.assertEqual(sent["prompt"], "Bring this picture to life")
        self.assertEqual([sent[k].split(",")[0] for k in ("image", "last_image")], ["data:image/png;base64"] * 2)

    def test_video_errors(self):
        self.failed(self.video(prompt=" "), "Describe the clip")
        self.failed({**self.video(), "load": None}, "isn't loaded")
        self.failed(self.video(size="1x1"), "exported for [1344x768")
        dirs = [d / "images" for d in self.h.tmp[-2:]]
        before = [len(list(d.iterdir())) for d in dirs]
        self.failed(self.video(size="1x1", image=PNG), "exported for")  # its saved first frame goes too
        self.assertEqual([len(list(d.iterdir())) for d in dirs], before)

    def test_video_rides_out_a_dropped_link(self):
        # a render takes many minutes: polls and the download that get no answer are retried, not the end of the job
        for flags in ({"video_flaky": 3}, {"content_flaky": 2}):
            py, rs = self.same_job(self.video(flags=flags))
            self.assertEqual((py["status"], rs["status"]), ("done", "done"), (flags, py.get("error")))
            self.files_equal(py, rs)

    def test_video_cancel(self):
        results = []
        for base in (self.h.py_url, self.h.rs_url):
            request(self.h.fake_url, "POST", "/__reset")
            self.load_on_fake(VIDEO)
            request(self.h.fake_url, "POST", "/__reset_log")
            request(self.h.fake_url, "POST", "/__flags", {"video_hold": True})
            job = request(base, "POST", "/api/jobs", {k: v for k, v in self.video().items() if k != "load"}).json
            self.wait_until(lambda: any(m == "GET" and p.startswith("/v1/videos/") for m, p, *_ in self.calls()), "the studio's first poll of the clip")
            self.assertEqual(request(base, "POST", f"/api/jobs/{job['id']}/cancel").status, 200)
            job, calls = self.finish(base, job)
            self.assertEqual(job["status"], "cancelled")
            results.append([c[:2] for i, c in enumerate(calls) if i == 0 or c != calls[i - 1]])
        self.assertEqual(results[0], results[1])
        self.assertIn("DELETE", [m for m, p in results[0]])  # deleting the clip stops the render

    def test_video_info_and_workspace(self):
        request(self.h.fake_url, "POST", "/__reset")
        self.load_on_fake(VIDEO)
        infos = [request(b, "GET", "/api/model/info?id=" + VIDEO.replace("/", "%2F")).json for b in (self.h.py_url, self.h.rs_url)]
        self.assertEqual(infos[0], infos[1])
        self.assertEqual(infos[0]["info"]["step_options"], [4, 5, 6, 7, 8])
        rows = [next(m for m in request(b, "GET", "/api/catalog").json["models"] if m["id"] == VIDEO) for b in (self.h.py_url, self.h.rs_url)]
        self.assertEqual([r["workspace"] for r in rows], ["video", "video"])

    def test_media_ranges(self):
        # a <video> seeks with byte ranges
        py, rs = self.same_job(self.video())
        for base, job in ((self.h.py_url, py), (self.h.rs_url, rs)):
            path = "/images/" + job["entry"]["outputs"][0]
            n = len(fake_tiiny.MP4)
            for rng, code, cr, body in [("bytes=4-11", 206, f"bytes 4-11/{n}", fake_tiiny.MP4[4:12]),
                                        ("bytes=-4", 206, f"bytes {n - 4}-{n - 1}/{n}", fake_tiiny.MP4[-4:]),
                                        ("bytes=10-", 206, f"bytes 10-{n - 1}/{n}", fake_tiiny.MP4[10:]),
                                        (f"bytes={n}-", 416, f"bytes */{n}", b"")]:
                got = request(base, "GET", path, headers={"Range": rng})
                self.assertEqual((got.status, got.headers.get("Content-Range"), got.body), (code, cr, body), (base, rng))
            self.assertEqual(request(base, "GET", path).headers.get("Accept-Ranges"), "bytes")


def strip(obj):
    if isinstance(obj, dict):
        return {k: (sorted(v.values()) if k == "tracks" else strip(v)) for k, v in obj.items() if k not in IGNORE}
    if isinstance(obj, list):
        return [strip(v) for v in obj]
    return obj


if __name__ == "__main__":
    unittest.main()
