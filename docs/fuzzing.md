# Fuzzing

PDF files are untrusted input, and the app and its PDF engine talk over a binary protocol. Both are fuzzed with [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) (libFuzzer with AddressSanitizer). The fuzz crate is in `fuzz/`. It has its own lock file and is not part of the main Cargo workspace, so it never changes what the app is built from.

## Targets

| Target | What it checks |
|---|---|
| `protocol_request_decode` | Arbitrary bytes into `Request::decode`. No panic or hang. A request that decodes is inside the string caps and the cap of a piece of a saved file (`SaveChunk`, 1 MiB), and encodes back to the same bytes (a request has one encoding, and the encoder cuts strings at their cap). |
| `protocol_response_decode` | Arbitrary bytes into `Response::decode`, including the outline, page text, links and search answers. A response that decodes is inside every cap (list lengths, string lengths, outline depth, bitmap size) and encodes and decodes again to the same bytes. The seeds hold a response that is one over each cap that fits in `-max_len` (outline entries, depth, title, links, address, matches, a string, bitmap width and height, the size of a saved file, a piece of it that ends one byte past the biggest file, a piece shorter than it says) and some exactly on a cap. A piece of a saved file of 1 MiB does not fit in `-max_len`: its caps are tried by the response round-trip target. Page text (1.7 MB) and the pixel count (96 MB) do not fit: the first is reached by the response round-trip target, the second by a unit test in the protocol crate. |
| `protocol_read_frame` | Arbitrary bytes as a stream of frames, through `read_frame` and `read_frame_limited`. The biggest single allocation of a run must stay in proportion to the bytes that were really there (a 4 byte header that announces 128 MiB must not cost 128 MiB). The same stream given in small pieces, with interrupted reads, must give the same frames and the same ending. Every frame is also given to both decoders, and what they return must be inside the caps. |
| `protocol_roundtrip_request` | Valid requests of all sixteen kinds (the page tools are one kind with twelve forms of their own) built from the fuzzer's bytes (strings of exactly the longest allowed size, multi-byte characters, a piece of a saved file of exactly the longest length) must survive encode, framing and decode unchanged. One run in eight builds a request that is one byte over a cap (a string, or a piece of a saved file), which the decoder must refuse. |
| `protocol_roundtrip_response` | The same for all eighteen kinds of response (the page tool answers are one kind with six forms). Boxes that are not finite must come back as zeros. Lists sometimes have the most entries the protocol allows, and a piece of a saved file sometimes has the longest length. One run in eight builds a response that is one over a cap (outline entries, outline depth, title, links, address, matches, page text, strings, bitmap width and height, the size of a saved file, a piece of it that is too long or reaches past the biggest file or is shorter than it says), which the decoder must refuse; several cases sit exactly on a cap and must be accepted. |
| `signature_check` | The check of one digital signature against the bytes of a file (`partridge_engine::signatures::check`): the CMS parser, the signed attributes, the key arithmetic, the chain against a trust list, the byte range and the hash of the file. The fuzzer's bytes are a script that starts from a signed file made by the test kit (a real P-256 signature, made when the target starts) and spoils it: a bit flipped, bytes cut out or put in, the signature data cut or filled, the byte range moved, the sub-filter changed, the file given another size. Half of the runs keep the file and the signature data in step, so that the work goes past the first comparison. No panic or hang. A signature may be called intact only when the byte range, worked out again by a separate and simpler routine in the target, starts at the beginning of the file, stays inside it, leaves out exactly one hexadecimal string in `<` and `>` and that string is the signature data; an intact signature reaches the end of the file, or else it is called changed; nothing is called trusted when the trust list is empty; the report is inside every cap of the protocol and survives encode and decode. |
| `engine_open_render` | Arbitrary bytes as a PDF file, through the engine's request handler with the real, pinned PDFium: open (also with a password), outline, then for the first pages and the last page the text, the links, searches and a small render, then a save (`DocInfo`, `SaveBegin`, all the pieces, `SaveEnd`: the file starts like a PDF, the pieces are refused past the end, and the saved file opens again with the same pages, the same kind of protection and the same fingerprint of its pages), then close. The answers are checked as the engine builds them, before the encoder cuts any list; see below. |

What `engine_open_render` checks:

- Open: page sizes are finite, positive and at most 14 400 points.
- Outline: depth at most 15, titles at most 512 bytes, and an entry that has a page names a page that exists. The check for at most 5 000 entries is in the target, but no input reaches it: such an outline is far bigger than the `-max_len` of 100000.
- Links: at most 500 (the seed `links_over_the_cap.pdf` has 501), addresses not empty and at most 2 048 bytes, and a link to a page names a page that exists.
- Search: matches are not empty, in order, do not overlap, and lie inside the text of the page (counted in characters of the page text answer). The text a match covers must read like the search text, checked with a second, separate way of folding case and white space. Two of the searches are for pieces of the page itself, so matches do happen.
- Page text: the check for at most 100 000 characters is in the target, but no input reaches it: the test kit writes no compressed streams, so a page of that size does not fit in the `-max_len`. (That there is one box for each character is not checked on the engine's answer, which is built from one list; the decode targets check it.) Render: a bitmap of exactly the size asked for.
- Close answers `Closed`, and the document is gone afterwards. A page past the end gives `PageOutOfRange`. A page that exists fails only with `Corrupt` or `RenderFailed`.
- Save: `DocInfo` names the pages the document has; the size from `SaveBegin` is within the cap; the pieces add up to a file that starts with `%PDF-`; a piece past the end or longer than a piece may be gives `InvalidRequest`, as does a piece after `SaveEnd`; the saved file, opened again, has the same number of pages and the same kind of protection (a file that PDFium had to repair and cannot open again is not a failure of the target: the app would refuse that save).
- Page tools, on the document as it was opened (a person meets the tools on the file as it is, damaged or not) and then on the saved file opened again: a turn, a crop, a blank page, a delete and the copy it leaves (restore), a move of every page and its copy, the copy of the first page into a new document (the bytes open with one page and the digest the engine gave) and the insert of that copy into the document in pieces of an odd size. An insert that the document cannot take (its written form reads back with other pages) is refused by the engine, which is a refusal and not a finding. The number of pages after each is the number it must be, a refusal is one of a short list of errors, and the document, saved after all of it, opens again with the same pages and the same digest. A page tool that has to give up half way and cannot put the document back closes it (the tests `a_change_that_fails_half_way_*` make PDFium fail on purpose). The seeds include a page tree that loops or is deep, a count of pages that lies, odd rotations and odd boxes. The first run of this found a crash of PDFium when a page is added to a list of pages that holds a null object; the tools now refuse such a document (the test `a_document_with_a_broken_list_of_pages_cannot_take_the_engine_down` keeps it from coming back).
- Every answer can be encoded and decoded as the app does, and the encoder cut no list.

The seeds include documents whose link and outline destinations are page numbers far past the end of the document, and negative ones, so the page bounds are reached without luck.

### What the engine target can and cannot find

- **A copy of pages points into the other document until the document is written out and read in again.** PDFium's copy of pages leaves some references (the `/Parent` of a pop-up note or of a form widget, and `/First` and `/Prev`) as they were in the other document, and the other document was let go of before the next save: the save read freed memory and stopped the engine. This is not rare and not limited to damaged files: any file with a pop-up note or a field shown on two pages does it. A move now uses PDFium's own move of pages and copies nothing; an insert is written out and read in again while the other document is still alive. Both tools are in every run, on the document as it was opened and on the saved file opened again, and a crash in them is a finding. A page added to a list that holds a null object crashed PDFium too; the tools prevent that (they look at the list first, and refuse). The test kit has documents with the things that triggered it (a note with its pop-up, a field on two pages, an entry with `/First`), and a damaged protected file that PDFium reads with one page and writes out so that it reads back with none (`page_count_lost_pdf`), which no change may put in place of the document that is open.

PDFium is the prebuilt library from `pdfium.lock.json` (checked against its SHA-256 before it is unpacked). It is loaded into the fuzzer process, and it has no coverage instrumentation. So:

- The fuzzer sees which paths the engine's own code takes and the comparisons PDFium makes through the C library (these feed the dictionary), but not the paths inside PDFium. It mutates real PDFs from the seed corpus fairly blindly. Expect few new inputs per hour compared with the protocol targets.
- Memory errors inside PDFium are found only when they crash the process (a signal, or an abort from an out-of-memory). AddressSanitizer does not check the library's own reads and writes.
- A PDF that makes PDFium use huge amounts of memory, or hang, ends the fuzz run too. In the app that only ends the engine process of one tab. Such inputs are still worth a look; see "A crash was found" below.

The target checks that the engine code around PDFium (page sizes, text, links, outline, search, bitmap sizes, the limits of the answers) behaves on hostile files, and it is a cheap crash search of PDFium. It does not replace PDFium's own fuzzing. It is also no substitute for a sandbox: the engine is not sandboxed by the operating system yet (see the README), so a bug in PDFium that is not a crash is not found here and is not contained.

## Requirements

- Linux on x86-64. This is what CI and the long runs use. macOS should work; Windows was not tried.
- The pinned nightly Rust toolchain, installed by `rustup` from `fuzz/rust-toolchain.toml` (a date, so that a run can be repeated). Install it with `rustup toolchain install --profile minimal $(sed -n 's/^channel = "\(.*\)"/\1/p' fuzz/rust-toolchain.toml)`.
- `cargo install cargo-fuzz --version =0.13.2 --locked`. The same version is pinned in `.github/workflows/fuzz.yml`.
- A C++ compiler (libFuzzer is built from source by `libfuzzer-sys`).
- For `engine_open_render`: the pinned PDFium, `node scripts/fetch-pdfium.mjs` (puts it in `vendor/pdfium/<platform>`). Set `PARTRIDGE_PDFIUM_DIR` to use a copy somewhere else.

## Running a target

From the `fuzz` folder:

```sh
cd fuzz
cargo fuzz run protocol_response_decode corpus/protocol_response_decode seeds/protocol_response_decode -- \
  -dict=dictionaries/protocol.dict -max_len=131072 -malloc_limit_mb=2 -max_total_time=300
```

- `corpus/<target>` is where the fuzzer keeps what it finds (git ignores it). `seeds/<target>` is the small committed starting set; it is read, never written.
- Use `dictionaries/protocol.dict` for the five protocol targets and `signature_check`, and `dictionaries/pdf.dict` for the engine target.
- `-max_len` has to be large for the decoders and the frame reader (131072): the seeds that are one over a cap are up to 65 KiB, and the frame reader grows its buffer in chunks of 64 KiB, which a smaller input never reaches. The round-trip targets use 4096, the engine target 100000.
- `-malloc_limit_mb` makes libFuzzer stop at a single allocation above that size (2 for the decoders, 4 for the frame reader, 8 for the round trips). The engine target uses 3072 and `-rss_limit_mb=3072`, about what the engine itself may use on Linux (3 GiB).
- For the engine target set `ASAN_OPTIONS=detect_leaks=0` (PDFium does not free everything at exit). Do not use libFuzzer's `-detect_leaks=0` instead: that also turns off the check on single big allocations.
- Without `-max_total_time` the run does not stop. All options after `--` go to libFuzzer; `-help=1` lists them.
- The exact options used by CI for each target are in the matrix of `.github/workflows/fuzz.yml`.

`cargo fuzz build` builds all targets without running them. `cargo fuzz coverage <target>` and `cargo fuzz cmin <target>` work as in the cargo-fuzz book.

## The seed corpus

`fuzz/seeds/` is made by `make-seeds`, from the protocol crate (a valid message of every kind, a message one over each cap, a message exactly on a cap, frame streams) and the PDF generator in `crates/testkit` (plain documents, mixed page sizes, outlines with loops and depth, many links, destinations past the end of the document, an encrypted file, a JavaScript action, a damaged file and a cut-off file). Nothing is downloaded, and no file from outside the repository is used.

```sh
cd fuzz
cargo run --release --bin make-seeds -- seeds
```

Run it again after the protocol or the generator changes, and commit what changed. The binary files are marked `binary` in `fuzz/seeds/.gitattributes`, which `make-seeds` writes too.

## CI

`.github/workflows/fuzz.yml` builds every target with the pinned toolchain and runs each for 60 seconds from the seed corpus, one job per target. It runs once a week, on request (with the seconds, 1 to 800, as input), and on pull requests that touch `crates/protocol`, `crates/engine`, `crates/testkit`, `pdfium.lock.json`, `fuzz/` or the workflow. It does not run on other pushes. The job has read-only permissions. It fails when `fuzz/Cargo.lock` does not cover the manifests (`cargo metadata --locked`; cargo-fuzz has no `--locked` of its own). When a target crashes, the input that did it is kept as the artifact `crash-<target>`.

A 60 second run finds shallow bugs only. Run the targets for much longer by hand (hours, in the background) before a release.

## A crash was found

1. Download the artifact, or take the file from `fuzz/artifacts/<target>/`. From the `fuzz` folder, reproduce it: `cargo fuzz run <target> artifacts/<target>/crash-...`.
2. Make it small: `cargo fuzz tmin <target> artifacts/<target>/crash-...`.
3. Fix the bug in its own commit, with a regression test in the same commit.
   - Protocol bugs: a `#[test]` in `crates/protocol/src/lib.rs` with the minimized bytes written out in the test.
   - Engine bugs: a test in `crates/engine/tests/` that writes the minimized PDF to a temporary file and runs it through the engine as the target does. A small input can be put in the test as a byte string; a larger one in a new `crates/engine/tests/data/` folder (keep it small).
4. Also add the minimized input to the matching `fuzz/seeds/<target>/` folder, so that the CI run replays it. The seed generator removes nothing from that folder, but it must be able to write it again: add the file to `make-seeds` if it should survive a full regeneration.
5. A crash that is only memory use or time from a hostile PDF in PDFium itself (an abort from out-of-memory, `timeout` or `out-of-memory` reports) cannot be fixed here. The engine runs in its own process, with a memory limit (enforced by the system on Linux and Windows, and by a watch of the app on macOS), so the app survives it. Report it to the PDFium build's upstream and note it in the pull request.

## Adding a target

1. Write `fuzz/fuzz_targets/<name>.rs` with `#![no_main]` and `fuzz_target!`. Prefer a property to a bare "does not crash": a round trip, a differential check (two ways to get the same answer), or an invariant on the output.
2. Add the `[[bin]]` entry for it to `fuzz/Cargo.toml` (copy an existing one: `test = false`, `doc = false`, `bench = false`).
3. Put shared generators in `fuzz/src/lib.rs`, and the caps as assertions in `fuzz/src/caps.rs`. A generator that builds a valid value from the fuzzer's bytes must stay inside the limits of the thing under test; to test the limits, write the bytes of a message that is one over (the encoder cannot).
4. Add its seeds to `fuzz/src/bin/make-seeds.rs` and run it. A small, valid, varied seed set matters more than many files. Seeds must be shorter than the `-max_len` of the target, or libFuzzer cuts them.
5. Add a dictionary entry if the format has keywords or magic numbers (`fuzz/dictionaries/`). A dictionary line is `name="\x01\x02"`; libFuzzer reads only `\\`, `\"` and `\xNN` escapes.
6. Add the target to the matrix in `.github/workflows/fuzz.yml` and to the table above.
7. Check that the target can fail: change the code it tests on purpose (for example, remove a length check or a bound) on a throwaway copy, and see that the target reports it within the time CI gives it. Then throw the copy away. A check that cannot fail is not a check.

## Updating the toolchain or PDFium

- Nightly: change the date in `fuzz/rust-toolchain.toml`, build all targets, run each for a minute, commit.
- PDFium: follow the steps in `pdfium.lock.json`. The engine target uses the same pinned build and picks it up from `vendor/pdfium`.
