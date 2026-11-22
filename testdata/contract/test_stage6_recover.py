"""Stage 6 contract tests: results left on the Tiiny (a finished clip, songs and loops) are pulled into the gallery,
and deleting a song or loop deletes the Tiiny's copy. The server lives through one story against the fake Tiiny and
must answer as the test says and make the device requests it lists."""
import hashlib
import json
import unittest
from pathlib import Path

from harness import request, strip  # first: puts testdata/ on the path
from test_stage5_audio import Jobs, LOOP, SONG
import fake_tiiny  # noqa: E402

IGNORE = ("id", "device_id", "created", "outputs", "sources", "downloads")


class RecoverBase(Jobs):
    maxDiff = None

    def post_fake(self, path, body=None):
        return request(self.h.fake_url, "POST", path, body)

    def recover(self):
        """POST /api/device/recover; returns (answer, requests it made to the fake)."""
        self.post_fake("/__reset_log")
        got = request(self.h.rs_url, "POST", "/api/device/recover", {})
        self.assertEqual(got.status, 200)
        return got.json, self.calls()

    def history(self):
        return request(self.h.rs_url, "GET", "/api/history").json

    # ---------- the story ----------
    def story(self):
        """Everything the server does with the fake, as observations."""
        base = self.h.rs_url
        seen = {}
        self.post_fake("/__reset")
        # first look: what is on the device now is history, never imported
        self.post_fake("/__seed", {"videos": [{"id": "vid-before"}], "sessions": [{"session_id": "s-before", "age": 100}, {"session_id": "s-ancient", "age": 3 * 86400}]})
        seen["baseline"] = self.recover()
        seen["history0"] = self.history()
        # the studio comes back: a finished clip, a new song and loop are waiting, and some that must be left alone
        self.post_fake("/__seed", {"videos": [{"id": "vid-new", "seconds": 3}, {"id": "vid-busy", "status": "in_progress"}],
                              "sessions": [{"session_id": "s-song", "model_id": SONG, "age": 60}, {"session_id": "s-loop", "model_id": LOOP, "route": "generate_wav", "age": 30},
                                           {"session_id": "s-old", "age": 2 * 86400}, {"session_id": "s-failed", "status": "failed", "age": 10}]})
        answer, calls = self.recover()
        seen["pulled"] = (strip(answer, ("id",)), calls)
        entries = self.history()
        seen["history"] = [strip(e, IGNORE) for e in entries]
        seen["files"] = [(e["kind"], len(e["outputs"]), hashlib.sha1(request(base, "GET", "/images/" + e["outputs"][0]).body).hexdigest()) for e in entries]
        seen["again"] = self.recover()
        # deleting the song and the loop deletes the Tiiny's copy; the clip had none (it was deleted when saved)
        song = next(e for e in entries if e["kind"] == "song")
        loop = next(e for e in entries if e["kind"] == "loop")
        self.assertEqual((song["device"]["music_session"], loop["device"]["music_session"]), ("s-song", "s-loop"))
        self.assertTrue(song["device"]["device_id"] and loop["device"]["device_id"])
        self.post_fake("/__reset_log")
        request(base, "DELETE", "/api/history/" + song["id"])
        self.wait_sent("DELETE", "/v1/music/sessions/s-song")
        request(base, "POST", "/api/history/delete", {"ids": [loop["id"]]})
        self.wait_sent("DELETE", "/v1/music/sessions/s-loop")
        seen["deleted"] = [c for c in self.calls() if c[0] == "DELETE"]
        seen["after"] = self.recover()
        return seen


class RecoverStory(RecoverBase):
    """Its own server: the first call on a fresh data folder is the baseline."""

    def test_recover_story(self):
        py = self.story()
        self.assertEqual(py["baseline"][0], {"recovered": [], "baseline": True})
        self.assertEqual(py["history0"], [])
        self.assertEqual([c[:2] for c in py["baseline"][1]], [("GET", "/v1/videos"), ("GET", "/v1/music/sessions")])  # nothing fetched or deleted
        self.assertEqual([r["kind"] for r in py["pulled"][0]["recovered"]], ["video", "song", "loop"])
        calls = [c[:2] for c in py["pulled"][1]]
        self.assertIn(("DELETE", "/v1/videos/vid-new"), calls)  # the clip is the studio's now
        self.assertFalse(any(c[0] == "DELETE" and "/music/" in c[1] for c in calls))  # the sessions stay until their entries go
        self.assertFalse(any("s-old" in c[1] or "s-failed" in c[1] or "vid-busy" in c[1] or "-before" in c[1] for c in calls if c[0] != "GET" or "/download" in c[1] or "/content" in c[1]))
        self.assertEqual(calls, [("GET", "/v1/videos"), ("GET", "/v1/music/sessions"), ("GET", "/v1/videos/vid-new/content"), ("DELETE", "/v1/videos/vid-new"),
                                 ("GET", "/v1/music/sessions/s-song/download"), ("GET", "/v1/music/sessions/s-loop/download")])
        self.assertEqual(py["pulled"][0]["baseline"], False)
        self.assertEqual(py["history"], [{"kind": "loop", "seconds": 0, "params": {"model": "Fake/Foundation-1", "recovered": True}, "duration": None,
                                          "device": {"music_session": "s-loop"}},
                                         {"kind": "song", "seconds": 0, "params": {"model": "Fake/SongGeneration-v2", "recovered": True}, "duration": None,
                                          "device": {"music_session": "s-song"}},
                                         {"kind": "video", "seconds": 0, "params": {"model": fake_tiiny.VIDEO, "prompt": "", "size": "832x480", "seconds": 3, "recovered": True},
                                          "duration": 3.3}])
        self.assertEqual([(k, n) for k, n, _ in py["files"]], [("loop", 1), ("song", 1), ("video", 1)])  # each entry has its file
        self.assertEqual(py["files"][0][2], py["files"][1][2])  # the loop and the song are the same bytes: what the fake hands out
        self.assertNotEqual(py["files"][2][2], py["files"][0][2])
        video = next(e for e in py["history"] if e["kind"] == "video")
        self.assertEqual(video["params"], {"model": fake_tiiny.VIDEO, "prompt": "", "size": "832x480", "seconds": 3, "recovered": True})
        self.assertEqual(py["again"][0], {"recovered": [], "baseline": False})
        self.assertEqual([c[:2] for c in py["again"][1]], [("GET", "/v1/videos"), ("GET", "/v1/music/sessions")])
        self.assertEqual([c[:2] for c in py["deleted"]], [("DELETE", "/v1/music/sessions/s-song"), ("DELETE", "/v1/music/sessions/s-loop")])
        self.assertEqual(py["after"][0], {"recovered": [], "baseline": False})


class RecoverJobs(RecoverBase):
    def test_jobs_keep_their_sessions_and_deleting_deletes_them(self):
        """A song or loop made here is not pulled again, and its entry knows the Tiiny's copy."""
        base = self.h.rs_url
        self.post_fake("/__reset")
        self.recover()  # the baseline exists from here on
        song = {"kind": "song", "model": SONG, "caption": "RUST TEST calm", "generation_type": "bgm", "seconds_total": 45, "load": SONG, "flags": {"closed_port": True}}
        song, _ = self.run_job(song)
        self.assertEqual(song["status"], "done", song.get("error"))
        loop, _ = self.run_job({"kind": "loop", "model": LOOP, "prompt": "RUST TEST piano", "load": LOOP})
        self.assertEqual(loop["status"], "done", loop.get("error"))
        sessions = [j["entry"]["device"]["music_session"] for j in (song, loop)]
        self.assertTrue(all(s.startswith("sess-") for s in sessions), sessions)
        self.assertLessEqual(set(sessions), set(json.loads((Path(self.h.data) / "device_seen.json").read_text("utf-8"))))
        self.post_fake("/__reset_log")
        request(base, "DELETE", "/api/history/" + song["entry"]["id"])
        self.wait_sent("DELETE", "/v1/music/sessions/" + sessions[0])
        request(base, "POST", "/api/history/delete", {"ids": [loop["entry"]["id"]]})
        self.wait_sent("DELETE", "/v1/music/sessions/" + sessions[1])

    def test_unreadable_device_recovers_nothing(self):
        self.post_fake("/__reset")
        self.post_fake("/__flags", {"fail": {"/v1/videos": [500, "boom"], "/v1/music/sessions": [500, "boom"]}})
        self.assertEqual(self.recover()[0], {"recovered": [], "baseline": False})


if __name__ == "__main__":
    unittest.main()
