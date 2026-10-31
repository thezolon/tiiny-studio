"""Hardening contract tests: a job keeps the device it started on, answers that aren't JSON, request bodies, files left
behind by failed jobs. Both servers run against the fake Tiiny and must answer alike."""
import base64
import json
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


if __name__ == "__main__":
    unittest.main()
