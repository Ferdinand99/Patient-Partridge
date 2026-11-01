#![no_main]
//! Arbitrary bytes as a stream of frames, as the app reads the engine's output and the engine
//! reads the app's. Checks that:
//! - `read_frame` and `read_frame_limited` never panic and never allocate by the announced length;
//! - a stream that arrives in small pieces, with interrupted reads, gives the same frames and the
//!   same ending as one that arrives whole;
//! - every frame that comes out can be given to both decoders.

use std::io::{Cursor, Read};

use libfuzzer_sys::fuzz_target;
use partridge_fuzz::ChunkReader;
use partridge_protocol::{read_frame, read_frame_limited, ProtocolError, Request, Response};

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
    let (whole, whole_end) = drain(&mut Cursor::new(input.data), input.limit);
    let (pieces, pieces_end) = drain(&mut ChunkReader::new(input.data, input.pieces), input.limit);
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
        let _ = Request::decode(&frame);
        let _ = Response::decode(frame);
    }
});
