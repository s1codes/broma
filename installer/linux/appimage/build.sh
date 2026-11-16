#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
LINUX_DIR="$(cd -- "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$(cd -- "$LINUX_DIR/../.." && pwd)"
DIST_DIR="$REPO_ROOT/dist/linux"

ARCH_REQUEST="${1:-}"
if [[ -z "$ARCH_REQUEST" ]]; then
  case "$(uname -m)" in
    x86_64) ARCH_REQUEST=x86_64 ;;
    aarch64) ARCH_REQUEST=arm64 ;;
    *)
      echo "Could not infer a supported AppImage architecture from $(uname -m)." >&2
      echo "Usage: $0 <x86_64|arm64>" >&2
      exit 2
      ;;
  esac
fi

case "$ARCH_REQUEST" in
  x86_64)
    RUST_TARGET="x86_64-unknown-linux-gnu"
    APPIMAGE_ARCH=x86_64
    HOST_ARCH=x86_64
    LINUXDEPLOY_SHA256=df908b19f1ed1cdafa79508d415aa12d6c56ce4d56d4149aa8c09286718706e5
    APPIMAGE_PLUGIN_SHA256=49d6a17160675a6bd1781699aae6bdf7692d98552e02a3671d2183d10547842e
    APPIMAGE_RUNTIME_SHA256=617aa77314d57c8b8e4fae9c3f0b35af7b30d5529cb69e98f64f92510ed98a46
    ;;
  arm64)
    RUST_TARGET="aarch64-unknown-linux-gnu"
    APPIMAGE_ARCH=aarch64
    HOST_ARCH=aarch64
    LINUXDEPLOY_SHA256=f38e4d7cebf20d8073ec4e00e0a167ac146cd8585fe2f8fe82df27d74f2bd8e1
    APPIMAGE_PLUGIN_SHA256=518f4b49a561c157b23fd1ba4e857b8fa2271a038ac711bdedab6f018acbe7e5
    APPIMAGE_RUNTIME_SHA256=9a72a744d221576c9347ec074243acc5c3c7568896b12714e0e82e9f0bf47900
    ;;
  *)
    echo "Usage: $0 <x86_64|arm64>" >&2
    exit 2
    ;;
esac

if [[ "$(uname -m)" != "$HOST_ARCH" ]]; then
  echo "AppImage packaging for $ARCH_REQUEST needs a native $HOST_ARCH Linux host." >&2
  echo "This script does not cross-package AppImages." >&2
  exit 1
fi

for tool in cargo curl file sha256sum; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "$tool is required to build the AppImage." >&2
    exit 1
  fi
done

VERSION="$(awk '
  /^\[package\]$/ { in_package = 1; next }
  /^\[/ { in_package = 0 }
  in_package && /^version[[:space:]]*=/ {
    sub(/^[^"]*"/, "")
    sub(/".*/, "")
    print
    exit
  }
' "$REPO_ROOT/Cargo.toml")"
if [[ -z "$VERSION" ]]; then
  echo "Could not read the package version from Cargo.toml." >&2
  exit 1
fi

mkdir -p "$DIST_DIR"
BUILD_DIR="$(mktemp -d "$DIST_DIR/.appimage-build-${ARCH_REQUEST}.XXXXXX")"
TOOL_DIR="$(mktemp -d "${TMPDIR:-/tmp}/broma-appimage-tools.XXXXXX")"
cleanup() {
  rm -rf -- "$BUILD_DIR" "$TOOL_DIR"
}
trap cleanup EXIT

LINUXDEPLOY="$TOOL_DIR/linuxdeploy-${APPIMAGE_ARCH}.AppImage"
APPIMAGE_PLUGIN="$TOOL_DIR/linuxdeploy-plugin-appimage-${APPIMAGE_ARCH}.AppImage"
LINUXDEPLOY_URL="https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-${APPIMAGE_ARCH}.AppImage"
APPIMAGE_PLUGIN_URL="https://github.com/linuxdeploy/linuxdeploy-plugin-appimage/releases/download/continuous/linuxdeploy-plugin-appimage-${APPIMAGE_ARCH}.AppImage"
APPIMAGE_RUNTIME="$TOOL_DIR/runtime-$APPIMAGE_ARCH"
APPIMAGE_RUNTIME_URL="https://github.com/AppImage/type2-runtime/releases/download/continuous/runtime-$APPIMAGE_ARCH"

# These hashes pin linuxdeploy (commit 07333c6), its AppImage plugin (536b068),
# and the AppImage runtime (fedfad5) despite the upstream continuous URLs.
download_verified() {
  local url="$1"
  local destination="$2"
  local expected_sha256="$3"

  curl --fail --location --retry 3 --output "$destination" "$url"
  if ! printf '%s  %s\n' "$expected_sha256" "$destination" | sha256sum --check --status; then
    echo "SHA-256 verification failed for $url" >&2
    exit 1
  fi
  chmod +x "$destination"
}

download_verified "$LINUXDEPLOY_URL" "$LINUXDEPLOY" "$LINUXDEPLOY_SHA256"
download_verified "$APPIMAGE_PLUGIN_URL" "$APPIMAGE_PLUGIN" "$APPIMAGE_PLUGIN_SHA256"
download_verified "$APPIMAGE_RUNTIME_URL" "$APPIMAGE_RUNTIME" "$APPIMAGE_RUNTIME_SHA256"

cd "$REPO_ROOT"
cargo build --locked --release --target "$RUST_TARGET"
BIN_PATH="$REPO_ROOT/target/$RUST_TARGET/release/broma"
if [[ ! -x "$BIN_PATH" ]]; then
  echo "Expected release binary not found at $BIN_PATH" >&2
  exit 1
fi

APPDIR="$BUILD_DIR/Broma.AppDir"
DESKTOP_FILE="$LINUX_DIR/shared/io.github.s1codes.broma.desktop"
ICON_FILE="$BUILD_DIR/io.github.s1codes.broma.svg"
OUTPUT="$DIST_DIR/Broma-$VERSION-$APPIMAGE_ARCH.AppImage"

mkdir -p \
  "$APPDIR/usr/share/metainfo" \
  "$APPDIR/usr/share/licenses/broma"
cp "$LINUX_DIR/shared/io.github.s1codes.broma.metainfo.xml" \
  "$APPDIR/usr/share/metainfo/io.github.s1codes.broma.appdata.xml"
cp "$REPO_ROOT/LICENSE" "$APPDIR/usr/share/licenses/broma/"
cp "$REPO_ROOT/assets/broma.svg" "$ICON_FILE"

rm -f -- "$OUTPUT"
(
  cd "$DIST_DIR"
  export APPIMAGE_EXTRACT_AND_RUN=1
  export ARCH="$APPIMAGE_ARCH"
  export LDAI_OUTPUT="$OUTPUT"
  export LINUXDEPLOY_OUTPUT_APP_NAME=Broma
  export LINUXDEPLOY_OUTPUT_VERSION="$VERSION"
  export LDAI_RUNTIME_FILE="$APPIMAGE_RUNTIME"
  export LDAI_NO_APPSTREAM=1
  export VERSION

  "$LINUXDEPLOY" \
    --appdir "$APPDIR" \
    --executable "$BIN_PATH" \
    --desktop-file "$DESKTOP_FILE" \
    --icon-file "$ICON_FILE" \
    --output appimage
)

if [[ ! -s "$OUTPUT" ]]; then
  echo "AppImage tool did not create the expected output: $OUTPUT" >&2
  exit 1
fi
chmod +x "$OUTPUT"
echo "Created $OUTPUT"
