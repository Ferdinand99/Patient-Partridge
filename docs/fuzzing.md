# Fuzzing

PDF files are untrusted input, and the app and its PDF engine talk over a binary protocol. Both are fuzzed with [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) (libFuzzer with AddressSanitizer). The fuzz crate is in `fuzz/`. It has its own lock file and is not part of the main Cargo workspace, so it never changes what the app is built from.

## Targets

| Target | What it checks |
|---|---|
| `protocol_request_decode` | Arbitrary bytes into `Request::decode`. No panic or hang. A request that decodes is inside the string caps and the cap of a piece of a saved file (`SaveChunk`, 1 MiB), and encodes back to the same bytes (a request has one encoding, and the encoder cuts strings at their cap). |
| `protocol_response_decode` | Arbitrary bytes into `Response::decode`, including the outline, page text, links and search answers. A response that decodes is inside every cap (list lengths, string lengths, outline depth, bitmap size) and encodes and decodes again to the same bytes. The seeds hold a response that is one over each cap that fits in `-max_len` (outline entries, depth, title, links, address, matches, a string, bitmap width and height, the size of a saved file, a piece of it that ends one byte past the biggest file, a piece shorter than it says) and some exactly on a cap. A piece of a saved file of 1 MiB does not fit in `-max_len`: its caps are tried by the response round-trip target. Page text (1.7 MB) and the pixel count (96 MB) do not fit: the first is reached by the response round-trip target, the second by a unit test in the protocol crate. |
| `protocol_read_frame` | Arbitrary bytes as a stream of frames, through `read_frame` and `read_frame_limited`. The biggest single allocation of a run must stay in proportion to the bytes that were really there (a 4 byte header that announces 128 MiB must not cost 128 MiB). The same stream given in small pieces, with interrupted reads, must give the same frames and the same ending. Every frame is also given to both decoders, and what they return must be inside the caps. |
| `protocol_roundtrip_request` | Valid requests of all twenty-five kinds built from the fuzzer's bytes (strings of exactly the longest allowed size, multi-byte characters, a piece of a saved file of exactly the longest length) must survive encode, framing and decode unchanged. One run in eight builds a request that is one byte over a cap (a string, or a piece of a saved file), which the decoder must refuse. |
| `protocol_roundtrip_response` | The same for all twenty-one kinds of response. Boxes that are not finite must come back as zeros. Lists sometimes have the most entries the protocol allows, and a piece of a saved file sometimes has the longest length. One run in eight builds a response that is one over a cap (outline entries, outline depth, title, links, address, matches, page text, strings, bitmap width and height, the size of a saved file, a piece of it that is too long or reaches past the biggest file or is shorter than it says), which the decoder must refuse; several cases sit exactly on a cap and must be accepted. |
| `engine_open_render` | Arbitrary bytes as a PDF file, through the engine's request handler with the real, pinned PDFium: open (also with a password), outline, then for the first pages and the last page the text, the links, searches and a small render, then a save (`DocInfo`, `SaveBegin`, all the pieces, `SaveEnd`: the file starts like a PDF, the pieces are refused past the end, and the saved file opens again with the same number of pages and the same kind of protection and the same fingerprint as the document had just before the save (where both can be read in full), and each page that is looked at and that the saved file, a fresh open of the original and the document in the session can all read, and on which the last two agree, has the same size and the same text in the saved file), then close. The answers are checked as the engine builds them, before the encoder cuts any list; see below. |

What `engine_open_render` checks:

- Open: page sizes are finite, positive and at most 14 400 points.
- Outline: depth at most 15, titles at most 512 bytes, and an entry that has a page names a page that exists. The check for at most 5 000 entries is in the target, but no input reaches it: such an outline is far bigger than the `-max_len` of 100000.
- Links: at most 500 (the seed `links_over_the_cap.pdf` has 501), addresses not empty and at most 2 048 bytes, and a link to a page names a page that exists.
- Search: matches are not empty, in order, do not overlap, and lie inside the text of the page (counted in characters of the page text answer). The text a match covers must read like the search text, checked with a second, separate way of folding case and white space. Two of the searches are for pieces of the page itself, so matches do happen.
- Page text: the check for at most 100 000 characters is in the target, but no input reaches it: the test kit writes no compressed streams, so a page of that size does not fit in the `-max_len`. (That there is one box for each character is not checked on the engine's answer, which is built from one list; the decode targets check it.) Render: a bitmap of exactly the size asked for.
- Close answers `Closed`, and the document is gone afterwards. A page past the end gives `PageOutOfRange`. A page that exists fails only with `Corrupt` or `RenderFailed`.
- Forms: `FormInfo` answers (a form whose handle cannot be reached is told with `FormUnsupported`). For a form the reader fills in, the fields of each page looked at are listed (inside the caps, each with a place on the page) and the first three are set to a value of their kind; a change is answered with the list or refused with `FormFieldRefused`, and a document that is not an AcroForm is refused with `FormUnsupported`.
- Annotations: the list of the first two pages is inside the caps with ids that differ; a highlight, a note, a drawing and a stamp are added (or refused with an error that means "not this one"), one is changed, all are removed and put back with the ids they had, and an id that was never given out is refused.
- Save: `DocInfo` names the pages the document has; the size from `SaveBegin` is within the cap; the pieces add up to a file that starts with `%PDF-`; a piece past the end or longer than a piece may be gives `InvalidRequest`, as does a piece after `SaveEnd`; the saved file, opened again, has the same number of pages and the same kind of protection, and the fingerprint of the document just before the save where both can be read in full (the fields and annotations are in both), and every page looked at that the saved file, a fresh open of the original and the session can all read, and on which the last two agree, has the same size and the same text. The original's own fingerprint is not compared, because the fields and annotations the target adds change what the file holds besides that. A file that PDFium had to repair and cannot open again, and a page it cannot read in one of the two, are not failures of the target: the app would refuse that save.
- Every answer can be encoded and decoded as the app does, and the encoder cut no list.

The seeds include documents whose link and outline destinations are page numbers far past the end of the document, and negative ones, so the page bounds are reached without luck.

### What the engine target can and cannot find

PDFium is the prebuilt library from `pdfium.lock.json` (checked against its SHA-256 before it is unpacked). It is loaded into the fuzzer process, and it has no coverage instrumentation. So:

- The fuzzer sees which paths the engine's own code takes and the comparisons PDFium makes through the C library (these feed the dictionary), but not the paths inside PDFium. It mutates real PDFs from the seed corpus fairly blindly. Expect few new inputs per hour compared with the protocol targets.
- Memory errors inside PDFium are found only when they crash the process (a signal, or an abort from an out-of-memory). AddressSanitizer does not check the library's own reads and writes.
- A PDF that makes PDFium use huge amounts of memory, or hang, ends the fuzz run too. In the app that only ends the engine process of one tab. Such inputs are still worth a look; see "A crash was found" below.

The target checks that the engine code around PDFium (page sizes, text, links, outline, search, bitmap sizes, the limits of the answers) behaves on hostile files, and it is a cheap crash search of PDFium. It does not replace PDFium's own fuzzing. It is also no substitute for a sandbox: the engine is not sandboxed by the operating system yet (see [security.md](security.md)), so a bug in PDFium that is not a crash is not found here and is not contained.

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
- Use `dictionaries/protocol.dict` for the five protocol targets and `dictionaries/pdf.dict` for the engine target.
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

### Long runs of the engine target

The engine target keeps one engine in one process for the whole run, and PDFium (or its Rust wrapper) keeps some memory after a document with annotations is closed: about 15 to 20 KiB for each stamp, about 3 KiB for each highlight or note, nothing for a document that is only opened and closed. Under AddressSanitizer, with a corpus that adds annotations to every file, the process grows by roughly 60 KiB per execution, and a run of several hours ends with `out-of-memory` at `-rss_limit_mb` even though no single input needs much. The app does not behave this way: it starts a new engine process for each document, so this memory goes away when the document is closed.

For runs longer than about an hour, stop and start the process again every 20 to 30 minutes, with the same corpus folder, so that the memory starts from zero each time:

```sh
cd fuzz
for i in $(seq 1 12); do
  cargo fuzz run engine_open_render corpus/engine_open_render seeds/engine_open_render --     -dict=dictionaries/pdf.dict -max_len=100000 -timeout=30 -rss_limit_mb=3072     -malloc_limit_mb=3072 -max_total_time=1800 || break
done
```

The loop stops at the first run that ends with a finding.

## A crash was found

1. Download the artifact, or take the file from `fuzz/artifacts/<target>/`. From the `fuzz` folder, reproduce it: `cargo fuzz run <target> artifacts/<target>/crash-...`.
2. Make it small: `cargo fuzz tmin <target> artifacts/<target>/crash-...`.
3. Fix the bug in its own commit, with a regression test in the same commit.
   - Protocol bugs: a `#[test]` in `crates/protocol/src/lib.rs` with the minimized bytes written out in the test.
   - Engine bugs: a test in `crates/engine/tests/` that writes the minimized PDF to a temporary file and runs it through the engine as the target does. A small input can be put in the test as a byte string; a larger one in a new `crates/engine/tests/data/` folder (keep it small).
4. Also add the minimized input to the matching `fuzz/seeds/<target>/` folder, so that the CI run replays it. The seed generator removes nothing from that folder, but it must be able to write it again: add the file to `make-seeds` if it should survive a full regeneration.
5. A crash that is only memory use or time from a hostile PDF in PDFium itself (an abort from out-of-memory, `timeout` or `out-of-memory` reports) cannot be fixed here. The engine runs in its own process, with a memory limit (enforced by the system on Linux and Windows, and by a watch of the app on macOS), so the app survives it. Report it to the PDFium build's upstream and note it in the pull request.

   Before that, replay the file alone (`cargo fuzz run <target> <file> -- -runs=1 -print_final_stats=1`) and read `peak_rss_mb`. If it is small, the input is not what ran out of memory: the process grew over the run (see "Long runs of the engine target"), and the file is only the one that happened to be running when the limit was crossed. Such a report needs no fix and no seed.

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
