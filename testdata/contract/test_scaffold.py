"""Stage 0 contract tests: the shell of the Rust server matches the Python one, and the fake Tiiny behaves."""
import json
import time
import unittest

import harness
from harness import ContractCase, request

AUTH = {"Authorization": "Bearer fake-key"}


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
        self.assertEqual(set(py.json), set(rs.json))
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
        time.sleep(0.5)
        run = request(self.url, "GET", "/api/v1/models/running", headers=AUTH).json
        self.assertIn("Qwen/Qwen-Image-2.1", [i["model_id"] for i in run["instances"]["running"]])


if __name__ == "__main__":
    unittest.main()
