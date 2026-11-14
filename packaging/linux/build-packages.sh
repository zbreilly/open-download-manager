#!/bin/bash
# Build the native Linux package for this machine's distribution family and
# CPU: a .deb on Debian/Ubuntu, an .rpm on Fedora. One package holds both
# interfaces (GTK/libadwaita and Kirigami) and the launcher that picks one.
#
#   packaging/linux/build-packages.sh            # build the binaries, then the package
#   packaging/linux/build-packages.sh --no-build # package what target/release already has
#
# Needs Rust, the GTK 4, libadwaita and Qt 6 (base + declarative)
# development packages, and dpkg-deb or rpmbuild. Output: dist/linux-native/.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
HERE="packaging/linux"
NAME="open-download-manager"
APP_ID="app.opendm.Desktop"
VERSION="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
SUMMARY="Fast, resumable, segmented downloads"
OUT="dist/linux-native"

BUILD=1
for a in "$@"; do
  case "$a" in
    --no-build) BUILD=0 ;;
    -h|--help) sed -n '2,11p' "$0"; exit 0 ;;
    *) echo "unknown option: $a"; exit 2 ;;
  esac
done

step() { printf '\n\033[1;34m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

if command -v dpkg-deb >/dev/null && [ -f /etc/debian_version ]; then FORMAT=deb
elif command -v rpmbuild >/dev/null; then FORMAT=rpm
else die "neither dpkg-deb (Debian/Ubuntu) nor rpmbuild (Fedora) found"; fi

if [ "$BUILD" = 1 ]; then
  step "Building the launcher and both interfaces"
  cargo build --release -p odm-linux -p odm-gnome -p odm-plasma
  [ -d extension/dist/chromium ] || bash extension/build.sh
fi
for b in open-download-manager odm-gnome odm-plasma; do
  [ -x "target/release/$b" ] || die "missing target/release/$b"
done

step "Staging files"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
T="$STAGE/root"
install -Dm755 target/release/open-download-manager "$T/usr/bin/$NAME"
install -Dm755 target/release/odm-gnome "$T/usr/libexec/$NAME/odm-gnome"
install -Dm755 target/release/odm-plasma "$T/usr/libexec/$NAME/odm-plasma"
install -Dm644 "$HERE/$APP_ID.desktop" "$T/usr/share/applications/$APP_ID.desktop"
install -Dm644 "$HERE/$APP_ID.metainfo.xml" "$T/usr/share/metainfo/$APP_ID.metainfo.xml"
for icon in download-manager-ui/brand/dist/app-icons/linux/hicolor/*/apps/open-download-manager.png; do
  size="$(basename "$(dirname "$(dirname "$icon")")")"
  install -Dm644 "$icon" "$T/usr/share/icons/hicolor/$size/apps/$APP_ID.png"
done
if [ -d extension/dist/chromium ]; then
  mkdir -p "$T/usr/share/$NAME/extension"
  cp -R extension/dist/chromium extension/dist/firefox "$T/usr/share/$NAME/extension/"
fi
install -Dm644 LICENSE "$T/usr/share/licenses/$NAME/LICENSE"
mkdir -p "$OUT"

DESCRIPTION="Open Download Manager accelerates downloads with multiple connections,
 resumes interrupted transfers, schedules queues and integrates with your
 browser. On KDE Plasma it starts its Kirigami interface, elsewhere its GTK
 and libadwaita one."

if [ "$FORMAT" = deb ]; then
  step "Debian package"
  ARCH="$(dpkg --print-architecture)"
  # shared-library dependencies of the three programs
  mkdir -p "$STAGE/shlibs/debian"
  printf 'Source: %s\n\nPackage: %s\nArchitecture: any\n' "$NAME" "$NAME" > "$STAGE/shlibs/debian/control"
  SHLIBS="$(cd "$STAGE/shlibs" && dpkg-shlibdeps -O --ignore-missing-info \
    "$T/usr/bin/$NAME" "$T/usr/libexec/$NAME/odm-gnome" "$T/usr/libexec/$NAME/odm-plasma" 2>/dev/null | sed 's/^shlibs:Depends=//')"
  [ -n "$SHLIBS" ] || die "dpkg-shlibdeps found no dependencies"
  # what the Kirigami interface imports at run time, and the helpers both use
  QML="qml6-module-org-kde-kirigami, qml6-module-org-kde-desktop, qml6-module-qtquick, qml6-module-qtquick-controls, qml6-module-qtquick-layouts, qml6-module-qtquick-dialogs, qml6-module-qtquick-window, qml6-module-qtquick-templates, qml6-module-qtqml-workerscript, qml6-module-qt-labs-folderlistmodel"
  mkdir -p "$T/DEBIAN"
  SIZE="$(du -sk "$T" | cut -f1)"
  cat > "$T/DEBIAN/control" <<CONTROL
Package: $NAME
Version: $VERSION
Architecture: $ARCH
Maintainer: Open Download Manager <noreply@opendm.app>
Installed-Size: $SIZE
Depends: $SHLIBS, $QML, xdg-utils, desktop-file-utils
Recommends: gnome-shell-extension-appindicator | plasma-workspace
Section: net
Priority: optional
Homepage: https://github.com/zbreilly/open-download-manager
Description: $SUMMARY
 $DESCRIPTION
CONTROL
  cat > "$T/DEBIAN/postinst" <<'SCRIPT'
#!/bin/sh
set -e
if command -v update-desktop-database >/dev/null; then update-desktop-database -q /usr/share/applications || true; fi
if command -v gtk-update-icon-cache >/dev/null; then gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true; fi
SCRIPT
  cp "$T/DEBIAN/postinst" "$T/DEBIAN/postrm"
  chmod 755 "$T/DEBIAN/postinst" "$T/DEBIAN/postrm"
  # Debian keeps licenses under /usr/share/doc
  install -Dm644 LICENSE "$T/usr/share/doc/$NAME/copyright"
  rm -rf "$T/usr/share/licenses"
  PKG="$OUT/${NAME}_${VERSION}_${ARCH}.deb"
  dpkg-deb --root-owner-group --build "$T" "$PKG" >/dev/null
  dpkg-deb -I "$PKG"
else
  step "RPM package"
  ARCH="$(rpm --eval '%{_arch}')"
  TOP="$STAGE/rpmbuild"
  mkdir -p "$TOP/SPECS" "$TOP/RPMS"
  cat > "$TOP/SPECS/$NAME.spec" <<SPEC
Name:           $NAME
Version:        $VERSION
Release:        1
Summary:        $SUMMARY
License:        AGPL-3.0-only
URL:            https://github.com/zbreilly/open-download-manager
# shared-library requirements are found automatically; these are what the
# Kirigami interface imports at run time, and the helpers both use
Requires:       kf6-kirigami
Requires:       kf6-qqc2-desktop-style
Requires:       qt6-qtdeclarative
Requires:       xdg-utils
Requires:       desktop-file-utils
Recommends:     gnome-shell-extension-appindicator

%description
$DESCRIPTION

%install
cp -a "$T/." "%{buildroot}/"

%post
update-desktop-database -q %{_datadir}/applications || :
gtk-update-icon-cache -q -t -f %{_datadir}/icons/hicolor || :

%postun
update-desktop-database -q %{_datadir}/applications || :

%files
%license %{_datadir}/licenses/$NAME/LICENSE
%{_bindir}/$NAME
%{_libexecdir}/$NAME
%{_datadir}/applications/$APP_ID.desktop
%{_datadir}/metainfo/$APP_ID.metainfo.xml
%{_datadir}/icons/hicolor/*/apps/$APP_ID.png
%{_datadir}/$NAME
SPEC
  rpmbuild -bb --quiet --define "_topdir $TOP" --define "_build_id_links none" --define "debug_package %{nil}" "$TOP/SPECS/$NAME.spec"
  PKG="$OUT/${NAME}-${VERSION}-1.${ARCH}.rpm"
  cp "$TOP/RPMS/$ARCH/"*.rpm "$PKG"
  rpm -qpR "$PKG"
fi

step "Done: $ROOT/$PKG"
