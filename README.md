# Patient Partridge

An open source PDF reader and editor for Windows, macOS and Linux. It is meant to be fast and small, and to work without an account, a cloud, advertising or tracking.

Today it reads PDFs. The editing tools are planned and do not exist yet.

Patient Partridge is part of the Hedgerow family of apps, next to [Nimble Newt](https://github.com/Ferdinand99/Nimble-Newt).

**Status: in development.** There is no release yet and nothing to install. Expect rough edges and missing features.

## What works now

- Open one or more PDFs from the Open button (Ctrl+O), by dropping files on the window, from the command line, or with "Open with" from a file manager. Each file opens in its own tab (Ctrl+W closes a tab, Ctrl+Tab and Ctrl+Shift+Tab move between tabs). Opening a file that is already open shows its tab.
- Read in one continuous scrolling view. Only the pages you can see (and a few around them) are drawn, so opening is quick and memory stays flat as documents get longer. A tab you are not looking at gives up its drawn pages.
- A side panel (Ctrl+B) shows page thumbnails and the table of contents of the PDF (its bookmarks). Thumbnails are drawn only for the part of the list you can see, and only after the page you asked for is on screen, so the number of pages does not slow down opening (measured with a 500-page file; larger ones have not been timed). Entries of the contents that point to a web address or a file are shown but never followed.
- Select text with the mouse (double click for a word, triple click for a line, Ctrl+A for everything) and copy it with Ctrl+C, the system's Copy, or the Copy entry of the right mouse button. The text and the place of every character come from PDFium through the engine; nothing is read from the file by the window.
- Search in the document (Ctrl+F). The matches are highlighted on the pages, Enter and Shift+Enter (or F3) move between them, and the count is shown. Search ignores upper and lower case and line breaks, and runs page by page in the background, so you can stop it at any time. Words that a PDF writes with a ligature such as "fi" in one character may not be found.
- Links inside a PDF are never opened by themselves. Pressing one shows the whole address and asks first. Only web addresses (http and https) can be opened, in your normal browser; anything else (a file, a program, a script, a mail address) is refused with an explanation. Links to another page of the same document just go there. Attachments in a PDF are never opened or even listed.
- Print (Ctrl+P): all pages, the page in view, or a list such as 1-3, 7, 9-. The pages are drawn by the engine at 150 dpi, packed as JPEG pictures one at a time (a few hundred kilobytes each) and then the print dialog of your system opens; nothing is sent anywhere else. A printed page is a picture of the page, not text. This is covered by unit tests with a fake printer and by a browser test that checks what is shown in print media; it has not been tried on a real printer on any system yet.
- Zoom in and out, or fit the page to the window width. Ctrl with the mouse wheel zooms too.
- Move with the mouse wheel, the arrow keys, Page Up and Page Down, Space, Home and End, or jump to a page number (Ctrl+G).
- Patient Partridge remembers where you stopped in each file and puts you back there when you open it again. The place (page, how far down, zoom) is kept in a small file on your computer, together with the path of the file, for the last 500 files. Nothing is sent anywhere. "Forget remembered positions" in the settings menu (the three dots) empties it.
- The tabs you had open come back the next time you start the app, in the same order with the same tab in front, each at its remembered place. Only the file paths are kept, in the same folder as the positions. A file that has gone is skipped with one short notice, and a password-protected file stays closed until you open it. "Reopen tabs at start" in the settings menu turns this off; that, and "Forget remembered positions", delete the list.
- The look follows the system's light or dark setting. The settings menu has Auto, Light and Dark. The pages themselves stay white, like paper.
- Password-protected files ask for the password. A damaged file, or a file that is not a PDF, gives a clear message instead of a crash.
- The installers register Patient Partridge as a viewer for PDF files, so it appears under "Open with". It does not take over as the default reader by itself.
- **Saving, as groundwork for the editing tools.** Nothing in the app can change a document yet, so there is nothing to save, but the way a change will be saved is built and tested. Ctrl+Shift+S (Save a copy) writes the open document to a new file that you choose, and the tab then follows that file; Ctrl+S saves a document that has changes (none can have any yet). How a save works, whatever the document: the engine, which still writes nothing, has PDFium put the whole document into memory and hands the bytes to the app piece by piece; the app writes them to a new file in the same folder and flushes it to the disk; a second, sandboxed engine opens that new file and checks it (it opens with the same password, has the same number of pages, is protected the same way, holds the same signatures); and only then does the new file replace the old one in a single step. If anything goes wrong on the way (a full disk, a crash, a file that does not pass the check) the file on disk is exactly as it was. A read-only file is not replaced, a link is followed, and the file keeps its permissions. If something else changed the file after you opened it, you are asked first: replace it, save a copy, or cancel. A password-protected document stays protected with the same password, or the save is refused with a message. A document with a digital signature is not saved without a warning (see Known limits).
- A tab with changes that are not saved shows a dot, and the window title starts with a dot. Closing the tab, closing the window or quitting asks first (Save, Don't save, Cancel) and holds the request back until you answer. An update waits for you too: it is downloaded, but it is not installed while a document has changes that are not saved; you can save and install in one step, or install later from the toolbar. The app itself refuses to install while any document has unsaved changes. The model for undo and redo (Ctrl+Z, Ctrl+Y or Ctrl+Shift+Z) is in the window code for the editing tools to use.
- Updates. Patient Partridge looks for a newer version when it starts and every 6 hours (a small pill appears in the toolbar), and **Check for Updates…** in the settings menu does it on request. The release notes are shown first; nothing is downloaded until you press Download and Install. The update is checked against the project's signing key before anything is installed, the PDF engines are stopped for the install, and the files you had open come back after the restart. The automatic check is on by default and can be turned off in the same menu. The Windows installer, the macOS app and the Linux AppImage update this way; a Linux deb package cannot, and shows a notice with a link to the releases page instead. How it works, what is sent and what has been tried: [docs/updates.md](docs/updates.md).

## What is not there yet

The editor side is planned, and none of it exists yet. There are no dates; each step is done when it works:

- Annotations, filling in forms, creating fillable forms, and digital signatures (viewing, validating and signing).
- Editing text and images on the page.
- Real redaction that removes the content, not just covers it.
- Merging, splitting, reordering and cropping pages.
- Flattening a document, so annotations and form fields become part of the page.
- Compressing files.
- Converting to and from Word, Excel, PowerPoint and images, locally, and converting images (JPG, PNG, HEIC), text and Office files to PDF.
- Text recognition (OCR) for scanned documents, locally.
- Watermarks, page numbers, password protection, and removing a password you already know.

## Known limits

- **Memory.** Measured on Linux (WebKitGTK, software rendering) with a 500-page file open: the whole app, all its processes together, uses about 490 MB of memory (proportional share), of which about 325 MB is the web view and about 14 MB the engine. The goal of 150 MB is not met there. Windows and macOS have not been measured on real hardware; their web views behave differently.
- **Web view.** The look uses CSS `light-dark()`, so the web view must be recent: WebView2 (always up to date), WebKitGTK 2.46 or newer, and a macOS version whose web view is Safari 17.5 or newer.
- **Search** finds text as PDFium reports it. It does not join words split by a hyphen at the end of a line, and it does not match a ligature character against its letters. A search stops after 20,000 matches and says so (for example "20,000+ (stopped at the limit)"); a page with more than 2,000 matches shows the first 2,000 and the find bar says that some pages have more.
- **Text selection** follows the order of the text in the file. In documents with several columns that are stored in an odd order, a selection can jump between columns. Right-to-left text (Hebrew, Arabic) is selected from its right end, in reading order; text that runs sideways or up and down is not placed well yet.
- **Text layer.** For each document the window keeps the text of at most 1.5 million characters (about 60 MB) for the pages on screen, and the addresses of their links up to 1 million characters. A page that would go past that is drawn as usual but has no text layer for the moment: its text cannot be selected, searched on the page or pressed as a link, and a message says so once. This only happens with files made to do it, such as thousands of tiny pages that each hold the most text a page can have.
- **Copying** gathers at most 20 million characters at a time, one copy at a time for each document. When a copy is cut short, or a page cannot be read, or a page has more text than the engine hands over (100,000 characters), a message says what is missing. On some systems the web view allows the text to be put on the clipboard only straight after a key press; if it refuses, a message says so.
- **Printing** sends pictures of the pages, at 150 dpi, to the system's print dialog. Pages of different sizes are fitted to the paper. One print job holds at most 1,000 pages: for a longer document the dialog offers "The first 1000 of N pages" instead of "All", and a list of pages can be printed in parts.
- **Fonts that a PDF does not embed.** The engine runs in a sandbox that does not let it open files, so it cannot look at the fonts installed on your system. It uses the fonts built into PDFium, which cover Latin text in the usual faces (Helvetica, Times, Courier and anything named like them). Text in a script that needs a font the file does not carry (Chinese, Japanese, Korean, Arabic, Hebrew, Thai) shows as blanks. Almost every PDF made for print or the web embeds its fonts and is not affected. Fonts put in a `fonts` folder next to the PDFium library are read at start and used; the app does not ship any yet.
- **Saving and digital signatures.** A save always writes the whole document again. PDFium has an incremental save that appends the changes to the original bytes and so keeps a signature valid, but the Rust binding used here (pdfium-render, 0.9.5 and the newest, 0.9.6) always saves without it, and the document handle it would need is not public. So a save of a signed document breaks the signature: programs that check it will say the document was changed after it was signed. The app asks before it does that (Cancel, Save a copy, Save anyway). It cannot tell whether a signature allows the change; it warns for every signed document. The signature itself stays in the file. Validating signatures is not there yet.
- **Saving, size and the file system.** A document is saved only if the saved file would be at most 512 MiB (the engine builds it in memory). A save needs room for a second copy of the file on the same disk. Replacing a file keeps its permissions (on Linux and macOS the mode; on Windows only the read-only attribute, and the new file gets the access rights of its folder) but not its owner, and not extended attributes such as macOS tags and the quarantine flag; a hard link to the file becomes a separate file; a symbolic link is followed and stays a link. On a network drive the replacement is as safe as the drive makes it: the new file is written and flushed next to the old one and checked before the rename, so a lost connection leaves the old file, but a drive that cannot rename over an open file refuses the save. "Changed by something else" is judged by size, time of change and, where the system has one, the file identity; the last check comes just before the rename and cannot close the gap of that moment. A program that was killed in the middle of a save can leave a file named like `.name.pdf.partridge-123-456.tmp` next to the document; the next save of that document removes it once it is a day old. For a password-protected document the app keeps the password in memory while the document is open, to check the saved file.
- **Reloading the window** (F5, Ctrl+R, the context menu of the page) is switched off in release builds, because a reload would lose the tabs. The right mouse button on selected text opens a small menu of the app with Copy; text boxes keep the menu of the web view (Cut, Copy, Paste). There is no menu on the page itself, so there is no Paste or Select all by mouse outside the boxes. Whatever else restarts the page, the app ends the engines of the old page and starts clean.

## How it is built

- The window is a [Tauri 2](https://tauri.app/) app written in TypeScript (Vite) with a small Rust core.
- PDF files are read by a separate program, `partridge-engine`, which uses [PDFium](https://pdfium.googlesource.com/pdfium/) (the PDF library of Chrome) through [pdfium-render](https://crates.io/crates/pdfium-render). The app starts it and talks to it over standard input and output with a small binary protocol.
- A PDF is untrusted input. Every open document is read by an engine process of its own, so a file that crashes or hangs its engine closes only its own tab: you get an error message, the other tabs carry on, and you can open another file. At most 16 documents are open at once.
- PDF JavaScript is turned off. The PDFium build used has no JavaScript engine at all.
- The engine runs in an operating system sandbox (see Security below): it cannot open files or start programs (and on Linux and macOS it cannot use the network; on Windows it can, see Security below), and it reads the one document the app opens for it through a read-only handle that the app passes on at start.
- The engine writes nothing, also when a document is saved: it serialises the document into memory and the app writes the file (see Saving above). The protocol messages for that (`DocInfo`, `SaveBegin`, `SaveChunk`, `SaveEnd`) are covered by the fuzz targets, with the caps of the file size and of one piece.
- The engine code has no network code, and a check in CI keeps it that way. The window loads nothing from the internet (strict content security policy). **The only network use of the app is the update:** it checks GitHub for a newer version when it starts and every 6 hours, and downloads one only when you press Download and Install. The check can be turned off in the settings menu, and documents are never sent anywhere (see [docs/updates.md](docs/updates.md) for exactly what is sent). The one other thing that leaves the app is a link you say yes to: your normal browser is then started with that address. A link in a PDF goes nowhere unless you say yes: your normal browser is then started with that address.

- The PDFium library is downloaded from [bblanchon/pdfium-binaries](https://github.com/bblanchon/pdfium-binaries) at a pinned release, and its SHA-256 checksum is checked before it is used (`pdfium.lock.json`).

```
crates/protocol       the message format between app and engine
crates/engine         partridge-engine, the only code that parses PDFs
crates/engine-client  starts the engine, talks to it, recovers when it crashes
crates/testkit        generates small test PDFs (also a broken one and a password-protected one)
src-tauri             the app process (Rust)
src                   the window (TypeScript)
scripts               PDFium download, bundling, measurements
```

## Security: what is protected and what is not

Please read this before you open files from people you do not trust.

**What the sandbox does.** PDFium is the main attack surface: a large C++ library that parses files nobody has checked. Every open document has an engine process of its own, and that process locks itself into an operating system sandbox before it reads the first byte of the document. If an attacker finds a bug in PDFium and builds a PDF that exploits it, the code runs inside that sandbox:

- **It cannot open files.** The engine does not open files at all. The app opens the document and hands the engine a read-only handle to that one file when it starts the engine. The engine can read that file and nothing else, cannot write or create files, and cannot see the other open documents (each has its own engine). One limit: on Windows the protection comes from file permissions, and FAT and exFAT volumes (many USB sticks and SD cards) have none, so a program in the engine could read, write and delete files there if it knew the path. NTFS and the file systems of macOS and Linux are protected. On Linux the system call filter cannot read a path, so where the system does not allow the extra layer below (an empty root file system, which needs user namespaces) the engine can still ask whether a path exists and how big it is, but not read it.
- **It cannot start programs or touch other processes**, and on Linux and macOS it **cannot use the network**. The engine's code contains no network code (a check in CI keeps that), and on Linux and macOS the sandbox stops the operating system from letting any code in the process make a socket.
- **It cannot use more than 3 GiB of memory, by different means.** Windows: the job object, in the kernel. Linux: a data limit, and the filter refuses shared, grow-down and executable memory, which the limit would not count. macOS: the system does not enforce limits on memory, so the app watches the engine from outside and ends it above 3 GiB; the engine has a watch of its own as a first line, but code that has taken the engine over can stop that one, and the app's cannot be stopped from inside. This is a watch, not a wall: it looks about every 25 milliseconds (longer when the computer is busy), so what it promises is that an engine that stays above 3 GiB is ended within a fraction of a second, and what it does not promise is anything about memory used for less time than that. An engine can go past the limit by what it manages to take between two looks (a few hundred megabytes on current hardware), and one that takes memory and lets go of it again, or ends, within one look is never ended, though it also leaves nothing behind. The limit counts the memory footprint of the engine, the figure the system uses to choose which process to end when memory is short (compressed memory counts), not address space. A test in CI starts a program that takes three times the limit and keeps it, with the engine's own watch switched off, and requires that the app ends it from outside.
- **It knows nothing of the app's environment.** The engine is started with an empty environment (on Windows only the system folder), so tokens and keys in the shell the app was started from do not reach it.
- **How, per system.** Linux: a seccomp filter that allows only the few system calls the engine needs (always applied; nothing that opens files, creates sockets, starts processes, traces other processes, sends signals or makes memory executable or shared), plus Landlock where the kernel has it (5.13 or newer), plus an empty root file system (a user and a mount namespace) where the system allows it; the engine tells the app which of these ran. macOS: a Seatbelt profile that denies everything by default, the mechanism browsers use for their renderers. Windows: a restricted token with no privileges and no accessible group, the untrusted integrity level, a job object of its own (one process, a memory limit, no child processes, no clipboard, no user interface access), a desktop of its own (the engine cannot see the windows of the user, read the screen or draw over their windows; if Windows refuses to create the desktop, the engine runs on your desktop instead and the app neither tells you nor stops it), process mitigation policies (no dynamic code, no extension points, strict handle checks and others), and an explicit list of the four handles the engine inherits.
- **It checks itself.** After the sandbox is applied the engine tries to open a file and must be refused; on Linux and macOS it also tries to use a socket and must be refused. On Windows the engine also tries a socket and the network device, but only to report whether the network is blocked: it is not, so no network layer is reported and the engine goes on (see below). A release build that cannot show a working sandbox does not open the document and tells you so. The app also refuses an engine that does not report a sandbox. A debug build, used for development only, may go on without one.

**What it does not cover.**

- **On Windows the engine can use the network.** The restricted token does not stop it: a test in CI found that a process with this token can still make a socket and connect (on Windows 11 and Server, checked by connecting to a local listener and by opening the network device with a native call). The sandbox on Windows therefore keeps files, windows, the clipboard, other processes and memory away from the engine, but an exploited engine could send out what it can read, which is the document it was given and what it can reach inside its own process. An AppContainer would stop it (a test in CI shows that a process in an AppContainer without capabilities cannot connect), but it needs the installed files to be readable by "all application packages" and is not built yet. This is the largest open item of the Windows sandbox.
- **On Windows the engine can clear the clipboard**, though not read it or put anything on it.
- **The app writes the saved files, and the bytes it writes come from the engine.** They are untrusted: the size and every piece are checked against the caps, and a new sandboxed engine has to open the written file before it replaces the old one. That check shows that the file opens and is the same document in page count, protection and signatures; it does not show that every page looks the same.
- **The app itself is not sandboxed.** The window, the web view and the code that starts the engines run with your user rights. The sandbox protects you from a bad PDF, not from a bug in the app.
- **The engine can still read the document it was given** and send back whatever it likes. The handle also tells a program that can ask which file it is on some systems (Windows for sure): the folder the document is in, and so the user's name. The app treats every answer from the engine as untrusted (sizes are checked, frames are limited, text only reaches the window as text), but a bug in that handling would be reachable from a hostile PDF.
- **A bug in the operating system kernel** can be used to leave a sandbox. This is true of every sandbox. On Windows the win32k system calls (a large part of that surface) are not switched off for the engine, because PDFium's DLL does not load without them; on Linux the system call list is short.
- **Weaker spots by system.** Windows has no AppContainer (the restricted token is the older mechanism and is what keeps files away; it does not keep the network away). macOS uses the `sandbox_init` call, which Apple has marked deprecated but still uses itself and which browsers rely on; it is not the App Sandbox. On Linux without Landlock only the seccomp filter protects, and it does that completely for opening files, but not against a kernel bug.
- **CPU time and speed side channels** are not limited beyond the 60 second answer timeout, and attacks such as Spectre are not mitigated by the sandbox.
- **The sandbox starts after PDFium and the fonts are loaded**, and loading is not protected. Both come from the app's own folder, and PDFium is checked against a pinned checksum.
- **Documents the engine opens are untrusted by design; PDFium itself is not tested for correctness.** The sandbox limits the harm of a PDFium bug. It does not fix it.
- PDFium is pinned to one release and updated by hand ([steps](docs/pdfium-updates.md)); a workflow that runs twice a week keeps one issue open while a newer release exists, and CI checks that the library that ships has no JavaScript engine (V8).

Other protections: the engine ends when the app ends (even when stuck inside PDFium), and PDFium is loaded only from the app's own folder after a checksum check at build time.


## Building from source

You need Rust (stable), Node.js 22 and, on Linux, the [Tauri system packages](https://tauri.app/start/prerequisites/).

```
npm ci
npm run prepare:dev     # downloads PDFium (checked) and builds the engine
npm run tauri dev
```

Tests:

```
npm test                          # TypeScript unit tests
cargo test --workspace --locked   # needs the PDFium download from prepare:dev
npm run build && npx playwright install chromium && npm run e2e   # the window in a real browser
```

The browser tests (`e2e/`) run the real window code in Chromium with the content security policy of the app and a stand-in for the Rust side that serves two made-up documents. They cover tabs, the light and dark look, thumbnails and contents, selecting and copying with the mouse, search, the question before a link is opened, and printing. They do not start the Rust side, the engine or a real web view; the CI job "Build" does that for the release build (it opens a generated 500-page PDF and checks that the first page is drawn).

The protocol decoders and the engine (with PDFium) are also fuzzed: see [docs/fuzzing.md](docs/fuzzing.md). The app and the engine sandbox are also run on Windows as a standard user, not only as the administrator of the CI runner: see [docs/windows-standard-user.md](docs/windows-standard-user.md).

Installers for testing (not signed, no update files): `npm run bundle`. Releases are made by the release workflow, with the macOS app signed and notarised ([docs/signing.md](docs/signing.md)) and the update files signed ([docs/updates.md](docs/updates.md)). An update can be tried from start to end on one computer with `node scripts/update-e2e.mjs` (it makes a throwaway key, builds two versions and lets the old one update itself; see docs/updates.md).

## Measuring speed and memory

`scripts/measure.sh` opens a generated 500-page PDF and prints how long the engine and the whole app take, and how much memory they use. It is written for Linux and needs `xvfb` for the app part.

## Licence

Patient Partridge is free software under the [GNU Affero General Public License, version 3](LICENSE). PDFium is under a BSD-style licence; its licence files are shipped with the app.

The licence texts of the Rust crates and npm packages inside the app are in [THIRD-PARTY-NOTICES](THIRD-PARTY-NOTICES), which every installer ships next to the AGPL text. The file is made by `scripts/notices.sh` (cargo-about, pinned) and CI fails when it is out of date or when a dependency has a licence that is not on the allowlist in `about.toml`. After changing a dependency, run `npm ci && scripts/notices.sh` and commit the result.
