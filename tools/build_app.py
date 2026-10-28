"""Build the standalone download for this OS with PyInstaller: one program, no Python or Docker needed.
Output: dist/TiinyStudio-<version>-<os>-<arch>.zip (the program plus README, LICENSE and NOTICE).

Usage (from the repo root, Python 3.12 with PyInstaller installed):
    python -m pip install pyinstaller==6.11.1
    python tools/build_app.py
The release workflow (.github/workflows/release.yml) runs this on Windows, macOS and Linux."""
import os
import platform
import shutil
import subprocess
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
version = (ROOT / "VERSION").read_text("utf-8").strip()
osname = {"win32": "windows", "darwin": "macos"}.get(sys.platform, "linux")
arch = {"amd64": "x64", "x86_64": "x64", "arm64": "arm64", "aarch64": "arm64"}.get(platform.machine().lower(), platform.machine().lower())
build = ROOT / "build" / "app"
shutil.rmtree(build, ignore_errors=True)

sep = os.pathsep  # PyInstaller's --add-data separator
data = [("static", "static"), ("docs", "docs"), ("VERSION", "."), ("LICENSE", "."), ("NOTICE", ".")]
cmd = [sys.executable, "-m", "PyInstaller", "--onefile", "--console", "--noconfirm", "--clean", "--name", "TiinyStudio",
       "--distpath", str(build / "dist"), "--workpath", str(build / "work"), "--specpath", str(build)]
for src, dest in data:
    cmd += ["--add-data", f"{ROOT / src}{sep}{dest}"]
subprocess.run(cmd + [str(ROOT / "server.py")], check=True, cwd=ROOT)

exe = build / "dist" / ("TiinyStudio.exe" if osname == "windows" else "TiinyStudio")
out = ROOT / "dist" / f"TiinyStudio-{version}-{osname}-{arch}.zip"
out.parent.mkdir(exist_ok=True)
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
    info = zipfile.ZipInfo(f"TiinyStudio-{version}/{exe.name}")
    info.external_attr = 0o755 << 16  # keep it executable when unzipped on macOS/Linux
    info.compress_type = zipfile.ZIP_DEFLATED
    z.writestr(info, exe.read_bytes())
    for name in ("README.md", "LICENSE", "NOTICE", "CHANGELOG.md"):
        z.write(ROOT / name, f"TiinyStudio-{version}/{name}")
print(f"{out.relative_to(ROOT)}  {out.stat().st_size // 1024} KB")
