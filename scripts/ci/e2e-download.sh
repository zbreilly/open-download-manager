#!/bin/bash
# End-to-end check against a running app that was started with
#   ODM_E2E_URL=http://127.0.0.1:8919/odm-ci-e2e.bin
# in its environment: serve that file, wait for the app to download it and
# compare the result byte for byte with the source. Also checks that the
# browser bridge turns away everyone who isn't a paired extension.
#
# usage: e2e-download.sh DOWNLOAD_DIR [BRIDGE_PORT]
set -euo pipefail
DL="$1"
PORT="${2:-8901}"
HERE="$(cd "$(dirname "$0")" && pwd)"
WORK="$(mktemp -d)"
NAME="odm-ci-e2e.bin"
SRV=8919

sha() { if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }

# 48 MB of random data, served at 2 MB/s per connection so segmenting shows
head -c $((48 * 1024 * 1024)) /dev/urandom > "$WORK/$NAME"
"${PYTHON:-python3}" "$HERE/range_server.py" "$WORK" "$SRV" $((2 * 1024 * 1024)) 2> "$WORK/server.log" &
SRV_PID=$!
trap 'kill $SRV_PID 2>/dev/null || true' EXIT
for _ in $(seq 1 50); do curl -sf -o /dev/null -I "http://127.0.0.1:$SRV/$NAME" && break; sleep 0.1; done

# the bridge must refuse a web page, a forged Host, anything not sealed with
# the pairing key, and the endpoints that took unsealed requests
api="http://127.0.0.1:$PORT/api/v2"
post() { curl -s -o /dev/null -w '%{http_code}' -X POST -H 'Content-Type: application/json' "$@"; }
code=$(post -H 'Origin: https://evil.example' -d '{}' "$api")
[ "$code" = 403 ] || { echo "bridge accepted a web origin ($code)"; exit 1; }
code=$(post -H "Host: evil.example:$PORT" -d '{}' "$api")
[ "$code" = 403 ] || { echo "bridge accepted a forged Host ($code)"; exit 1; }
code=$(post -H 'Origin: chrome-extension://odmcitest' -d "{\"url\":\"http://127.0.0.1:$SRV/$NAME\",\"silent\":true}" "$api")
[ "$code" = 401 ] || { echo "bridge accepted an unsealed request ($code)"; exit 1; }
code=$(post -H 'Origin: chrome-extension://odmcitest' -d '{"v":2,"n":"AAAAAAAAAAAAAAAA","c":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}' "$api")
[ "$code" = 401 ] || { echo "bridge accepted a request sealed with another key ($code)"; exit 1; }
code=$(post -H 'Origin: chrome-extension://odmcitest' -d "{\"url\":\"http://127.0.0.1:$SRV/$NAME\",\"silent\":true}" "http://127.0.0.1:$PORT/api/add")
[ "$code" = 404 ] || { echo "the unsealed add endpoint still answers ($code)"; exit 1; }
echo "bridge refuses unpaired callers"

# wait for the finished file (the engine renames .odmpart when done)
OUT=""
for _ in $(seq 1 120); do
  OUT=$(find "$DL" -maxdepth 3 -name "$NAME" -type f 2>/dev/null | head -1)
  [ -n "$OUT" ] && break
  sleep 1
done
[ -n "$OUT" ] || { echo "download did not finish"; ls -laR "$DL" 2>/dev/null | tail -20; tail -20 "$WORK/server.log"; exit 1; }
sleep 1
ranges=$(grep -c '^bytes=' "$WORK/server.log" || true)
if [ "$(sha "$OUT")" = "$(sha "$WORK/$NAME")" ]; then
  echo "e2e ok: $OUT matches the source ($ranges range requests)"
else
  echo "e2e FAILED: $OUT differs from the source"; exit 1
fi
