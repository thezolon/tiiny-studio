#!/usr/bin/env bash
# Runs a built studio image once: start, GET / and /api/settings (the version must be VERSION's), stop.
# Used by the Rust workflow (every PR that touches the Dockerfile or rust/) and by the Release workflow before it pushes.
# Usage: testdata/docker_smoke.sh <image>      Needs docker and curl. Never contacts a Tiiny.
set -euo pipefail
export MSYS_NO_PATHCONV=1   # Git Bash on Windows would turn "--tmpfs /tmp" into a C:\ path
image=${1:?usage: docker_smoke.sh <image>}
root=$(cd "$(dirname "$0")/.." && pwd)
want=$(tr -d '\r\n' < "$root/VERSION")
port=$((20000 + RANDOM % 20000))
name=studio-smoke-$$
cleanup() { docker logs "$name" 2>&1 | tail -n 20 || true; docker rm -f "$name" >/dev/null 2>&1 || true; }
trap cleanup EXIT
# the same hardening as compose.yaml, so CI starts what `docker compose up` users get
docker run -d --name "$name" -p "127.0.0.1:$port:7860"   --read-only --tmpfs /tmp --cap-drop ALL --cap-add CHOWN --cap-add SETUID --cap-add SETGID   --security-opt no-new-privileges "$image" >/dev/null
up=
for _ in $(seq 1 60); do
  if curl -sf "http://127.0.0.1:$port/" >/dev/null; then up=1; break; fi
  sleep 1
done
[ -n "$up" ] || { echo "the image did not serve the page on port $port in 60 s" >&2; exit 1; }
settings=$(curl -sf "http://127.0.0.1:$port/api/settings")
if ! grep -Eq "\"version\" *: *\"$want\"" <<<"$settings"; then
  echo "/api/settings does not say version $want: ${settings:0:300}" >&2
  exit 1
fi
echo "image $image serves version $want"
