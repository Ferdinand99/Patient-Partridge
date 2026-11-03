# Development

How to build, test and measure Patient Partridge. The [README](../README.md#building-from-source) has the short version.

## Building

You need Rust (stable), Node.js 22 and, on Linux, the [Tauri system packages](https://tauri.app/start/prerequisites/).

```
npm ci
npm run prepare:dev     # downloads PDFium (checked) and builds the engine
npm run tauri dev
```

## Tests

```
npm test                          # TypeScript unit tests
cargo test --workspace --locked   # needs the PDFium download from prepare:dev
npm run build && npx playwright install chromium && npm run e2e   # the window in a real browser
```

The browser tests (`e2e/`) run the real window code in Chromium with the content security policy of the app and a stand-in for the Rust side that serves two made-up documents. They cover tabs, the light and dark look, thumbnails and contents, selecting and copying with the mouse, search, the question before a link is opened, and printing. They do not start the Rust side, the engine or a real web view; the CI job "Build" does that for the release build (it opens a generated 500-page PDF and checks that the first page is drawn).

The protocol decoders and the engine (with PDFium) are also fuzzed: see [fuzzing.md](fuzzing.md). The app and the engine sandbox are also run on Windows as a standard user, not only as the administrator of the CI runner: see [windows-standard-user.md](windows-standard-user.md).

## Installers and releases

Installers for testing (not signed, no update files): `npm run bundle`. Releases are made by the release workflow, with the macOS app signed and notarised ([signing.md](signing.md)) and the update files signed ([updates.md](updates.md)). An update can be tried from start to end on one computer with `node scripts/update-e2e.mjs` (it makes a throwaway key, builds two versions and lets the old one update itself; see [updates.md](updates.md)).

## Third-party notices

The licence texts of the Rust crates and npm packages inside the app are in [THIRD-PARTY-NOTICES](../THIRD-PARTY-NOTICES), which every installer ships next to the AGPL text. The file is made by `scripts/notices.sh` (cargo-about, pinned) and CI fails when it is out of date or when a dependency has a licence that is not on the allowlist in `about.toml`. After changing a dependency, run `npm ci && scripts/notices.sh` and commit the result.

## Measuring speed and memory

`scripts/measure.sh` opens a generated 500-page PDF and prints how long the engine and the whole app take, and how much memory they use. It is written for Linux and needs `xvfb` for the app part.

## PDFium updates

PDFium is pinned to one release and updated by hand: see [pdfium-updates.md](pdfium-updates.md).
