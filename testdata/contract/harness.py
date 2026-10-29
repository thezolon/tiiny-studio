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
               "PYTHONUNBUFFERED": "1", "STUDIO_WARMUP_WAIT": "0.2", "STUDIO_DOCKER": "1"}  # as the Docker image sets it
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
               "-e", "STUDIO_BIND=127.0.0.1"]  # as compose.yaml sets it: published on this computer only
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
