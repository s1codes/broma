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
    RPM_ARCH="x86_64"
    RPM_TARGET="x86_64-redhat-linux"
    ;;
  arm64)
    RUST_TARGET="aarch64-unknown-linux-gnu"
    RPM_ARCH="aarch64"
    RPM_TARGET="aarch64-redhat-linux"
    ;;
  *)
    echo "Unsupported architecture '$1' (expected x86_64 or arm64)." >&2
    exit 2
    ;;
esac
BIN_PATH="$REPO_ROOT/target/$RUST_TARGET/release/broma"

if ! command -v rpmbuild >/dev/null 2>&1; then
  echo "rpmbuild is required (install the rpm-tools package)." >&2
  exit 1
fi
if [[ ! -x "$BIN_PATH" ]]; then
  echo "Missing $BIN_PATH; run installer/linux/build-packages.sh $1 first." >&2
  exit 1
fi

VERSION="$(cd "$REPO_ROOT" && python3 -c 'import pathlib, tomllib; print(tomllib.loads(pathlib.Path("Cargo.toml").read_text())["package"]["version"])')"
WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT
RPM_TOP="$WORK_DIR/rpmbuild"
SOURCE_DIR="$WORK_DIR/broma-$VERSION"
mkdir -p "$RPM_TOP/BUILD" "$RPM_TOP/BUILDROOT" "$RPM_TOP/RPMS" \
  "$RPM_TOP/SOURCES" "$RPM_TOP/SPECS" "$RPM_TOP/SRPMS" "$RPM_TOP/rpmdb" "$SOURCE_DIR"

install -m0755 "$BIN_PATH" "$SOURCE_DIR/broma"
install -m0644 "$REPO_ROOT/LICENSE" "$SOURCE_DIR/LICENSE"
install -m0644 "$SHARED_DIR/io.github.s1codes.broma.desktop" "$SOURCE_DIR/"
install -m0644 "$ASSETS_DIR/broma.svg" "$SOURCE_DIR/"
install -m0644 "$SHARED_DIR/io.github.s1codes.broma.metainfo.xml" "$SOURCE_DIR/"
tar -czf "$RPM_TOP/SOURCES/broma-$VERSION.tar.gz" -C "$WORK_DIR" "broma-$VERSION"

mkdir -p "$DIST_DIR"
rpm --dbpath "$RPM_TOP/rpmdb" --initdb
rpmbuild -bb \
  --target "$RPM_TARGET" \
  --define "_topdir $RPM_TOP" \
  --define "_rpmdir $RPM_TOP/RPMS" \
  --define "_dbpath $RPM_TOP/rpmdb" \
  --define "_enable_debug_packages 0" \
  --define "broma_version $VERSION" \
  "$SCRIPT_DIR/broma.spec"
find "$RPM_TOP/RPMS" -type f -name '*.rpm' -exec cp -f -- {} "$DIST_DIR/" \;
echo "Created $RPM_ARCH RPM package(s) in $DIST_DIR"
