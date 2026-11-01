"""Contract-test support: the fake Tiiny, the Python server and the Rust server, side by side.
Subclass ContractCase; self.get/self.post send the same request to both servers, self.same() compares.
The Rust binary is built with `cargo build` in rust/ when missing (tests skip if cargo isn't there)."""
import json
import os
import shutil
import socket
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
# STUDIO_RUST_IMAGE=tiiny-studio:rust runs the Rust side as that Docker image instead of the local binary; both
# servers then reach the fake Tiiny as host.docker.internal (it listens on all interfaces for the test).
IMAGE = os.environ.get("STUDIO_RUST_IMAGE", "")
RUST_BIN = ROOT / "rust" / "target" / "debug" / ("tiiny-studio-server.exe" if os.name == "nt" else "tiiny-studio-server")
# Keys that differ between runs and servers: ids, timestamps.
IGNORE = ("id", "t", "created", "created_at", "updated", "updated_at", "timestamp")


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
        r = subprocess.run(["cargo", "build", "-p", "studio-server"], cwd=ROOT / "rust", capture_output=True, text=True)
        if r.returncode:
            raise RuntimeError("cargo build failed:\n" + r.stdout[-2000:] + r.stderr[-2000:])
    elif not RUST_BIN.exists():
        raise unittest.SkipTest("Rust server not built and cargo isn't installed (cd rust && cargo build)")
    return RUST_BIN


class Harness:
    def __init__(self):
        self.procs, self.tmp = [], []
        self.fake, self.fake_url = None, None
        self.py_url = self.rs_url = None
        self.containers = []

    def _spawn(self, cmd, port, data_dir):
        env = {**os.environ, "PORT": str(port), "DATA_DIR": str(data_dir), "TIINY_URL": self.fake_url, "TIINY_KEY": KEY,
               "PYTHONUNBUFFERED": "1", "STUDIO_WARMUP_WAIT": "0.2", "STUDIO_DOCKER": "1",
               "STUDIO_UPDATE_URL": f"{self.fake_url}/__release",  # "GitHub" is the fake
               "STUDIO_DISK_TTL": "0"}  # no free-space cache, so a test can change what the fake reports  # as the Docker image sets it
        for k in ("STUDIO_PASSWORD", "STUDIO_BIND", "HOST"):
            env.pop(k, None)
        p = subprocess.Popen(cmd, cwd=ROOT, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.procs.append(p)
        url = f"http://127.0.0.1:{port}"
        for _ in range(100):
            if p.poll() is not None:
                raise RuntimeError(f"{cmd[-1]} exited with {p.returncode}")
            try:
                urllib.request.urlopen(url + "/", timeout=1).read(1)
                return url
            except (urllib.error.URLError, OSError):
                time.sleep(0.1)
        raise RuntimeError(f"{cmd[-1]} didn't answer on {url}")

    def _docker(self, port, data_dir):
        name = f"studio-contract-{port}"
        env = ["-e", f"TIINY_URL={self.fake_url}", "-e", f"TIINY_KEY={KEY}", "-e", "STUDIO_WARMUP_WAIT=0.2",
               "-e", "STUDIO_BIND=127.0.0.1", "-e", "STUDIO_DISK_TTL=0",
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
            dirs = [Path(tempfile.mkdtemp(prefix=f"studio-{n}-")) for n in ("py", "rs")]
            self.tmp += dirs
            self.py_url = self._spawn([sys.executable, str(ROOT / "server.py")], free_port(), dirs[0])
            self.rs_url = self._docker(free_port(), dirs[1]) if IMAGE else self._spawn([str(binary)], free_port(), dirs[1])
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
        self.procs, self.tmp, self.fake = [], [], None


class ContractCase(unittest.TestCase):
    """One harness per test class; self.fake is the fake Tiiny (its request log is self.fake.state.requests)."""

    @classmethod
    def setUpClass(cls):
        cls.h = Harness().start()
        cls.fake = cls.h.fake

    @classmethod
    def tearDownClass(cls):
        cls.h.stop()

    def get(self, path, **kw):
        return request(self.h.py_url, "GET", path, **kw), request(self.h.rs_url, "GET", path, **kw)

    def post(self, path, body=None, **kw):
        return request(self.h.py_url, "POST", path, body, **kw), request(self.h.rs_url, "POST", path, body, **kw)

    def post_raw(self, path, data, headers):
        def send(base):
            req = urllib.request.Request(base + path, data, method="POST", headers=headers)
            try:
                with urllib.request.urlopen(req, timeout=30) as r:
                    return Response(r.status, {k.title(): v for k, v in r.headers.items()}, r.read())
            except urllib.error.HTTPError as e:
                return Response(e.code, {k.title(): v for k, v in e.headers.items()}, e.read())
        return send(self.h.py_url), send(self.h.rs_url)

    def same(self, pair, ignore=IGNORE):
        """Same status code and equal JSON bodies, ignoring the given keys (at any depth)."""
        py, rs = pair
        self.assertEqual(py.status, rs.status, f"status: python {py.status}, rust {rs.status}")
        # in Docker the studio correctly can't open a folder on this computer
        ignore = tuple(ignore) + (("can_open_folder",) if IMAGE else ())
        self.assertEqual(strip(py.json, ignore), strip(rs.json, ignore))

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

    def run_job(self, base, req, reset=True):
        """Run one job to the end; returns (job, requests made to the fake). `load` loads models first (a name or a list),
        `flags` sets fake behaviour (see fake_tiiny.py); reset=False keeps the fake's state (with a fresh log)."""
        req = dict(req)
        load, flags = req.pop("load", None), req.pop("flags", None)
        if reset:
            request(self.h.fake_url, "POST", "/__reset")
            if load:
                self.load_on_fake(*([load] if isinstance(load, str) else load))
        request(self.h.fake_url, "POST", "/__reset_log")
        if flags:
            request(self.h.fake_url, "POST", "/__flags", flags)
        job = request(base, "POST", "/api/jobs", req).json
        if "id" not in job:  # refused at submit (e.g. an unknown kind)
            return job, []
        return self.finish(base, job)

    def finish(self, base, job, timeout=60):
        """Poll a job until it ends; returns (job, the requests the fake logged)."""
        last = {}

        def ended():
            last.clear()
            last.update(request(base, "GET", f"/api/jobs/{job['id']}").json)
            return last.get("finished")

        self.wait_until(ended, lambda: f"job {job['id']} ({job.get('kind')}) to end; last: {last.get('status')} {last.get('message')!r}", timeout)
        return dict(last), self.calls()

    def wait_job(self, base, job, cond, what, timeout=20):
        """Poll a job until cond(job) holds (e.g. it reached a stage) and return it."""
        def check():
            j = request(base, "GET", f"/api/jobs/{job['id']}").json
            return j if cond(j) else None

        return self.wait_until(check, what, timeout)

    def view(self, job):
        """A job as same_job compares it: without what differs per run (ids, times); a test class sets its own."""
        return strip(job, IGNORE)

    def comparable(self, calls):
        """The device requests as same_job compares them; a test class can drop what legitimately differs."""
        return calls

    def same_job(self, req, **kw):
        """Run a job on both servers; same job result and same device requests. self.last_calls keeps Rust's."""
        py, py_calls = self.run_job(self.h.py_url, req, **kw)
        rs, rs_calls = self.run_job(self.h.rs_url, req, **kw)
        self.assertEqual(self.view(py), self.view(rs))
        self.assertEqual(self.comparable(py_calls), self.comparable(rs_calls))
        self.last_calls = rs_calls
        return py, rs

    def files_equal(self, py, rs, key="outputs"):
        """Both servers saved the same files (same suffix, same bytes)."""
        p, r = py["entry"][key], rs["entry"][key]
        self.assertEqual(len(p), len(r))
        for a, b in zip(p, r, strict=True):
            self.assertEqual(a.split("-", 1)[-1], b.split("-", 1)[-1])  # "-0.png", "-speech.wav"
            self.assertEqual(request(self.h.py_url, "GET", "/images/" + a).body, request(self.h.rs_url, "GET", "/images/" + b).body)
