#!/usr/bin/env bash
# Guards the promises of the design:
#   1. The PDF engine has no network code: no networking crate in its dependency tree and no
#      socket use in its own source.
#   2. The app process never links the PDFium bindings: PDF parsing stays in the engine process.
#   3. The signer (the program that holds a certificate key) has no network code and no PDF code,
#      and the programs that do have PDF code or a window never link the code that opens a
#      certificate file: the key is in one process only.
# Checks the dependency tree of the Linux, Windows and macOS targets, not only this one.
# Run from the repository root: scripts/check-engine-deps.sh
set -euo pipefail

forbidden='^(reqwest|hyper|hyper-util|hyper-tls|h2|ureq|curl|curl-sys|isahc|attohttpc|surf|openssl|openssl-sys|native-tls|rustls|tungstenite|tokio|tokio-native-tls|async-std|smol|mio|socket2|tauri|tauri-plugin-updater|minisign-verify|wry|tao|webview2-com)$'

fail=0

targets="x86_64-unknown-linux-gnu x86_64-pc-windows-msvc aarch64-apple-darwin"
tree_of() {
  for t in $targets; do
    cargo tree -p "$1" --edges normal --prefix none --locked --target "$t" | awk '{print $1}'
  done | sort -u
}

engine_crates=$(tree_of partridge-engine)
bad=$(echo "$engine_crates" | grep -E "$forbidden" || true)
if [ -n "$bad" ]; then
  echo "The engine depends on crates it must not use:" >&2
  echo "$bad" >&2
  fail=1
fi

# The sandbox module makes socket attempts on purpose, to check that they are refused.
if grep -rnE 'std::net|os::unix::net|TcpStream|TcpListener|UdpSocket|UnixStream|UnixListener|ToSocketAddrs|WSAStartup|fn socket|fn connect' crates/engine/src --exclude-dir=sandbox; then
  echo "The engine source uses sockets." >&2
  fail=1
fi

# Attachments are never opened and links are only reported, never followed: the engine must have
# no code that reads an attachment, launches a file, or follows a remote or embedded destination.
# (`attachment_points` is the box list of a highlight, not an attachment of the file.)
if grep -rnE 'attachments(_mut)?\(|\.attachment(_mut)?([^_[:alnum:]]|$)|FPDFAttachment|FPDFDoc_(Get|Add|Delete)Attachment|as_launch_action|as_remote_destination_action|as_embedded_destination_action|PdfActionLaunch|PdfAttachment|Command::new' crates/engine/src; then
  echo "The engine source touches attachments, launches something, or follows links." >&2
  fail=1
fi

app_crates=$(tree_of patient-partridge)
if echo "$app_crates" | grep -qx 'pdfium-render'; then
  echo "The app process links pdfium-render. PDF parsing must stay in the engine." >&2
  fail=1
fi
if echo "$app_crates" | grep -qx 'partridge-engine'; then
  echo "The app process links the engine library. It must only start the engine program." >&2
  fail=1
fi

# Signatures are read and checked in the engine, in the sandbox. The crates that parse the signature
# data (CMS) and the certificates must not be in the app process: it never parses hostile data of
# this kind. (The crypto library `ring` is not on this list: the app links it through its update
# check, and it parses nothing from a PDF.)
for crate in cms x509-cert; do
  if echo "$app_crates" | grep -qx "$crate"; then
    echo "The app process links $crate. Signature data must be parsed in the engine only." >&2
    fail=1
  fi
done

# The signer: no network, no PDF code, no window.
signer_crates=$(tree_of partridge-signer)
bad=$(echo "$signer_crates" | grep -E "$forbidden" || true)
if [ -n "$bad" ]; then
  echo "The signer depends on crates it must not use:" >&2
  echo "$bad" >&2
  fail=1
fi
for crate in pdfium-render partridge-engine partridge-engine-client; do
  if echo "$signer_crates" | grep -qx "$crate"; then
    echo "The signer links $crate. It must not have PDF code." >&2
    fail=1
  fi
done
if grep -rnE 'std::net|os::unix::net|TcpStream|TcpListener|UdpSocket|UnixStream|UnixListener|ToSocketAddrs|WSAStartup|fn socket|fn connect|Command::new|std::fs::|File::open' crates/signer/src crates/signer-protocol/src; then
  echo "The signer source uses sockets, files or other programs." >&2
  fail=1
fi
# The key, and the code that opens a certificate file, are in the signer and nowhere else.
for crate in p12-keystore x509-parser partridge-signer; do
  if echo "$app_crates" | grep -qx "$crate"; then
    echo "The app process links $crate. A certificate file must be opened in the signer only." >&2
    fail=1
  fi
  if echo "$engine_crates" | grep -qx "$crate"; then
    echo "The engine links $crate. The engine must never have a key or a certificate file." >&2
    fail=1
  fi
done

if [ "$fail" -ne 0 ]; then
  exit 1
fi
echo "ok: the engine and the signer have no network code, the app does not link PDFium, and only the signer opens certificate files."
echo "signer dependencies: $(echo "$signer_crates" | tr '\n' ' ')"
echo "engine dependencies: $(echo "$engine_crates" | tr '\n' ' ')"
