#!/bin/bash
# Build the native macOS app (AppKit interface in Rust + Rust engine). Options are passed
# through: --install, --universal, --dmg. The Tauri build for macOS is
# scripts/build-macos-tauri.sh.
set -euo pipefail
cd "$(dirname "$0")/.."
bash extension/build.sh
exec apps/macos/build.sh "$@"
