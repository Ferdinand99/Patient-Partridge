# Patient Partridge

An open PDF reader for Windows, macOS and Linux. It is meant to be fast and small, and to work without an account, a cloud, advertising or tracking.

Patient Partridge is part of the Hedgerow family of apps, next to [Nimble Newt](https://github.com/Ferdinand99/Nimble-Newt).

**Status: in development.** There is no release yet and nothing to install. Expect rough edges and missing features.

## What works now

- Open a PDF from the Open button (Ctrl+O), by dropping a file on the window, or from the command line.
- Read it in one continuous scrolling view. Only the pages you can see (and a few around them) are drawn, so long documents open quickly and use little memory.
- Zoom in and out, or fit the page to the window width. Ctrl with the mouse wheel zooms too.
- Move with the mouse wheel, the arrow keys, Page Up and Page Down, Space, Home and End, or jump to a page number (Ctrl+G).
- Password-protected files ask for the password. A damaged file, or a file that is not a PDF, gives a clear message instead of a crash.

## What is not there yet

Tabs, page thumbnails, the table of contents, selecting and copying text, search, printing, dark mode, remembering where you stopped reading, annotations, forms, signatures and OCR. They are planned and will come step by step.

## How it is built

- The window is a [Tauri 2](https://tauri.app/) app written in TypeScript (Vite) with a small Rust core.
- PDF files are read by a separate program, `partridge-engine`, which uses [PDFium](https://pdfium.googlesource.com/pdfium/) (the PDF library of Chrome) through [pdfium-render](https://crates.io/crates/pdfium-render). The app starts it and talks to it over standard input and output with a small binary protocol.
- A PDF is untrusted input. Because it is read in its own process, a file that crashes or hangs the reader cannot take the app down: you get an error message and can open another file.
- PDF JavaScript is turned off. The PDFium build used has no JavaScript engine at all.
- The engine has no network code, and the app makes no network connections. The window loads nothing from the internet (strict content security policy).
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
```

Installers for testing (not signed): `npm run bundle`.

## Measuring speed and memory

`scripts/measure.sh` opens a generated 500-page PDF and prints how long the engine and the whole app take, and how much memory they use. It is written for Linux and needs `xvfb` for the app part.

## Licence

Patient Partridge is free software under the [GNU Affero General Public License, version 3](LICENSE). PDFium is under a BSD-style licence; its licence files are shipped with the app.
