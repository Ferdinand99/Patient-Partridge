# Code signing and notarisation

This is separate from [update signing](updates.md). Update signing (Tauri's key pair) is what makes the app trust an update. Code signing is what makes the operating system trust the program. The macOS app is **signed and notarised in every release** (a release that cannot do that is not built); Windows is not signed yet; Linux has nothing to sign.

## What users see

- **macOS, signed and notarised (every release):** the app should open without a warning from Gatekeeper. The signing check opens the signed app from a copy of the disk image that carries the "downloaded" mark and shows it accepted; nobody has yet tried a real download in a browser on a real Mac, which should be done once before the first release.
- **macOS, a build that is not signed (a build for testing):** "Patient Partridge cannot be opened because the developer cannot be verified" (or "is damaged"). Right-click the app, Open, Open; or System Settings, Privacy & Security, "Open Anyway". If it says "damaged": `xattr -dr com.apple.quarantine "/Applications/Patient Partridge.app"`.
- **Windows (not signed):** SmartScreen shows "Windows protected your PC". Click **More info**, then **Run anyway**. Updates inside the app are not affected: they are checked with the update key, not with a certificate.
- **Linux:** no signature. The AppImage and the deb package come with the update signature (`.sig`) of the release, which only the app's updater uses.

## macOS

The Developer ID certificate belongs to the team, so Patient Partridge uses the same certificate as the other Hedgerow apps. It needs an Apple Developer Program membership.

Repository secrets (six):

| Secret | Value |
|---|---|
| `APPLE_CERTIFICATE` | base64 of the exported **Developer ID Application** certificate with its private key (`.p12`) |
| `APPLE_CERTIFICATE_PASSWORD` | the password chosen when exporting the `.p12` |
| `APPLE_SIGNING_IDENTITY` | `Developer ID Application: Name (TEAMID)` |
| `APPLE_API_ISSUER` | Issuer ID (App Store Connect, Users and Access, Integrations) |
| `APPLE_API_KEY` | Key ID of the API key |
| `APPLE_API_KEY_CONTENT` | the full text of the downloaded `AuthKey_XXXX.p8` file |

The workflows put the certificate into a temporary keychain (`scripts/macos-import-certificate.sh`), write `APPLE_API_KEY_CONTENT` to a temporary file and point `APPLE_API_KEY_PATH` at it. Notarisation uses the App Store Connect API key.

Base64 of the certificate, on one line:

```sh
base64 -i certificate.p12 | tr -d '\n' | pbcopy      # macOS
base64 -w0 certificate.p12                            # Linux
```

If the identity has letters that are not ASCII (æ, ø, å), do not type it on a Git Bash command line, which damages them. Read it from the certificate:

```sh
openssl x509 -in devid.pem -noout -subject -nameopt RFC2253,-esc_msb \
  | sed -n 's/.*CN=\([^,]*\).*/\1/p' | tr -d '\n' > identity.txt
gh secret set APPLE_SIGNING_IDENTITY < identity.txt
```

### What is signed, and how

The app is a **universal** app (Apple silicon and Intel in one). Three kinds of program are inside it, and notarisation fails if one of them is not signed with the hardened runtime and a secure timestamp:

| Inside the app | Where it comes from | Signed by |
|---|---|---|
| `Contents/MacOS/patient-partridge` (the app) | built by `tauri build --target universal-apple-darwin` from both targets | Tauri, with the hardened runtime |
| `Contents/MacOS/partridge-engine` (the PDF engine, a sidecar) | built for both targets, joined with `lipo` (`scripts/prepare-bundle.mjs`) | Tauri, with the hardened runtime |
| `Contents/Resources/libpdfium.dylib` (PDFium) | the two pinned, checksum-checked downloads (arm64 and x64), joined with `lipo` | `scripts/prepare-bundle.mjs`, **before** the bundle is made: Tauri does not sign files that are only resources, and Apple rejects an unsigned library |

The PDFium library is never downloaded as a joined file: each of the two architectures is checked against its checksum in `pdfium.lock.json` before it is unpacked, and the join is made on the runner. The engine and the library sit in the app in the same layout as on the other systems, so `locate.rs` finds them in the same places.

### Entitlements

**None are added.** The hardened runtime without entitlements is enough:

- The engine loads only the PDFium library that is in the same app, signed by the same team, so the library validation of the hardened runtime is satisfied (`com.apple.security.cs.disable-library-validation` is not needed and must not be added).
- The app itself starts no code it generates (no JIT): the web view's JIT runs in Apple's own web content process (`allow-jit` and `allow-unsigned-executable-memory` are not needed).
- The update check is a plain outgoing connection; `com.apple.security.network.client` only matters for apps in the App Sandbox, and this app is distributed outside the store.
- The engine gets a sandbox profile of its own (Seatbelt on macOS; a separate change). An entitlement that loosens the hardened runtime of the engine would work against it, so an entitlement is added only with a written reason on this page and a review of the sandbox profile.

`scripts/macos-verify.sh` fails the build when any program in the app carries entitlements.

### Checking the setup

The **macOS signing check** workflow (`.github/workflows/macos-signing.yml`) builds the universal app with these secrets, signs and notarises it, and then runs `scripts/macos-verify.sh`: every program inside is signed by the same team with the hardened runtime and a secure timestamp, each has both architectures, no entitlements, the seal verifies, Gatekeeper accepts the app and the ticket is stapled. Nothing is published. It runs when the files that make the signed build change, and by hand from the Actions tab (run it after renewing the certificate or the API key). With the **beta** input on it makes the beta build described below.

To look at a release by hand:

```sh
codesign --verify --deep --strict --verbose=2 "/Applications/Patient Partridge.app"
codesign -dv --verbose=4 "/Applications/Patient Partridge.app/Contents/MacOS/partridge-engine"   # flags=0x10000(runtime)
codesign -dv --verbose=4 "/Applications/Patient Partridge.app/Contents/Resources/libpdfium.dylib"
lipo -archs "/Applications/Patient Partridge.app/Contents/Resources/libpdfium.dylib"           # x86_64 arm64
spctl --assess --type execute --verbose=4 "/Applications/Patient Partridge.app"
xcrun stapler validate "/Applications/Patient Partridge.app"
```

The disk image is signed by Tauri, and the release build notarises it and staples the ticket to it (`scripts/release-build.mjs`: `notarytool submit --wait`, then `stapler staple`). A disk image that is signed but not notarised is "Unnotarized Developer ID" to Gatekeeper, so the check fails unless `spctl` accepts the disk image and `stapler validate` passes for both.

### A beta build next to the stable app

Run the **macOS signing check** by hand from the Actions tab (on `main`) with **beta** switched on. It makes the same signed, notarised and stapled universal app, with the same secrets on the same step only, but as **Patient Partridge Beta**. The disk image in the artifact `patient-partridge-macos-beta-signed` is the delivery (kept for 3 days). Nothing is published: no release, no tag, no update file.

What differs (`scripts/beta-build.mjs` merges it over the generated config; `src-tauri/tauri.conf.json` is not changed):

| | Stable | Beta |
|---|---|---|
| Name of the app, disk image and window title | Patient Partridge | Patient Partridge Beta (`Patient Partridge Beta_<version>_universal.dmg`) |
| Identifier | `com.ferdinand99.patientpartridge` | `com.ferdinand99.patientpartridge.beta` |
| Version | `0.1.0` | `0.1.0-beta.<run number>` |
| Icon | the bird | the bird with a violet "BETA" ribbon across the lower right corner, drawn at build time from `assets/app-icon.png` (the icons in the repository are not changed) |
| Update files, update server | made, and asked | none made, and the program has no address to ask |
| File association (.pdf) | yes (as an alternative viewer) | none |
| Program inside the app | `patient-partridge` | `patient-partridge`; the launch test reads the name from `Info.plist` |

**Settings, positions and tabs are separate.** The app keeps `settings.json`, `positions.json` and `session.json` in its app data folder, which Tauri names after the identifier (`~/Library/Application Support/<identifier>`), and the web view keeps the colour theme in storage that is also kept per identifier. The beta therefore starts empty, and neither app sees the other's files. The single-instance lock is per identifier too, so both can run at the same time.

**It cannot update itself, and the stable app cannot take it for a release.** The beta build has no update file and no update endpoint, so a check ends with "Updates are not set up in this build", and an automatic check stays quiet. The check of the build fails if the configuration inside the program still holds the address of `latest.json`, if any `.tar.gz` or `.sig` file was made, or if the disk image is not named after the beta. The stable app reads only the `latest.json` of the project's releases, and nothing of the beta is ever published there.

**File association.** Without it the beta is not offered in "Open With" for PDF files, and Finder keeps opening them with the stable app (or whatever the user chose). In the beta, use the Open button or drop the file on the window. Dropping a file on the beta's Dock icon does not work, because the app does not declare the file type. To offer the beta for PDF files, take `fileAssociations: []` out of `betaOverride` in `scripts/beta-build.mjs`.

**The window title** is the product name, so it reads "book.pdf - Patient Partridge Beta". The app asks Tauri for its name when it starts (`core:app:allow-name`); the stable app gets "Patient Partridge", as before.

**Checking the icon.** The job uploads the artifact `patient-partridge-beta-icon`: the icon of the finished app, cut from the `.icns` inside it, at 512 px and at 64 px (about the size in the Dock).

## Windows (not signed yet)

Nothing is set up. Since June 2023 publicly trusted code signing keys must be in hardware or in a cloud service, so a downloadable `.pfx` is rare. When it is time: Azure Trusted Signing or a certificate in a cloud HSM (a `bundle.windows.signCommand` in the Tauri config), or SignPath Foundation (free signing for open source projects, approved per release). None of this is wired up. Until then Windows users see the SmartScreen warning described above; updates inside the app work the same.

## Where the secrets are

The update key and the Apple certificate are the most valuable secrets of the project: the first signs code for every user, the second is the team's Developer ID, shared with the other Hedgerow apps. The workflows keep them away from third-party code as far as Tauri allows:

- No secret is set for a whole job. Each is set only on the step that reads it.
- The steps that run third-party code have no secret: `npm ci`, the compile (`tauri build --no-bundle`, which runs the build scripts of every Rust dependency and the Vite build), the making of the Windows installers and the Linux packages (`tauri bundle`), and the fetching of the pinned AppImage tools.
- One step has them, and nothing else: the "tauri script" of `tauri-action` (`scripts/release-build.mjs`). On Windows and Linux it only signs the update files with the update key. On macOS it puts the certificate into a temporary keychain, signs the PDFium library, runs `tauri bundle` (which signs the app, notarises it, and makes and signs the update archive), and notarises and staples the disk image. The Apple secrets are not set on the Windows and Linux jobs at all.
- The preflight sets the update key only on the step that signs a test file.

What cannot be split: the Tauri CLI (an npm package with a native program) holds the secrets while it signs and notarises, and `tauri-action` (pinned by hash) holds them while it uploads. That is the code that has to have them. The macOS keychain stays unlocked on the runner until the machine is thrown away, which is harmless on a GitHub-hosted runner and would not be on a self-hosted one.
