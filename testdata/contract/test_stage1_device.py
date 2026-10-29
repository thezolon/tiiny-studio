"""Stage 1 contract tests: settings and saved devices, the device client, models, stats, power off.
Each test sends the same requests to the Python and Rust servers (both seeded from TIINY_URL/TIINY_KEY with
the fake Tiiny) and compares answers and, where it matters, what each server asked the device."""
import json
import unittest

from harness import ContractCase, request

# Device ids are random per server; probe times differ.
IGNORE = ("id", "active", "ms", "t")


class Device(ContractCase):
    def device_calls(self, base, method, path, body=None):
        """The requests one server makes to the fake Tiiny while answering one request (probes of "/" left out)."""
        request(self.fake_url, "POST", "/__reset")
        res = request(base, method, path, body)
        log = json.loads(request(self.fake_url, "GET", "/__requests").body)
        return res, [(r["method"], r["path"], r["body"]) for r in log if r["path"] != "/"]

    def same_calls(self, method, path, body=None, ignore=IGNORE):
        py, py_calls = self.device_calls(self.h.py_url, method, path, body)
        rs, rs_calls = self.device_calls(self.h.rs_url, method, path, body)
        self.same((py, rs), ignore)
        self.assertEqual(py_calls, rs_calls)
        return py, rs

    @property
    def fake_url(self):
        return self.h.fake_url

    def test_settings_seeded_from_env(self):
        self.same(self.post("/api/settings/check"), IGNORE)
        py, rs = self.get("/api/settings")
        self.same((py, rs), IGNORE)
        self.assertTrue(py.json["configured"])

    def test_test_connection(self):
        self.same(self.post("/api/settings/test", {}), IGNORE)
        self.same(self.post("/api/settings/test", {"url": self.fake_url, "key": "wrong"}), IGNORE)
        self.same(self.post("/api/settings/test", {"url": ""}), IGNORE)

    def test_settings_test_spots_a_relay(self):
        # an OpenAI-style relay answers /v1/models but has no Tiiny model controls: not "Connected"
        request(self.fake_url, "POST", "/__flags", {"fail": {"/api/v1/models/running": [404, "Not Found"]}})
        try:
            py, rs = self.post("/api/settings/test", {"url": self.fake_url, "key": "fake-key"})
        finally:
            request(self.fake_url, "POST", "/__reset")
        self.same((py, rs), IGNORE)
        self.assertFalse(py.json["ok"])
        self.assertIn("relay or proxy", py.json["message"])

    def test_settings_test_wrong_key(self):
        # the device ID sits next to the API key in TiinyOS and is easy to paste by mistake
        py, rs = self.post("/api/settings/test", {"url": self.fake_url, "key": "not-the-key"})
        self.same((py, rs), IGNORE)
        self.assertIn("not the device ID", py.json["message"])

    def test_settings_test_explains_docker_and_usb(self):
        # in Docker, a 172.17.x.x USB address can't be reached (Docker's own network): say so, not just "unreachable"
        py, rs = self.post("/api/settings/test", {"url": "http://172.17.0.250", "key": "k"})
        self.same((py, rs), IGNORE)
        self.assertIn("Docker's own network", py.json["message"])
        self.assertTrue(self.get("/api/settings")[0].json["docker"])

    def test_models_catalog_plan(self):
        self.same_calls("GET", "/api/models")
        py, _ = self.same_calls("GET", "/api/catalog")
        self.assertTrue(py.json["models"])
        self.same_calls("GET", "/api/model/plan?id=Qwen%2FQwen-Image-2.1")
        self.same(self.get("/api/model/plan"))

    def test_launch_and_stop(self):
        mid = json.loads(request(self.fake_url, "GET", "/api/v1/models/", headers={"Authorization": "Bearer fake-key"}).body)["data"][-1]["id"]
        self.same_calls("POST", "/api/model/launch", {"id": mid})
        self.same_calls("POST", "/api/model/stop", {"id": mid})
        self.same(self.post("/api/model/launch", {}))
        self.same_calls("POST", "/api/model/unload_all")

    def test_stats_busy(self):
        self.same(self.get("/api/stats"))
        self.same(self.get("/api/busy"))

    def test_shutdown(self):
        # The fake records the call; nothing powers off.
        self.same_calls("POST", "/api/device/shutdown")

    def test_icon(self):
        py, rs = self.get("/api/icon/Qwen/Qwen-Image-2.1")
        self.assertEqual((py.status, rs.status), (200, 200))
        self.assertEqual(py.body, rs.body)

    def test_devices_lifecycle(self):
        steps = [
            ("/api/settings", {"new": True, "name": "Hotspot", "url": "192.168.1.60", "key": "key-bbbb-2222"}),
            ("/api/settings/auto", {"auto": False}),
            ("/api/settings", {"url": ""}),
        ]
        for path, body in steps:
            self.same(self.post(path, body), IGNORE)
        # move/activate/delete address devices by their own (different) ids on each server
        py_ids = [d["id"] for d in self.get("/api/settings")[0].json["devices"]]
        rs_ids = [d["id"] for d in self.get("/api/settings")[1].json["devices"]]
        for action, extra in (("move", {"dir": -1}), ("activate", {}), ("delete", {})):
            py = request(self.h.py_url, "POST", f"/api/settings/{action}", {"id": py_ids[1], **extra})
            rs = request(self.h.rs_url, "POST", f"/api/settings/{action}", {"id": rs_ids[1], **extra})
            self.same((py, rs), IGNORE)
        self.same(self.post("/api/settings/auto", {"auto": True}), IGNORE)


if __name__ == "__main__":
    unittest.main()
