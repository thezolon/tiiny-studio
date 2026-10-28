"""Build the release package: dist/tiiny-studio-<VERSION>.zip from the committed tree (git archive), so
nothing uncommitted or ignored (data/, .env, test outputs) can end up in it. Test scripts and this tools/
folder are left out via .gitattributes export-ignore.

Usage: python tools/release.py            (refuses to run with uncommitted changes)
       python tools/release.py --tag      (also creates the annotated tag v<VERSION>, locally only)"""
import hashlib
import subprocess
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def git(*args):
    return subprocess.run(["git", *args], cwd=ROOT, check=True, capture_output=True, text=True).stdout.strip()


version = (ROOT / "VERSION").read_text("utf-8").strip()
if git("status", "--porcelain"):
    sys.exit("Uncommitted changes: commit them first, so the package matches a commit.")
name = f"tiiny-studio-{version}"
out = ROOT / "dist" / f"{name}.zip"
out.parent.mkdir(exist_ok=True)
git("archive", "--format=zip", f"--prefix={name}/", "-o", str(out), "HEAD")

with zipfile.ZipFile(out) as z:
    names = z.namelist()
    bad = [n for n in names if any(part in n for part in ("/data/", "/.env", "/testdata/", "/tools/", "/dist/"))
           and not n.endswith(".env.example")]
    if bad:
        out.unlink()
        sys.exit(f"Refusing: the package would contain {bad[:5]}")
    for need in ("Dockerfile", "compose.yaml", "server.py", "LICENSE", "NOTICE", "README.md", "VERSION", ".env.example"):
        if f"{name}/{need}" not in names:
            out.unlink()
            sys.exit(f"Refusing: {need} is missing from the package")

digest = hashlib.sha256(out.read_bytes()).hexdigest()
(out.parent / f"{name}.zip.sha256").write_text(f"{digest}  {out.name}\n", "utf-8")
print(f"{out.relative_to(ROOT)}  {out.stat().st_size // 1024} KB  {len(names)} entries  commit {git('rev-parse', '--short', 'HEAD')}")
print(f"sha256 {digest}")

if "--tag" in sys.argv:
    tag = f"v{version}"
    if tag in git("tag", "--list", tag).split():
        print(f"tag {tag} already exists")
    else:
        git("tag", "-a", tag, "-m", f"Tiiny Studio {version}")
        print(f"tagged {tag} (local only; nothing pushed)")
