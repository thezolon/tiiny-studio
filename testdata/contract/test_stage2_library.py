"""Stage 2 contract tests: jobs table, history, voices, saved chats, export, wipe, smart search.
Labelled test data only; each server has its own temporary data folder."""
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
            self.same(self.get(path))

    def test_jobs_errors(self):
        self.same(self.post("/api/jobs", {"kind": "no-such-kind"}))
        self.same(self.post("/api/jobs/abc123/cancel"))
        self.same(self.get("/api/jobs/abc123"))

    def test_history_edits_on_missing_entries(self):
        self.same(self.post("/api/history/nope/fav", {"fav": True}))
        self.same(self.post("/api/history/delete", {"ids": ["a", "b", "a"]}))
        py = request(self.h.py_url, "DELETE", "/api/history/nope")
        rs = request(self.h.rs_url, "DELETE", "/api/history/nope")
        self.same((py, rs))

    def test_voices(self):
        py, rs = self.post("/api/voices", {"name": "RUST TEST voice", "text": "hello", "audio": WAV})
        self.same((py, rs), IGNORE)
        self.same(self.post("/api/voices", {"name": "x", "audio": "not a data uri"}))
        for r, base in ((py, self.h.py_url), (rs, self.h.rs_url)):
            img = request(base, "GET", "/images/" + r.json["file"])
            self.assertEqual((img.status, img.headers.get("Content-Type")), (200, "audio/wav"))
            self.assertEqual(img.body, b"RIFF....WAVEfmt TEST")
        self.same(self.get("/api/voices"), IGNORE)
        dp = request(self.h.py_url, "DELETE", f"/api/voices/{py.json['id']}")
        dr = request(self.h.rs_url, "DELETE", f"/api/voices/{rs.json['id']}")
        self.same((dp, dr))
        self.same(self.get("/api/voices"))

    def test_chats(self):
        convo = {"title": "RUST TEST chat", "messages": [{"role": "user", "content": "hi"}]}
        self.same(self.post("/api/chats/rusttest1", convo))
        self.same(self.get("/api/chats"), ("updated",))
        self.same(self.get("/api/chats/rusttest1"), ("updated",))
        self.same(self.get("/api/chats/missing"))
        self.same(self.post("/api/chats/rusttest1/delete"))
        self.same(self.get("/api/chats"))

    def test_export_empty(self):
        py, rs = self.post("/api/export", {"ids": []})
        self.assertEqual((py.status, rs.status), (200, 200))
        for r in (py, rs):
            self.assertEqual(r.headers.get("Content-Type"), "application/zip")
            z = zipfile.ZipFile(io.BytesIO(r.body))
            self.assertEqual(z.namelist(), ["info.json"])
            self.assertEqual(json.loads(z.read("info.json")), [])

    def test_search_needs_embedding_model(self):
        self.same(self.post("/api/search", {"query": "duck"}))

    def test_wipe(self):
        self.post("/api/voices", {"name": "RUST TEST wipe", "audio": WAV})
        self.same(self.post("/api/privacy/wipe"))
        self.same(self.get("/api/voices"))


if __name__ == "__main__":
    unittest.main()
