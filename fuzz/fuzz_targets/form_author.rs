#![no_main]
//! The writer of form fields (`partridge_engine::author`): the code that reads the bytes PDFium
//! saved and writes new objects after them. It is a second writer of PDF structure inside the
//! engine and its input comes from a file a stranger wrote, so it has a target of its own.
//!
//! The fuzzer's bytes are a script. It takes a document of the test kit (a plain page, forms of
//! every shape the kit makes, a form that is nested, one with scripts, a page that is turned) or
//! bytes of its own, spoils them (a byte changed, the end cut off, a window overwritten), gives the
//! writer the geometry of four pages, and asks it for the names of the fields, the widgets of each
//! page, and a batch of changes built from the same bytes: fields of every kind with names that
//! meet each other, boxes that are not on the page, properties that do not fit their kind.
//!
//! Whatever the script does, the writer must not panic, hang or use more memory than a file the
//! size of the input, and:
//!
//! - every field it made has a name the rules allow (not empty, no dot);
//! - what it appends to the bytes it was given is a well formed update: the bytes it was given are
//!   untouched, the last line names a table, the table lists objects that are where it says they
//!   are, and the trailer names the table of the file it was given.

use std::collections::HashMap;
use std::sync::OnceLock;

use arbitrary::Unstructured;
use libfuzzer_sys::fuzz_target;
use partridge_engine::author::{rules, Author, PageGeo};
use partridge_fuzz::arb_ops;
use partridge_protocol::AuthorOp;
use partridge_testkit as kit;

fn fixtures() -> &'static [Vec<u8>] {
    static FIXTURES: OnceLock<Vec<Vec<u8>>> = OnceLock::new();
    FIXTURES.get_or_init(|| {
        vec![
            kit::simple_pdf(2),
            kit::forms_pdf(),
            kit::forms_nested_pdf(),
            kit::forms_script_pdf(),
            kit::forms_rotated_pdf(90),
            kit::forms_many_pdf(25),
            kit::turned_pdf(),
        ]
    })
}

fn spoil(u: &mut Unstructured, bytes: &mut Vec<u8>) {
    if bytes.is_empty() {
        return;
    }
    match u.int_in_range(0..=5u8).unwrap_or(0) {
        0 | 1 => {}
        2 => {
            for _ in 0..u.int_in_range(1..=6u8).unwrap_or(1) {
                let at = u.int_in_range(0..=bytes.len() - 1).unwrap_or(0);
                bytes[at] = u.arbitrary().unwrap_or(0);
            }
        }
        3 => {
            let keep = u.int_in_range(0..=bytes.len()).unwrap_or(bytes.len());
            bytes.truncate(keep);
        }
        4 => {
            let at = u.int_in_range(0..=bytes.len() - 1).unwrap_or(0);
            let n = u
                .int_in_range(1..=32usize)
                .unwrap_or(1)
                .min(bytes.len() - at);
            for b in &mut bytes[at..at + n] {
                *b = u.arbitrary().unwrap_or(0);
            }
        }
        _ => {
            // A number inside the cross reference table or the trailer made another number.
            let digits: Vec<usize> = bytes
                .iter()
                .enumerate()
                .filter(|(_, b)| b.is_ascii_digit())
                .map(|(i, _)| i)
                .collect();
            if !digits.is_empty() {
                let at = digits[u.int_in_range(0..=digits.len() - 1).unwrap_or(0)];
                bytes[at] = b'0' + u.int_in_range(0..=9u8).unwrap_or(0);
            }
        }
    }
}

fn geometry(u: &mut Unstructured) -> HashMap<usize, PageGeo> {
    const BOXES: [[f32; 4]; 3] = [
        [0.0, 0.0, 612.0, 792.0],
        [10.0, 20.0, 500.0, 700.0],
        [0.0, 0.0, 200.0, 100.0],
    ];
    (0..4)
        .map(|page| {
            (
                page,
                PageGeo {
                    turns: u.int_in_range(0..=3u8).unwrap_or(0),
                    visible: BOXES[u.int_in_range(0..=2usize).unwrap_or(0)],
                },
            )
        })
        .collect()
}

fn number_at(bytes: &[u8], from: usize) -> Option<(usize, usize)> {
    let end = bytes[from..].iter().position(|b| !b.is_ascii_digit())? + from;
    let n = std::str::from_utf8(&bytes[from..end]).ok()?.parse().ok()?;
    Some((n, end))
}

/// Checks the update that was appended to `original`.
fn check_update(original: &[u8], out: &[u8]) {
    assert!(
        out.starts_with(original),
        "the bytes it was given were changed"
    );
    assert!(out.ends_with(b"%%EOF\n"));
    let at = out
        .windows(10)
        .rposition(|w| w == b"startxref\n")
        .expect("no startxref");
    let (table, _) = number_at(out, at + 10).expect("no number after startxref");
    assert!(
        table >= original.len() && table < at,
        "the table is not in the update"
    );
    assert!(
        out[table..].starts_with(b"xref\n"),
        "startxref does not name a table"
    );
    let mut i = table + 5;
    while !out[i..].starts_with(b"trailer\n") {
        let (_start, e) = number_at(out, i).expect("subsection start");
        assert_eq!(out[e], b' ');
        let (count, e) = number_at(out, e + 1).expect("subsection count");
        assert_eq!(out[e], b'\n');
        i = e + 1;
        for _ in 0..count {
            let entry = &out[i..i + 20];
            assert!(
                entry.ends_with(b" n \n"),
                "entry {:?}",
                String::from_utf8_lossy(entry)
            );
            let offset: usize = std::str::from_utf8(&entry[..10]).unwrap().parse().unwrap();
            let generation: u32 = std::str::from_utf8(&entry[11..16])
                .unwrap()
                .parse()
                .unwrap();
            assert!(
                offset >= original.len() && offset < table,
                "object outside the update"
            );
            let (number, e) = number_at(out, offset).expect("object number");
            let head = format!(" {generation} obj\n");
            assert!(
                out[e..].starts_with(head.as_bytes()),
                "object {number} is not where the table says"
            );
            i += 20;
        }
    }
    let trailer = String::from_utf8_lossy(&out[i..at]).into_owned();
    assert!(
        trailer.contains("/Prev "),
        "the trailer does not name the table before"
    );
    assert!(trailer.contains("/Root "));
}

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    let fixtures = fixtures();
    let pick = u.int_in_range(0..=fixtures.len()).unwrap_or(0);
    let mut bytes = if pick < fixtures.len() {
        fixtures[pick].clone()
    } else {
        let n = u.len().min(1024);
        u.bytes(n).map(<[u8]>::to_vec).unwrap_or_default()
    };
    spoil(&mut u, &mut bytes);
    let original = bytes.clone();
    let geo = geometry(&mut u);
    let Ok(mut author) = Author::new(bytes, geo) else {
        return;
    };
    let _ = author.names();
    for page in 0..4 {
        let _ = author.list_page(page);
    }
    let ops = arb_ops(&mut u, 12).unwrap_or_default();
    if author.apply(&ops).is_err() {
        // Nothing is kept of a batch that did not work.
        return;
    }
    for op in &ops {
        if let AuthorOp::Add(field) = op {
            assert!(
                rules::check_name(&field.name).is_ok(),
                "a field was made with the name {:?}",
                field.name
            );
        }
    }
    let _ = author.names();
    for page in 0..4 {
        let _ = author.list_page(page);
    }
    if let Ok(outcome) = author.finish() {
        check_update(&original, &outcome.bytes);
    }
});
