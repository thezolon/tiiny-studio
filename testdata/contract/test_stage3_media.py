"""Stage 3 contract tests: image create/edit, OCR and the chat-model helpers, run as real jobs against the fake Tiiny.
Checks the finished job, the saved files and the requests the server made to the device."""
import base64
import time
import unittest

from harness import ContractCase, request  # first: puts testdata/ on the path
import fake_tiiny  # noqa: E402

PNG = "data:image/png;base64," + base64.b64encode(fake_tiiny.tiny_png()).decode()
# ids, times and file names (they contain the job id) differ per run
IGNORE = ("id", "created", "started", "finished", "seconds", "t", "outputs", "sources")
IMAGE = "Qwen/Qwen-Image-2.1"
CHAT = fake_tiiny.CHAT_MODEL
STATUS = ("GET", "/api/v1/sys/status")
GEN = ("POST", "/v1/images/generations")
CHAT_CALLS = [("GET", "/api/v1/models/running"), ("GET", "/v1/models"), ("POST", "/v1/chat/completions")]
USAGE = {"prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8}


def image_entry(mode="create", **params):
    return {"kind": "image", "mode": mode, "params": {"model": IMAGE, "n": 1, "seed": None, **params}}


def assist_entry(task):
    return {"kind": "assist", "params": {"model": CHAT, "task": task}, "text": "Fake reply.", "usage": USAGE}


class Media(ContractCase):
    def view(self, job):
        return strip({k: job.get(k) for k in ("status", "message", "error", "entry")})

    def shape(self, r):
        return (r["method"], r["path"], r["body"])

    def comparable(self, calls):
        return scrub(calls)

    def test_image_create_two(self):
        job, _ = self.check_job({"kind": "image", "model": IMAGE, "prompt": "RUST TEST duck", "n": 2, "seed": 7, "size": "512x512"},
                                entry=image_entry(prompt="RUST TEST duck", size="512x512", seed=7, n=2), calls=[STATUS, GEN, GEN])
        # the saved files are the fake's pictures (once the tags the studio writes into them are left out, #58)
        self.assertEqual(self.file_media(job), [("0.png", fake_tiiny.tiny_png()), ("1.png", fake_tiiny.tiny_png())])
        newest = self.get("/api/history").json[0]  # the gallery has the entry, newest first
        self.assertEqual((newest["kind"], newest["mode"], newest["params"], newest["outputs"]),
                         ("image", "create", job["entry"]["params"], job["entry"]["outputs"]))

    def test_image_warmup_retry(self):
        # the first two requests fail like a freshly loaded Qwen-Image; the server retries and succeeds
        request(self.h.fake_url, "POST", "/__reset")
        request(self.h.fake_url, "POST", "/__flags", {"image_warmup": 2})
        job, calls = self.finish(request(self.h.rs_url, "POST", "/api/jobs", {"kind": "image", "model": IMAGE, "prompt": "RUST TEST warm"}).json)
        self.assertEqual((job["status"], [p for _, p, *_ in calls if p != "/api/v1/sys/status"]), ("done", ["/v1/images/generations"] * 3))

    def test_cancel_running_job_discards_result(self):
        # Cancel while the device is still working: the job shows "stopping", then ends cancelled and nothing is saved.
        base = self.h.rs_url
        request(self.h.fake_url, "POST", "/__reset")
        request(self.h.fake_url, "POST", "/__flags", {"delay": {"/v1/images/generations": 1.5}})
        job = request(base, "POST", "/api/jobs", {"kind": "image", "model": IMAGE, "prompt": "RUST TEST cancel"}).json
        self.wait_sent("POST", "/v1/images/generations")  # the device is working on it
        request(base, "POST", f"/api/jobs/{job['id']}/cancel")
        mid = request(base, "GET", f"/api/jobs/{job['id']}").json
        job, _ = self.finish(job)
        self.wait_sent("POST", "/api/v1/models/Qwen%2FQwen-Image-2.1/launch")  # unloaded and loaded back in the background
        halts = [(m, p) for m, p, _ in self.calls() if p.endswith(("/stop", "/launch", "/interrupt"))]
        q = "Qwen%2FQwen-Image-2.1"
        self.assertEqual((mid["status"], mid.get("stopping"), job["status"], job["message"], len(request(base, "GET", "/api/history").json), halts),
                         ("running", True, "cancelled", "Cancelled", 0, [("POST", f"/api/v1/models/{q}/stop"), ("POST", f"/api/v1/models/{q}/launch")]))

    def test_image_edit(self):
        job, _ = self.check_job({"kind": "image", "model": IMAGE, "prompt": "RUST TEST hat", "images": [PNG], "steps": ""},
                                entry=image_entry("edit", prompt="RUST TEST hat"), calls=[STATUS, GEN])
        self.assertEqual(self.file_media(job, "sources"), [("src0.png", fake_tiiny.tiny_png())])

    def test_image_from_references(self):
        job, _ = self.check_job({"kind": "image", "model": IMAGE, "prompt": "RUST TEST image 1 in a cafe", "images": [PNG, PNG], "mode": "refs", "size": "1024x576"},
                                entry=image_entry("refs", prompt="RUST TEST image 1 in a cafe", size="1024x576"), calls=[STATUS, GEN])
        self.assertEqual(self.file_media(job, "sources"), [("src0.png", fake_tiiny.tiny_png()), ("src1.png", fake_tiiny.tiny_png())])
        self.check_job({"kind": "image", "model": IMAGE, "prompt": "RUST TEST fox", "images": [], "mode": "refs"},
                       entry=image_entry("create", prompt="RUST TEST fox"), calls=[STATUS, GEN])  # no pictures: a plain create

    def test_ocr(self):
        job, _ = self.check_job({"kind": "ocr", "model": "Fake/PP-OCRv6", "image": PNG, "ephemeral": True, "load": "Fake/PP-OCRv6"},
                                entry={"kind": "ocr", "params": {"model": "Fake/PP-OCRv6"}, "text": "FAKE OCR", "lines": [{"text": "FAKE OCR", "score": 0.99}]},
                                calls=[STATUS, ("GET", "/api/v1/models/running"), ("POST", "/v1/ocr")])
        self.check_job({"kind": "ocr", "model": "Fake/PP-OCRv6", "image": "nope"}, "error", error="Expected a base64 data URI", calls=[STATUS])
        self.check_job({"kind": "ocr", "model": "Fake/Not-Loaded", "image": PNG}, "error", error="Not-Loaded isn't loaded. Load it from the Models panel.",
                       calls=[STATUS, ("GET", "/api/v1/models/running")])

    def test_disk_space(self):
        job = {"kind": "image", "model": IMAGE, "prompt": "RUST TEST duck", "size": "512x512"}
        self.check_job({**job, "flags": {"disk_free_gb": 1.5}}, "error", calls=[STATUS],
                       error="The Tiiny is almost out of space (1.5 GB free). "
                             "Free some up (delete a model you don't use in Models \u2192 Installed, or files in the TiinyOS app) and try again.")
        done = image_entry(prompt="RUST TEST duck", size="512x512")
        self.check_job({**job, "flags": {"disk_free_gb": 50}}, entry=done, calls=[STATUS, GEN])
        self.check_job({**job, "flags": {"no_disk": True}}, entry=done, calls=[STATUS, GEN])  # unknown free space never blocks
        self.check_job({"kind": "assist", "model": CHAT, "task": "prompt", "text": "RUST TEST a fox", "flags": {"disk_free_gb": 0.5}},
                       entry=assist_entry("prompt"), calls=CHAT_CALLS)  # chat and helpers aren't checked

    def test_disk_stats(self):
        for flags in ({}, {"disk_free_gb": 7.26}, {"no_disk": True}):
            request(self.h.fake_url, "POST", "/__reset")
            request(self.h.fake_url, "POST", "/__flags", flags)
            time.sleep(3.2)  # the stats answer is cached for 3 s
            stats = self.get("/api/stats").json
            if flags.get("no_disk"):
                self.assertIsNone(stats["disk_free_gb"])
                self.assertIsNone(stats["disk_total_gb"])
            else:
                self.assertEqual(stats["disk_total_gb"], 921.1)
                self.assertEqual(stats["disk_free_gb"], round(flags.get("disk_free_gb", 400), 1))

    def test_assist(self):
        model = CHAT
        job, _ = self.check_job({"kind": "assist", "model": model, "task": "prompt", "text": "RUST TEST a fox"}, entry=assist_entry("prompt"), calls=CHAT_CALLS)
        self.assertEqual(job["entry"]["text"], "Fake reply.")
        self.check_job({"kind": "assist", "model": model, "task": "edit", "text": "hat", "context": "a duck"}, entry=assist_entry("edit"), calls=CHAT_CALLS)
        self.check_job({"kind": "assist", "model": model, "task": "describe", "image": PNG}, entry=assist_entry("describe"), calls=CHAT_CALLS)
        # the fake's reply has no sections: an error
        self.check_job({"kind": "assist", "model": model, "task": "lyrics", "text": "rain"}, "error", calls=CHAT_CALLS,
                       error="The model didn't return lyrics with [verse]/[chorus] sections. Try again.")
        self.check_job({"kind": "assist", "model": model, "task": "nope", "text": "x"}, "error", error="unknown helper nope", calls=[])
        self.check_job({"kind": "assist", "model": model, "task": "tags", "text": ""}, "error", error="Nothing to work from: type something first", calls=[])


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
