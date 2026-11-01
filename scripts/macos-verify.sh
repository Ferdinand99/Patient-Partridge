#!/usr/bin/env bash
# Checks a built macOS app after signing and notarisation. Fails on the first kind of problem it
# finds, but looks at everything before it does:
#   - every program inside the app (the app, the engine, the PDFium library) is signed by the same
#     team, with the hardened runtime and a secure timestamp, and carries no entitlements
#   - each of them runs on both Apple silicon and Intel (a universal app)
#   - Gatekeeper accepts the app and the disk image, and the notarisation ticket is stapled to both
#
#   scripts/macos-verify.sh "<path to the .app>" [path to the .dmg]
set -uo pipefail

app="${1:?path to the .app}"
dmg="${2:-}"
fail=0
bad() { echo "::error title=macOS signing::$*"; fail=1; }

echo "== The seal of the app =="
codesign --verify --deep --strict --verbose=2 "$app" || bad "the signature of the app does not verify"

team_of() { codesign -dv --verbose=4 "$1" 2>&1 | sed -n 's/^TeamIdentifier=//p'; }
app_team="$(team_of "$app")"
[ -n "$app_team" ] || bad "the app has no team identifier (it is signed ad hoc or not at all)"

echo
echo "== Every program inside =="
printf '%-48s %-14s %-12s %s\n' file architectures runtime timestamp
required=("Contents/MacOS/partridge-engine" "Contents/Resources/libpdfium.dylib")
seen=()
while IFS= read -r f; do
  kind="$(file -b "$f")"
  case "$kind" in *Mach-O*) ;; *) continue ;; esac
  rel="${f#"$app"/}"
  seen+=("$rel")
  archs="$(lipo -archs "$f" 2>/dev/null || echo '?')"
  info="$(codesign -dv --verbose=4 "$f" 2>&1)" || { bad "not signed: $rel"; continue; }
  runtime=no
  grep -q 'flags=.*(.*runtime.*)' <<<"$info" && runtime=yes
  stamp=no
  grep -q '^Timestamp=' <<<"$info" && stamp=yes
  printf '%-48s %-14s %-12s %s\n' "$rel" "$archs" "$runtime" "$stamp"
  [ "$runtime" = yes ] || bad "no hardened runtime: $rel"
  [ "$stamp" = yes ] || bad "no secure timestamp: $rel"
  [ "$(sed -n 's/^TeamIdentifier=//p' <<<"$info")" = "$app_team" ] || bad "signed by another team: $rel"
  for arch in arm64 x86_64; do
    grep -qw "$arch" <<<"$archs" || bad "$rel has no $arch part"
  done
  entitlements="$(codesign -d --entitlements :- "$f" 2>/dev/null | tr -d '[:space:]')"
  [ -z "$entitlements" ] || bad "$rel carries entitlements; none are meant to be added (see docs/signing.md)"
done < <(find "$app/Contents" -type f \( -perm -u+x -o -name '*.dylib' -o -name '*.so' \))

for need in "${required[@]}"; do
  printf '%s\n' "${seen[@]}" | grep -qx "$need" || bad "$need is not in the app, or is not a program"
done

echo
echo "== Gatekeeper and notarisation =="
spctl --assess --type execute --verbose=4 "$app" || bad "Gatekeeper does not accept the app"
xcrun stapler validate "$app" || bad "the notarisation ticket is not stapled to the app"
if [ -n "$dmg" ]; then
  # Tauri signs the disk image, and the release build notarises and staples it too (a signed disk
  # image that is not notarised is "Unnotarized Developer ID" to Gatekeeper).
  codesign --verify --verbose=2 "$dmg" || bad "the disk image is not signed"
  spctl --assess --type open --context context:primary-signature --verbose=4 "$dmg" || bad "Gatekeeper does not accept the disk image"
  xcrun stapler validate "$dmg" || bad "the notarisation ticket is not stapled to the disk image"
fi

{
  echo "### macOS signing check"
  echo
  echo "Team identifier: \`$app_team\`"
  echo
  echo "Programs inside the app, all signed with the hardened runtime and a secure timestamp:"
  echo
  printf -- '- `%s`\n' "${seen[@]}"
} >> "${GITHUB_STEP_SUMMARY:-/dev/null}"

exit "$fail"
