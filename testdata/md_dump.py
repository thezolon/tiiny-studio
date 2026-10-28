"""Print the Markdown extract.py makes for each test document (run make_docs.py first).
Usage: python md_dump.py"""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent.parent))
import extract  # noqa: E402

sys.stdout.reconfigure(encoding="utf-8")
for f in sorted((Path(__file__).parent / "docs").iterdir()):
    if f.suffix.lower() in (".pdf", ".tiff", ".bmp", ".gif", ".heic"):
        continue
    print("=====", f.name)
    try:
        for p in extract.extract(f.name, f.read_bytes()):
            print(f"  [{p['label']}]", "IMAGE" if "image" in p else "\n" + p["md"])
    except extract.ExtractError as e:
        print("  ERROR", e)
