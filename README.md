# Patient Partridge

An open source PDF reader and editor for Windows, macOS and Linux. It is meant to be fast and small, and to work without an account, a cloud, advertising or tracking.

Today it reads PDFs. The editing tools are planned and do not exist yet.

Patient Partridge is part of the Hedgerow family of apps, next to [Nimble Newt](https://github.com/Ferdinand99/Nimble-Newt).

**Status: in development.** There is no release yet and nothing to install. Expect rough edges and missing features.

## What works now

- Open one or more PDFs from the Open button (Ctrl+O), by dropping files on the window, from the command line, or with "Open with" from a file manager. Each file opens in its own tab (Ctrl+W closes a tab, Ctrl+Tab and Ctrl+Shift+Tab move between tabs). Opening a file that is already open shows its tab.
- Read in one continuous scrolling view. Only the pages you can see (and a few around them) are drawn, so opening is quick and memory stays flat as documents get longer. A tab you are not looking at gives up its drawn pages.
- A side panel (Ctrl+B) shows page thumbnails and the table of contents of the PDF (its bookmarks). Thumbnails are drawn only for the part of the list you can see, and only after the page you asked for is on screen, so the number of pages does not slow down opening (measured with a 500-page file; larger ones have not been timed). Entries of the contents that point to a web address or a file are shown but never followed.
- Select text with the mouse (double click for a word, triple click for a line, Ctrl+A for everything) and copy it with Ctrl+C or the system's Copy. The text and the place of every character come from PDFium through the engine; nothing is read from the file by the window.
- Search in the document (Ctrl+F). The matches are highlighted on the pages, Enter and Shift+Enter (or F3) move between them, and the count is shown. Search ignores upper and lower case and line breaks, and runs page by page in the background, so you can stop it at any time. Words that a PDF writes with a ligature such as "fi" in one character may not be found.
- Links inside a PDF are never opened by themselves. Pressing one shows the whole address and asks first. Only web addresses (http and https) can be opened, in your normal browser; anything else (a file, a program, a script, a mail address) is refused with an explanation. Links to another page of the same document just go there. Attachments in a PDF are never opened or even listed.
- Print (Ctrl+P): all pages, the page in view, or a list such as 1-3, 7, 9-. The pages are drawn by the engine at 150 dpi, packed as JPEG pictures one at a time (a few hundred kilobytes each) and then the print dialog of your system opens; nothing is sent anywhere else. A printed page is a picture of the page, not text. This is covered by unit tests with a fake printer and by a browser test that checks what is shown in print media; it has not been tried on a real printer on any system yet.
- Zoom in and out, or fit the page to the window width. Ctrl with the mouse wheel zooms too.
- Move with the mouse wheel, the arrow keys, Page Up and Page Down, Space, Home and End, or jump to a page number (Ctrl+G).
- Patient Partridge remembers where you stopped in each file and puts you back there when you open it again. The place (page, how far down, zoom) is kept in a small file on your computer, together with the path of the file, for the last 500 files. Nothing is sent anywhere. "Forget remembered positions" in the settings menu (the three dots) empties it.
- The look follows the system's light or dark setting. The settings menu has Auto, Light and Dark. The pages themselves stay white, like paper.
- Password-protected files ask for the password. A damaged file, or a file that is not a PDF, gives a clear message instead of a crash.
- The installers register Patient Partridge as a viewer for PDF files, so it appears under "Open with". It does not take over as the default reader by itself.

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

- **Memory.** Measured on Linux (WebKitGTK, software rendering) with a 500-page file open: the whole app, all its processes together, uses about 490 MB of memory (proportional share), of which about 325 MB is the web view and about 14 MB the engine. The plan's target of 150 MB is not met there. Windows and macOS have not been measured on real hardware; their web views behave differently.
- **Web view.** The look uses CSS `light-dark()`, so the web view must be recent: WebView2 (always up to date), WebKitGTK 2.46 or newer, and a macOS version whose web view is Safari 17.5 or newer.
- **Search** finds text as PDFium reports it. It does not join words split by a hyphen at the end of a line, and it does not match a ligature character against its letters.
- **Text selection** follows the order of the text in the file. In documents with several columns that are stored in an odd order, a selection can jump between columns.
- **Printing** sends pictures of the pages, at 150 dpi, to the system's print dialog. Pages of different sizes are fitted to the paper.

## How it is built

- The window is a [Tauri 2](https://tauri.app/) app written in TypeScript (Vite) with a small Rust core.
- PDF files are read by a separate program, `partridge-engine`, which uses [PDFium](https://pdfium.googlesource.com/pdfium/) (the PDF library of Chrome) through [pdfium-render](https://crates.io/crates/pdfium-render). The app starts it and talks to it over standard input and output with a small binary protocol.
- A PDF is untrusted input. Every open document is read by an engine process of its own, so a file that crashes or hangs its engine closes only its own tab: you get an error message, the other tabs carry on, and you can open another file. At most 16 documents are open at once.
- PDF JavaScript is turned off. The PDFium build used has no JavaScript engine at all.
- The engine code has no network code, and a check in CI keeps it that way. The app makes no network connections of its own, and the window loads nothing from the internet (strict content security policy). The one thing that leaves the app is a link you say yes to: your normal browser is then started with that address.

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

## Security: what is protected and what is not yet

Please read this before you open files from people you do not trust.

- **The engine is not sandboxed by the operating system yet.** Being in a separate process protects the app from crashes and hangs, but it is not a security boundary. If an attacker finds a bug in PDFium and builds a PDF that exploits it, the code would run with your user rights: it could read your files and use the network, even though the engine's own code never does. A real sandbox (restricted rights per operating system) is planned and is a requirement for the first release.
- "No network" means that the engine's code contains none, which CI checks. It does not mean the operating system blocks the engine from the network.
- What the engine does have today: a limit on the memory it may use (3 GiB on Linux and Windows, none on macOS), it ends when the app ends (even when stuck inside PDFium), the app treats everything it receives from the engine as untrusted, and PDFium is loaded only from the app's own folder after a checksum check at build time.
- PDFium is the main attack surface. It is pinned to one release and updated by hand ([steps](docs/pdfium-updates.md)); a weekly workflow opens an issue when a newer release exists, and CI checks that the library that ships has no JavaScript engine (V8).

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

Installers for testing (not signed): `npm run bundle`.

## Measuring speed and memory

`scripts/measure.sh` opens a generated 500-page PDF and prints how long the engine and the whole app take, and how much memory they use. It is written for Linux and needs `xvfb` for the app part.

## Licence

Patient Partridge is free software under the [GNU Affero General Public License, version 3](LICENSE). PDFium is under a BSD-style licence; its licence files are shipped with the app.
