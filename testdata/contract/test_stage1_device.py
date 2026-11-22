"""Stage 1 contract tests: settings and saved devices, the device client, models, stats, power off.
Each test sends requests to the Rust server (seeded from TIINY_URL/TIINY_KEY with the fake Tiiny) and checks the
answer and, where it matters, what the server asked the device."""
import json
import unittest
from pathlib import Path

from harness import ContractCase, request  # first: puts testdata/ on the path
import fake_tiiny  # noqa: E402

VERSION = (Path(__file__).resolve().parents[2] / "VERSION").read_text("utf-8").strip()
# Device ids are random per run; probe times differ; the folder button is off in Docker; the swap, MCP and update settings have their own tests.
IGNORE = ("id", "active", "ms", "t", "can_open_folder", "source")  # source: env until a test saves the devices, then file
Q = "Qwen%2FQwen3-4B-Instruct"
KEY_ERROR = "The device rejected the API key. Copy the API key (not the device ID) from TiinyOS: avatar \u2192 Settings \u2192 API Key."
LOCKED = "Your Tiiny is locked after a restart. Open the TiinyOS app once (or run \"tiiny login\" with the Tiiny CLI) to unlock it."
CATALOG = [("Qwen/Qwen3-4B-Instruct", "running"), ("Fake/Always-Thinks", "stopped"), ("Fake/No-Thinking", "stopped"), ("Fake/Quiet-Thinker", "stopped"),
           ("Qwen/Qwen-Image-2.1", "stopped"), ("Qwen/Qwen3-TTS-Base", "stopped"), ("Fake/CustomVoice", "stopped"), ("Fake/VoiceDesign", "stopped"),
           ("Fake/Broken-TTS", "stopped"), ("Fake/Whisper-ASR", "stopped"), ("Fake/Foundation-1", "stopped"), ("Fake/SongGeneration-v2", "stopped"),
           ("ACE-Step/AceStep-Fake", "stopped"), ("Fake/YuE2", "stopped"), ("Tencent/Hunyuan3D-2.1", "stopped"), ("microsoft/TRELLIS.2-4B", "stopped"),
           ("lightx2v/MiniMax-H3-Turbo", "stopped"), ("Fake/PP-OCRv6", "stopped"), ("Qwen/Qwen3-Embedding-0.6B", "stopped")]
THINKING = {"supported": True, "toggleable": True, "enabled_by_default": True, "levels": [], "default_level": None}


class Device(ContractCase):
    def device_calls(self, method, path, body=None):
        """The requests the server makes to the fake Tiiny while answering one request (probes of "/" left out)."""
        request(self.fake_url, "POST", "/__reset")
        res = request(self.h.rs_url, method, path, body)
        log = json.loads(request(self.fake_url, "GET", "/__requests").body)
        return res, [(r["method"], r["path"], r["body"]) for r in log if r["path"] != "/"]

    @property
    def fake_url(self):
        return self.h.fake_url

    def settings(self, devices, auto=True, active=0, **more):
        """GET /api/settings as the server answers it for these devices ([(name, url, key_hint, status)]; ids are ignored)."""
        rows = [{"id": "", "name": n, "url": u, "host": u.split("//")[1].split(":")[0], "key_set": True, "key_hint": k, "status": st} for n, u, k, st in devices]
        cur = rows[active]
        return {"devices": rows, "active": "", "name": cur["name"], "url": cur["url"], "host": cur["host"], "auto": auto, "key_set": True,
                "key_hint": cur["key_hint"], "configured": True, "source": more.pop("source", "env"), "lan_access": False, "password": False,
                "last_switch": None, "version": VERSION, "swap": {"on": True, "chat": True, "image": True, "audio": True, "mesh": True, "queue": True},
                "mcp": {"on": False, "token_set": False}, "helper_model": "", "docker": True, "update_check": False, "update_ask": False,
                "tag_prompts": True, "app": False, **more}

    def own(self):
        """The device the server was started with (named after its address; in Docker that is host.docker.internal)."""
        return (f"Tiiny ({self.fake_url.split('//')[1].split(':')[0]})", self.fake_url, "\u2026-key", {"ok": True})

    def test_settings_seeded_from_env(self):
        r = self.post("/api/settings/check")
        self.assertEqual(r.status, 200)
        want = self.settings([self.own()])
        self.expect(r, 200, want, IGNORE)
        r = self.get("/api/settings")
        self.expect(r, 200, want, IGNORE)
        self.assertTrue(r.json["configured"])
        self.assertIn(r.json["source"], ("env", "file"))

    def test_test_connection(self):
        ok = {"ok": True, "message": "Connected. 1 model(s) loaded: Qwen3-4B-Instruct", "loaded": ["Qwen3-4B-Instruct"]}
        self.expect(self.post("/api/settings/test", {}), 200, ok)
        self.expect(self.post("/api/settings/test", {"url": self.fake_url, "key": "wrong"}), 200, {"ok": False, "message": KEY_ERROR})
        self.expect(self.post("/api/settings/test", {"url": ""}), 200, ok)  # no address given: the saved device

    def test_settings_test_spots_a_relay(self):
        # an OpenAI-style relay answers /v1/models but has no Tiiny model controls: not "Connected"
        request(self.fake_url, "POST", "/__flags", {"fail": {"/api/v1/models/running": [404, "Not Found"]}})
        try:
            r = self.post("/api/settings/test", {"url": self.fake_url, "key": "fake-key"})
        finally:
            request(self.fake_url, "POST", "/__reset")
        self.expect(r, 200, {"ok": False, "message": "This address answers chat requests but has no Tiiny model controls, so it looks like a relay or proxy "
                                                     "(for example an OpenAI-compatible gateway). Use your Tiiny's own address and API key from the TiinyOS app."})
        self.assertIn("relay or proxy", r.json["message"])

    def test_settings_test_wrong_key(self):
        # the device ID sits next to the API key in TiinyOS and is easy to paste by mistake
        r = self.post("/api/settings/test", {"url": self.fake_url, "key": "not-the-key"})
        self.expect(r, 200, {"ok": False, "message": KEY_ERROR})
        self.assertIn("not the device ID", r.json["message"])

    def test_settings_test_explains_docker_and_usb(self):
        # in Docker, a 172.17.x.x USB address can't be reached (Docker's own network): say so, not just "unreachable"
        r = self.post("/api/settings/test", {"url": "http://172.17.0.250", "key": "k"})
        self.expect(r, 200, {"ok": False, "message": "This is a USB address in Docker's own network (172.17.x.x), so the studio in Docker can't reach it. "
                                                     "Use the Tiiny's Wi-Fi/LAN address, or the desktop app, or move Docker's network (see the connection guide)."})
        self.assertIn("Docker's own network", r.json["message"])
        self.assertTrue(self.get("/api/settings").json["docker"])

    def test_catalog_and_model_info(self):
        r, calls = self.device_calls("GET", "/api/catalog")
        self.assertEqual(r.status, 200)
        self.assertEqual([(m["id"], m["status"]) for m in r.json["models"]], CATALOG)
        chat = next(m for m in r.json["models"] if m["id"] == fake_tiiny.CHAT_MODEL)
        self.assertEqual(chat, {"id": "Qwen/Qwen3-4B-Instruct", "display_name": "Qwen3 4B", "params": "4B", "type": "Text Generation", "input": "text",
                                "output": "text", "supported": ["Tool Use"], "desc": "Fake Qwen3 4B", "npu_usage": 30, "load_time": 20, "costMemory": 3000,
                                "size": 1073741824, "version": 1, "thinking": THINKING, "reasoning_levels": [], "capabilities": ["main"],
                                "status": "running", "busy": False, "workspace": "text"})
        self.assertEqual((chat["thinking"]["toggleable"], chat["capabilities"]), (True, ["main"]))
        self.assertEqual(calls, [("GET", "/api/v1/models/", None), ("GET", "/api/v1/models/running", None), ("GET", "/api/v1/models/running", None)])
        q = fake_tiiny.CHAT_MODEL.replace("/", "%2F")
        r, calls = self.device_calls("GET", f"/api/model/info?id={q}")  # loaded: the model's own answer
        self.expect(r, 200, {"id": fake_tiiny.CHAT_MODEL, "type": "Text Generation", "info": {
            "object": "capabilities", "service": "chat", "model": fake_tiiny.CHAT_MODEL, "supports_chat": True, "supported": ["Tool Use"], "thinking": THINKING}})
        self.assertEqual(calls, [("GET", "/api/v1/models/", None), ("GET", "/v1/capabilities", None)])
        r, calls = self.device_calls("GET", "/api/model/info?id=Fake%2FSongGeneration-v2")  # not loaded: no info
        self.expect(r, 200, {"id": "Fake/SongGeneration-v2", "type": "Music Generation", "info": None})
        self.assertEqual(calls, [("GET", "/api/v1/models/", None), ("GET", "/v1/music/info", None)])
        r, calls = self.device_calls("GET", "/api/model/info?id=Fake%2FNope")
        self.expect(r, 404, {"error": {"message": "Fake/Nope isn't installed on the device"}})
        self.assertEqual(calls, [("GET", "/api/v1/models/", None)])
        r, calls = self.device_calls("GET", "/api/model/info")
        self.expect(r, 400, {"error": {"message": "id required"}})
        self.assertEqual(calls, [])

    def test_models_catalog_plan(self):
        r, calls = self.device_calls("GET", "/api/models")
        self.expect(r, 200, {"object": "list", "data": [{"id": "Qwen/Qwen3-4B-Instruct", "object": "model", "aliases": ["default"], "supports_vision": False,
                                                          "supports_chat": True, "context_window": 32768, "thinking": THINKING}]})
        self.assertEqual(calls, [("GET", "/v1/models", None)])
        r, _ = self.device_calls("GET", "/api/catalog")
        self.assertTrue(r.json["models"])
        r, calls = self.device_calls("GET", "/api/model/plan?id=Qwen%2FQwen-Image-2.1")
        self.expect(r, 200, {"unload_required": False, "model_id": [], "npu_required": 60, "npu_available": 70})
        self.assertEqual(calls, [("GET", "/api/v1/models/Qwen%2FQwen-Image-2.1/unload_candidate", None)])
        self.expect(self.get("/api/model/plan"), 400, {"error": {"message": "id required"}})

    def test_launch_and_stop(self):
        mid = json.loads(request(self.fake_url, "GET", "/api/v1/models/", headers={"Authorization": "Bearer fake-key"}).body)["data"][-1]["id"]
        self.assertEqual(mid, "Qwen/Qwen3-Embedding-0.6B")
        r, calls = self.device_calls("POST", "/api/model/launch", {"id": mid})
        self.expect(r, 200, {"status": "ok", "message": "launch accepted"})
        self.assertEqual(calls, [("POST", "/api/v1/models/Qwen%2FQwen3-Embedding-0.6B/launch", {"model_id": mid})])
        r, calls = self.device_calls("POST", "/api/model/stop", {"id": mid})
        self.expect(r, 200, {"status": "ok", "message": "stop accepted"})
        self.assertEqual(calls, [("POST", "/api/v1/models/Qwen%2FQwen3-Embedding-0.6B/stop", None)])
        r, calls = self.device_calls("POST", "/api/model/launch", {})
        self.expect(r, 400, {"error": {"message": "id required"}})
        self.assertEqual(calls, [])
        r, calls = self.device_calls("POST", "/api/model/unload_all")
        self.expect(r, 200, {"status": "ok"})
        self.assertEqual(calls, [("POST", "/api/v1/models/unload_all", None)])

    def test_stats_busy(self):
        self.expect(self.get("/api/stats"), 200, {"source": ["gateway"], "npu_util": 7, "npu_mem_used_mb": 3000, "npu_mem_total_mb": 10000, "npu_temp_c": 41.5,
                                                  "npu_power_w": 9.5, "cpu_pct": 12.5, "mem_pct": 40.0, "npu_status": "ok", "disk_free_gb": 400.0,
                                                  "disk_total_gb": 921.1})
        self.expect(self.get("/api/busy"), 200, {"busy": False})

    def test_shutdown(self):
        # The fake records the call; nothing powers off.
        r, calls = self.device_calls("POST", "/api/device/shutdown")
        self.expect(r, 200, {"status": "ok", "message": "System shutdown will be initiated in seconds via D-Bus"})
        self.assertEqual(calls, [("POST", "/api/v1/sys/shutdown/", None)])

    def test_icon(self):
        r = self.get("/api/icon/Qwen/Qwen-Image-2.1")
        self.assertEqual((r.status, r.headers.get("Content-Type")), (200, "image/png"))
        self.assertEqual(r.body, fake_tiiny.tiny_png())

    def test_locked_after_restart(self):
        # after a power cycle the Tiiny's web server answers 502 until TiinyOS unlocks it: say so, not "can't reach"
        self.addCleanup(request, self.h.fake_url, "POST", "/__flags", {"locked": False, "fail": {}})
        request(self.h.fake_url, "POST", "/__flags", {"locked": True})
        self.expect(self.get("/api/catalog"), 502, {"error": {"message": LOCKED, "type": "device_locked"}})
        # any other error from the device is passed on as before
        request(self.h.fake_url, "POST", "/__flags", {"locked": False, "fail": {"/api/v1/models/": [500, "boom"]}})
        self.expect(self.get("/api/catalog"), 500, {"error": {"message": "boom"}})

    def test_locked_in_the_connection_test_and_a_failed_running_list(self):
        self.addCleanup(request, self.h.fake_url, "POST", "/__flags", {"locked": False, "fail": {}})
        request(self.h.fake_url, "POST", "/__flags", {"locked": True})
        self.expect(self.post("/api/settings/test", {}), 200, {"ok": False, "message": LOCKED})
        # a failed /running call fails the catalog instead of showing every model as stopped
        request(self.h.fake_url, "POST", "/__flags", {"locked": False, "fail": {"/api/v1/models/running": [500, "boom"]}})
        self.expect(self.get("/api/catalog"), 500, {"error": {"message": "boom"}})

    def test_missing_icon_is_asked_for_once(self):
        # 0.1.38 has no icon for MiniMax-H3-Turbo (404); the drawer redraws every few seconds, so remember that
        request(self.h.fake_url, "POST", "/__reset_log")
        for _ in range(2):
            self.expect(self.get("/api/icon/lightx2v/MiniMax-H3-Turbo"), 404, {"error": {"message": "no icon"}})
        asked = [r["path"] for r in json.loads(request(self.h.fake_url, "GET", "/__requests").body) if r["path"].endswith("/icon")]
        self.assertEqual(asked, ["/api/v1/models/online_models/lightx2v/MiniMax-H3-Turbo/icon"])  # the second answer came from memory

    def test_devices_lifecycle(self):
        hotspot = ("Hotspot", "http://192.168.1.60", "\u20262222", None)
        both = [self.own(), hotspot]
        added = self.post("/api/settings", {"new": True, "name": "Hotspot", "url": "192.168.1.60", "key": "key-bbbb-2222"})
        self.expect(added, 200, self.settings(both), IGNORE)
        self.expect(self.post("/api/settings/auto", {"auto": False}), 200, self.settings(both, auto=False), IGNORE)
        self.expect(self.post("/api/settings", {"url": ""}), 400, {"error": {"message": "Enter the device address, e.g. http://192.168.1.50"}})
        self.expect(self.get("/api/settings"), 200, self.settings(both, auto=False), IGNORE)
        # move/activate/delete address a device by its id
        hid = self.get("/api/settings").json["devices"][1]["id"]
        self.expect(self.post("/api/settings/move", {"id": hid, "dir": -1}), 200, self.settings([hotspot, self.own()], auto=False, active=1), IGNORE)
        self.expect(self.post("/api/settings/activate", {"id": hid}), 200, self.settings([hotspot, self.own()], auto=False, active=0), IGNORE)
        self.expect(self.post("/api/settings/delete", {"id": hid}), 200, self.settings([self.own()], auto=False), IGNORE)
        self.expect(self.post("/api/settings/auto", {"auto": True}), 200, self.settings([self.own()]), IGNORE)


if __name__ == "__main__":
    unittest.main()
