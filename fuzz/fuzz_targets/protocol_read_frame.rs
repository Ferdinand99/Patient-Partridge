#![no_main]
//! Arbitrary bytes as a stream of frames, as the app reads the engine's output and the engine
//! reads the app's. Checks that:
//! - `read_frame` and `read_frame_limited` never panic;
//! - they never allocate by the announced length: the biggest single allocation of a run is
//!   measured and must stay in proportion to the bytes that were really there (a 4 byte header
//!   that announces 128 MiB must not cost 128 MiB). This needs input longer than the first chunk
//!   the reader makes room for (64 KiB), so run it with `-max_len` above 65536;
//! - a stream that arrives in small pieces, with interrupted reads, gives the same frames and the
//!   same ending as one that arrives whole;
//! - every frame that comes out can be given to both decoders, and what they return is inside
//!   the caps.

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::{Cursor, Read};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use libfuzzer_sys::fuzz_target;
use partridge_fuzz::{check_request_caps, check_response_caps, ChunkReader};
use partridge_protocol::{read_frame, read_frame_limited, ProtocolError, Request, Response};

/// The biggest single allocation (or growth of one) since the last reset.
static BIGGEST: AtomicUsize = AtomicUsize::new(0);

struct Tracking;

// SAFETY: every call is passed on unchanged to the system allocator.
unsafe impl GlobalAlloc for Tracking {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        BIGGEST.fetch_max(layout.size(), Relaxed);
        System.alloc(layout)
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        BIGGEST.fetch_max(layout.size(), Relaxed);
        System.alloc_zeroed(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        BIGGEST.fetch_max(new_size, Relaxed);
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static ALLOCATOR: Tracking = Tracking;

/// Input layout: two bytes of limit (little endian; 0 means use `read_frame`), one byte with the
/// number of pieces, that many bytes of piece sizes (0 means an interrupted read), then the data.
struct Input<'a> {
    limit: u16,
    pieces: &'a [u8],
    data: &'a [u8],
}

fn split(raw: &[u8]) -> Option<Input<'_>> {
    let (head, rest) = raw.split_first_chunk::<3>()?;
    let [a, b, n] = *head;
    let n = (n as usize).min(rest.len());
    Some(Input {
        limit: u16::from_le_bytes([a, b]),
        pieces: &rest[..n],
        data: &rest[n..],
    })
}

#[derive(Debug, PartialEq, Eq)]
enum Ending {
    Clean,
    Truncated,
    TooLarge(u64),
    Other(String),
}

fn drain<R: Read>(r: &mut R, limit: u16) -> (Vec<Vec<u8>>, Ending) {
    let mut frames = Vec::new();
    // Each frame takes at least 4 bytes of input, so this ends; the cap is only a safety net.
    for _ in 0..100_000 {
        let result = if limit == 0 {
            read_frame(r)
        } else {
            read_frame_limited(r, || limit as usize)
        };
        match result {
            Ok(Some(frame)) => frames.push(frame),
            Ok(None) => return (frames, Ending::Clean),
            Err(ProtocolError::Truncated) => return (frames, Ending::Truncated),
            Err(ProtocolError::FrameTooLarge(n)) => return (frames, Ending::TooLarge(n)),
            Err(e) => return (frames, Ending::Other(e.to_string())),
        }
    }
    unreachable!("a frame takes at least four bytes");
}

fuzz_target!(|raw: &[u8]| {
    let Some(input) = split(raw) else {
        return;
    };
    BIGGEST.store(0, Relaxed);
    let (whole, whole_end) = drain(&mut Cursor::new(input.data), input.limit);
    let (pieces, pieces_end) = drain(&mut ChunkReader::new(input.data, input.pieces), input.limit);

    // A frame buffer grows by doubling, so twice the data is the most it can take; the list of
    // frames takes 24 bytes for each frame of at least 4 bytes. Room for 128 MiB would not fit.
    let biggest = BIGGEST.load(Relaxed);
    assert!(
        biggest <= 8 * raw.len() + 256 * 1024,
        "an allocation of {biggest} bytes for {} bytes of input",
        raw.len()
    );

    assert_eq!(
        whole, pieces,
        "frames differ when the data arrives in pieces"
    );
    assert_eq!(
        whole_end, pieces_end,
        "ending differs when the data arrives in pieces"
    );

    let total: usize = whole.iter().map(|f| f.len() + 4).sum();
    assert!(total <= input.data.len(), "more bytes out than in");
    if input.limit != 0 {
        assert!(whole.iter().all(|f| f.len() <= input.limit as usize));
    }
    for frame in whole {
        if let Ok(request) = Request::decode(&frame) {
            check_request_caps(&request);
        }
        if let Ok(response) = Response::decode(frame) {
            check_response_caps(&response);
        }
    }
});
