"""Stage 6 contract tests: results left on the Tiiny (a finished clip, songs and loops) are pulled into the gallery,
and deleting a song or loop deletes the Tiiny's copy. Both servers run the same story against the fake Tiiny and must
answer the same and make the same device requests."""
import hashlib
import json
import time
import unittest
from pathlib import Path

from harness import request, strip  # first: puts testdata/ on the path
from test_stage5_audio import Jobs, LOOP, SONG
import fake_tiiny  # noqa: E402

IGNORE = ("id", "created", "outputs", "sources", "downloads")


class RecoverBase(Jobs):
    maxDiff = None

    def post_fake(self, path, body=None):
        return request(self.h.fake_url, "POST", path, body)

    def recover(self, base):
        """POST /api/device/recover; returns (answer, requests it made to the fake)."""
        self.post_fake("/__reset_log")
        got = request(base, "POST", "/api/device/recover", {})
        self.assertEqual(got.status, 200)
        return got.json, self.calls()

    def history(self, base):
        return request(base, "GET", "/api/history").json

    def wait_for(self, method, path):
        for _ in range(100):  # the Tiiny's copy goes in the background
            if any(c[:2] == (method, path) for c in self.calls()):
                return
            time.sleep(0.05)
        self.fail(f"the studio never sent {method} {path}")

    # ---------- the story ----------
    def story(self, base):
        """Everything one server does with the fake, as comparable observations."""
        seen = {}
        self.post_fake("/__reset")
        # first look: what is on the device now is history, never imported
        self.post_fake("/__seed", {"videos": [{"id": "vid-before"}], "sessions": [{"session_id": "s-before", "age": 100}, {"session_id": "s-ancient", "age": 3 * 86400}]})
        seen["baseline"] = self.recover(base)
        seen["history0"] = self.history(base)
        # the studio comes back: a finished clip, a new song and loop are waiting, and some that must be left alone
        self.post_fake("/__seed", {"videos": [{"id": "vid-new", "seconds": 3}, {"id": "vid-busy", "status": "in_progress"}],
                              "sessions": [{"session_id": "s-song", "model_id": SONG, "age": 60}, {"session_id": "s-loop", "model_id": LOOP, "route": "generate_wav", "age": 30},
                                           {"session_id": "s-old", "age": 2 * 86400}, {"session_id": "s-failed", "status": "failed", "age": 10}]})
        answer, calls = self.recover(base)
        seen["pulled"] = (strip(answer, ("id",)), calls)
        entries = self.history(base)
        seen["history"] = [strip(e, IGNORE) for e in entries]
        seen["files"] = [(e["kind"], len(e["outputs"]), hashlib.sha1(request(base, "GET", "/images/" + e["outputs"][0]).body).hexdigest()) for e in entries]
        seen["again"] = self.recover(base)
        # deleting the song and the loop deletes the Tiiny's copy; the clip had none (it was deleted when saved)
        song = next(e for e in entries if e["kind"] == "song")
        loop = next(e for e in entries if e["kind"] == "loop")
        self.assertEqual((song["device"], loop["device"]), ({"music_session": "s-song"}, {"music_session": "s-loop"}))
        self.post_fake("/__reset_log")
        request(base, "DELETE", "/api/history/" + song["id"])
        self.wait_for("DELETE", "/v1/music/sessions/s-song")
        request(base, "POST", "/api/history/delete", {"ids": [loop["id"]]})
        self.wait_for("DELETE", "/v1/music/sessions/s-loop")
        seen["deleted"] = [c for c in self.calls() if c[0] == "DELETE"]
        seen["after"] = self.recover(base)
        return seen


class RecoverStory(RecoverBase):
    """Its own servers: the first call on a fresh data folder is the baseline."""

    def test_recover_story(self):
        py, rs = self.story(self.h.py_url), self.story(self.h.rs_url)
        self.assertEqual(py, rs)
        self.assertEqual(py["baseline"][0], {"recovered": [], "baseline": True})
        self.assertEqual(py["history0"], [])
        self.assertEqual([c[:2] for c in py["baseline"][1]], [("GET", "/v1/videos"), ("GET", "/v1/music/sessions")])  # nothing fetched or deleted
        self.assertEqual([r["kind"] for r in py["pulled"][0]["recovered"]], ["video", "song", "loop"])
        calls = [c[:2] for c in py["pulled"][1]]
        self.assertIn(("DELETE", "/v1/videos/vid-new"), calls)  # the clip is the studio's now
        self.assertFalse(any(c[0] == "DELETE" and "/music/" in c[1] for c in calls))  # the sessions stay until their entries go
        self.assertFalse(any("s-old" in c[1] or "s-failed" in c[1] or "vid-busy" in c[1] or "-before" in c[1] for c in calls if c[0] != "GET" or "/download" in c[1] or "/content" in c[1]))
        video = next(e for e in py["history"] if e["kind"] == "video")
        self.assertEqual(video["params"], {"model": fake_tiiny.VIDEO, "prompt": "", "size": "832x480", "seconds": 3, "recovered": True})
        self.assertEqual(py["again"][0], {"recovered": [], "baseline": False})
        self.assertEqual([c[:2] for c in py["again"][1]], [("GET", "/v1/videos"), ("GET", "/v1/music/sessions")])
        self.assertEqual([c[:2] for c in py["deleted"]], [("DELETE", "/v1/music/sessions/s-song"), ("DELETE", "/v1/music/sessions/s-loop")])
        self.assertEqual(py["after"][0], {"recovered": [], "baseline": False})


class RecoverJobs(RecoverBase):
    def test_jobs_keep_their_sessions_and_deleting_deletes_them(self):
        """A song or loop made here is not pulled again, and its entry knows the Tiiny's copy."""
        for base in (self.h.py_url, self.h.rs_url):
            self.post_fake("/__reset")
            self.recover(base)  # the baseline exists from here on
        song = {"kind": "song", "model": SONG, "caption": "RUST TEST calm", "generation_type": "bgm", "seconds_total": 45, "load": SONG, "flags": {"closed_port": True}}
        py_song, rs_song = self.same_job(song)
        self.assertEqual(py_song["status"], "done", py_song.get("error"))
        py_loop, rs_loop = self.same_job({"kind": "loop", "model": LOOP, "prompt": "RUST TEST piano", "load": LOOP})
        self.assertEqual(py_loop["status"], "done", py_loop.get("error"))
        for base, tmp, song, loop in ((self.h.py_url, self.h.tmp[0], py_song, py_loop), (self.h.rs_url, self.h.tmp[1], rs_song, rs_loop)):
            sessions = [j["entry"]["device"]["music_session"] for j in (song, loop)]
            self.assertTrue(all(s.startswith("sess-") for s in sessions), sessions)
            self.assertLessEqual(set(sessions), set(json.loads((Path(tmp) / "device_seen.json").read_text("utf-8"))))
            self.post_fake("/__reset_log")
            request(base, "DELETE", "/api/history/" + song["entry"]["id"])
            self.wait_for("DELETE", "/v1/music/sessions/" + sessions[0])
            request(base, "POST", "/api/history/delete", {"ids": [loop["entry"]["id"]]})
            self.wait_for("DELETE", "/v1/music/sessions/" + sessions[1])

    def test_unreadable_device_recovers_nothing(self):
        for base in (self.h.py_url, self.h.rs_url):
            self.post_fake("/__reset")
            self.post_fake("/__flags", {"fail": {"/v1/videos": [500, "boom"], "/v1/music/sessions": [500, "boom"]}})
            self.assertEqual(self.recover(base)[0], {"recovered": [], "baseline": False})


if __name__ == "__main__":
    unittest.main()
