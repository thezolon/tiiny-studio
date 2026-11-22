"""Shared by the browser checks (a11y_check.py, layout_check.py, model_store_ui_check.py): starts the studio server they
drive and collects the page's JavaScript errors.
  --server rust    (default) the Rust server that ships in the installers and the Docker image (built with cargo if needed)
  --server python  the Python reference server (server.py)
The server's output goes to a temp file whose tail is printed when the server doesn't start."""
import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "testdata" / "contract"))
import harness  # noqa: E402


def server_kind(argv=None):
    """The --server choice, removed from argv (default: rust)."""
    argv = sys.argv if argv is None else argv
    kind = "rust"
    for i, a in enumerate(argv):
        if a == "--server" and i + 1 < len(argv):
            kind = argv[i + 1]
            del argv[i:i + 2]
            break
        if a.startswith("--server="):
            kind = a.split("=", 1)[1]
            del argv[i]
            break
    if kind not in ("rust", "python"):
        sys.exit(f"--server must be rust or python, not {kind!r}")
    return kind


class Server:
    def __init__(self, kind, data, device, update_url=None):
        self.kind, self.port = kind, harness.free_port()
        self.base = f"http://127.0.0.1:{self.port}"
        env = {**os.environ, "DATA_DIR": str(data), "TIINY_URL": device, "TIINY_KEY": "fake-key", "PORT": str(self.port),
               "TIINY_NO_BROWSER": "1"}
        if update_url:
            env["STUDIO_UPDATE_URL"] = update_url
        for k in ("STUDIO_DOCKER", "STUDIO_PASSWORD", "STUDIO_BIND", "HOST"):  # the update check is on by default outside Docker
            env.pop(k, None)
        cmd = [sys.executable, str(ROOT / "server.py")] if kind == "python" else [str(harness.rust_binary())]
        self.log = tempfile.NamedTemporaryFile(prefix="uicheck-server-", suffix=".log", delete=False)
        self.proc = subprocess.Popen(cmd, cwd=ROOT, env=env, stdout=self.log, stderr=subprocess.STDOUT)
        for _ in range(150):
            if self.proc.poll() is not None:
                self.fail(f"the {kind} server exited with {self.proc.returncode}")
            try:
                urllib.request.urlopen(self.base + "/api/settings", timeout=2).read(1)
                return
            except OSError:
                time.sleep(0.2)
        self.fail(f"the {kind} server didn't answer on {self.base}")

    def tail(self):
        self.log.flush()
        return Path(self.log.name).read_text("utf-8", "replace")[-2000:]

    def fail(self, msg):
        tail = self.tail()
        self.stop()
        sys.exit(f"{msg}\n--- server output (tail) ---\n{tail}")

    def post(self, path, body):
        req = urllib.request.Request(self.base + path, json.dumps(body).encode(), {"Content-Type": "application/json"})
        return urllib.request.urlopen(req, timeout=30).read()

    def stop(self):
        if self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(10)
            except subprocess.TimeoutExpired:
                self.proc.kill()
        self.log.close()
        try:
            os.unlink(self.log.name)
        except OSError:
            pass


def watch(page, errors, label=""):
    """Record uncaught exceptions and console.error output of a page into errors (a list of strings). The browser's own
    "Failed to load resource" lines are skipped: some 404s are by design (a model with no icon on the fake)."""
    def console(m):
        if m.type == "error" and not m.text.startswith("Failed to load resource"):
            errors.append(f"{label}console.error: {m.text}")
    page.on("pageerror", lambda e: errors.append(f"{label}pageerror: {e}"))
    page.on("console", console)
