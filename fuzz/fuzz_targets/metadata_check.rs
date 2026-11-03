#![no_main]
//! Arbitrary bytes as the file PDFium wrote for a save, through the check that refuses to hand
//! over a protected file whose XMP metadata would be written without encryption
//! (`partridge_engine::metadata::leaves_metadata_in_the_clear`). The check is a pure function over
//! bytes with a tokenizer in it, so it is a cheap and good target. It checks that:
//! - the function never panics, whatever the bytes, and does not take long: run it with a low
//!   `-timeout` (a search from every object to the next stream, which an earlier version made,
//!   takes minutes for a file of a few MiB);
//! - it gives the same answer, and the same number of reports of progress, for the same bytes.

use libfuzzer_sys::fuzz_target;
use partridge_engine::metadata::leaves_metadata_in_the_clear;

fuzz_target!(|data: &[u8]| {
    let mut calls = 0u32;
    let first = leaves_metadata_in_the_clear(data, &mut |_| calls = calls.wrapping_add(1));
    let mut again = 0u32;
    let second = leaves_metadata_in_the_clear(data, &mut |_| again = again.wrapping_add(1));
    assert_eq!(first, second, "the answer is not the same for the same bytes");
    assert_eq!(calls, again, "the progress is not the same for the same bytes");
});
