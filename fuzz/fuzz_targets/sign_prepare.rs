#![no_main]
//! Arbitrary bytes as the file that PDFium wrote, into the writer of the empty signature
//! (`partridge_engine::signprep::prepare`): a second reader and writer of PDF structure in the
//! engine, which reads the catalog, the form, the list of fields, the page and its list of
//! annotations, and writes an update after the bytes it was given.
//!
//! The first byte chooses the page, the place of the signature (invisible, a new box, a field the
//! document has or one it has not) and whether there is a look; the rest is the file. The seeds are
//! files that PDFium wrote (a plain page, a form, a form with a signature field, a turned page).
//!
//! The writer must not panic, hang or use more memory than the file asks for. When it makes a file:
//! the bytes it was given are the start of it, it ends with a table and a `%%EOF`, the last
//! `startxref` names that table, every object the table lists is where it says, the byte range
//! in the file is the one the hole makes (found again here by a simpler routine), and the hole is
//! `<`, zeros and `>`.

use libfuzzer_sys::fuzz_target;
use partridge_engine::signlook::build;
use partridge_engine::signprep::{prepare, Place, Spec};

fn text_after(bytes: &[u8], key: &[u8]) -> Option<usize> {
    bytes
        .windows(key.len())
        .rposition(|w| w == key)
        .map(|at| at + key.len())
}

fuzz_target!(|data: &[u8]| {
    let Some((&choice, file)) = data.split_first() else {
        return;
    };
    let look = (choice & 0x40 != 0).then(|| {
        build(
            200.0,
            60.0,
            [1.0, 0.0, 0.0, 1.0],
            &["Digitally signed by A".to_string()],
            None,
            [0, 0, 0, 255],
        )
        .unwrap()
    });
    let place = match choice & 3 {
        0 => Place::Invisible,
        1 => Place::New {
            bounds: [72.0, 600.0, 272.0, 660.0],
        },
        2 => Place::Existing {
            name: "empty_signature",
        },
        _ => Place::Existing { name: "Signature1" },
    };
    let spec = Spec {
        page: ((choice >> 2) & 3) as usize,
        place,
        name: "Fuzz",
        reason: "r",
        location: "l",
        signed_at: 1_781_524_800,
        reserve: 600,
        look: look.as_ref(),
    };
    let pages = (((choice >> 4) & 3) as usize) + 1;
    let Ok(made) = prepare(file.to_vec(), pages, &spec) else {
        return;
    };
    let out = &made.bytes;
    assert!(out.starts_with(file));
    assert!(out.ends_with(b"%%EOF\n"));
    // The hole.
    assert_eq!(out[made.hole_start], b'<');
    assert_eq!(out[made.hole_end - 1], b'>');
    assert_eq!(made.hole_end - made.hole_start, 600 * 2 + 2);
    assert!(out[made.hole_start + 1..made.hole_end - 1]
        .iter()
        .all(|b| *b == b'0'));
    // The byte range names the hole.
    let range = format!(
        "[{:010} {:010} {:010} {:010}]",
        0,
        made.hole_start,
        made.hole_end,
        out.len() - made.hole_end
    );
    let at = text_after(out, b"/ByteRange ").expect("a byte range");
    assert_eq!(&out[at..at + range.len()], range.as_bytes());
    // Nothing the file held was turned into the replacement character on the way (which is what
    // happens to a string that is not text when it is copied through text): a file that has none
    // gets none in the part that was added.
    let replacement = [0xef_u8, 0xbf, 0xbd];
    let has_replacement = |bytes: &[u8]| bytes.windows(3).any(|w| w == replacement);
    if !has_replacement(file) {
        assert!(!has_replacement(&out[file.len()..]), "a string was turned into text");
    }
    // The last table is where the last `startxref` says, and what it lists is where it says.
    let start = text_after(out, b"startxref\n").expect("startxref");
    let end = out[start..].iter().position(|b| *b == b'\n').unwrap();
    let table: usize = std::str::from_utf8(&out[start..start + end])
        .unwrap()
        .parse()
        .unwrap();
    assert!(out[table..].starts_with(b"xref\n"));
    // The trailer names the catalog, the new size, and the table before (inside the old file).
    let trailer = &out[table..];
    let trailer_at = trailer
        .windows(7)
        .position(|w| w == b"trailer")
        .expect("a trailer");
    let trailer = &trailer[trailer_at..];
    for key in [&b" /Root "[..], b" /Size ", b" /Prev "] {
        assert!(
            trailer.windows(key.len()).any(|w| w == key),
            "the trailer has no {}",
            String::from_utf8_lossy(key)
        );
    }
    // The last one is ours: the values copied from the file before it may hold the same words.
    let prev_at = trailer.windows(7).rposition(|w| w == b" /Prev ").unwrap() + 7;
    let prev: usize = String::from_utf8_lossy(&trailer[prev_at..])
        .split_whitespace()
        .next()
        .and_then(|n| n.parse().ok())
        .expect("a number after /Prev");
    assert!(prev < file.len(), "/Prev points past the file it was given");
    let text = String::from_utf8_lossy(&out[table + 5..]);
    let mut lines = text.lines();
    while let Some(head) = lines.next() {
        if head.starts_with("trailer") {
            break;
        }
        let mut it = head.split_whitespace();
        let first: u32 = it.next().unwrap().parse().unwrap();
        let count: u32 = it.next().unwrap().parse().unwrap();
        for i in 0..count {
            let entry = lines.next().unwrap();
            let offset: usize = entry[..10].parse().unwrap();
            let want = format!("{} ", first + i);
            assert!(
                out[offset..].starts_with(want.as_bytes()),
                "object {} is not at {offset}",
                first + i
            );
        }
    }
});
