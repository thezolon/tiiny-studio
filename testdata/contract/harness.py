"""Contract-test support: the fake Tiiny and the Rust server (the local binary, or the Docker image).
Subclass ContractCase; self.get/self.post send a request to the Rust server and return its Response, which the test
checks against explicit expectations (self.expect() for a status and a JSON body, self.run_job() for a job).
The Rust binary is built with `cargo build` in rust/ when missing (tests skip if cargo isn't there, unless
STUDIO_REQUIRE_RUST=1, which CI sets: then a missing Rust server is an error, never a silent skip)."""
import json
import os
import re
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import time
import unittest
import urllib.error
import urllib.request
from collections import namedtuple
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "testdata"))
import fake_tiiny  # noqa: E402

KEY = "fake-key"
AUTH = {"Authorization": f"Bearer {KEY}"}
# STUDIO_RUST_IMAGE=tiiny-studio:rust runs the server as that Docker image instead of the local binary; it then reaches
# the fake Tiiny as host.docker.internal (the fake listens on all interfaces for the test).
IMAGE = os.environ.get("STUDIO_RUST_IMAGE", "")
RUST_BIN = ROOT / "rust" / "target" / "debug" / ("tiiny-studio-server.exe" if os.name == "nt" else "tiiny-studio-server")
# Keys that differ between runs: ids, timestamps.
IGNORE = ("id", "device_id", "t", "created", "created_at", "updated", "updated_at", "timestamp")


_ANY = object()  # "no body given" for ContractCase.expect


class Response(namedtuple("Response", "status headers body")):
    @property
    def json(self):
        return json.loads(self.body)


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def request(base, method, path, body=None, headers=None, timeout=30):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(base + path, data, method=method, headers=headers or {})
    if data is not None:
        req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return Response(r.status, {k.title(): v for k, v in r.headers.items()}, r.read())
    except urllib.error.HTTPError as e:
        return Response(e.code, {k.title(): v for k, v in e.headers.items()}, e.read())


def strip(obj, ignore):
    if isinstance(obj, dict):
        return {k: strip(v, ignore) for k, v in obj.items() if k not in ignore}
    if isinstance(obj, list):
        return [strip(v, ignore) for v in obj]
    return obj


def rust_binary():
    """Path of the Rust server, (re)built first so tests never run a stale binary. Raises SkipTest without cargo."""
    if shutil.which("cargo"):
        r = subprocess.run(["cargo", "build", "-p", "studio-server", *(["--locked"] if os.environ.get("CI") else [])], cwd=ROOT / "rust", capture_output=True, text=True)
        if r.returncode:
            raise RuntimeError("cargo build failed:\n" + r.stdout[-2000:] + r.stderr[-2000:])
    elif not RUST_BIN.exists():
        msg = "Rust server not built and cargo isn't installed (cd rust && cargo build)"
        if os.environ.get("STUDIO_REQUIRE_RUST"):
            raise RuntimeError(msg)
        raise unittest.SkipTest(msg)
    return RUST_BIN


class Harness:
    def __init__(self, extra_env=None):
        self.extra_env = extra_env or {}  # added to the server's environment (after the harness's own settings)
        self.procs, self.tmp, self.logs = [], [], []
        self.fake, self.fake_url = None, None
        self.rs_url = None
        self.data = None  # the server's data folder
        self.containers = []

    def _spawn(self, cmd, port, data_dir):
        """Start a server on port and wait for it. If it dies at once (the port was taken between free_port() and its
        bind) it is started again on a new port; its output goes to a temp file whose tail is in the error."""
        env = {**os.environ, "PORT": str(port), "DATA_DIR": str(data_dir), "TIINY_URL": self.fake_url, "TIINY_KEY": KEY,
               "STUDIO_WARMUP_WAIT": "0.2", "STUDIO_DOCKER": "1",
               "STUDIO_UPDATE_URL": f"{self.fake_url}/__release",  # "GitHub" is the fake
               # no free-space or store cache, so a test can change what the fake reports; as the Docker image sets it
               "STUDIO_DISK_TTL": "0", "STUDIO_STORE_TTL": "0", **self.extra_env}
        for k in ("STUDIO_PASSWORD", "STUDIO_BIND", "HOST"):
            env.pop(k, None)
        for attempt in range(3):
            env["PORT"] = str(port)
            log = tempfile.NamedTemporaryFile(prefix="studio-contract-", suffix=".log", delete=False)
            self.logs.append(log.name)
            p = subprocess.Popen(cmd, cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT)
            log.close()
            self.procs.append(p)
            url = f"http://127.0.0.1:{port}"
            for _ in range(100):
                if p.poll() is not None:
                    break
                try:
                    urllib.request.urlopen(url + "/", timeout=1).read(1)
                    return url
                except (urllib.error.URLError, OSError):
                    time.sleep(0.1)
            else:
                raise RuntimeError(f"{cmd[-1]} didn't answer on {url}\n{self.tail(log.name)}")
            if attempt == 2:
                raise RuntimeError(f"{cmd[-1]} exited with {p.returncode}\n{self.tail(log.name)}")
            port = free_port()  # most likely the port was taken: try another
        raise AssertionError("unreachable")

    @staticmethod
    def tail(path):
        try:
            return "--- server output (tail) ---\n" + Path(path).read_text("utf-8", "replace")[-2000:]
        except OSError:
            return ""

    def _docker(self, port, data_dir):
        name = f"studio-contract-{port}"
        env = ["-e", f"TIINY_URL={self.fake_url}", "-e", f"TIINY_KEY={KEY}", "-e", "STUDIO_WARMUP_WAIT=0.2",
               "-e", "STUDIO_BIND=127.0.0.1", "-e", "STUDIO_DISK_TTL=0", "-e", "STUDIO_STORE_TTL=0",
               "-e", f"STUDIO_UPDATE_URL={self.fake_url}/__release"]  # as compose.yaml sets it: published on this computer only
        subprocess.run(["docker", "run", "-d", "--rm", "--name", name, "-p", f"127.0.0.1:{port}:7860", *env,
                        "-v", f"{data_dir}:/data", IMAGE], check=True, capture_output=True)
        self.containers.append(name)
        url = f"http://127.0.0.1:{port}"
        for _ in range(150):
            try:
                urllib.request.urlopen(url + "/", timeout=1).read(1)
                return url
            except (urllib.error.URLError, OSError):
                time.sleep(0.2)
        raise RuntimeError(f"container {name} didn't answer on {url}")

    def start(self):
        binary = None if IMAGE else rust_binary()
        try:
            if IMAGE:
                self.fake, _ = fake_tiiny.start(key=KEY, host="0.0.0.0")
                self.fake_url = f"http://host.docker.internal:{self.fake.server_address[1]}"
            else:
                self.fake, self.fake_url = fake_tiiny.start(key=KEY)
            self.data = Path(tempfile.mkdtemp(prefix="studio-rs-"))
            self.tmp.append(self.data)
            self.rs_url = self._docker(free_port(), self.data) if IMAGE else self._spawn([str(binary)], free_port(), self.data)
        except BaseException:
            self.stop()
            raise
        return self

    def stop(self):
        for p in self.procs:
            p.terminate()
        for p in self.procs:
            try:
                p.wait(10)
            except subprocess.TimeoutExpired:
                p.kill()
        for name in self.containers:
            subprocess.run(["docker", "rm", "-f", name], capture_output=True)
        self.containers = []
        if self.fake:
            self.fake.shutdown()
            self.fake.server_close()
        for d in self.tmp:
            shutil.rmtree(d, ignore_errors=True)
        for f in self.logs:
            try:
                os.unlink(f)
            except OSError:
                pass
        self.procs, self.tmp, self.logs, self.fake = [], [], [], None


class ContractCase(unittest.TestCase):
    """One harness per test class; self.fake is the fake Tiiny (its request log is self.fake.state.requests)."""

    extra_env = {}  # a subclass can set e.g. {"STUDIO_DISK_TTL": "60"} to run with the production cache times

    @classmethod
    def setUpClass(cls):
        cls.h = Harness(cls.extra_env).start()
        cls.fake = cls.h.fake

    @classmethod
    def tearDownClass(cls):
        unrouted = list(cls.fake.state.unrouted)
        cls.h.stop()
        # every device call a server makes must have a route in the fake (and so be documented): not a silent 404
        if unrouted:
            raise AssertionError("the fake Tiiny had no route for: " + ", ".join(dict.fromkeys(unrouted)))

    def get(self, path, **kw):
        return request(self.h.rs_url, "GET", path, **kw)

    def post(self, path, body=None, **kw):
        return request(self.h.rs_url, "POST", path, body, **kw)

    def delete(self, path, **kw):
        return request(self.h.rs_url, "DELETE", path, **kw)

    def post_raw(self, path, data, headers):
        req = urllib.request.Request(self.h.rs_url + path, data, method="POST", headers=headers)
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                return Response(r.status, {k.title(): v for k, v in r.headers.items()}, r.read())
        except urllib.error.HTTPError as e:
            return Response(e.code, {k.title(): v for k, v in e.headers.items()}, e.read())

    def expect(self, r, status, body=_ANY, ignore=IGNORE):
        """The answer has this status and, if given, this JSON body (equal once the ignored keys - ids, timestamps, which
        differ per run - are left out at any depth). Returns the answer's JSON (or None if it had no body)."""
        self.assertEqual(r.status, status, r.body[:300])
        if body is _ANY:
            return None
        self.assertEqual(strip(r.json, ignore), strip(body, ignore))
        return r.json

    # ---------- jobs against the fake ----------
    def wait_until(self, cond, what, timeout=10):
        """Poll cond() until it's truthy and return that; on timeout, fail saying what never happened (what may be a
        function, for a message that reports the last state)."""
        end = time.monotonic() + timeout
        while not (v := cond()):
            if time.monotonic() > end:
                self.fail(f"timed out after {timeout} s waiting for {what() if callable(what) else what}")
            time.sleep(0.05)
        return v

    def load_on_fake(self, *models):
        """Load models on the fake one after another (the order of its running list decides what a swap unloads) and wait
        until each is running (a launch takes the fake's launch_delay)."""
        for m in models:
            request(self.h.fake_url, "POST", "/api/v1/models/" + m.replace("/", "%2F") + "/launch", {"model_id": m}, headers=AUTH)
            self.wait_until(lambda m=m: m in self.fake.state.running, "the fake to run " + m)

    def shape(self, r):
        """One logged device request as a test compares it; a test class can keep less."""
        return (r["method"], r["path"], r["query"], r["body"], r.get("multipart"))

    def calls(self):
        """The requests the fake has logged since the last reset (page loads left out)."""
        return [self.shape(r) for r in json.loads(request(self.h.fake_url, "GET", "/__requests").body) if r["path"] != "/"]

    def wait_sent(self, method, path, timeout=10):
        """Wait until the studio has sent this request to the fake (for work it does in the background)."""
        self.wait_until(lambda: any(c[:2] == (method, path) for c in self.calls()), f"the studio to send {method} {path}", timeout)

    def run_job(self, req, reset=True, timeout=60):
        """Run one job to the end; returns (job, requests made to the fake). `load` loads models first (a name or a list),
        `flags` sets fake behaviour (see fake_tiiny.py); reset=False keeps the fake's state (with a fresh log).
        self.last_calls keeps the requests."""
        req = dict(req)
        load, flags = req.pop("load", None), req.pop("flags", None)
        if reset:
            request(self.h.fake_url, "POST", "/__reset")
            if load:
                self.load_on_fake(*([load] if isinstance(load, str) else load))
        request(self.h.fake_url, "POST", "/__reset_log")
        if flags:
            request(self.h.fake_url, "POST", "/__flags", flags)
        job = request(self.h.rs_url, "POST", "/api/jobs", req).json
        if "id" not in job:  # refused at submit (e.g. an unknown kind)
            self.last_calls = []
            return job, []
        return self.finish(job, timeout)

    def finish(self, job, timeout=60):
        """Poll a job until it ends; returns (job, the requests the fake logged)."""
        last = {}

        def ended():
            last.clear()
            last.update(request(self.h.rs_url, "GET", f"/api/jobs/{job['id']}").json)
            return last.get("finished")

        self.wait_until(ended, lambda: f"job {job['id']} ({job.get('kind')}) to end; last: {last.get('status')} {last.get('message')!r}", timeout)
        self.last_calls = self.calls()
        return dict(last), self.last_calls

    def wait_job(self, job, cond, what, timeout=20):
        """Poll a job until cond(job) holds (e.g. it reached a stage) and return it."""
        def check():
            j = request(self.h.rs_url, "GET", f"/api/jobs/{job['id']}").json
            return j if cond(j) else None

        return self.wait_until(check, what, timeout)

    def view(self, job):
        """A job as a test compares it: without what differs per run (ids, times); a test class sets its own."""
        return strip(job, IGNORE)

    def comparable(self, calls):
        """The device requests as a test compares them; a test class can drop what legitimately differs."""
        return calls

    def check_job(self, req, job, calls=None, **kw):
        """Run a job; its result (see view) must equal `job` and, if given, the requests it made to the fake (see
        comparable) must equal `calls`. Returns (job, calls) as run_job does."""
        got, made = self.run_job(req, **kw)
        self.assertEqual(self.view(got), job)
        if calls is not None:
            self.assertEqual(self.comparable(made), calls)
        return got, made

    def file_media(self, job, key="outputs"):
        """The saved files of a finished job as (name suffix, media bytes without the tags the server writes, #58)."""
        return [(name_suffix(n), media(n, request(self.h.rs_url, "GET", "/images/" + n).body)) for n in job["entry"][key]]


def name_suffix(name):
    """What a result's file name says beyond its date, time and job id: `2026-11-18_2215_<id>-0.png` -> `0.png` (#58)."""
    return re.sub(r"^(?:\d{4}-\d{2}-\d{2}_\d{4}_)?[0-9a-f]+-", "", name)


def media(name, b):
    """A file's media without the tags the Rust server writes into it (#58): PNG text chunks, an MP3's ID3 tag, and for
    an MP4 its media data box."""
    if name.endswith(".png") and b[:8] == b"\x89PNG\r\n\x1a\n":
        out, i = [b[:8]], 8
        while i + 8 <= len(b):
            n = struct.unpack(">I", b[i:i + 4])[0]
            if b[i + 4:i + 8] not in (b"tEXt", b"iTXt", b"zTXt"):
                out.append(b[i:i + 12 + n])
            i += 12 + n
        return b"".join(out)
    if name.endswith(".mp3") and b[:3] == b"ID3":
        return b[10 + sum(x << (7 * (3 - k)) for k, x in enumerate(b[6:10])):]
    if name.endswith(".mp4"):
        i = 0
        while i + 8 <= len(b):
            n = struct.unpack(">I", b[i:i + 4])[0]
            if b[i + 4:i + 8] == b"mdat" or n < 8:
                return b[i:]
            i += n
    return b
