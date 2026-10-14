#!/usr/bin/env bash
# Build a release binary for the current host:
#   1. builds the UI (ui/dist) — required, it is embedded into the binary
#   2. cargo build --release --locked
#   3. copies the binary to dist/sama-<version>-<target> (no archive)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

VERSION="$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')"
TARGET="${TARGET:-$(rustc -vV | sed -n 's/host: //p')}"
OUT="dist/sama-${VERSION}-${TARGET}"

echo "==> sama ${VERSION} for ${TARGET}"

echo "==> building UI"
cd ui
if [ -d node_modules ]; then
  npm run build
else
  npm ci
  npm run build
fi
cd "$ROOT"

echo "==> building release binary"
cargo build --release --locked

echo "==> staging ${OUT}"
mkdir -p dist
cp "target/release/sama" "$OUT"
chmod +x "$OUT"

echo "done: ${OUT}"
ls -la "$OUT"
