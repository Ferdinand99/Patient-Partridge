# Patient Partridge

An open PDF reader for Windows, macOS and Linux. It is meant to be fast and small, and to work without an account, a cloud, advertising or tracking.

Patient Partridge is part of the Hedgerow family of apps, next to [Nimble Newt](https://github.com/Ferdinand99/Nimble-Newt).

**Status: in development.** There is no release yet and nothing to install. Expect rough edges and missing features.

## What works now

- Open a PDF from the Open button (Ctrl+O), by dropping a file on the window, or from the command line.
- Read it in one continuous scrolling view. Only the pages you can see (and a few around them) are drawn, so opening is quick and memory stays flat as documents get longer.
- Zoom in and out, or fit the page to the window width. Ctrl with the mouse wheel zooms too.
- Move with the mouse wheel, the arrow keys, Page Up and Page Down, Space, Home and End, or jump to a page number (Ctrl+G).
- Password-protected files ask for the password. A damaged file, or a file that is not a PDF, gives a clear message instead of a crash.

## What is not there yet

Tabs, page thumbnails, the table of contents, selecting and copying text, search, printing, dark mode, remembering where you stopped reading, annotations, forms, signatures and OCR. They are planned and will come step by step.

## How it is built

- The window is a [Tauri 2](https://tauri.app/) app written in TypeScript (Vite) with a small Rust core.
- PDF files are read by a separate program, `partridge-engine`, which uses [PDFium](https://pdfium.googlesource.com/pdfium/) (the PDF library of Chrome) through [pdfium-render](https://crates.io/crates/pdfium-render). The app starts it and talks to it over standard input and output with a small binary protocol.
- A PDF is untrusted input. It is read in its own process, so a file that crashes or hangs the reader cannot take the app down: you get an error message and can open another file.
- PDF JavaScript is turned off. The PDFium build used has no JavaScript engine at all.
- The engine code has no network code, and a check in CI keeps it that way. The app makes no network connections, and the window loads nothing from the internet (strict content security policy).

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
- PDFium is the main attack surface. It is pinned to one release and updated by hand for now; nothing warns yet when a newer release exists.

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
