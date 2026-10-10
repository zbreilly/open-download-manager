#!/bin/bash
# Every version source must agree; with a tag (v1.2.3) it must match too.
# usage: check-versions.sh [TAG]
set -euo pipefail
cd "$(dirname "$0")/../.."
v_cargo=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
v_tauri=$(python3 -c 'import json;print(json.load(open("src-tauri/tauri.conf.json"))["version"])')
v_ui=$(python3 -c 'import json;print(json.load(open("ui/package.json"))["version"])')
v_chromium=$(python3 -c 'import json;print(json.load(open("extension/manifests/chromium.json"))["version"])')
v_firefox=$(python3 -c 'import json;print(json.load(open("extension/manifests/firefox.json"))["version"])')
printf 'Cargo.toml %s\ntauri.conf.json %s\nui/package.json %s\nchromium manifest %s\nfirefox manifest %s\n' \
  "$v_cargo" "$v_tauri" "$v_ui" "$v_chromium" "$v_firefox"
for v in "$v_tauri" "$v_ui" "$v_chromium" "$v_firefox"; do
  [ "$v" = "$v_cargo" ] || { echo "version mismatch: $v vs Cargo.toml $v_cargo"; exit 1; }
done
if [ -n "${1:-}" ]; then
  [ "${1#v}" = "$v_cargo" ] || { echo "tag $1 does not match version $v_cargo"; exit 1; }
fi
echo "versions agree: $v_cargo"
