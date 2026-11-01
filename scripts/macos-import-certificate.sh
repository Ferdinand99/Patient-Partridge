#!/usr/bin/env bash
# Puts the Developer ID certificate into a temporary keychain on a macOS runner, so that `codesign`
# (here and inside `tauri build`) can sign with it. The certificate and its password come from the
# environment (repository secrets); nothing is printed except how many identities were found.
#
#   APPLE_CERTIFICATE (base64 of the .p12), APPLE_CERTIFICATE_PASSWORD  scripts/macos-import-certificate.sh
set -euo pipefail

: "${APPLE_CERTIFICATE:?APPLE_CERTIFICATE is empty}"
: "${APPLE_CERTIFICATE_PASSWORD:?APPLE_CERTIFICATE_PASSWORD is empty}"
: "${RUNNER_TEMP:?RUNNER_TEMP is not set}"

keychain="$RUNNER_TEMP/partridge-signing.keychain-db"
password="$(uuidgen)"
p12="$RUNNER_TEMP/developer-id.p12"

printf '%s' "$APPLE_CERTIFICATE" | base64 --decode > "$p12"
security create-keychain -p "$password" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$password" "$keychain"
security import "$p12" -k "$keychain" -P "$APPLE_CERTIFICATE_PASSWORD" -T /usr/bin/codesign -T /usr/bin/security
rm -f "$p12"
# Lets codesign use the key without asking, which a runner cannot answer.
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$password" "$keychain" > /dev/null
# The temporary keychain goes first in the search list; the others stay.
# shellcheck disable=SC2046
security list-keychains -d user -s "$keychain" $(security list-keychains -d user | tr -d '"')

found=$(security find-identity -v -p codesigning "$keychain" | grep -c 'Developer ID Application' || true)
echo "Developer ID Application identities in the keychain: $found"
if [ "$found" -lt 1 ]; then
  echo "::error title=Certificate::The certificate in APPLE_CERTIFICATE has no usable Developer ID Application identity (is it the right file, with its private key, and is the password right?)."
  exit 1
fi
