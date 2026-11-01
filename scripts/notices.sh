#!/usr/bin/env bash
# Makes THIRD-PARTY-NOTICES from the dependencies of the app and the engine, or with --check
# only tests that the file in the repository is up to date. Fails when a dependency has a
# licence that is not on the allowlist in about.toml.
#
# Needs: cargo-about 0.9.3 (CI installs it with a pinned checksum), Node.js, and `npm ci` done.
# Run from anywhere: scripts/notices.sh [--check]
set -euo pipefail

want_version="0.9.3"
cd "$(dirname "$0")/.."

have_version="$(cargo about --version 2>/dev/null | awk '{print $2}' || true)"
if [ "$have_version" != "$want_version" ]; then
  echo "ERROR: cargo-about $want_version is needed, found '${have_version:-none}'." >&2
  echo "Install it: cargo install --locked cargo-about --version $want_version" >&2
  exit 1
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# One report for the app process and one for the engine process: they are separate programs.
cargo about generate --config about.toml --locked --fail --format json \
  --manifest-path src-tauri/Cargo.toml --output-file "$tmp/app.json"
cargo about generate --config about.toml --locked --fail --format json \
  --manifest-path crates/engine/Cargo.toml --output-file "$tmp/engine.json"

if [ "${1:-}" = "--check" ]; then
  node scripts/third-party-notices.mjs --rust "$tmp/app.json" --rust "$tmp/engine.json" --check
else
  node scripts/third-party-notices.mjs --rust "$tmp/app.json" --rust "$tmp/engine.json" --out THIRD-PARTY-NOTICES
fi
