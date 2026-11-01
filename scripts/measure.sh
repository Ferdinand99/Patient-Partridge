#!/usr/bin/env bash
# Measures open time and memory with a generated 500-page PDF.
# Linux (needs xvfb for the app part). Usage: scripts/measure.sh [runs]
#
#   1. The engine on its own: start, open, draw, memory (cargo example `measure`).
#   2. The whole app: time from "open requested" to "first page drawn", and the memory of the
#      app and the processes it started (scripts/smoke.mjs).
#
# Fetches PDFium (checked against its pinned checksum) and builds release binaries if needed.
set -euo pipefail

runs="${1:-5}"
cd "$(dirname "$0")/.."

node scripts/fetch-pdfium.mjs
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

cargo build --release --locked -p partridge-engine -p partridge-testkit
target="${CARGO_TARGET_DIR:-target}/release"
"$target/make-pdf" 500 "$tmp/test-500.pdf"
echo "test file: 500 pages, $(stat -c %s "$tmp/test-500.pdf") bytes"
echo

echo "== engine on its own =="
cargo run --release --locked -p partridge-engine --example measure -- "$tmp/test-500.pdf" "$runs"
echo

if command -v xvfb-run >/dev/null 2>&1; then
  echo "== whole app (release build) =="
  npm run build >/dev/null
  cargo build --release --locked -p patient-partridge --features custom-protocol
  # The app looks for the engine and the library next to itself.
  cp "vendor/pdfium/linux-x64/libpdfium.so" "$target/" 2>/dev/null || cp "vendor/pdfium/linux-arm64/libpdfium.so" "$target/"
  xvfb-run -a node scripts/smoke.mjs --bin "$target/patient-partridge" --pdf "$tmp/test-500.pdf" --runs "$runs" --memory
else
  echo "xvfb-run not found: skipping the app measurement."
fi
