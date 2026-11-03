#![no_main]
//! Arbitrary bytes as the file PDFium wrote for a save, through the mend of a compressed XMP
//! metadata stream that PDFium wrote without its filter
//! (`partridge_engine::metadata::repair::mend`). The mend reads the structure of the file with the
//! tokenizer of the metadata check and inflates data from the file, so it is a pure function over
//! untrusted bytes. It checks that:
//! - the function never panics and does not take long, whatever the bytes (run it with a low
//!   `-timeout`; a stream that swells is stopped at the limit for inflating);
//! - what it gives back is the bytes it was given, or the same bytes with an update added to the
//!   end, and the update is not longer than the stream data it carries plus a small amount for
//!   each stream;
//! - it gives the same answer, and the same number of reports of progress, for the same bytes;
//! - a second mend of its own result changes nothing.

use libfuzzer_sys::fuzz_target;
use partridge_engine::metadata::repair::mend;

/// A limit like the one of a save, so that the refusal for a file that grows too much is reached.
const LIMIT: usize = 64 * 1024 * 1024;

fuzz_target!(|data: &[u8]| {
    let mut calls = 0u32;
    let first = mend(data.to_vec(), LIMIT, &mut |_| calls = calls.wrapping_add(1));
    let mut again = 0u32;
    let second = mend(data.to_vec(), LIMIT, &mut |_| again = again.wrapping_add(1));
    assert_eq!(first, second, "the answer is not the same for the same bytes");
    assert_eq!(calls, again, "the progress is not the same for the same bytes");
    if let Ok(out) = first {
        assert!(out.starts_with(data), "the bytes of the file were changed");
        assert!(
            out.len() <= data.len() * 4 + 1024,
            "the update is longer than the streams in it can explain"
        );
        let twice = mend(out.clone(), LIMIT, &mut |_| {}).expect("a mended file is mended");
        assert_eq!(twice, out, "a second mend changed the bytes");
    }
});
