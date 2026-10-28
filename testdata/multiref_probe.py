"""Find how Qwen-Image takes extra reference images. A field the server reads fails fast with a JSON type
error when given a number; an ignored field just runs a small 256x256 edit.
Usage: python multiref_probe.py DEVICE_URL KEY IMAGE_PATH"""
import base64
import json
import sys
import time
import urllib.request

url, key, img = sys.argv[1].rstrip("/"), sys.argv[2], sys.argv[3]
src = "data:image/png;base64," + base64.b64encode(open(img, "rb").read()).decode()


def gen(extra):
    body = {"model": "Qwen/Qwen-Image-2.1", "prompt": "make it night", "size": "256x256", "n": 1, "image": src, "response_format": "b64_json", **extra}
    req = urllib.request.Request(url + "/v1/images/generations", json.dumps(body).encode(),
                                 {"Content-Type": "application/json", "Authorization": f"Bearer {key}"})
    t0 = time.time()
    try:
        with urllib.request.urlopen(req, timeout=300) as r:
            d = json.loads(r.read())
            return f"200 in {time.time() - t0:.0f}s ({len(d.get('data', []))} image) -> field ignored"
    except urllib.error.HTTPError as e:
        return f"{e.code} in {time.time() - t0:.1f}s: {e.read().decode()[:160]}"


for field in ["images", "image_2", "image2", "reference_images", "ref_images", "extra_images", "input_images", "image_urls"]:
    print(f"{field:18}", gen({field: 123}), flush=True)
