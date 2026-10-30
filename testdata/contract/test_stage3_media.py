"""Stage 3 contract tests: image create/edit, OCR and the chat-model helpers, run as real jobs on both servers
against the fake Tiiny. Compares the finished job, the saved files and the requests each server made."""
import base64
import json
import time
import unittest

from harness import ContractCase, request  # first: puts testdata/ on the path
import fake_tiiny  # noqa: E402

PNG = "data:image/png;base64," + base64.b64encode(fake_tiiny.tiny_png()).decode()
# ids, times and file names (they contain the job id) differ per run
IGNORE = ("id", "created", "started", "finished", "seconds", "t", "outputs", "sources")


class Media(ContractCase):
    def run_job(self, base, req):
        request(self.h.fake_url, "POST", "/__reset")
        if req.get("load"):  # a model the job needs loaded first (the reset leaves only the chat model running)
            path = "/api/v1/models/" + req["load"].replace("/", "%2F") + "/launch"
            request(self.h.fake_url, "POST", path, {"model_id": req["load"]}, headers={"Authorization": "Bearer fake-key"})
            time.sleep(0.4)
            request(self.h.fake_url, "POST", "/__reset_log")
        if req.get("flags"):  # fake device behaviour for this run only (the reset above cleared the last)
            request(self.h.fake_url, "POST", "/__flags", req["flags"])
        req = {k: v for k, v in req.items() if k not in ("load", "flags")}
        job = request(base, "POST", "/api/jobs", req).json
        if "id" not in job:  # refused at submit (e.g. unknown kind)
            return job, []
        for _ in range(300):
            job = request(base, "GET", f"/api/jobs/{job['id']}").json
            if job.get("finished"):
                break
            time.sleep(0.05)
        log = json.loads(request(self.h.fake_url, "GET", "/__requests").body)
        return job, [(r["method"], r["path"], r["body"]) for r in log if r["path"] != "/"]

    def same_job(self, req):
        py, py_calls = self.run_job(self.h.py_url, req)
        rs, rs_calls = self.run_job(self.h.rs_url, req)
        self.assertEqual(strip(py), strip(rs))
        self.assertEqual(scrub(py_calls), scrub(rs_calls))
        return py, rs

    def files_equal(self, py, rs, key="outputs"):
        p, r = py["entry"][key], rs["entry"][key]
        self.assertEqual(len(p), len(r))
        for a, b in zip(p, r):
            self.assertEqual(a.rsplit("-", 1)[-1], b.rsplit("-", 1)[-1])  # "-0.png", "-src0.png"
            self.assertEqual(request(self.h.py_url, "GET", "/images/" + a).body, request(self.h.rs_url, "GET", "/images/" + b).body)

    def test_image_create_two(self):
        py, rs = self.same_job({"kind": "image", "model": "Qwen/Qwen-Image-2.1", "prompt": "RUST TEST duck", "n": 2, "seed": 7, "size": "512x512"})
        self.assertEqual(py["status"], "done")
        self.files_equal(py, rs)
        self.same(self.get("/api/history"), IGNORE)

    def test_image_warmup_retry(self):
        # the first two requests fail like a freshly loaded Qwen-Image; both servers retry and succeed
        def run(base):
            request(self.h.fake_url, "POST", "/__reset")
            request(self.h.fake_url, "POST", "/__flags", {"image_warmup": 2})
            job = request(base, "POST", "/api/jobs", {"kind": "image", "model": "Qwen/Qwen-Image-2.1", "prompt": "RUST TEST warm"}).json
            for _ in range(300):
                job = request(base, "GET", f"/api/jobs/{job['id']}").json
                if job.get("finished"):
                    break
                time.sleep(0.05)
            log = json.loads(request(self.h.fake_url, "GET", "/__requests").body)
            return job["status"], [r["path"] for r in log if r["path"] not in ("/", "/api/v1/sys/status")]
        py, rs = run(self.h.py_url), run(self.h.rs_url)
        self.assertEqual(py, rs)
        self.assertEqual(py, ("done", ["/v1/images/generations"] * 3))

    def test_cancel_running_job_discards_result(self):
        # Cancel while the device is still working: the job shows "stopping", then ends cancelled and nothing is saved.
        def run(base):
            request(self.h.fake_url, "POST", "/__reset")
            request(self.h.fake_url, "POST", "/__flags", {"delay": {"/v1/images/generations": 1.5}})
            job = request(base, "POST", "/api/jobs", {"kind": "image", "model": "Qwen/Qwen-Image-2.1", "prompt": "RUST TEST cancel"}).json
            time.sleep(0.4)
            request(base, "POST", f"/api/jobs/{job['id']}/cancel")
            mid = request(base, "GET", f"/api/jobs/{job['id']}").json
            for _ in range(100):
                job = request(base, "GET", f"/api/jobs/{job['id']}").json
                if job.get("finished"):
                    break
                time.sleep(0.05)
            time.sleep(0.5)  # the model is unloaded and loaded back in the background
            log = json.loads(request(self.h.fake_url, "GET", "/__requests").body)
            halts = [(r["method"], r["path"]) for r in log if r["path"].endswith(("/stop", "/launch", "/interrupt"))]
            return (mid["status"], mid.get("stopping"), job["status"], job["message"], len(request(base, "GET", "/api/history").json), halts)
        py, rs = run(self.h.py_url), run(self.h.rs_url)
        self.assertEqual(py, rs)
        q = "Qwen%2FQwen-Image-2.1"
        self.assertEqual(py, ("running", True, "cancelled", "Cancelled", 0,
                              [("POST", f"/api/v1/models/{q}/stop"), ("POST", f"/api/v1/models/{q}/launch")]))

    def test_image_edit(self):
        py, rs = self.same_job({"kind": "image", "model": "Qwen/Qwen-Image-2.1", "prompt": "RUST TEST hat", "images": [PNG], "steps": ""})
        self.assertEqual(py["entry"]["mode"], "edit")
        self.files_equal(py, rs, "sources")

    def test_image_from_references(self):
        py, rs = self.same_job({"kind": "image", "model": "Qwen/Qwen-Image-2.1", "prompt": "RUST TEST image 1 in a cafe",
                                "images": [PNG, PNG], "mode": "refs", "size": "1024x576"})
        self.assertEqual(py["entry"]["mode"], "refs")
        self.assertEqual(py["entry"]["params"]["size"], "1024x576")
        self.files_equal(py, rs, "sources")
        py, _ = self.same_job({"kind": "image", "model": "Qwen/Qwen-Image-2.1", "prompt": "RUST TEST fox", "images": [], "mode": "refs"})
        self.assertEqual(py["entry"]["mode"], "create")  # no pictures: a plain create

    def test_ocr(self):
        py, _ = self.same_job({"kind": "ocr", "model": "Fake/PP-OCRv6", "image": PNG, "ephemeral": True, "load": "Fake/PP-OCRv6"})
        self.assertEqual(py["status"], "done")
        self.same_job({"kind": "ocr", "model": "Fake/PP-OCRv6", "image": "nope"})
        self.same_job({"kind": "ocr", "model": "Fake/Not-Loaded", "image": PNG})

    def test_disk_space(self):
        job = {"kind": "image", "model": "Qwen/Qwen-Image-2.1", "prompt": "RUST TEST duck", "size": "512x512"}
        py, _ = self.same_job({**job, "flags": {"disk_free_gb": 1.5}})
        self.assertEqual(py["status"], "error")
        self.assertEqual(py["error"], "The Tiiny is almost out of space (1.5 GB free). "
                         "Free some up (delete models or files in the TiinyOS app) and try again.")
        py, _ = self.same_job({**job, "flags": {"disk_free_gb": 50}})
        self.assertEqual(py["status"], "done")
        py, _ = self.same_job({**job, "flags": {"no_disk": True}})  # unknown free space never blocks
        self.assertEqual(py["status"], "done")
        py, _ = self.same_job({"kind": "assist", "model": fake_tiiny.CHAT_MODEL, "task": "prompt", "text": "RUST TEST a fox",
                               "flags": {"disk_free_gb": 0.5}})  # chat and helpers aren't checked
        self.assertEqual(py["status"], "done")

    def test_disk_stats(self):
        for flags in ({}, {"disk_free_gb": 7.26}, {"no_disk": True}):
            request(self.h.fake_url, "POST", "/__reset")
            request(self.h.fake_url, "POST", "/__flags", flags)
            time.sleep(3.2)  # the stats answer is cached for 3 s on both servers
            py, rs = (r.json for r in self.get("/api/stats"))
            self.assertEqual((py["disk_free_gb"], py["disk_total_gb"]), (rs["disk_free_gb"], rs["disk_total_gb"]))
            if flags.get("no_disk"):
                self.assertIsNone(py["disk_free_gb"])
                self.assertIsNone(py["disk_total_gb"])
            else:
                self.assertEqual(py["disk_total_gb"], 921.1)
                self.assertEqual(py["disk_free_gb"], round(flags.get("disk_free_gb", 400), 1))

    def test_assist(self):
        model = fake_tiiny.CHAT_MODEL
        py, _ = self.same_job({"kind": "assist", "model": model, "task": "prompt", "text": "RUST TEST a fox"})
        self.assertEqual(py["entry"]["text"], "Fake reply.")
        self.same_job({"kind": "assist", "model": model, "task": "edit", "text": "hat", "context": "a duck"})
        self.same_job({"kind": "assist", "model": model, "task": "describe", "image": PNG})
        self.same_job({"kind": "assist", "model": model, "task": "lyrics", "text": "rain"})  # fake reply has no sections: error
        self.same_job({"kind": "assist", "model": model, "task": "nope", "text": "x"})
        self.same_job({"kind": "assist", "model": model, "task": "tags", "text": ""})


def strip(obj):
    if isinstance(obj, dict):
        return {k: strip(v) for k, v in obj.items() if k not in IGNORE}
    if isinstance(obj, list):
        return [strip(v) for v in obj]
    return obj


def scrub(calls):
    return [(m, p, strip(b)) for m, p, b in calls]


if __name__ == "__main__":
    unittest.main()
