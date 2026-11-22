"""Stage 0 contract tests: the shell of the Rust server matches the Python one, and the fake Tiiny behaves."""
import json
import socket
import time
import unittest

import harness
from harness import AUTH, ContractCase, request


class Scaffold(ContractCase):
    def test_index_is_html(self):
        for r in self.get("/"):
            self.assertEqual(r.status, 200)
            self.assertIn("text/html", r.headers.get("Content-Type", ""))
            self.assertIn(b"<html", r.body.lower())

    def test_static_bytes_equal(self):
        py, rs = self.get("/static/shared.js")
        self.assertEqual((py.status, rs.status), (200, 200))
        self.assertEqual(py.body, rs.body)

    def test_docs(self):
        py, rs = self.get("/docs/CONNECTING.md")
        self.assertEqual((py.status, rs.status), (200, 200))
        self.assertEqual(py.body, rs.body)
        self.assertEqual(rs.headers.get("Content-Type"), "text/plain; charset=utf-8")
        self.same(self.get("/docs/../server.py"))

    def test_extract(self):
        for name, body in (("RUST%20TEST%20notes.md", b"# Title\n\n- one\n- two\n"), ("old.doc", b"\xd0\xcf\x11\xe0junk"),
                           ("t%C3%A9st.csv", b"a,b\n1,2\n")):
            self.same(self.post_raw("/api/extract", body, {"X-Filename": name}))

    def test_big_json_body(self):
        # base64 pictures make large JSON bodies; both servers must accept them (3 MB here)
        big = "data:image/png;base64," + "A" * (3 << 20)
        self.same(self.post("/api/voices", {"name": "RUST TEST big", "audio": big}), ("id", "created", "file"))

    def test_unknown_path_is_json_404(self):
        py, rs = self.get("/api/no-such-route")
        self.assertEqual(py.status, 404)
        self.same((py, rs))

    def test_settings_keys(self):
        # The Python server is configured from TIINY_URL (it shows a device); the Rust stub shows none until
        # Stage 1 brings saved devices. So compare only what Stage 0 supports: the key set and "version".
        py, rs = self.get("/api/settings")
        self.assertEqual((py.status, rs.status), (200, 200))
        self.assertEqual(set(py.json), set(rs.json) - {"mcp"})  # the MCP server is Rust only
        self.assertEqual(py.json["version"], rs.json["version"])


class OtherWebsites(ContractCase):
    """Both servers refuse other websites: cross-site changes (CSRF) and foreign Host names (DNS rebinding)."""

    def test_cross_site_post_refused(self):
        for origin in ("https://evil.example", "null"):
            pair = self.post_raw("/api/privacy/wipe", b"{}", {"Origin": origin, "Content-Type": "text/plain"})
            self.assertEqual([r.status for r in pair], [403, 403], origin)
            self.same(pair)

    def test_foreign_host_refused(self):
        pair = self.get("/api/history", headers={"Host": "evil.example"})
        self.assertEqual([r.status for r in pair], [403, 403])
        self.same(pair)

    def test_own_page_allowed(self):
        for base in (self.h.py_url, self.h.rs_url):
            own = base.split("//", 1)[1]
            r = request(base, "POST", "/api/voices", {"name": "RUST TEST own page", "audio": ""}, headers={"Origin": "http://" + own})
            self.assertNotEqual(r.status, 403, base)


class FakeTiiny(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.srv, cls.url = harness.fake_tiiny.start()

    @classmethod
    def tearDownClass(cls):
        cls.srv.shutdown()
        cls.srv.server_close()

    def log(self):
        return json.loads(request(self.url, "GET", "/__requests").body)

    def test_records_request_and_requires_key(self):
        request(self.url, "POST", "/__reset")
        self.assertEqual(request(self.url, "GET", "/").status, 200)  # status page needs no key
        r = request(self.url, "GET", "/v1/models")
        self.assertEqual(r.status, 401)
        self.assertIn("message", r.json["error"])
        r = request(self.url, "POST", "/api/v1/models/unload_all", {"x": 1}, headers=AUTH)
        self.assertEqual(r.status, 200)
        seen = self.log()
        self.assertEqual([(q["method"], q["path"]) for q in seen],
                         [("GET", "/"), ("GET", "/v1/models"), ("POST", "/api/v1/models/unload_all")])
        self.assertEqual(seen[-1]["body"], {"x": 1})
        self.assertNotIn("Authorization", json.dumps(seen))
        request(self.url, "POST", "/__reset")
        self.assertEqual(self.log(), [])

    def test_launch_makes_model_running(self):
        r = request(self.url, "POST", "/api/v1/models/Qwen%2FQwen-Image-2.1/launch", {"model_id": "x"}, headers=AUTH)
        self.assertEqual(r.status, 200)
        end = time.monotonic() + 10  # the launch takes the fake's launch_delay: poll, don't guess a pause
        while True:
            run = request(self.url, "GET", "/api/v1/models/running", headers=AUTH).json
            if "Qwen/Qwen-Image-2.1" in [i["model_id"] for i in run["instances"]["running"]] or time.monotonic() > end:
                break
            time.sleep(0.05)
        self.assertIn("Qwen/Qwen-Image-2.1", [i["model_id"] for i in run["instances"]["running"]])

    def load(self, *ids):
        """Load models on the fake and wait until they run: it answers 404 for a model that isn't loaded, like the device."""
        for mid in ids:
            request(self.url, "POST", "/api/v1/models/" + mid.replace("/", "%2F") + "/launch", {"model_id": mid}, headers=AUTH)
            end = time.monotonic() + 10
            while mid not in self.srv.state.running and time.monotonic() < end:
                time.sleep(0.05)
            self.assertIn(mid, self.srv.state.running)

    def raw(self, method, path, body=b"", key=True):
        """One request over a bare socket; returns everything the server wrote back before it closed."""
        host, port = self.url.removeprefix("http://").split(":")
        head = f"{method} {path} HTTP/1.1\r\nHost: x\r\nContent-Length: {len(body)}\r\nContent-Type: application/json\r\nConnection: close\r\n"
        if key:
            head += "Authorization: Bearer fake-key\r\n"
        with socket.create_connection((host, int(port)), timeout=10) as s:
            s.sendall(head.encode() + b"\r\n" + body)
            out = b""
            while chunk := s.recv(65536):
                out += chunk
        return out

    def test_one_answer_per_request(self):
        # an image_warmup 500 and the video model's icon 404 once got a second "404 endpoint not found" behind them
        request(self.url, "POST", "/__reset")
        self.srv.state.unrouted.clear()
        request(self.url, "POST", "/__flags", {"image_warmup": 1})
        self.load("Qwen/Qwen-Image-2.1")
        out = self.raw("POST", "/v1/images/generations", b'{"model": "Qwen/Qwen-Image-2.1", "prompt": "y"}')
        self.assertEqual(out.count(b"HTTP/1."), 1, out[:300])
        self.assertIn(b" 500 ", out.split(b"\r\n", 1)[0])
        out = self.raw("GET", "/api/v1/models/online_models/MiniMax-H3-Turbo/icon", key=False)
        self.assertEqual(out.count(b"HTTP/1."), 1, out[:300])
        self.assertEqual(self.srv.state.unrouted, [])

    def test_device_rules_the_docs_state(self):
        request(self.url, "POST", "/__reset")
        self.load("Qwen/Qwen-Image-2.1", "Qwen/Qwen3-TTS-Base")
        body = {"model": "Qwen/Qwen-Image-2.1", "prompt": "x", "size": "512x512"}
        self.assertEqual(request(self.url, "POST", "/v1/images/generations", {**body, "n": 1}, headers=AUTH).status, 200)
        self.assertEqual(request(self.url, "POST", "/v1/images/generations", {**body, "n": 2}, headers=AUTH).status, 500)  # one image per request
        self.assertEqual(request(self.url, "POST", "/v1/images/edits", body, headers=AUTH).status, 404)  # doesn't exist
        self.assertIn("POST /v1/images/edits", self.srv.state.unrouted)  # and is on the record
        self.srv.state.unrouted.clear()
        speech = {"model": "Qwen/Qwen3-TTS-Base", "input": "hi"}  # the Base model only clones: ref_audio and ref_text, both
        self.assertEqual(request(self.url, "POST", "/v1/audio/speech", speech, headers=AUTH).status, 400)
        self.assertEqual(request(self.url, "POST", "/v1/audio/speech", {**speech, "ref_audio": "AAAA"}, headers=AUTH).status, 400)
        self.assertEqual(request(self.url, "POST", "/v1/audio/speech", {**speech, "ref_audio": "AAAA", "ref_text": "hi"}, headers=AUTH).status, 200)
        song = {"model": "Fake/SongGeneration-v2", "prompt": "x"}  # plain /generate fails for SongGeneration
        self.assertEqual(request(self.url, "POST", "/v1/music/generate", song, headers=AUTH).status, 404)
        self.assertEqual(request(self.url, "POST", "/v1/music/generate/preview", song, headers=AUTH).status, 200)

    def test_locked_answers_502_before_the_key_is_checked(self):
        request(self.url, "POST", "/__reset")
        request(self.url, "POST", "/__flags", {"locked": True})
        self.addCleanup(request, self.url, "POST", "/__flags", {"locked": False})
        self.assertEqual(request(self.url, "GET", "/v1/models", headers=AUTH).status, 502)
        self.assertEqual(request(self.url, "GET", "/v1/models", headers={"Authorization": "Bearer wrong"}).status, 502)  # not 401


if __name__ == "__main__":
    unittest.main()
