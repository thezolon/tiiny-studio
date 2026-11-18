"""The Rust studio's built-in MCP server (issue #87), against the fake Tiiny. Rust only: the Python server has no MCP.
Turns it on through /api/settings/mcp, speaks JSON-RPC to POST /mcp with the bearer token, and turns it off again."""
import json
import urllib.error
import urllib.request

from harness import AUTH, ContractCase, request  # first: puts testdata/ on the path

MODEL = "Qwen/Qwen-Image-2.1"


class Mcp(ContractCase):
    def setUp(self):
        self.base = self.h.rs_url
        request(self.h.fake_url, "POST", "/__reset")
        self.token = request(self.base, "POST", "/api/settings/mcp", {"on": True}).json["mcp"]["token"]

    def tearDown(self):
        request(self.base, "POST", "/api/settings/mcp", {"on": False})

    def rpc(self, method, params=None, token="", msg_id=1):
        headers = {"Authorization": f"Bearer {self.token if token == '' else token}"} if token is not None else {}
        body = {"jsonrpc": "2.0", "id": msg_id, "method": method}
        if params is not None:
            body["params"] = params
        return request(self.base, "POST", "/mcp", body, headers=headers)

    def tool(self, name, args=None):
        """The result of a tools/call: (isError, content list)."""
        res = self.rpc("tools/call", {"name": name, "arguments": args or {}})
        self.assertEqual(res.status, 200, res.body)
        result = res.json["result"]
        return result["isError"], result["content"]

    def unload(self, model):
        request(self.h.fake_url, "POST", "/api/v1/models/" + model.replace("/", "%2F") + "/stop", headers=AUTH)

    def test_off_is_404(self):
        request(self.base, "POST", "/api/settings/mcp", {"on": False})
        self.assertEqual(self.rpc("ping").status, 404)
        self.assertEqual(request(self.base, "GET", "/mcp").status, 404)

    def test_token_is_required(self):
        self.assertEqual(self.rpc("ping", token=None).status, 401)
        self.assertEqual(self.rpc("ping", token="not-the-token").status, 401)
        self.assertEqual(self.rpc("ping").status, 200)
        self.assertEqual(request(self.base, "GET", "/mcp", headers={"Authorization": f"Bearer {self.token}"}).status, 405)
        # a new token replaces the old one
        new = request(self.base, "POST", "/api/settings/mcp", {"new_token": True}).json["mcp"]["token"]
        self.assertNotEqual(new, self.token)
        self.assertEqual(self.rpc("ping").status, 401)
        self.token = new
        self.assertEqual(self.rpc("ping").status, 200)

    def test_initialize_and_tools(self):
        res = self.rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}).json
        self.assertEqual(res["result"]["serverInfo"]["name"], "tiiny-studio")
        self.assertEqual(res["result"]["protocolVersion"], "2025-06-18")
        note = request(self.base, "POST", "/mcp", {"jsonrpc": "2.0", "method": "notifications/initialized"},
                       headers={"Authorization": f"Bearer {self.token}"})
        self.assertEqual((note.status, note.body), (202, b""))
        names = [t["name"] for t in self.rpc("tools/list").json["result"]["tools"]]
        for want in ("create_image", "edit_image", "read_document", "speak", "clone_voice", "transcribe", "make_loop", "make_song",
                     "make_3d", "make_video", "chat", "list_models", "load_model", "unload_model", "get_job", "cancel_job"):
            self.assertIn(want, names)
        self.assertEqual(self.rpc("no/such/method").json["error"]["code"], -32601)

    def test_other_sites_are_refused(self):
        req = urllib.request.Request(self.base + "/mcp", json.dumps({"jsonrpc": "2.0", "id": 1, "method": "ping"}).encode(), method="POST",
                                     headers={"Authorization": f"Bearer {self.token}", "Origin": "http://evil.example", "Content-Type": "application/json"})
        try:
            code = urllib.request.urlopen(req, timeout=10).status
        except urllib.error.HTTPError as e:
            code = e.code
        self.assertEqual(code, 403)

    def test_list_models(self):
        self.load_on_fake(MODEL)
        failed, content = self.tool("list_models")
        self.assertFalse(failed)
        models = json.loads(content[0]["text"])["models"]
        row = next(m for m in models if m["id"] == MODEL)
        self.assertTrue(row["loaded"])
        self.assertFalse(next(m for m in models if m["id"] == "Fake/Whisper-ASR")["loaded"])

    def test_create_image_end_to_end(self):
        self.load_on_fake(MODEL)
        failed, content = self.tool("create_image", {"prompt": "RUST TEST agent duck", "size": "512x512", "seed": 3})
        self.assertFalse(failed, content)
        job = json.loads(content[0]["text"])
        self.assertIn(job["status"], ("queued", "running"))
        self.assertIn("estimate_seconds", job)

        def done():
            _, c = self.tool("get_job", {"job_id": job["job_id"]})
            info = json.loads(c[0]["text"])
            return (info, c) if info["status"] in ("done", "error", "cancelled") else None

        info, content = self.wait_until(done, "the agent's image job to end", 60)
        self.assertEqual(info["status"], "done", info)
        self.assertEqual(len(info["files"]), 1)
        url = info["files"][0]
        self.assertTrue(url.startswith(self.base + "/images/") and url.endswith(".png"), url)
        self.assertEqual(request("", "GET", url).status, 200)
        inline = [c for c in content if c["type"] == "image"]
        self.assertEqual(len(inline), 1)
        self.assertEqual(inline[0]["mimeType"], "image/png")
        # the studio's own jobs menu says whose it is
        mine = next(j for j in request(self.base, "GET", "/api/jobs").json if j["id"] == job["job_id"])
        self.assertTrue(mine["label"].startswith("Agent: "), mine["label"])
        # clean up what the test made
        request(self.base, "DELETE", "/api/history/" + mine["entry"]["id"])

    def test_not_loaded_is_a_tool_error(self):
        self.unload(MODEL)
        failed, content = self.tool("create_image", {"prompt": "RUST TEST nothing loaded", "model": MODEL})
        self.assertTrue(failed)
        self.assertIn("load_model", content[0]["text"])
        failed, content = self.tool("create_image", {})
        self.assertTrue(failed)
        self.assertIn("prompt is required", content[0]["text"])

    def test_an_agent_cannot_reach_other_jobs(self):
        self.load_on_fake(MODEL)
        theirs = request(self.base, "POST", "/api/jobs", {"kind": "image", "model": MODEL, "prompt": "RUST TEST not the agent's"}).json
        for name in ("get_job", "cancel_job"):
            failed, content = self.tool(name, {"job_id": theirs["id"]})
            self.assertTrue(failed, name)
            self.assertIn("not one of this agent's jobs", content[0]["text"])
        job, _ = self.finish(self.base, theirs)
        self.assertEqual(job["status"], "done")  # the refused cancel did nothing
        request(self.base, "DELETE", "/api/history/" + job["entry"]["id"])
