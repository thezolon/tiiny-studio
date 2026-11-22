"""Stage 7 contract tests: the model store, downloads, updates and delete (requested on the Tiiny Discord).
Both servers share one fake Tiiny, so each scenario runs once per server from a fresh fake (reset), and the answers
and the requests each server made to the device are compared. Nothing here touches a real Tiiny."""
import json
import threading
import time
import unittest

from harness import AUTH, ContractCase, request, strip  # first: puts testdata/ on the path

SMALL, BIG, CHAT = "Fake/Store-Small", "Fake/Store-Big", "Qwen/Qwen3-4B-Instruct"
IGNORE = ("progress", "t", "device", "last_switch")  # progress depends on timing; device ids differ per server


class Models(ContractCase):
    def scenario(self, steps):
        """Run steps(call) against each server from a fresh fake; call(method, path, body) returns (status, json).
        The device log is compared without its GETs (polling counts differ), so only the changes must match.
        Returns (python results, the device requests python made); asserts Rust got the same of both."""
        out = []
        for base in (self.h.py_url, self.h.rs_url):
            request(self.h.fake_url, "POST", "/__reset")
            results = []

            def call(method, path, body=None, record=True, base=base, results=results):
                r = request(base, method, path, body)
                if record:  # polls until something happens are left out: how many there are depends on timing
                    results.append((method, path, r.status, strip(json.loads(r.body or b"{}"), IGNORE)))
                return r.status, json.loads(r.body or b"{}")
            self.forget(base)
            try:
                steps(call)
            finally:
                self.forget(base)  # nothing queued may run on into the other server's turn or the next test
            log = json.loads(request(self.h.fake_url, "GET", "/__requests").body)
            out.append((results, [(r["method"], r["path"]) for r in log if r["method"] != "GET"]))
        (py, py_log), (rs, rs_log) = out
        self.assertEqual(py, rs)
        self.assertEqual(py_log, rs_log)
        return py, py_log

    def forget(self, base):
        """Clear a server's download queue and its list of models an update left missing (both live in memory)."""
        request(base, "POST", "/api/model/dequeue", {"all": True})
        r = request(base, "GET", "/api/store")
        for mid in (json.loads(r.body).get("missing") or []) if r.status == 200 else []:
            request(base, "POST", "/api/model/dismiss", {"id": mid})

    def flags(self, **f):
        request(self.h.fake_url, "POST", "/__flags", f)

    def wait_status(self, call, mid, want, timeout=10):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            m = next(m for m in call("GET", "/api/store", record=False)[1]["models"] if m["id"] == mid)
            if m["status"] == want:
                return m
            time.sleep(0.1)
        self.fail(f"{mid} never reached {want}")

    def test_store_lists_everything(self):
        def steps(call):
            code, j = call("GET", "/api/store")
            self.assertEqual(code, 200)
            by = {m["id"]: m for m in j["models"]}
            self.assertFalse(by[SMALL]["installed"])
            self.assertEqual(by[SMALL]["status"], "not_downloaded")
            self.assertEqual(by[BIG]["size"], 18_300_000_000)
            self.assertEqual(by[BIG]["context_window"], 262144)
            self.assertTrue(by[CHAT]["installed"])
            self.assertEqual(by[CHAT]["workspace"], "text")
            self.assertEqual((j["downloading"], j["updates"]), (None, 0))
            self.assertIsNotNone(j["disk"])
        self.scenario(steps)

    def test_download_to_installed(self):
        def steps(call):
            self.flags(download_rate=40)
            self.assertEqual(call("POST", "/api/model/download", {"id": SMALL})[0], 200)
            j = call("GET", "/api/downloads")[1]
            self.assertEqual([d["id"] for d in j["downloads"]], [SMALL])
            self.wait_status(call, SMALL, "downloaded")
            self.assertIn(SMALL, [m["id"] for m in call("GET", "/api/catalog")[1]["models"]])
            self.assertEqual(call("GET", "/api/downloads")[1]["downloads"], [])
            self.assertEqual(call("POST", "/api/model/download", {"id": SMALL})[0], 409)  # already there
        _, log = self.scenario(steps)
        # the id keeps its "/" on the device, as tested on 0.1.39
        self.assertIn(("POST", f"/api/v1/models/{SMALL}/download"), log)

    def test_download_refusals(self):
        def steps(call):
            self.assertEqual(call("POST", "/api/model/download", {})[0], 400)
            self.assertEqual(call("POST", "/api/model/download", {"id": "../../etc"})[0], 400)
            self.assertEqual(call("POST", "/api/model/download", {"id": "Fake/Not-In-Store"})[0], 404)
            self.flags(download_hold=True)
            self.assertEqual(call("POST", "/api/model/download", {"id": SMALL})[0], 200)
            code, j = call("POST", "/api/model/download", {"id": "Fake/Store-Image"})  # one at a time
            self.assertEqual(code, 409)
            self.assertIn("One download at a time", j["error"]["message"])
            self.assertEqual(call("POST", "/api/model/download", {"id": SMALL})[0], 409)  # already downloading
        _, log = self.scenario(steps)
        self.assertEqual(sum(1 for c in log if c[0] == "POST" and c[1].endswith("/download")), 1)

    def test_download_needs_space(self):
        def steps(call):
            self.flags(disk_free_gb=19)  # 18.3 GB + the 2 GB floor doesn't fit
            code, j = call("POST", "/api/model/download", {"id": BIG})
            self.assertEqual(code, 409)
            self.assertIn("Not enough space", j["error"]["message"])
            self.assertEqual(call("POST", "/api/model/download", {"id": SMALL})[0], 200)
        _, log = self.scenario(steps)
        self.assertNotIn(("POST", f"/api/v1/models/{BIG}/download"), log)

    def test_offline_download_errors(self):
        def steps(call):
            self.flags(offline=True, updates=[CHAT])
            call("POST", "/api/model/download", {"id": SMALL})
            m = self.wait_status(call, SMALL, "error")
            self.assertFalse(m["installed"])
            j = call("GET", "/api/store")[1]
            self.assertEqual(j["updates"], 0)  # the device hides updates while offline
            self.assertEqual([d["status"] for d in call("GET", "/api/downloads")[1]["downloads"]], ["error"])
        self.scenario(steps)

    def test_pause_resume_cancel(self):
        def steps(call):
            self.assertEqual(call("POST", "/api/model/pause", {"id": SMALL})[0], 409)  # nothing to pause
            self.flags(download_rate=5)
            call("POST", "/api/model/download", {"id": SMALL})
            self.assertEqual(call("POST", "/api/model/pause", {"id": SMALL})[0], 200)
            self.wait_status(call, SMALL, "paused")
            self.assertEqual(call("POST", "/api/model/download", {"id": SMALL})[0], 200)  # resume
            self.wait_status(call, SMALL, "downloading")
            self.assertEqual(call("POST", "/api/model/cancel", {"id": SMALL})[0], 200)
            self.wait_status(call, SMALL, "not_downloaded")
        _, log = self.scenario(steps)
        # 0.1.39 pauses as "download_stop" and /interrupt only stops chats: cancel is pause + delete
        self.assertEqual(log[-2:], [("POST", f"/api/v1/models/{SMALL}/pause_download"), ("DELETE", f"/api/v1/models/{SMALL}")])
        self.assertFalse(any(c[1].endswith("/interrupt") for c in log))

    def test_delete(self):
        def steps(call):
            self.assertEqual(call("POST", "/api/model/delete", {"id": CHAT})[0], 400)  # no confirm
            self.assertEqual(call("POST", "/api/model/delete", {"id": CHAT, "confirm": "Fake/Other"})[0], 400)
            self.assertEqual(call("POST", "/api/model/delete", {"id": SMALL, "confirm": SMALL})[0], 404)
            self.load_on_fake(CHAT)
            code, j = call("POST", "/api/model/delete", {"id": CHAT, "confirm": CHAT})
            self.assertEqual(code, 409)
            self.assertIn("Unload it first", j["error"]["message"])
            request(self.h.fake_url, "POST", "/api/v1/models/Qwen%2FQwen3-4B-Instruct/stop", {}, headers=AUTH)
            self.assertEqual(call("POST", "/api/model/delete", {"id": CHAT, "confirm": CHAT})[0], 200)
            self.assertNotIn(CHAT, [m["id"] for m in call("GET", "/api/catalog")[1]["models"]])
            self.assertFalse(next(m for m in call("GET", "/api/store")[1]["models"] if m["id"] == CHAT)["installed"])
        _, log = self.scenario(steps)
        self.assertEqual([c for c in log if c[0] == "DELETE"], [("DELETE", f"/api/v1/models/{CHAT}")])  # only the allowed one

    def test_delete_refused_while_downloading(self):
        def steps(call):
            self.flags(download_hold=True)
            call("POST", "/api/model/download", {"id": SMALL})
            code, j = call("POST", "/api/model/delete", {"id": SMALL, "confirm": SMALL})
            self.assertEqual(code, 409)
            self.assertIn("Cancel the download", j["error"]["message"])
        self.scenario(steps)

    def test_update(self):
        def steps(call):
            self.assertEqual(call("POST", "/api/model/update", {"id": CHAT, "confirm": CHAT})[0], 409)  # none waiting
            self.flags(updates=[CHAT], download_rate=40)
            j = call("GET", "/api/store")[1]
            self.assertEqual(j["updates"], 1)
            self.assertTrue(next(m for m in j["models"] if m["id"] == CHAT)["need_update"])
            self.load_on_fake(CHAT)
            code, j = call("POST", "/api/model/update", {"id": CHAT, "confirm": CHAT})
            self.assertEqual(code, 409)
            request(self.h.fake_url, "POST", "/api/v1/models/Qwen%2FQwen3-4B-Instruct/stop", {}, headers=AUTH)
            self.assertEqual(call("POST", "/api/model/update", {"id": CHAT, "confirm": CHAT})[0], 200)
            m = self.wait_status(call, CHAT, "downloaded")
            self.assertFalse(m["need_update"])
        _, log = self.scenario(steps)
        changes = [c for c in log if c[0] == "DELETE" or c[1].endswith("/download")]
        self.assertEqual(changes, [("DELETE", f"/api/v1/models/{CHAT}"), ("POST", f"/api/v1/models/{CHAT}/download")])

    def test_update_download_refused_after_delete(self):
        def steps(call):
            request(self.h.fake_url, "POST", "/api/v1/models/Qwen%2FQwen3-4B-Instruct/stop", {}, headers=AUTH)  # the harness loads it
            self.flags(updates=[CHAT], fail={f"/api/v1/models/{CHAT}/download": [500, "store unreachable"]})
            code, j = call("POST", "/api/model/update", {"id": CHAT, "confirm": CHAT})
            self.assertEqual(code, 502, j)
            self.assertEqual(j["error"]["type"], "update_missing")
            self.assertIn("missing until you download it again", j["error"]["message"])
            self.assertNotIn(CHAT, [m["id"] for m in call("GET", "/api/catalog")[1]["models"]])
        self.scenario(steps)

    def test_locked_tiiny(self):
        def steps(call):
            self.flags(locked=True)
            for path in ("/api/store", "/api/downloads"):
                code, j = call("GET", path)
                self.assertEqual((code, j["error"]["type"]), (502, "device_locked"))
        self.scenario(steps)

    # ---------- from the review (2026-11-21) ----------
    def test_resume_needs_only_the_rest(self):
        # 18.3 GB with 21 GB free starts; paused at 10% it has fetched 1.83 GB, so 19.17 GB is free: resuming needs
        # the other 16.47 GB + 2, not the whole 18.3 + 2
        def steps(call):
            self.flags(disk_free_gb=21, download_hold=True)
            self.assertEqual(call("POST", "/api/model/download", {"id": BIG})[0], 200)
            self.assertEqual(call("POST", "/api/model/pause", {"id": BIG})[0], 200)
            code, j = call("POST", "/api/model/download", {"id": BIG})
            self.assertEqual(code, 200, j)
            self.assertEqual(call("POST", "/api/model/cancel", {"id": BIG})[0], 200)
        self.scenario(steps)

    def test_two_updates_at_once_delete_once(self):
        asr = "Fake/Whisper-ASR"

        def steps(call):
            self.flags(updates=[asr], download_hold=True)
            codes = []
            threads = [threading.Thread(target=lambda: codes.append(call("POST", "/api/model/update", {"id": asr, "confirm": asr}, record=False)[0]))
                       for _ in range(2)]
            for t in threads:
                t.start()
            for t in threads:
                t.join()
            self.assertEqual(sorted(codes), [200, 409])
        _, log = self.scenario(steps)
        self.assertEqual(sum(1 for c in log if c[0] == "DELETE"), 1)

    def test_unknown_status_is_never_installed(self):
        odd = "Fake/Store-Image"

        def steps(call):
            self.flags(odd_status={odd: "verifying"})
            self.assertNotIn(odd, [m["id"] for m in call("GET", "/api/catalog")[1]["models"]])  # no Load button
            m = next(m for m in call("GET", "/api/store")[1]["models"] if m["id"] == odd)
            self.assertEqual((m["installed"], m["status"]), (False, "downloading"))
            self.assertEqual(call("POST", "/api/model/delete", {"id": odd, "confirm": odd})[0], 409)
            self.assertEqual(call("POST", "/api/model/download", {"id": SMALL})[0], 409)  # counts as the one download
        _, log = self.scenario(steps)
        self.assertFalse(any(c[0] == "DELETE" for c in log))

    def test_store_unreadable_installed_still_managed(self):
        asr = "Fake/Whisper-ASR"

        def steps(call):
            self.flags(fail={"/api/v1/models/online_models": [500, "store down"]})
            code, j = call("GET", "/api/store")
            self.assertEqual(code, 200)
            self.assertIn("store down", j["store_error"])
            self.assertTrue(next(m for m in j["models"] if m["id"] == CHAT)["installed"])
            self.assertEqual(call("POST", "/api/model/delete", {"id": asr, "confirm": asr})[0], 200)
            self.assertEqual(call("POST", "/api/model/download", {"id": SMALL})[0], 500)  # a download needs the store
        self.scenario(steps)

    def test_update_goes_on_when_the_delete_answer_is_lost(self):
        asr = "Fake/Whisper-ASR"

        def steps(call):
            self.flags(updates=[asr], delete_lost=True, download_hold=True)
            code, j = call("POST", "/api/model/update", {"id": asr, "confirm": asr})
            self.assertEqual(code, 200, j)
        _, log = self.scenario(steps)
        self.assertEqual([c for c in log if c[0] == "DELETE" or c[1].endswith("/download")],
                         [("DELETE", "/api/v1/models/Fake/Whisper-ASR"), ("POST", "/api/v1/models/Fake/Whisper-ASR/download")])

    def test_update_space_counts_the_growth(self):
        asr = "Fake/Whisper-ASR"  # installed: 1.07 GB; the update: 6 GB, so it needs 4.9 GB more + 2

        def steps(call):
            self.flags(updates=[asr], update_size={asr: 6_000_000_000}, disk_free_gb=5, download_hold=True)
            code, j = call("POST", "/api/model/update", {"id": asr, "confirm": asr})
            self.assertEqual(code, 409, j)
            self.assertIn("Not enough space", j["error"]["message"])
            self.flags(disk_free_gb=8)
            self.assertEqual(call("POST", "/api/model/update", {"id": asr, "confirm": asr})[0], 200)
        _, log = self.scenario(steps)
        self.assertEqual(sum(1 for c in log if c[0] == "DELETE"), 1)  # only the update that fits deleted anything

    # ---------- the download queue (requested: set up several, cancel one or all) ----------
    def queue_of(self, call):
        return call("GET", "/api/store", record=False)[1]["queue"]

    def wait_for(self, cond, what, timeout=20):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            if cond():
                return
            time.sleep(0.2)
        self.fail(f"timed out waiting for {what}")

    def test_queue_runs_in_order(self):
        img = "Fake/Store-Image"

        def steps(call):
            self.flags(download_rate=40)
            self.assertEqual(call("POST", "/api/model/queue", {"ids": [SMALL, img], "kind": "download"})[0], 200)
            self.wait_status(call, img, "downloaded", timeout=30)
            self.assertEqual(self.wait_status(call, SMALL, "downloaded")["installed"], True)
            self.assertEqual(self.queue_of(call), {"items": [], "stopped": None})
        _, log = self.scenario(steps)
        starts = [c[1] for c in log if c[1].endswith("/download")]
        self.assertEqual(starts, [f"/api/v1/models/{SMALL}/download", f"/api/v1/models/{img}/download"])

    def test_queue_remove_one_and_clear_all(self):
        img = "Fake/Store-Image"

        def steps(call):
            self.flags(download_hold=True)
            call("POST", "/api/model/queue", {"ids": [SMALL, img, BIG], "kind": "download"})
            self.wait_status(call, SMALL, "downloading")
            q = self.queue_of(call)
            self.assertEqual([i["id"] for i in q["items"]], [img, BIG])
            self.assertEqual(q["items"][1]["size"], 18_300_000_000)
            self.assertEqual(call("POST", "/api/model/dequeue", {"id": img})[0], 200)
            self.assertEqual([i["id"] for i in self.queue_of(call)["items"]], [BIG])
            self.assertEqual(call("POST", "/api/model/dequeue", {"all": True})[0], 200)
            self.assertEqual(self.queue_of(call)["items"], [])
            self.assertEqual(self.wait_status(call, SMALL, "downloading")["status"], "downloading")  # clearing leaves the current one
            call("POST", "/api/model/cancel", {"id": SMALL})
            self.wait_status(call, SMALL, "not_downloaded")
        _, log = self.scenario(steps)
        self.assertEqual([c[1] for c in log if c[1].endswith("/download")], [f"/api/v1/models/{SMALL}/download"])

    def test_queue_waits_while_a_download_is_paused(self):
        img = "Fake/Store-Image"

        def steps(call):
            self.flags(download_hold=True)
            call("POST", "/api/model/download", {"id": SMALL})
            call("POST", "/api/model/pause", {"id": SMALL})
            call("POST", "/api/model/queue", {"ids": [img], "kind": "download"})
            time.sleep(4.5)  # longer than the worker's tick
            self.assertEqual(self.wait_status(call, img, "not_downloaded")["status"], "not_downloaded")
            call("POST", "/api/model/cancel", {"id": SMALL})
            self.wait_status(call, img, "downloading", timeout=15)
        self.scenario(steps)

    def test_queue_stops_on_a_failed_download(self):
        img = "Fake/Store-Image"

        def steps(call):
            self.flags(download_fail=True, download_rate=40)
            call("POST", "/api/model/queue", {"ids": [SMALL, img], "kind": "download"})
            self.wait_status(call, SMALL, "error")
            self.wait_for(lambda: self.queue_of(call)["stopped"], "the queue to stop")
            q = self.queue_of(call)
            self.assertIn("Does the Tiiny have internet", q["stopped"])
            self.assertEqual([i["id"] for i in q["items"]], [img])
            self.assertEqual(self.wait_status(call, img, "not_downloaded")["status"], "not_downloaded")
            self.flags(download_fail=False)
            call("POST", "/api/model/cancel", {"id": SMALL})  # dismiss the failed one
            self.assertEqual(call("POST", "/api/model/resume", {})[0], 200)
            self.wait_status(call, img, "downloaded", timeout=30)
        self.scenario(steps)

    def test_queue_stops_with_the_reason_when_an_item_does_not_fit(self):
        def steps(call):
            self.flags(disk_free_gb=19)
            call("POST", "/api/model/queue", {"ids": [BIG], "kind": "download"})
            self.wait_for(lambda: self.queue_of(call)["stopped"], "the queue to stop")
            q = self.queue_of(call)
            self.assertIn("Not enough space", q["stopped"])
            self.assertEqual([i["id"] for i in q["items"]], [BIG])  # kept, to retry after making room
        _, log = self.scenario(steps)
        self.assertFalse(any(c[1].endswith("/download") for c in log))

    def test_queued_updates_need_confirm_and_a_cancel_leaves_it_missing(self):
        asr = "Fake/Whisper-ASR"

        def steps(call):
            self.flags(updates=[asr], download_hold=True)
            self.assertEqual(call("POST", "/api/model/queue", {"ids": [asr], "kind": "update"})[0], 400)
            self.assertEqual(call("POST", "/api/model/queue", {"ids": [asr], "kind": "update", "confirm": [asr]})[0], 200)
            m = self.wait_status(call, asr, "downloading", timeout=15)
            self.assertTrue(m["update"])
            self.assertEqual(call("GET", "/api/downloads", record=False)[1]["downloads"][0]["update"], True)
            call("POST", "/api/model/cancel", {"id": asr})
            self.wait_for(lambda: call("GET", "/api/store", record=False)[1]["missing"] == [asr], "it to show as missing")
            self.assertEqual(call("POST", "/api/model/dismiss", {"id": asr})[0], 200)
            self.assertEqual(call("GET", "/api/store")[1]["missing"], [])
        self.scenario(steps)

    def test_queue_refuses_what_it_cannot_do(self):
        def steps(call):
            self.assertEqual(call("POST", "/api/model/queue", {"ids": [SMALL], "kind": "launch"})[0], 400)
            self.assertEqual(call("POST", "/api/model/queue", {"ids": [], "kind": "download"})[0], 400)
            self.assertEqual(call("POST", "/api/model/queue", {"ids": ["../x/y"], "kind": "download"})[0], 400)
            self.assertEqual(call("POST", "/api/model/queue", {"ids": ["Fake/Not-In-Store"], "kind": "download"})[0], 404)
            self.assertEqual(call("POST", "/api/model/queue", {"ids": [CHAT], "kind": "download"})[0], 409)  # installed
            self.assertEqual(call("POST", "/api/model/queue", {"ids": [CHAT], "kind": "update", "confirm": [CHAT]})[0], 409)
            self.assertEqual(call("POST", "/api/model/dequeue", {})[0], 400)
            self.assertEqual(self.queue_of(call), {"items": [], "stopped": None})
        self.scenario(steps)

    def test_a_downloaded_model_can_be_loaded_and_used(self):
        # download a store model, load it, speak with it, unload it: the fake knows every model in the store, and (like the
        # device) refuses to speak with a model that isn't loaded
        say = {"kind": "speech", "model": SMALL, "text": "RUST TEST hello there."}
        for base in (self.h.py_url, self.h.rs_url):
            request(self.h.fake_url, "POST", "/__reset")
            self.forget(base)
            self.flags(download_rate=1000)
            self.assertEqual(request(base, "POST", "/api/model/download", {"id": SMALL}).status, 200)
            self.wait_until(lambda base=base: any(m["id"] == SMALL and m["status"] == "downloaded"
                                                  for m in request(base, "GET", "/api/store").json["models"]), "the download to finish")
            job, _ = self.run_job(base, say, reset=False)  # installed, but not loaded: the device says so
            self.assertEqual(job["status"], "error", job)
            self.load_on_fake(SMALL)
            code = request(self.h.fake_url, "GET", f"/api/v1/models/{SMALL}/unload_candidate", headers=AUTH).status
            self.assertEqual(code, 200)
            job, calls = self.run_job(base, say, reset=False)
            self.assertEqual(job["status"], "done", job.get("error"))
            self.assertIn(("POST", "/v1/audio/speech"), [c[:2] for c in calls])
            request(self.h.fake_url, "POST", f"/api/v1/models/{SMALL}/stop", {}, headers=AUTH)
            self.assertNotIn(SMALL, self.fake.state.running)
            self.forget(base)

    def test_the_fake_answers_a_model_that_is_not_loaded_like_the_device(self):
        # tested on 0.1.39: 404 for a named model that isn't running, 503 for `default` when no such model runs
        request(self.h.fake_url, "POST", "/__reset")
        gone = {"model": SMALL, "input": "hi", "prompt": "hi", "messages": [{"role": "user", "content": "hi"}]}
        for path in ("/v1/chat/completions", "/v1/audio/speech", "/v1/embeddings", "/v1/images/generations"):
            r = request(self.h.fake_url, "POST", path, gone, headers=AUTH)
            self.assertEqual(r.status, 404, path)
            self.assertEqual(r.json["error"], {"message": f'"{SMALL}" is not loaded.', "type": "model_not_found", "code": 404})
        request(self.h.fake_url, "POST", "/api/v1/models/unload_all", {}, headers=AUTH)
        r = request(self.h.fake_url, "POST", "/v1/chat/completions", {**gone, "model": "default"}, headers=AUTH)
        self.assertEqual((r.status, r.json["error"]["type"]), (503, "service_unavailable"))
        r = request(self.h.fake_url, "POST", "/v1/ocr", {"input": "x"}, headers=AUTH)
        self.assertEqual((r.status, r.json["error"]["message"]), (503, "No suitable model is currently running."))

    def test_the_fake_pauses_and_deletes_downloads_like_the_device(self):
        # tested on 0.1.39 (2026-11-22): a second pause is a 200 saying so; DELETE of a download that isn't paused works at once
        request(self.h.fake_url, "POST", "/__reset")
        self.flags(download_hold=True)
        api = f"/api/v1/models/{SMALL}"
        request(self.h.fake_url, "POST", api + "/download", headers=AUTH)
        r = request(self.h.fake_url, "POST", api + "/pause_download", headers=AUTH)
        self.assertEqual((r.status, r.json["status"], r.json["message"]), (200, "download_stop", f"Model {SMALL} download paused successfully."))
        r = request(self.h.fake_url, "POST", api + "/pause_download", headers=AUTH)
        self.assertEqual((r.status, r.json["status"], r.json["message"]), (200, "download_stop", f"Model {SMALL} is not currently downloading."))
        request(self.h.fake_url, "POST", api + "/download", headers=AUTH)  # resume
        r = request(self.h.fake_url, "DELETE", api, headers=AUTH)
        self.assertEqual((r.status, r.json["message"]), (200, f"Model {SMALL} deleted finish."))
        rows = request(self.h.fake_url, "GET", "/api/v1/models/?including_downloading=true", headers=AUTH).json["data"]
        self.assertNotIn(SMALL, [m["id"] for m in rows])


class DefaultCaches(ContractCase):
    """The production cache times (60 s for the device's free space and the store's facts); every other class runs with
    them off so a test can change what the fake reports."""
    extra_env = {"STUDIO_DISK_TTL": "60", "STUDIO_STORE_TTL": "60"}
    scenario, forget, flags = Models.scenario, Models.forget, Models.flags

    def test_free_space_is_cached_but_a_change_through_the_studio_shows_at_once(self):
        def steps(call):
            def free():
                return call("GET", "/api/store", record=False)[1]["disk"]["free_gb"]
            before = free()  # read when the scenario began, so it is cached now
            self.flags(disk_free_gb=before - 100)  # the device changed by itself: the studio still shows what it read before
            self.assertEqual(free(), before)
            request(self.h.fake_url, "POST", "/api/v1/models/Qwen%2FQwen3-4B-Instruct/stop", {}, headers=AUTH)
            self.assertEqual(call("POST", "/api/model/delete", {"id": CHAT, "confirm": CHAT})[0], 200)
            self.assertLess(free(), before - 90)  # a delete through the studio: read again, so the 100 GB less shows
        self.scenario(steps)


if __name__ == "__main__":
    unittest.main()
