#!/bin/bash
# Build the native macOS app: SwiftUI interface + Rust engine (crates/odm-ffi).
#
#   apps/macos/build.sh                 # this Mac's CPU, .app only
#   apps/macos/build.sh --dmg           # also a DMG
#   apps/macos/build.sh --universal     # Apple silicon + Intel
#   apps/macos/build.sh --install       # copy to /Applications and open
#   apps/macos/build.sh --typecheck     # Swift type-check only (fast, no Rust build)
#
# Needs Xcode (or the Command Line Tools) and Rust.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
HERE="apps/macos"
APP_NAME="Open Download Manager"
EXE="OpenDownloadManager"
BUNDLE_ID="app.opendm.desktop"
MIN_OS="13.0"
VERSION="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
OUT="dist/macos-native"

UNIVERSAL=0; DMG=0; INSTALL=0; TYPECHECK=0
for a in "$@"; do
  case "$a" in
    --universal) UNIVERSAL=1 ;;
    --dmg) DMG=1 ;;
    --install) INSTALL=1 ;;
    --typecheck) TYPECHECK=1 ;;
    -h|--help) sed -n '2,11p' "$0"; exit 0 ;;
    *) echo "unknown option: $a"; exit 2 ;;
  esac
done

step() { printf '\n\033[1;34m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = "Darwin" ] || die "the native app builds on macOS only"
command -v swiftc >/dev/null || die "swiftc not found. Install Xcode or run: xcode-select --install"
SWIFT_SRC=("$HERE"/Sources/*.swift)
HEADER="crates/odm-ffi/include/odm.h"

host_arch="$(uname -m)"
if [ "$UNIVERSAL" = 1 ]; then ARCHS="arm64 x86_64"; else ARCHS="$host_arch"; fi

if [ "$TYPECHECK" = 1 ]; then
  step "Type-checking Swift ($host_arch)"
  swiftc -typecheck -parse-as-library -swift-version 5 \
    -target "$host_arch-apple-macos$MIN_OS" \
    -import-objc-header "$HEADER" "${SWIFT_SRC[@]}"
  echo "Swift sources type-check cleanly"
  exit 0
fi

command -v cargo >/dev/null || { [ -x "$HOME/.cargo/bin/cargo" ] && export PATH="$HOME/.cargo/bin:$PATH"; }
command -v cargo >/dev/null || die "Rust is not installed: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"

rust_target() { [ "$1" = "arm64" ] && echo aarch64-apple-darwin || echo x86_64-apple-darwin; }

export MACOSX_DEPLOYMENT_TARGET="$MIN_OS"
BINS=()
for arch in $ARCHS; do
  t="$(rust_target "$arch")"
  step "Rust engine ($t)"
  rustup target add "$t" >/dev/null 2>&1 || true
  cargo build --release -p odm-ffi --target "$t"
  LIBDIR="target/$t/release"
  [ -f "$LIBDIR/libodm_ffi.a" ] || die "missing $LIBDIR/libodm_ffi.a"

  step "Swift interface ($arch)"
  mkdir -p "$OUT/build/$arch"
  swiftc -O -parse-as-library -swift-version 5 \
    -module-name "$EXE" \
    -target "$arch-apple-macos$MIN_OS" \
    -import-objc-header "$HEADER" \
    "${SWIFT_SRC[@]}" \
    -L "$LIBDIR" -lodm_ffi \
    -framework AppKit -framework SwiftUI -framework UserNotifications \
    -framework ServiceManagement -framework UniformTypeIdentifiers \
    -framework Security -framework SystemConfiguration -framework CoreFoundation \
    -Xlinker -dead_strip \
    -o "$OUT/build/$arch/$EXE"
  BINS+=("$OUT/build/$arch/$EXE")
done

step "Assembling $APP_NAME.app"
APP="$OUT/$APP_NAME.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
if [ "${#BINS[@]}" -gt 1 ]; then
  lipo -create -output "$APP/Contents/MacOS/$EXE" "${BINS[@]}"
else
  cp "${BINS[0]}" "$APP/Contents/MacOS/$EXE"
fi
cp src-tauri/icons/icon.icns "$APP/Contents/Resources/AppIcon.icns"
cp src-tauri/icons/tray/tray-*Template*.png "$APP/Contents/Resources/"
if [ -d extension/dist/chromium ]; then
  mkdir -p "$APP/Contents/Resources/extension"
  cp -R extension/dist/chromium extension/dist/firefox "$APP/Contents/Resources/extension/"
fi
sed -e "s/__VERSION__/$VERSION/g" -e "s/__BUILD__/$(date +%Y%m%d%H%M)/g" \
    -e "s/__BUNDLE_ID__/$BUNDLE_ID/g" -e "s/__EXE__/$EXE/g" -e "s/__MIN_OS__/$MIN_OS/g" \
    "$HERE/Info.plist" > "$APP/Contents/Info.plist"
printf 'APPL????' > "$APP/Contents/PkgInfo"

step "Code signing"
IDENTITY="${ODM_SIGN_IDENTITY:-$(security find-identity -v -p codesigning 2>/dev/null | grep 'Developer ID Application' | head -1 | awk '{print $2}' || true)}"
if [ -n "$IDENTITY" ]; then
  codesign --force --timestamp --options runtime --entitlements "$HERE/entitlements.plist" -s "$IDENTITY" "$APP"
  echo "signed with Developer ID ($IDENTITY)"
else
  codesign --force --entitlements "$HERE/entitlements.plist" -s - "$APP"
  echo "ad-hoc signed (no Developer ID certificate found)"
fi
codesign --verify --strict "$APP" && echo "signature OK"

if [ "$DMG" = 1 ]; then
  step "DMG"
  DMG_OUT="$OUT/OpenDownloadManager-$VERSION-macOS.dmg"
  STAGE="$OUT/dmg-stage"
  rm -rf "$STAGE" "$DMG_OUT"
  mkdir -p "$STAGE"
  cp -R "$APP" "$STAGE/"
  ln -s /Applications "$STAGE/Applications"
  hdiutil create -volname "$APP_NAME" -srcfolder "$STAGE" -ov -format UDZO "$DMG_OUT" >/dev/null
  rm -rf "$STAGE"
  if [ -n "$IDENTITY" ]; then codesign --force --timestamp -s "$IDENTITY" "$DMG_OUT"; fi
  echo "built $DMG_OUT"
fi

if [ "$INSTALL" = 1 ]; then
  step "Installing"
  osascript -e "tell application \"$APP_NAME\" to quit" >/dev/null 2>&1 || true
  sleep 1
  rm -rf "/Applications/$APP_NAME.app"
  cp -R "$APP" /Applications/
  open "/Applications/$APP_NAME.app"
fi

step "Done: $ROOT/$APP"
