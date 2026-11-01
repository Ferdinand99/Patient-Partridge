# Updating PDFium

PDFium is the main attack surface of Patient Partridge: it parses files from people you may not trust. The app ships one pinned build of it, taken from [bblanchon/pdfium-binaries](https://github.com/bblanchon/pdfium-binaries). Nothing updates it by itself, so it is updated by hand, and a workflow tells us when a newer build exists.

## How you find out

The workflow "PDFium update check" (`.github/workflows/pdfium-update.yml`) runs on Tuesday and Friday and can be started by hand from the Actions tab. It reads `build` in `pdfium.lock.json` and compares it with the latest release of bblanchon/pdfium-binaries. Upstream publishes about once a week, mostly on Monday, so a release is seen within a few days.

- There is **at most one open issue** about this, titled `PDFium update available: chromium/NNNN`. When a newer release appears and an issue is open, the workflow changes that issue's title and text to the newest build instead of opening another one. It opens a new issue only when none is open.
- Closing the issue means "skip this build": the workflow does not open an issue again for the same build. The next, newer build gets a new issue.
- If it can not read the release list, or the latest tag does not look like `chromium/NNNN`, the run fails. A failed scheduled run is visible in the Actions tab and in the notification mail.
- It also fails when `pdfium.lock.json` and the `pdfium_NNNN` feature in `crates/engine/Cargo.toml` name different builds. The two must always match. A unit test (`scripts/pdfium-update-check.test.mjs`) checks the same.
- **GitHub switches off scheduled workflows in a public repository after 60 days without repository activity.** A quiet project would then stop getting these warnings without any message. Check the Actions tab now and then, and start the workflow by hand after a long quiet time (this also re-enables it).

The workflow has read-only rights. Only its last job, which opens or updates the issue, has `issues: write`, and nothing else. The issue text is built only from a release tag that matched `chromium/<digits>`, numbers and a plain version number; everything else from the network is left out and never put into a shell command.

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
   - If `Cargo.lock` changed (a new pdfium-render does that), regenerate the third-party notices and commit them: `npm ci && scripts/notices.sh`. The "Third-party notices" check in CI fails while `THIRD-PARTY-NOTICES` is stale. The same is true for every dependency update.
5. **Test.** Open a pull request; CI does the rest, on Linux, Windows and macOS:
   - `node scripts/fetch-pdfium.mjs` refuses a download whose checksum differs, and a library that contains V8.
   - The engine tests run against the real library.
   - The build job opens a generated 500-page PDF with the release build.
   - The build job unpacks the real installers and checks that the shipped library has no V8, and that each installer (deb, dmg, msi and NSIS setup, one by one) carries `THIRD-PARTY-NOTICES`, the AGPL text and the PDFium licence texts.
6. **Merge, then close the issue.** The pull request can say `Closes #N`. If you do not update, closing the issue skips that build only.

## What the V8 check does

`scripts/check-pdfium-v8.mjs` searches each PDFium library for the symbol names that only a V8 build has (`_ZN2v8`, `@v8@`, `v8::`, `V8Flags`). A plain build has none of them, and the V8 build of every platform has hundreds. It runs when the library is fetched, and again on the library inside each installer built by CI. It fails when it finds no library at all, so it can not pass by looking in the wrong folder.
