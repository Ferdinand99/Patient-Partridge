# Updating PDFium

PDFium is the main attack surface of Patient Partridge: it parses files from people you may not trust. The app ships one pinned build of it, taken from [bblanchon/pdfium-binaries](https://github.com/bblanchon/pdfium-binaries). Nothing updates it by itself, so it is updated by hand, and a workflow tells us when a newer build exists.

## How you find out

The workflow "PDFium update check" (`.github/workflows/pdfium-update.yml`) runs on Tuesday and Friday and can be started by hand from the Actions tab. It reads `build` in `pdfium.lock.json` and compares it with the latest release of bblanchon/pdfium-binaries. Upstream publishes about once a week, mostly on Monday, so a release is seen within a few days.

- There is **at most one open issue** about this, titled `PDFium update available: chromium/NNNN`. When a newer release appears and an issue is open, the workflow changes that issue's title and text to the newest build and adds a comment (a changed title sends no notification, a comment does) instead of opening another issue. It opens a new issue only when none is open.
- Only issues that the workflow itself opened count (author `github-actions`). The repository is public, so anyone can open an issue with any title and close it; such an issue is ignored and can not switch off the alert for a build.
- **Close the issue as "not planned" to skip this build**: the workflow then does not open an issue again for the same build, and the next, newer build gets a new issue. Closing it as completed (what `Closes #N` in a merged pull request does) does not skip anything. This matters because the issue may have been renamed to a newer build after the work on an older one started.
- If it can not read the release list, or the latest tag does not look like `chromium/NNNN`, the run fails. A failed scheduled run is visible in the Actions tab and in the notification mail.
- It also fails when `pdfium.lock.json` and the `pdfium_NNNN` feature in `crates/engine/Cargo.toml` name different builds. The two must always match. A unit test (`scripts/pdfium-update-check.test.mjs`) checks the same.
- **GitHub switches off scheduled workflows in a public repository after 60 days without repository activity.** A quiet project would then stop getting these warnings without any message. Check the Actions tab now and then, and after a long quiet time open the workflow in the Actions tab and press "Enable workflow" (or run `gh workflow enable pdfium-update.yml`). A disabled workflow can not be started by hand until it is enabled.

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
6. **Merge, then close the issue.** The pull request can say `Closes #N`; that closes the issue as completed and does not skip a newer build. If you decide not to update, close the issue as "not planned": that skips this build only.

## What the V8 check does

`scripts/check-pdfium-v8.mjs` searches each PDFium library for the symbol names that only a V8 build has (`_ZN2v8`, `@v8@`, `v8::`, `V8Flags`). A plain build has none of them, and the V8 build of every platform has hundreds. It runs when the library is fetched, and again on the library inside each installer built by CI. It fails when it finds no library at all, so it can not pass by looking in the wrong folder.

## What the annotation tests rely on

Some of what the README promises about annotations is a fact about PDFium, and the engine tests (`crates/engine/tests/annots_from_files.rs`, `annots_readers.rs`) would show a change in it:

- A save leaves out the objects of a loaded file that nothing refers to any more, and writes the objects made during the session as they are, whether or not anything refers to them. That is why the text of a deleted comment is not in the saved file, why the text of an annotation made in the session is overwritten before it is removed, and why a save writes the document, reads it in again and writes it a second time (`save::serialise_clean`): the second writing leaves out what was made in the session and then taken off. The tests in `crates/engine/tests/save_clean.rs` look at the streams of the saved file for a picture of a signature, a drawing, a stamp and a note that were put on and taken off, and fail if the second writing stops dropping them.
- A free text box that came without an appearance stream gets one, with its words, when the page is first drawn, and no call can overwrite it. It is written in the session like the rest, so the second writing of a save leaves it out once the box is deleted. The test `a_free_text_box_without_an_appearance_leaves_its_words_out_of_the_saved_file_after_a_delete` states that.
- A closed popup gets no appearance (`CPDF_Annot::ShouldGenerateAP`). Older builds drew the comment into one for every draw, which would leave every old version of a comment in the saved file. The tests may not notice a change here, because such an appearance would hold the comment letter by letter, and the text search in the tests looks for whole words. On each update, read `ShouldGenerateAP` for the popup and check that a closed popup still gets no appearance.
- `FPDF_GetDocPermissions` gives all rights to a document that was opened with its owner password and the stored permissions to any other, while `FPDF_GetDocUserPermissions` always gives the stored permissions. The engine uses the first one for annotating, so an owner who typed the password may annotate. The test `the_owner_password_allows_what_the_user_password_does_not` fails if that changes.
