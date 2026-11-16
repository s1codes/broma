#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/../.." && pwd)"

ARCH_REQUEST="${1:-all}"
case "$ARCH_REQUEST" in
  all) ARCHES=(x86_64 arm64) ;;
  x86_64|arm64) ARCHES=("$ARCH_REQUEST") ;;
  *)
    echo "Usage: $0 [x86_64|arm64|all]" >&2
    exit 2
    ;;
esac

cd "$REPO_ROOT"
for ARCH in "${ARCHES[@]}"; do
  case "$ARCH" in
    x86_64)
      RUST_TARGET="x86_64-unknown-linux-gnu"
      if ! rustup target list --installed | grep -Fxq "$RUST_TARGET"; then
        rustup target add "$RUST_TARGET"
      fi
      cargo build --locked --release --target "$RUST_TARGET"
      ;;
    arm64)
      RUST_TARGET="aarch64-unknown-linux-gnu"
      if ! command -v cross >/dev/null 2>&1; then
        echo "cross is required for Linux ARM64 builds (install cross and Docker)." >&2
        exit 1
      fi
      if ! rustup target list --installed | grep -Fxq "$RUST_TARGET"; then
        rustup target add "$RUST_TARGET"
      fi
      CROSS_CONFIG="$SCRIPT_DIR/arm64/Cross.toml" \
        cross build --locked --release --target "$RUST_TARGET"
      ;;
  esac

  "$SCRIPT_DIR/deb/build.sh" "$ARCH"
  "$SCRIPT_DIR/rpm/build.sh" "$ARCH"
done
