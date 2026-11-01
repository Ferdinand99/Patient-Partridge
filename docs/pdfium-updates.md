# Updating PDFium

PDFium is the main attack surface of Patient Partridge: it parses files from people you may not trust. The app ships one pinned build of it, taken from [bblanchon/pdfium-binaries](https://github.com/bblanchon/pdfium-binaries). Nothing updates it by itself, so it is updated by hand, and a workflow tells us when a newer build exists.

## How you find out

The workflow "PDFium update check" (`.github/workflows/pdfium-update.yml`) runs every Monday and can be started by hand from the Actions tab. It reads `build` in `pdfium.lock.json` and compares it with the latest release of bblanchon/pdfium-binaries.

- If the latest release is newer, the workflow opens **one issue** titled `PDFium update available: chromium/NNNN`. It does not open a second one for the same release, also not when the first issue is closed (closing it is how you say "skip this build"). The next release gets a new issue.
- If it can not read the release list, or the latest tag does not look like `chromium/NNNN`, the run fails. A failed scheduled run is visible in the Actions tab and in the notification mail.
- It also fails when `pdfium.lock.json` and the `pdfium_NNNN` feature in `crates/engine/Cargo.toml` name different builds. The two must always match. A unit test (`scripts/pdfium-update-check.test.mjs`) checks the same.

The workflow has read-only rights. Only its last job, which opens the issue, has `issues: write`, and nothing else.

## Update steps

1. **Read what changed.** Open the release in the issue and read the notes. Look for security fixes in the PDFium range since the pinned build; an update with a security fix should not wait.
2. **Check that pdfium-render knows the build.** The engine uses PDFium through [pdfium-render](https://crates.io/crates/pdfium-render), which has one Cargo feature per supported build (`pdfium_8125`, ...). Look at the feature list of its newest version on crates.io.
   - If there is a feature for the new build, use it.
   - If the newest feature is older than the new build, **pdfium-render lags behind PDFium**. Either wait for a new pdfium-render release, or try the newest existing feature with the new library: it only works if PDFium did not change the functions that pdfium-render calls. Run the whole engine test suite and the browser tests before you trust it. Do not use `pdfium_future` or `pdfium_latest`; the build must be named exactly.
3. **Get the new checksums.** Download the six plain assets of the release (`pdfium-linux-x64.tgz`, `pdfium-linux-arm64.tgz`, `pdfium-mac-arm64.tgz`, `pdfium-mac-x64.tgz`, `pdfium-win-x64.tgz`, `pdfium-win-arm64.tgz`) and compute SHA-256 yourself (`sha256sum`). Use the assets **without** `v8` in the name: the V8 builds contain a JavaScript engine, and PDF JavaScript must stay off.
4. **Change the files, together.**
   - `pdfium.lock.json`: `release` (`chromium/NNNN`), `build` (the number), the six `sha256` values, and the build number in the `comment`.
   - `crates/engine/Cargo.toml`: the `pdfium_NNNN` feature, and the version of pdfium-render if you moved it. Keep the `=` pin. Run `cargo update -p pdfium-render --precise <version>` if the version changed, and commit `Cargo.lock`.
   - The `pdfium_NNNN` feature must equal `build`. The check and the unit test fail if it does not.
5. **Test.** Open a pull request; CI does the rest, on Linux, Windows and macOS:
   - `node scripts/fetch-pdfium.mjs` refuses a download whose checksum differs, and a library that contains V8.
   - The engine tests run against the real library.
   - The build job opens a generated 500-page PDF with the release build.
   - The build job unpacks the real installers and checks that the shipped library has no V8.
6. **Merge, then close the issue.** The pull request can say `Closes #N`.

## What the V8 check does

`scripts/check-pdfium-v8.mjs` searches each PDFium library for the symbol names that only a V8 build has (`_ZN2v8`, `@v8@`, `v8::`, `V8Flags`). A plain build has none of them, and the V8 build of every platform has hundreds. It runs when the library is fetched, and again on the library inside each installer built by CI. It fails when it finds no library at all, so it can not pass by looking in the wrong folder.
