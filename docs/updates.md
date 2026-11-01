# Updates

Patient Partridge updates itself from GitHub Releases. This page says what the app does, what it sends, how a release is made and checked, and what has and has not been tried. Signing of the installers (Apple's certificate) is a separate thing: see [signing.md](signing.md).

## What the app does

- When it starts (after a few seconds) and then every 6 hours, the app fetches
  `https://github.com/Ferdinand99/Patient-Partridge/releases/latest/download/latest.json`.
- If `latest.json` names a newer version for this computer, a small pill appears in the toolbar ("Update 0.2.0"). Nothing else happens by itself. Pressing the pill shows the release notes and two buttons, **Later** and **Download and Install**.
- **Download and Install** downloads the update, checks its signature, stops every PDF engine, installs, and then asks to **Restart to Update**. The files that were open come back after the restart, at the place the reader had reached.
- The check is **on by default** and can be turned off with **Check for updates automatically** in the settings menu (the three dots). **Check for Updates…**, in the same menu, always checks when asked and says what it found.
- Only published releases are visible at `releases/latest`. The release workflow keeps a release as a draft until `latest.json` and the signatures are attached and checked, and then publishes it.

### What is sent

The update code runs in the app process. The window loads nothing from the network (its content security policy allows nothing but the app itself), and the PDF engine has no network code at all (CI checks that: `scripts/check-engine-deps.sh`).

- **The check:** one `GET` of the fixed address above. There is no version, system, document or identifier in the address or in extra headers. GitHub, like any web server, sees your IP address and the user agent `tauri-plugin-updater/<version>`.
- **The download:** only after you press **Download and Install**, one `GET` of the update file named in `latest.json` (GitHub forwards it to its own download servers).
- Documents, file names and the places you stopped at are never sent.
- With the setting off, the app makes no network connection at all, unless you choose **Check for Updates…**. The app refuses an automatic check itself when the setting is off; the window cannot override that.

The setting is kept in `settings.json` in the app's data folder, next to the file of remembered positions.

### Why the update cannot be a trick

- The update is signed with a minisign key (Tauri's updater). The public key is in the app (`plugins.updater.pubkey` in `src-tauri/tauri.conf.json`); the private key exists only as a repository secret. A download with a missing or wrong signature is thrown away before anything on the computer is touched.
- `requireSignedVersion` is on: the signature also covers the version it was made for, and the app refuses an answer whose version differs from the one signed. Without it, an attacker who could change `latest.json` could point a new version number at an older, genuinely signed release (a downgrade).
- The release notes in `latest.json` are not covered by the signature. The window shows them as plain text only, never as HTML, and the app cuts them at 20,000 characters.
- Updates come only from the address in the app's configuration; the window cannot choose another one. The page opened by "Open the releases page" is fixed in the app too.

## How each system updates

| System | Update file | What happens |
|---|---|---|
| Windows | NSIS installer (`…-setup.exe`) | The installer runs in "passive" mode (a progress bar, no questions) and starts the new version. The app ends when the installer starts. |
| macOS | `….app.tar.gz` (a universal app, Apple silicon and Intel) | The app bundle is replaced and the app restarts. |
| Linux, **AppImage** | the new `.AppImage` | The AppImage file you started is replaced and the app restarts from it. |
| Linux, **deb** | none | A deb package cannot update itself. The pill opens a notice with the release notes and a button that opens the releases page (https://github.com/Ferdinand99/Patient-Partridge/releases/latest) in the browser. |

### How a deb or other copy is recognised

`src-tauri/src/updates.rs` (`install_method`) decides, and errs on the side of "manual". A copy installs updates by itself only where it is positively recognised:

- **Windows:** always (the app is installed by the installers, and a development build never updates itself).
- **macOS:** the program must be inside `Something.app/Contents/MacOS/`.
- **Linux:** `$APPIMAGE` names an existing file, `$APPDIR` is set, **and the running program is inside `$APPDIR`**. The last part matters because the variables are inherited: a deb package started from a terminal that is itself an AppImage sees the variables of that terminal, but its program is in `/usr/bin`.

Everything else (a deb, a copy built from source, a program moved out of its bundle) is offered the notice and the link. The app also refuses the install on its own side, whatever the window asks for. `scripts/update-e2e.mjs --deb` runs the real deb with and without inherited AppImage variables.

### Stopping the engines, and the open files

Windows cannot replace a program that is running, and each open document has an engine process of its own in the install folder. So before the install:

1. The open files are written down (`restore-after-update.json` in the app's data folder, paths only, valid for one hour, read once at the next start and deleted).
2. A gate closes: no document can be opened any more, whether by the window, a drop, or a second copy of the app.
3. Every engine is stopped.
4. The opens that were already under way are waited for, and every engine is stopped again (one of them could have started an engine after the first stop).
5. The installer runs.

If the install fails after that, the gate opens again, and the window tells the person that the documents were closed and offers a restart, which brings them back. After a successful install on macOS and Linux the gate stays closed: the window asks for the restart (there is no "later", because the engines are gone).

This is tested with real engine processes (`real_engine_processes_are_gone_when_the_install_runs`), with a late open that starts its engine after the first stop, and, in the update test below, on each system.

## The update test (what was run for real)

`scripts/update-e2e.mjs` runs a whole update on the system it is started on, and the workflow `.github/workflows/update-e2e.yml` runs it on Linux, Windows and macOS:

1. A **throwaway signing key** is made (the script refuses to run when a real key is in the environment, and the workflow has no secrets).
2. Two builds of the real app are made, version 0.0.1 and 0.0.2, signed with it. They differ from a release build in three ways: the public key and the update address (`http://127.0.0.1`) in the configuration are the test's, link-time optimisation is off (only slower), and the Cargo feature `update-driver` is on. That feature (`src-tauri/src/updates.rs`, module `driver`, absent from release builds) lets the app press the buttons by itself: it runs `check` and `install`, the same functions the window's buttons call, and writes what happened to a log file. Everything else, from the download and the signature check to stopping the engines and replacing the program, is the code of the release.
3. A local server answers with `latest.json` and the new version. The old version is **installed the way a user has it** (an AppImage file; the NSIS installer run silently; an app folder), started with a PDF, and left to update itself.
4. Checked: the old version found the new one; a document was open; the engines were stopped (the app counts the open documents before the stop and when the install starts) before the install; the new version started and was handed the document that was open (`restored=1`); it reported itself up to date.
5. Refused, with the old version left as it was and the engines not stopped: a file changed after it was signed, an answer whose version is not the version the file was signed for, and a signature made with another key.
6. On Linux: the deb package is installed and run, with and without `$APPIMAGE` and `$APPDIR` inherited from another AppImage, and must report itself as manual and only offer the notice.

What this does **not** prove: the buttons of the window were pressed by the driver, not by a person (the window is tested separately: `src/app/updater.test.ts` and the browser tests in `e2e/updates.spec.ts`); the builds are not the signed and notarised release builds; `tauri-action` and the publishing of the draft (see below) only run in a real release; GitHub's own servers are not involved (no network use beyond 127.0.0.1); an update across a Gatekeeper-quarantined download on macOS, and the Windows installer in its default per-user mode on a PC with antivirus, were not tried.

## Release notes and the lockfile

- `tauri-action` leaves `notes` empty when it uploads to an existing release, so the `publish` job downloads `latest.json`, fills `notes` from the release body (the release-please changelog) with `scripts/update-notes.mjs`, and uploads it again before the draft is published. The window turns the markdown into plain lines (`notesToText`): links and commit hashes go, bullets stay.
- release-please cannot edit `Cargo.lock`. The `lockfile` job runs `cargo update --workspace` on the release PR branch and pushes a `chore: refresh Cargo.lock for the release` commit when the versions changed. Commits pushed with the default `GITHUB_TOKEN` do not start CI, so if status checks are required on the release PR, close and reopen it to run them.

## The release workflow

`.github/workflows/release-please.yml` (on every push to `main`, and by hand):

1. **release-please** keeps the release PR up to date. Merging that PR creates the tag and a **draft** release.
2. **preflight** (only for a new release) fails with a clear message before anything is built if `TAURI_SIGNING_PRIVATE_KEY` or one of the six `APPLE_*` secrets is empty, if the public key in the configuration is a placeholder, or if the signing key does not belong to the public key in the configuration (it signs a small file and checks the signature, `scripts/updater-key.mjs`). With a key that does not match, every installed copy would refuse every update.
3. **build-release** builds Windows (NSIS and MSI), macOS (one universal app and a disk image, signed and notarised) and Linux (deb and **AppImage**, on Ubuntu 22.04 so that the AppImage runs on older systems), one after the other: `tauri-action` adds each system to `latest.json` by downloading, changing and uploading it, and two jobs at the same time could lose an entry.
4. **publish** checks that `latest.json` and signatures exist, fills in the notes, checks that **every system is in `latest.json` with a signature and an asset** (`scripts/check-latest.mjs`), and only then publishes the draft.

The site (`site/`) is deployed by its own workflow (`pages.yml`), which this one does not touch.

### One-time setup (done)

The key pair was made by the owner on 2026-11-01. The public key is in `plugins.updater.pubkey`; the private key and its password are the repository secrets `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. The key of another project is not used. Keep a backup of the private key and the password in a password manager. **Never commit the private key.**

To make a new pair: `npm run tauri signer generate -- -w ~/.tauri/patient-partridge.key`, then put the content of the `.pub` file in `plugins.updater.pubkey` and the private key and password in the two secrets.

### Losing or changing the key

Installed copies accept only updates signed with the key whose public part they were built with. If the private key is lost or replaced, installed copies can **never** update automatically: people have to download and install a new version by hand, which carries the new public key. Change the key only if it was stolen, and announce the manual re-install.

### CI without secrets

`ci.yml` builds installers on all systems without the signing key. `scripts/prepare-bundle.mjs` then turns update files off (`bundle.createUpdaterArtifacts: false`), so those builds are green without secrets. They are for testing only and cannot be used for updates.

### Recovering from a failed release

The tag and the draft release already exist, and the merged release PR is labelled `autorelease: tagged`. After fixing the problem on `main`:

1. Delete the draft release and the tag: `gh release delete vX.Y.Z --cleanup-tag` (find the draft on the Releases page if `gh` cannot see it).
2. On the merged release PR, replace the label `autorelease: tagged` with `autorelease: pending`.
3. Run the latest `Release` workflow run on `main` again (or push any commit to `main`). release-please recreates the draft and the builds run again.

If only one build job failed (a flaky runner) and the preflight passed, use "Re-run failed jobs" on the workflow run instead.

## Dependencies and `cargo deny`

`tauri-plugin-updater` is pinned (`=2.14.1`). The Nimble Newt project ignores three advisories about `zip` older than 9 (RUSTSEC-2026-0367, -0368, -0369) because its updater 2.14.0 pulls in `zip` 4. They do **not** apply here and are not ignored: 2.14.1 uses `zip` 9 (and only on Windows), and `cargo deny check` is clean. The advisories about unmaintained crates of the Linux stack (RUSTSEC-2024-0370, 2026-0370 to -0372) were already ignored in `deny.toml` for the same reasons.

## Troubleshooting

| What the window says | Cause | What to do |
|---|---|---|
| "could not reach the update server" | No internet, a proxy or firewall, or GitHub is down | Try again later; the app works as usual |
| "does not carry the right signature" | The update was signed with another key, was changed on the way, or its version does not match the signed one | Do not install it. Check the release; if the key changed, see "Losing or changing the key" |
| "Updates are not set up in this build" | No endpoint in the configuration | Build from the repository's configuration |
| "update information could not be read, or has nothing for this computer" | `latest.json` is missing, is not valid, or has no entry for this system (also while no published release exists) | Check that the latest published release has a `latest.json` with all four systems |
| "is up to date" | Nothing newer is published | None |
| "An update is being installed" when opening a file | The engines were stopped for an update | Restart the app |
