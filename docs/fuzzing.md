# Fuzzing

PDF files are untrusted input, and the app and its PDF engine talk over a binary protocol. Both are fuzzed with [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) (libFuzzer with AddressSanitizer). The fuzz crate is in `fuzz/`. It has its own lock file and is not part of the main Cargo workspace, so it never changes what the app is built from.

## Targets

| Target | What it checks |
|---|---|
| `protocol_request_decode` | Arbitrary bytes into `Request::decode`. No panic or hang. A payload that decodes must encode back to the same bytes (a request has one encoding). |
| `protocol_response_decode` | Arbitrary bytes into `Response::decode`, including the outline, page text, links and search answers. A response that decodes must encode and decode again to the same bytes. |
| `protocol_read_frame` | Arbitrary bytes as a stream of frames, through `read_frame` and `read_frame_limited`. The same stream given in small pieces, with interrupted reads, must give the same frames and the same ending. Every frame is also given to both decoders. |
| `protocol_roundtrip_request` | Valid requests built from the fuzzer's bytes (strings of exactly the longest allowed size, multi-byte characters) must survive encode, framing and decode unchanged. |
| `protocol_roundtrip_response` | The same for all nine kinds of response. Boxes that are not finite must come back as zeros. Lists sometimes have the most entries the protocol allows. |
| `engine_open_render` | Arbitrary bytes as a PDF file, through the engine's request handler with the real, pinned PDFium: open (also with a password), outline, then for the first pages and the last page the text, the links, a search and a small render, then close. Every answer must be one the app can decode, and a page that exists may only fail with the errors the engine promises. |

### What the engine target can and cannot find

PDFium is the prebuilt library from `pdfium.lock.json` (checked against its SHA-256 before it is unpacked). It is loaded into the fuzzer process, and it has no coverage instrumentation. So:

- The fuzzer sees which paths the engine's own code takes and the comparisons PDFium makes through the C library (these feed the dictionary), but not the paths inside PDFium. It mutates real PDFs from the seed corpus fairly blindly. Expect few new inputs per hour compared with the protocol targets.
- Memory errors inside PDFium are found only when they crash the process (a signal, or an abort from an out-of-memory). AddressSanitizer does not check the library's own reads and writes.
- A PDF that makes PDFium use huge amounts of memory, or hang, ends the fuzz run too. In the app that only ends the engine process of one tab. Such inputs are still worth a look; see "A crash was found" below.

The target is a check that the engine code around PDFium (page sizes, text, links, outline, bitmap sizes, the answers' limits) behaves on hostile files, and a cheap crash search of PDFium. It does not replace PDFium's own fuzzing, and it does not replace the OS sandbox of the engine process.

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
  -dict=dictionaries/protocol.dict -max_len=4096 -max_total_time=300
```

- `corpus/<target>` is where the fuzzer keeps what it finds (git ignores it). `seeds/<target>` is the small committed starting set; it is read, never written.
- Use `dictionaries/protocol.dict` for the five protocol targets and `dictionaries/pdf.dict` for the engine target.
- `-max_len` is 4096 for the protocol targets (65536 for `protocol_read_frame`), and 100000 for the engine target, which also wants `-timeout=30 -rss_limit_mb=3072 -detect_leaks=0` (PDFium does not free everything at exit; `detect_leaks=0` keeps that out of the way).
- Without `-max_total_time` the run does not stop. All options after `--` go to libFuzzer; `-help=1` lists them.
- The exact options used by CI for each target are in the matrix of `.github/workflows/fuzz.yml`.

`cargo fuzz build` builds all targets without running them. `cargo fuzz coverage <target>` and `cargo fuzz cmin <target>` work as in the cargo-fuzz book.

## The seed corpus

`fuzz/seeds/` is made by `make-seeds`, from the protocol crate (a valid message of every kind) and the PDF generator in `crates/testkit` (plain documents, mixed page sizes, outlines with loops and depth, many links, an encrypted file, a JavaScript action, a damaged file and a cut-off file). Nothing is downloaded.

```sh
cd fuzz
cargo run --release --bin make-seeds -- seeds
```

Run it again after the protocol or the generator changes, and commit what changed. The binary files are marked `binary` in `fuzz/seeds/.gitattributes`.

## CI

`.github/workflows/fuzz.yml` builds every target with the pinned toolchain and runs each for 60 seconds from the seed corpus, one job per target. It runs once a week, on request (with the seconds as input), and on pull requests that touch `crates/protocol`, `crates/engine`, `crates/testkit`, `pdfium.lock.json`, `fuzz/` or the workflow. It does not run on other pushes. The job has read-only permissions; when a target crashes, the input that did it is kept as the artifact `crash-<target>`.

A 60 second run finds shallow bugs only. Run the targets for much longer by hand (hours, in the background) before a release.

## A crash was found

1. Download the artifact, or take the file from `fuzz/artifacts/<target>/`. From the `fuzz` folder, reproduce it: `cargo fuzz run <target> artifacts/<target>/crash-...`.
2. Make it small: `cargo fuzz tmin <target> artifacts/<target>/crash-...`.
3. Fix the bug in its own commit, with a regression test in the same commit.
   - Protocol bugs: a `#[test]` in `crates/protocol/src/lib.rs` with the minimized bytes written out in the test.
   - Engine bugs: a test in `crates/engine/tests/` that writes the minimized PDF to a temporary file and runs it through the engine as the target does. A small input can be put in the test as a byte string; a larger one in a new `crates/engine/tests/data/` folder (keep it small).
4. Also add the minimized input to the matching `fuzz/seeds/<target>/` folder, so that the CI run replays it. The seed generator removes nothing from that folder, but it must be able to write it again: add the file to `make-seeds` if it should survive a full regeneration.
5. A crash that is only memory use or time from a hostile PDF in PDFium itself (an abort from out-of-memory, `timeout` or `out-of-memory` reports) cannot be fixed here. The engine runs in its own process with a memory limit, so the app survives it. Report it to the PDFium build's upstream and note it in the pull request.

## Adding a target

1. Write `fuzz/fuzz_targets/<name>.rs` with `#![no_main]` and `fuzz_target!`. Prefer a property to a bare "does not crash": a round trip, a differential check (two ways to get the same answer), or an invariant on the output.
2. Add the `[[bin]]` entry for it to `fuzz/Cargo.toml` (copy an existing one: `test = false`, `doc = false`, `bench = false`).
3. Put shared generators in `fuzz/src/lib.rs`. A generator that builds a valid value from the fuzzer's bytes must stay inside the limits of the thing under test.
4. Add its seeds to `fuzz/src/bin/make-seeds.rs` and run it. A small, valid, varied seed set matters more than many files.
5. Add a dictionary entry if the format has keywords or magic numbers (`fuzz/dictionaries/`). A dictionary line is `name="\x01\x02"`; libFuzzer reads only `\\`, `\"` and `\xNN` escapes.
6. Add the target to the matrix in `.github/workflows/fuzz.yml` and to the table above.
7. Check that the target can fail: change the code it tests on purpose (for example, remove a length check) and see that the target reports it within a minute. Then undo the change.

## Updating the toolchain or PDFium

- Nightly: change the date in `fuzz/rust-toolchain.toml`, build all targets, run each for a minute, commit.
- PDFium: follow the steps in `pdfium.lock.json`. The engine target uses the same pinned build and picks it up from `vendor/pdfium`.
