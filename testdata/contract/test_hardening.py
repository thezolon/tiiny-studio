"""Hardening contract tests: a job keeps the device it started on, answers that aren't JSON, request bodies, files left
behind by failed jobs. Both servers run against the fake Tiiny and must answer alike."""
import base64
import contextlib
import json
import shutil
import time
import unittest

from harness import ContractCase, IMAGE as DOCKER, request  # first: puts testdata/ on the path
from test_stage5_audio import ASR, AUTH, Jobs, MESH, PNG, SONG, TTS, VIDEO, WAV  # noqa: E402
import fake_tiiny  # noqa: E402

IMAGE_MODEL = "Qwen/Qwen-Image-2.1"


class Hardening(Jobs):
    # ---------- the key only goes to the device it belongs to ----------
    def test_job_keeps_its_device_when_the_active_one_changes(self):
        # A video takes several requests (submit, polls, download, delete). The active device changes during the first;
        # everything after it must still go to the first device, with the first device's key (the other would answer 401).
        if DOCKER:
            other, _ = fake_tiiny.start(key="other-key", host="0.0.0.0")
            other_url = f"http://host.docker.internal:{other.server_address[1]}"
        else:
            other, other_url = fake_tiiny.start(key="other-key")
        self.addCleanup(lambda: (other.shutdown(), other.server_close()))
        results = []
        for base in (self.h.py_url, self.h.rs_url):
            saved = request(base, "POST", "/api/settings", {"new": True, "name": "RUST TEST other", "url": other_url, "key": "other-key"}).json
            first = saved["active"]
            second = next(d["id"] for d in saved["devices"] if d["url"] == other_url)
            self.addCleanup(request, base, "POST", "/api/settings/delete", {"id": second})
            request(base, "POST", "/api/settings/auto", {"auto": False})
            request(other_url, "POST", "/__reset")
            request(self.h.fake_url, "POST", "/__reset")
            request(self.h.fake_url, "POST", "/api/v1/models/" + VIDEO.replace("/", "%2F") + "/launch", {"model_id": VIDEO}, headers=AUTH)
            time.sleep(0.4)
            request(self.h.fake_url, "POST", "/__reset_log")
            request(self.h.fake_url, "POST", "/__flags", {"delay": {"/v1/videos": 1.5}})
            job = request(base, "POST", "/api/jobs", {"kind": "video", "model": VIDEO, "prompt": "RUST TEST duck", "seconds": 1}).json
            time.sleep(0.7)  # inside the slow submit
            request(base, "POST", "/api/settings/activate", {"id": second})
            job, calls = self.finish(base, job)
            request(base, "POST", "/api/settings/activate", {"id": first})
            request(base, "POST", "/api/settings/auto", {"auto": True})
            seen = json.loads(request(other_url, "GET", "/__requests").body)
            results.append((job["status"], [(m, p) for m, p, *_ in calls], [r["path"] for r in seen if r["path"] != "/"]))
        self.assertEqual(results[0], results[1])
        status, calls, other_paths = results[0]
        self.assertEqual(status, "done")
        self.assertEqual([m for m, p in calls if p.endswith("/content") or m in ("POST", "DELETE")][:1], ["POST"])
        self.assertTrue(any(p.endswith("/content") for _, p in calls) and ("DELETE", calls[-1][1]) == calls[-1])  # downloaded and cleaned up there
        self.assertEqual(other_paths, [])

    # ---------- a 200 that isn't JSON ----------
    def test_non_json_answer_is_an_error_not_a_crash(self):
        request(self.h.fake_url, "POST", "/__reset")
        request(self.h.fake_url, "POST", "/__flags", {"html": ["/v1/models"]})
        py, rs = self.get("/api/models")
        self.assertEqual((py.status, rs.status), (502, 502))
        self.assertEqual(py.json, rs.json)
        self.assertIn("Please log in", py.json["error"]["message"])
        request(self.h.fake_url, "POST", "/__reset")
        # and inside a job: Media.http / media::http
        self.failed({"kind": "transcribe", "model": ASR, "audio": WAV, "flags": {"html": ["/v1/audio/transcriptions"]}}, "Please log in")

    # ---------- request bodies ----------
    JSON = {"Content-Type": "application/json"}
    BODY_ROUTES = ["/api/history/delete", "/api/history/abc123/fav", "/api/export", "/api/search", "/api/jobs", "/api/voices", "/api/chats/abc123",
                   "/api/settings"]

    def test_bad_json_and_non_object_bodies_are_a_400(self):
        for path in self.BODY_ROUTES:
            for body, why in [(b"not json", "isn't valid JSON"), (b"[1, 2]", "must be a JSON object"), (bytes([0xFF, 0xFE]), "isn't valid JSON"), (b"null", "must be a JSON object")]:
                py, rs = self.post_raw(path, body, self.JSON)
                self.same((py, rs))
                self.assertEqual(py.status, 400, (path, body))
                self.assertIn("bad chat" if path.startswith("/api/chats/") else why, py.json["error"]["message"], (path, body))  # that route has its own words
        self.assertEqual(self.get("/api/history")[0].json, self.get("/api/history")[1].json)  # nothing was touched

    def test_oversized_request_is_refused_before_it_is_read(self):
        for path in ("/api/history/delete", "/api/jobs", "/api/export"):
            py, rs = self.post_raw(path, b"{}", {**self.JSON, "Content-Length": str(151 << 20)})
            self.same((py, rs))
            self.assertEqual(py.status, 413)
            self.assertIn("too large", py.json["error"]["message"])

    # ---------- writes that fail ----------
    @contextlib.contextmanager
    def blocked(self, name):
        """A data folder (images, chats) replaced by a plain file on both servers: nothing can be written inside it."""
        dirs = [d / name for d in self.h.tmp[-2:]]
        for d in dirs:
            shutil.rmtree(d)
            d.write_bytes(b"")
        try:
            yield
        finally:
            for d in dirs:
                d.unlink()
                d.mkdir()

    def test_a_result_that_cannot_be_saved_fails_the_job(self):
        with self.blocked("images"):
            for base in (self.h.py_url, self.h.rs_url):  # the reason is the operating system's own words: only the start is compared
                job, _ = self.run_job(base, {"kind": "image", "model": IMAGE_MODEL, "prompt": "RUST TEST duck", "size": "512x512"})
                self.assertEqual(job["status"], "error")
                self.assertTrue(job["error"].startswith("Couldn't save the result: ") and len(job["error"]) > 30, job["error"])
            self.assertEqual(self.get("/api/history")[0].json, [])
            self.assertEqual(self.get("/api/history")[1].json, [])

    def test_a_voice_or_chat_that_cannot_be_saved_is_a_500(self):
        voice = {"audio": WAV, "name": "RUST TEST voice", "text": "hi"}
        with self.blocked("images"):
            py, rs = self.post("/api/voices", voice)
        for r in (py, rs):
            self.assertEqual(r.status, 500)
            self.assertTrue(r.json["error"]["message"].startswith("Couldn't save your changes: "), r.body)
        with self.blocked("chats"):
            py, rs = self.post("/api/chats/abc123", {"title": "RUST TEST", "messages": []})
        for r in (py, rs):
            self.assertEqual(r.status, 500)
            self.assertTrue(r.json["error"]["message"].startswith("Couldn't save your changes: "), r.body)
        self.assertEqual([r.json for r in self.get("/api/voices")], [[], []])  # no half-saved voice

    # ---------- files left behind by failed jobs ----------
    def files(self):
        return [sorted(p.name for p in (d / "images").iterdir()) for d in self.h.tmp[-2:]]

    def test_a_failed_song_keeps_its_style_reference_no_longer(self):
        before = self.files()
        py = self.failed({"kind": "song", "model": SONG, "caption": "RUST TEST pop", "lyrics": "[verse]\nla la", "reference_audio": WAV, "load": SONG,
                          "flags": {"fail": {"/v1/generate": [500, "boom"]}}}, "boom")
        self.assertEqual(py["status"], "error")
        self.assertEqual(self.files(), before)

    def test_a_video_that_fails_after_the_download_keeps_no_files(self):
        # the clip and its first frame are saved, then the gallery can't take the entry (history.tmp is in the way)
        before = self.files()
        tmps = [d / "history.tmp" for d in self.h.tmp[-2:]]
        for d in tmps:
            d.mkdir()
        try:
            for base in (self.h.py_url, self.h.rs_url):
                job, calls = self.run_job(base, {"kind": "video", "model": VIDEO, "load": VIDEO, "prompt": "RUST TEST apple", "image": PNG, "seconds": 1})
                self.assertEqual(job["status"], "error")
                self.assertTrue(job["error"].startswith("Couldn't save the result: "), job["error"])
                self.assertTrue(any(p.endswith("/content") for _, p, *_ in calls))  # it did download the clip
        finally:
            for d in tmps:
                d.rmdir()
        self.assertEqual(self.files(), before)

    def test_settings_that_cannot_be_saved_are_a_500(self):
        tmps = [d / "config.tmp" for d in self.h.tmp[-2:]]
        for d in tmps:
            d.mkdir()  # the file the settings are written through can't be created
        try:
            py, rs = self.post("/api/settings/auto", {"auto": False})
        finally:
            for d in tmps:
                d.rmdir()
        for r in (py, rs):
            self.assertEqual(r.status, 500)
            self.assertTrue(r.json["error"]["message"].startswith("Couldn't save your changes: "), r.body)
        for r in self.post("/api/settings/auto", {"auto": True}):  # and back to normal once it can be
            self.assertEqual((r.status, r.json["auto"]), (200, True))


if __name__ == "__main__":
    unittest.main()
