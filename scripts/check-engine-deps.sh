#!/usr/bin/env bash
# Guards two promises of the design:
#   1. The PDF engine has no network code: no networking crate in its dependency tree and no
#      socket use in its own source.
#   2. The app process never links the PDFium bindings: PDF parsing stays in the engine process.
# Run from the repository root: scripts/check-engine-deps.sh
set -euo pipefail

forbidden='^(reqwest|hyper|hyper-util|hyper-tls|h2|ureq|curl|curl-sys|isahc|attohttpc|surf|openssl|openssl-sys|native-tls|rustls|tungstenite|tokio|tokio-native-tls|async-std|smol|mio|socket2|tauri|wry|tao|webview2-com)$'

fail=0

engine_crates=$(cargo tree -p partridge-engine --edges normal --prefix none --locked | awk '{print $1}' | sort -u)
bad=$(echo "$engine_crates" | grep -E "$forbidden" || true)
if [ -n "$bad" ]; then
  echo "The engine depends on crates it must not use:" >&2
  echo "$bad" >&2
  fail=1
fi

if grep -rnE 'std::net|TcpStream|TcpListener|UdpSocket|ToSocketAddrs' crates/engine/src; then
  echo "The engine source uses sockets." >&2
  fail=1
fi

app_crates=$(cargo tree -p patient-partridge --edges normal --prefix none --locked | awk '{print $1}' | sort -u)
if echo "$app_crates" | grep -qx 'pdfium-render'; then
  echo "The app process links pdfium-render. PDF parsing must stay in the engine." >&2
  fail=1
fi
if echo "$app_crates" | grep -qx 'partridge-engine'; then
  echo "The app process links the engine library. It must only start the engine program." >&2
  fail=1
fi

if [ "$fail" -ne 0 ]; then
  exit 1
fi
echo "ok: the engine has no network code and the app does not link PDFium."
echo "engine dependencies: $(echo "$engine_crates" | tr '\n' ' ')"
