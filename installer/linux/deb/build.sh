#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/../../.." && pwd)"
DIST_DIR="$REPO_ROOT/dist/linux"
SHARED_DIR="$SCRIPT_DIR/../shared"
ASSETS_DIR="$REPO_ROOT/assets"

if [[ $# -ne 1 ]]; then
  echo "Usage: $0 <x86_64|arm64>" >&2
  exit 2
fi
case "$1" in
  x86_64)
    RUST_TARGET="x86_64-unknown-linux-gnu"
    DEB_ARCH="amd64"
    ;;
  arm64)
    RUST_TARGET="aarch64-unknown-linux-gnu"
    DEB_ARCH="arm64"
    ;;
  *)
    echo "Unsupported architecture '$1' (expected x86_64 or arm64)." >&2
    exit 2
    ;;
esac
BIN_PATH="$REPO_ROOT/target/$RUST_TARGET/release/broma"

if ! command -v dpkg-deb >/dev/null 2>&1; then
  echo "dpkg-deb is required (install the dpkg package)." >&2
  exit 1
fi
if ! command -v readelf >/dev/null 2>&1; then
  echo "readelf is required (install binutils)." >&2
  exit 1
fi
if [[ ! -x "$BIN_PATH" ]]; then
  echo "Missing $BIN_PATH; run installer/linux/build-packages.sh $1 first." >&2
  exit 1
fi

VERSION="$(cd "$REPO_ROOT" && python3 -c 'import pathlib, tomllib; print(tomllib.loads(pathlib.Path("Cargo.toml").read_text())["package"]["version"])')"
GLIBC_MIN="$(readelf --version-info "$BIN_PATH" \
  | grep -oE 'GLIBC_[0-9]+(\.[0-9]+)+' \
  | sed 's/^GLIBC_//' \
  | sort -Vu \
  | tail -n1)"
if [[ -z "$GLIBC_MIN" ]]; then
  echo "Could not determine the minimum glibc version required by $BIN_PATH." >&2
  exit 1
fi
PKG_VERSION="${VERSION}-1"
PKGROOT="$(mktemp -d)"
trap 'rm -rf "$PKGROOT"' EXIT

install -Dpm0755 "$BIN_PATH" "$PKGROOT/usr/bin/broma"
install -Dpm0644 "$SHARED_DIR/io.github.s1codes.broma.desktop" \
  "$PKGROOT/usr/share/applications/io.github.s1codes.broma.desktop"
install -Dpm0644 "$ASSETS_DIR/broma.svg" \
  "$PKGROOT/usr/share/icons/hicolor/scalable/apps/io.github.s1codes.broma.svg"
install -Dpm0644 "$SHARED_DIR/io.github.s1codes.broma.metainfo.xml" \
  "$PKGROOT/usr/share/metainfo/io.github.s1codes.broma.metainfo.xml"
install -Dpm0644 "$SCRIPT_DIR/copyright" \
  "$PKGROOT/usr/share/doc/broma/copyright"

INSTALLED_SIZE="$(du -sk "$PKGROOT" | cut -f1)"
MAINTAINER="${DEB_MAINTAINER:-Broma contributors <noreply@github.com>}"
mkdir -p "$PKGROOT/DEBIAN"
cat > "$PKGROOT/DEBIAN/control" <<EOF
Package: broma
Version: ${PKG_VERSION}
Section: sound
Priority: optional
Architecture: ${DEB_ARCH}
Maintainer: ${MAINTAINER}
Installed-Size: ${INSTALLED_SIZE}
Depends: libc6 (>= ${GLIBC_MIN}), libgcc-s1, libasound2 | libasound2t64, libwayland-client0, libxkbcommon0
Description: Guilt free and efficient Soundboard
 Broma is a native soundboard for triggering audio clips from the desktop
 or global shortcuts, with optional routing to a virtual microphone.
EOF
chmod 0755 "$PKGROOT/DEBIAN"
mkdir -p "$DIST_DIR"
OUT="$DIST_DIR/broma_${PKG_VERSION}_${DEB_ARCH}.deb"
dpkg-deb --root-owner-group --build "$PKGROOT" "$OUT"
echo "Created $OUT"
