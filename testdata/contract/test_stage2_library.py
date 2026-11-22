"""Stage 2 contract tests: jobs table, history, voices, saved chats, export, wipe, smart search.
Labelled test data only; the server has its own temporary data folder."""
import base64
import io
import json
import unittest
import zipfile

from harness import ContractCase, request

IGNORE = ("id", "created", "file", "updated")
WAV = "data:audio/wav;base64," + base64.b64encode(b"RIFF....WAVEfmt TEST").decode()


class Library(ContractCase):
    def test_empty_library(self):
        for path in ("/api/history", "/api/voices", "/api/chats", "/api/jobs"):
            self.expect(self.get(path), 200, [])

    def test_jobs_errors(self):
        self.expect(self.post("/api/jobs", {"kind": "no-such-kind"}), 400, {"error": {"message": "unknown job kind no-such-kind"}})
        self.expect(self.post("/api/jobs/abc123/cancel"), 404, {"error": {"message": "no such job"}})
        self.expect(self.get("/api/jobs/abc123"), 404, {"error": {"message": "no such job"}})

    def test_history_edits_on_missing_entries(self):
        self.expect(self.post("/api/history/nope/fav", {"fav": True}), 200, {"ok": True, "fav": True})
        self.expect(self.post("/api/history/delete", {"ids": ["a", "b", "a"]}), 200, {"ok": True, "deleted": 2})
        self.expect(self.delete("/api/history/nope"), 200, {"ok": True})

    def test_voices(self):
        r = self.post("/api/voices", {"name": "RUST TEST voice", "text": "hello", "audio": WAV})
        self.expect(r, 200, {"name": "RUST TEST voice", "text": "hello", "file": f"voice-{r.json['id']}.wav", "created": r.json["created"]}, IGNORE)
        self.assertIsInstance(r.json["created"], int)
        self.expect(self.post("/api/voices", {"name": "x", "audio": "not a data uri"}), 400, {"error": {"message": "Expected a base64 data URI"}})
        img = self.get("/images/" + r.json["file"])
        self.assertEqual((img.status, img.headers.get("Content-Type")), (200, "audio/wav"))
        self.assertEqual(img.body, b"RIFF....WAVEfmt TEST")
        self.assertEqual(self.get("/api/voices").json, [r.json])
        self.expect(self.delete(f"/api/voices/{r.json['id']}"), 200, {"ok": True})
        self.expect(self.get("/api/voices"), 200, [])

    def test_chats(self):
        convo = {"title": "RUST TEST chat", "messages": [{"role": "user", "content": "hi"}]}
        self.expect(self.post("/api/chats/rusttest1", convo), 200, {"ok": True})
        r = self.get("/api/chats")
        self.expect(r, 200, [{"id": "rusttest1", "title": "RUST TEST chat", "count": 1}], ("updated",))
        self.assertIsInstance(r.json[0]["updated"], int)
        self.expect(self.get("/api/chats/rusttest1"), 200, {"id": "rusttest1", **convo}, ("updated",))
        self.expect(self.get("/api/chats/missing"), 404, {"error": {"message": "no such chat"}})
        self.expect(self.post("/api/chats/rusttest1/delete"), 200, {"ok": True})
        self.expect(self.get("/api/chats"), 200, [])

    def test_export_empty(self):
        r = self.post("/api/export", {"ids": []})
        self.assertEqual(r.status, 200)
        self.assertEqual(r.headers.get("Content-Type"), "application/zip")
        z = zipfile.ZipFile(io.BytesIO(r.body))
        self.assertEqual(z.namelist(), ["info.json"])
        self.assertEqual(json.loads(z.read("info.json")), [])

    def test_search_needs_embedding_model(self):
        self.expect(self.post("/api/search", {"query": "duck"}), 409,
                    {"error": {"message": "Smart search needs the embedding model loaded."}, "need_model": None})

    def test_wipe(self):
        self.post("/api/voices", {"name": "RUST TEST wipe", "audio": WAV})
        self.expect(self.post("/api/privacy/wipe"), 200, {"ok": True, "entries": 0, "voices": 1, "files": 1})
        self.expect(self.get("/api/voices"), 200, [])


if __name__ == "__main__":
    unittest.main()
