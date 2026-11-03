# Signing with a certificate

How Patient Partridge adds a digital signature made with a certificate file, what each part does, and what it does not do. The words the window uses are in `src/lib/certsign.ts`; none of them says what a signature is worth.

## What the person does

1. Presses Sign with a certificate. A document with changes that are not saved is saved first.
2. Chooses a certificate file (`.p12` or `.pfx`) in the dialog of the system and types its password. The box shows the name, the issuer, the period, the key and what is off about the certificate.
3. Chooses where the signature shows (a box dragged on a preview of a page, a signature field of the file, or nothing), and what it says (reason, place, an optional picture). The name in the signature is the name in the certificate: a page that says "Digitally signed by" and a program that reads the signature name the same holder.
4. Names the signed copy. The copy is written, checked and opened in a tab where nothing can be changed.

## What happens

```
window ── password ──▶ app process ── file + password ──▶ signer process (the key stays here)
                          │
                          ├── SignBegin ──▶ engine process: PDFium writes the document, the engine adds
                          │                 the empty signature (no key, no certificate in it)
                          ◀── the bytes, with the byte range already written and a hole of the size asked for
                          │
                          ├── SHA-256 or SHA-384 of the bytes except the hole ──▶ signer ──▶ the signature
                          │
                          ├── the signature is written into the hole
                          ├── the file is written next to its target, flushed and read back
                          ├── a fresh engine in its sandbox opens it and checks it
                          └── one rename puts it in place
```

- **The engine** (`crates/engine/src/signing.rs`, `signprep.rs`, `signlook.rs`) writes the document as for a save (written twice, so that nothing nobody refers to is in the file), reads that file with the bounded reader the page tools use (`pagetree.rs`) and appends an update: the signature field with its look, the signature dictionary (`/SubFilter /ETSI.CAdES.detached`, `/ByteRange`, `/Contents`, `/M`, `/Name`, `/Reason`, `/Location`), the catalog and the form that name the field (`/SigFlags 3`, version 1.7), the page that shows it, and a table and a trailer that point back to the table before. The bytes PDFium wrote are the start of the new file, untouched. A field the document already has is signed in place; a field the form designer made counts as one. The look is plain drawing instructions, set in the standard Helvetica (a letter outside the Windows Latin set shows as a question mark; the signature keeps the whole name) with the picture of a signature, if there is one, drawn as lines, filled curves or an image that is run-length packed.
- **The signer** (`crates/signer`) opens the PKCS #12 file with `p12-keystore`, pairs the key with its certificate by public key, builds the chain, and makes a CMS `SignedData` with `ring`: one signer, signed attributes (content type, message digest, the ESS signing-certificate-v2 attribute), the certificates of the chain, detached. It takes RSA of 2048 to 4096 bits (PKCS #1 v1.5) and ECDSA on P-256 and P-384, and refuses the other kinds of key by name. The crate `rsa` is not used (RUSTSEC-2023-0071). The signer is locked in by `partridge-sandbox`, the code the engine uses, before it reads its first request.
- **The app** (`crates/engine-client/src/sign.rs`, `signer.rs`, `src-tauri/src/certsign.rs`) hashes the file, asks the signer, writes the file through the same path a save takes, and checks it. It refuses the signature if the new file is not one intact signature made with the certificate that was used, covering the whole file but the hole, with the same pages as the document (a fingerprint that leaves out signature fields).

## Refusals

| Case | What is said |
|---|---|
| Wrong password | "That password is not right" (the file stays chosen for another try) |
| Not a certificate file, no key, key and certificate not a pair | one line each |
| Ed25519, RSA-PSS, a short RSA key, another curve | "This kind of key is not supported", with the kinds that are |
| Encrypted document, XFA form | refused |
| A field with a seed value or a lock | "asks for settings that Patient Partridge does not apply" |
| A chain of more than 12 certificates | "The chain of certificates is too long" (it is not cut) |
| A certificate file protected in a way that is not opened (more than a million rounds) | "cannot be opened here" |
| A document that holds signatures already | "Signing it again would remove them, so Patient Partridge does not do it in this version." |
| A document with changes not saved | saved first, or the signature is not made |
| The signed copy named like the open file | "Choose another name" |
| A structure the writer cannot follow | "cannot be signed", which says nothing about the document |

An expired certificate, one that is not in use yet, one that its owner made, and one whose key usage says it is not for signing are warned about and allowed.

## After signing

The signed file is opened as a document of its own and held: annotations, form fields, the form designer and the page tools are off in that tab, each with a short note that says why. The signed file is never saved over; if a save is asked for, only "Save a copy" is offered. The tab follows the copy, which is an ordinary document and can be changed again. Nothing in the program writes the signed file after it is in place.

The signer ends after the one signature it makes, so the key is not kept. If the signing fails after that (a name already taken, a disk that is full), the password is asked for again. A key that is opened and not used is let go after five minutes, and a key opened for one document signs no other.

## Fields the document already has

A signature field that nobody has signed is a place to sign. The document may have several: one is used, or a new box, or none, and the others stay empty. A field that carries a seed value (`/SV`) or a lock (`/Lock`) is not signed (this program applies neither, and a signature that ignores them is not what the field asks for); a box on the page is the way round. A field too small to hold a look gets an empty look, so a mark the file had on it ("sign here") is not left on a field that is signed.

## Tests

- `crates/signer/tests`: every kind of certificate file (made by `scripts/make-signing-fixtures.py`, committed, password `test`), wrong passwords, damaged files, the process in its sandbox.
- `crates/engine/tests/sign.rs`: the real engine and PDFium: each key kind, intact and trusted with a list, every shape of document (forms, turned pages, inherited boxes, existing and designed fields), the refusals, the pages unchanged.
- `crates/engine/tests/sign_readers.rs`: `qpdf --check`, Poppler's `pdfsig` and `pdftoppm`, `openssl cms -verify` (with and without the chain), pyHanko (`tests/sign_readers/pyhanko_check.py`).
- `crates/engine/tests/sign_process.rs` and `crates/engine-client/tests/hostile_signer.rs`: the whole pipeline with the real engine and signer processes, a hostile signer, a failing disk, a save after signing, documents that have signature fields nobody signed, and strings that are not text.
- Fuzz targets `signer_protocol`, `signer_load` and `sign_prepare` (see `docs/fuzzing.md`).
- `src/lib/certsign.test.ts`, `src/app/certsign.test.ts`, `e2e/certsign.spec.ts` for the window.

## Not done

RFC 3161 time stamps (they need the network, which only the update check may use), signatures in a document that already has some, certificates from the keychain or certificate store of the system, long-term checking data. See the Known limits of the README.
