# Architecture

How Patient Partridge is put together. The [README](../README.md#architecture) has the short version; the sandbox is described in [security.md](security.md), and the limits of each feature in [known-limits.md](known-limits.md).

```
crates/protocol       the message format between app and engine
crates/engine         partridge-engine, the only code that parses PDFs
crates/engine-client  starts the engine, talks to it, recovers when it crashes
crates/testkit        generates small test PDFs (also a broken one and a password-protected one)
src-tauri             the app process (Rust)
src                   the window (TypeScript)
scripts               PDFium download, bundling, measurements
```

## How it is built

- The window is a [Tauri 2](https://tauri.app/) app written in TypeScript (Vite) with a small Rust core.
- PDF files are read by a separate program, `partridge-engine`, which uses [PDFium](https://pdfium.googlesource.com/pdfium/) (the PDF library of Chrome) through [pdfium-render](https://crates.io/crates/pdfium-render). The app starts it and talks to it over standard input and output with a small binary protocol.
- A PDF is untrusted input. Every open document is read by an engine process of its own, so a file that crashes or hangs its engine closes only its own tab: you get an error message, the other tabs carry on, and you can open another file. At most 16 documents are open at once.
- PDF JavaScript is turned off. The PDFium build used has no JavaScript engine at all, and the form environment is made without a JavaScript platform. That stops scripts and nothing else: PDFium still runs the other actions of a field itself, and a reset or a hide would change the document. So the app reads the actions of a form before it lets you fill it in, and leaves a field alone when its own actions reach one of those two, and every field when an event that PDFium reads from another field does, or when the search cannot be settled (see [Known limits, Forms](known-limits.md#forms)). A file built to get round this could still clear or hide a field; nothing leaves the app, and the file on disk changes only if you save. A form with scripts opens and is filled in; its scripts do nothing.
- The engine runs in an operating system sandbox (see [security.md](security.md)): it cannot open files or start programs (and on Linux and macOS it cannot use the network; on Windows it can, see [security.md](security.md)), and it reads the one document the app opens for it through a read-only handle that the app passes on at start.
- The engine writes nothing, also when a document is saved: it serialises the document into memory and the app writes the file (see [Saving](features.md#saving)). The protocol messages for that (`DocInfo`, `SaveBegin`, `SaveChunk`, `SaveEnd`) are covered by the fuzz targets, with the caps of the file size and of one piece.
- Filling in a form goes through three more messages (`FormInfo`, `ListFormFields`, `SetFormField`). A change is made the way a person makes it (focus the field, type or click, leave it), by PDFium's own form code, so the value and the appearance are written together and in a form that other readers accept. The request names the field twice (its place on the page and its full name) and the engine refuses it when the two do not agree, when the field is read only, when it is left alone for an action that could change the document (see [Known limits, Forms](known-limits.md#forms)), or when the value does not fit the kind of field. A check box or a radio button gets the space bar, in the widget that has the focus, so nothing but the named widget can be pressed. A field with widgets on several pages is changed with all its pages loaded, so every widget gets a new appearance. These messages are covered by the protocol tests and the fuzz targets, and `scripts/check-forms.sh` hands the saved files to qpdf, pdftk and poppler.
- The engine code has no network code, and a check in CI keeps it that way. The window loads nothing from the internet (strict content security policy). **The only network use of the app is the update:** it checks GitHub for a newer version when it starts and every 6 hours, and downloads one only when you press Download and Install. The check can be turned off in the settings menu, and documents are never sent anywhere (see [updates.md](updates.md) for exactly what is sent). The one other thing that leaves the app is a link in a PDF that you say yes to: your normal browser is then started with that address, and a link goes nowhere unless you say yes.
- The PDFium library is downloaded from [bblanchon/pdfium-binaries](https://github.com/bblanchon/pdfium-binaries) at a pinned release, and its SHA-256 checksum is checked before it is used (`pdfium.lock.json`).
