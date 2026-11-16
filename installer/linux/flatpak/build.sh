#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/../../.." && pwd)"
DIST_DIR="$REPO_ROOT/dist/linux"
MANIFEST="$SCRIPT_DIR/io.github.s1codes.broma.yml"
APP_ID="io.github.s1codes.broma"
VERSION="$(cd "$REPO_ROOT" && python3 -c 'import pathlib, tomllib; print(tomllib.loads(pathlib.Path("Cargo.toml").read_text())["package"]["version"])')"

ARCH_REQUEST="${1:-all}"
case "$ARCH_REQUEST" in
  all) ARCHES=(x86_64 arm64) ;;
  x86_64|arm64) ARCHES=("$ARCH_REQUEST") ;;
  *)
    echo "Usage: $0 [x86_64|arm64|all]" >&2
    exit 2
    ;;
esac

for tool in flatpak flatpak-builder; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "$tool is required to build the Flatpak." >&2
    exit 1
  fi
done
if [[ ! -s "$SCRIPT_DIR/cargo-sources.json" ]]; then
  echo "Missing cargo-sources.json; see installer/linux/README.md to generate it." >&2
  exit 1
fi
if ! flatpak remotes --user --columns=name | grep -Fxq flathub; then
  echo "The user Flathub remote is missing. Add it with:" >&2
  echo "  flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo" >&2
  exit 1
fi

mkdir -p "$DIST_DIR"
for ARCH in "${ARCHES[@]}"; do
  case "$ARCH" in
    x86_64) FLATPAK_ARCH="x86_64" ;;
    arm64) FLATPAK_ARCH="aarch64" ;;
  esac
  BUILD_DIR="$DIST_DIR/flatpak-build-$ARCH"
  REPO_DIR="$DIST_DIR/flatpak-repo-$ARCH"
  OUT="$DIST_DIR/broma_${VERSION}_${ARCH}.flatpak"

  flatpak-builder \
    --force-clean \
    --user \
    --arch="$FLATPAK_ARCH" \
    --install-deps-from=flathub \
    --repo="$REPO_DIR" \
    "$BUILD_DIR" \
    "$MANIFEST"
  flatpak build-bundle "$REPO_DIR" "$OUT" "$APP_ID"
  echo "Created $OUT"
done
