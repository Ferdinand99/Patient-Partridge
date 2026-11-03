//! The verifier against files that leak in each layer, and against files that do not.
//!
//! Every leak below is a file made byte by byte in `common`. For each one the test states which
//! layers must see it, and then proves three things:
//!
//! 1. with all layers on, exactly those layers (and only the needle of this file) hit;
//! 2. each of those layers finds the leak on its own;
//! 3. with those layers off, nothing is found: no other layer sees it, so the layer is needed.
//!
//! The switches of the search (UTF-16, string decoding, joining split text) are shown the same
//! way: a layer on its own finds the leak with the switch on and does not find it with it off.
//!
//! This needs qpdf and Poppler on the PATH (the test container and CI have them).

mod common;

use std::collections::BTreeSet;

use common::*;
use redaction_verify::{verify, Features, Layer, Options, Report, Spec};

const MARKS: [&str; 28] = [
    "LEAK-00-JUNK",
    "LEAK-01-PLAIN",
    "LEAK-02-FLATE",
    "LEAK-03-HEXCONTENT",
    "LEAK-04-UTF16LIT",
    "LEAK-05-UTF16HEX",
    "LEAK-06-ESCAPED",
    "LEAK-07-SPLIT",
    "LEAK-08-OBJSTM",
    "LEAK-09-OLDFLATE",
    "LEAK-10-OLDPLAIN",
    "LEAK-11-XMP",
    "LEAK-12-BOOKMARK",
    "LEAK-13-ALT",
    "LEAK-14-ACTUALTEXT",
    "LEAK-15-FIELD",
    "LEAK-16-FIELDDV",
    "LEAK-17-ATTNAME",
    "LEAK-18-ATTBODY",
    "LEAK-19-PIECEINFO",
    "LEAK 20 NAMEKEY",
    "LEAK-21-ENCRYPTED",
    "LEAK-22-GLYPHS",
    "LEAK-23-INFO",
    "LEAK-24-ORPHAN",
    "LEAK-25-LZWEARLY",
    "LEAK-26-LZWLATE",
    "LEAK-27-PNGOBJSTM",
];

const IMG_A: [u8; 12] = [11, 22, 33, 44, 55, 66, 77, 88, 99, 111, 122, 133];
const IMG_B_INDEXES: [u8; 4] = [0, 1, 2, 3];
const IMG_B_PALETTE: [u8; 12] = [255, 0, 0, 0, 255, 0, 0, 0, 255, 7, 8, 9];
const IMG_C: [u8; 12] = [5, 15, 25, 35, 45, 55, 65, 75, 85, 95, 105, 115];
const IMG_D: [u8; 12] = [1, 3, 5, 7, 9, 11, 13, 15, 17, 19, 21, 23];

/// The needle index of the images: after the texts.
const A: usize = MARKS.len();
const B: usize = A + 1;
const C: usize = A + 2;
const D: usize = A + 3;

/// The pixels of image B as `pdfimages` draws them: the palette colours.
fn img_b_pixels() -> Vec<u8> {
    IMG_B_INDEXES
        .iter()
        .flat_map(|&i| IMG_B_PALETTE[usize::from(i) * 3..usize::from(i) * 3 + 3].to_vec())
        .collect()
}

fn spec() -> Spec {
    let mut s = Spec::default();
    for m in MARKS {
        s = s.text(m);
    }
    s.image_sha256(&sha(&IMG_A))
        .image_sha256(&sha(&img_b_pixels()))
        .image_sha256(&sha(&IMG_C))
        .image_sha256(&sha(&IMG_D))
}

fn check(bytes: &[u8], opts: &Options) -> Report {
    let dir = Dir::new();
    let path = dir.put("file.pdf", bytes);
    verify(&path, &spec(), opts).expect("the tools run")
}

fn show(r: &Report) -> String {
    r.render(&spec(), true)
}

// ---------------------------------------------------------------------------------------------
// Files.

const IMAGE_DRAW: &[u8] = b" q 100 0 0 100 72 500 cm /Im1 Do Q";

/// A page with the given text operators, and the image drawn.
fn page(ops: &[u8]) -> Bytes {
    [
        b"BT /F1 12 Tf 72 720 Td ".as_slice(),
        ops,
        b" ET",
        IMAGE_DRAW,
    ]
    .concat()
}

fn benign() -> Doc {
    Doc::rich().with_content(&page(b"(Quarterly report) Tj"))
}

fn tj(s: &[u8]) -> Bytes {
    [s, b" Tj"].concat()
}

struct Case {
    name: &'static str,
    bytes: Bytes,
    needle: usize,
    layers: &'static [Layer],
}

fn with(doc: &Doc) -> Bytes {
    doc.build()
}

fn glyph_case() -> Bytes {
    let mark = MARKS[22];
    let mut chars: Vec<char> = Vec::new();
    for c in mark.chars() {
        if !chars.contains(&c) {
            chars.push(c);
        }
    }
    let digits = [
        "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
    ];
    let names: Vec<String> = chars
        .iter()
        .map(|c| match c {
            '-' => "hyphen".to_owned(),
            d if d.is_ascii_digit() => digits[d.to_digit(10).unwrap() as usize].to_owned(),
            l => l.to_string(),
        })
        .collect();
    let font = format!(
        "/Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding << /Type /Encoding \
         /Differences [1 {}] >>",
        names
            .iter()
            .map(|n| format!("/{n}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let codes: String = mark
        .chars()
        .map(|c| format!("{:02X}", chars.iter().position(|x| *x == c).unwrap() + 1))
        .collect();
    let mut doc = benign();
    doc.font = font;
    doc.content = page(format!("<{codes}> Tj").as_bytes());
    doc.build()
}

fn cases() -> Vec<Case> {
    use Layer::*;
    let mut v = Vec::new();
    let mut add = |name, bytes, needle, layers| {
        v.push(Case {
            name,
            bytes,
            needle,
            layers,
        })
    };

    // Left behind after the last %%EOF.
    let mut junk = benign().build();
    junk.extend(format!("\n% {} left behind\n", MARKS[0]).into_bytes());
    add("trailing junk", junk, 0, &[Raw]);

    add(
        "plain content stream",
        with(&benign().with_content(&page(&tj(&lit(MARKS[1]))))),
        1,
        &[Raw, Qdf, Carve, Decode, Text],
    );

    let mut flate_doc = benign().with_content(&page(&tj(&lit(MARKS[2]))));
    flate_doc.content_flate = true;
    add(
        "Flate content stream",
        with(&flate_doc),
        2,
        &[Qdf, Carve, Decode, Text],
    );

    add(
        "text in a hex string",
        with(&benign().with_content(&page(&tj(&hex(MARKS[3]))))),
        3,
        &[Raw, Qdf, Carve, Decode, Text],
    );

    let mut d = benign();
    d.info = Some([b"/Title ".as_slice(), &utf16_literal(MARKS[4])].concat());
    add("UTF-16 literal string", with(&d), 4, &[Raw, Qdf, Structure]);

    let mut d = benign();
    d.info = Some([b"/Title ".as_slice(), &utf16_hex(MARKS[5])].concat());
    add("UTF-16 hex string", with(&d), 5, &[Raw, Qdf, Structure]);

    let mut d = benign();
    d.info = Some([b"/Title ".as_slice(), &escaped(MARKS[6])].concat());
    add(
        "escaped literal string",
        with(&d),
        6,
        &[Raw, Qdf, Structure],
    );

    add(
        "text split over a TJ array",
        with(&benign().with_content(&page(b"[(LEAK-0) -20 (7-SPLIT)] TJ"))),
        7,
        &[Raw, Qdf, Carve, Decode, Text],
    );

    add(
        "Info dictionary in an object stream",
        object_stream_file(
            &[b"/Title ".as_slice(), &lit(MARKS[8])].concat(),
            b"BT /F1 12 Tf 72 720 Td (Quarterly report) Tj ET",
        ),
        8,
        &[Qdf, Carve, Structure],
    );

    let mut d = benign().with_content(&page(&tj(&lit(MARKS[9]))));
    d.content_flate = true;
    add(
        "compressed old revision",
        d.build_with_new_content(&page(&tj(&lit("Quarterly report"))), true),
        9,
        &[Carve, Decode],
    );

    let d = benign().with_content(&page(&tj(&lit(MARKS[10]))));
    add(
        "plain old revision",
        d.build_with_new_content(&page(&tj(&lit("Quarterly report"))), false),
        10,
        &[Raw, Carve, Decode],
    );

    let mut d = benign();
    d.xmp = Some((
        format!("<x:xmpmeta><dc:title>{}</dc:title></x:xmpmeta>", MARKS[11]),
        true,
    ));
    add(
        "compressed XMP metadata",
        with(&d),
        11,
        &[Qdf, Carve, Structure],
    );

    let mut d = benign();
    d.outline_title = Some(utf16_hex(MARKS[12]));
    add("bookmark title", with(&d), 12, &[Raw, Qdf, Structure]);

    let mut d = benign();
    d.structure = Some((lit(MARKS[13]), lit("A paragraph")));
    add("structure /Alt", with(&d), 13, &[Raw, Qdf, Structure]);

    add(
        "marked-content /ActualText",
        with(
            &benign().with_content(&page(
                format!(
                    "/Span << /ActualText {} >> BDC (visible) Tj EMC",
                    lossy(&lit(MARKS[14]))
                )
                .as_bytes(),
            )),
        ),
        14,
        &[Raw, Qdf, Carve, Text],
    );

    let mut d = benign();
    d.field = Some((lit(MARKS[15]), lit("Name")));
    add(
        "form field value",
        with(&d),
        15,
        &[Raw, Qdf, Text, Structure],
    );

    let mut d = benign();
    d.field = Some((lit("Jane Doe"), lit(MARKS[16])));
    add(
        "form field default value",
        with(&d),
        16,
        &[Raw, Qdf, Structure],
    );

    let mut d = benign();
    d.attachment = Some((
        lit(MARKS[17]),
        lit("Meeting notes"),
        b"Agenda only.".to_vec(),
    ));
    add("attachment name", with(&d), 17, &[Raw, Qdf, Structure]);

    let mut d = benign();
    d.attachment = Some((
        lit("notes.txt"),
        lit("Meeting notes"),
        format!("the notes: {}", MARKS[18]).into_bytes(),
    ));
    add("compressed attachment body", with(&d), 18, &[Qdf, Carve]);

    let mut d = benign();
    d.piece_info = Some([b"/Data ".as_slice(), &lit(MARKS[19])].concat());
    add("PieceInfo value", with(&d), 19, &[Raw, Qdf, Structure]);

    let mut d = benign();
    d.piece_info = Some(format!("/{} (x)", name_escaped(MARKS[20])).into_bytes());
    add("name written with #20", with(&d), 20, &[Structure]);

    let dir = Dir::new();
    let mut d = benign();
    d.hidden_layer = true;
    d.content = page(
        format!(
            "/OC /oc1 BDC BT /F1 12 Tf 72 680 Td {} Tj ET EMC BT /F1 12 Tf 72 720 Td (Quarterly report) Tj ET",
            lossy(&lit(MARKS[21]))
        )
        .as_bytes(),
    );
    let encrypted = encrypt(&dir, &d.build());
    add(
        "hidden text in an encrypted file",
        encrypted,
        21,
        &[Qdf, Carve, Decode],
    );

    add("text as glyph codes", glyph_case(), 22, &[Decode, Text]);

    let mut d = benign();
    d.info = Some([b"/Title ".as_slice(), &lit(MARKS[23])].concat());
    add("Info /Title", with(&d), 23, &[Raw, Qdf, Structure]);

    // A stream that nothing refers to and that is not a content stream: only a scan finds it.
    let mut d = benign();
    d.orphans.push((
        20,
        "/Filter /FlateDecode".into(),
        flate(format!("the notes: {}", MARKS[24]).as_bytes()),
    ));
    add("data stream that nothing refers to", with(&d), 24, &[Carve]);

    // LZW with both settings of EarlyChange, and a PNG-predicted object stream. qpdf reads the
    // attachments too, so it must agree with the verifier's own decoders.
    for (idx, early, name) in [
        (25, true, "LZW attachment (EarlyChange 1)"),
        (26, false, "LZW attachment (EarlyChange 0)"),
    ] {
        let mut d = benign();
        let body = lzw_body(MARKS[idx]);
        let parms = if early {
            ""
        } else {
            " /DecodeParms << /EarlyChange 0 >>"
        };
        d.attachment_stream = Some((
            format!("/Filter /LZWDecode{parms}"),
            lzw_encode(&body, early),
        ));
        add(name, with(&d), idx, &[Qdf, Carve]);
    }
    add(
        "Info in a PNG-predicted object stream",
        object_stream_file_with(
            &[b"/Title ".as_slice(), &lit(MARKS[27])].concat(),
            b"BT /F1 12 Tf 72 720 Td (Quarterly report) Tj ET",
            true,
        ),
        27,
        &[Qdf, Carve, Structure],
    );

    // Images.
    let mut d = benign();
    d.page_image = Some(Image::rgb_flate(&IMG_A));
    add("image on the page", with(&d), A, &[Images]);

    let mut d = benign();
    d.page_image = Some(Image::indexed(&IMG_B_INDEXES, &IMG_B_PALETTE));
    add(
        "indexed image (pixels are palette colours)",
        with(&d),
        B,
        &[Images],
    );

    let mut d = benign();
    d.page_image = Some(Image::rgb_hex_flate(&IMG_C));
    let replacement = Image::rgb_flate(&[9; 12]);
    add(
        "image in an old revision",
        d.build_with_new_image(&replacement),
        C,
        &[Images],
    );

    let mut d = benign();
    d.thumb = Some(Image::rgb_flate(&IMG_D));
    add("page thumbnail", with(&d), D, &[Images]);

    v
}

/// Several thousand bytes of pseudo-random words, then the mark: long enough for LZW to run its
/// table through the code widths 9 to 12.
fn lzw_body(mark: &str) -> Bytes {
    let mut x: u32 = 0x1234_5678;
    let mut out = Bytes::new();
    while out.len() < 6000 {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        out.push(b'a' + (x % 26) as u8);
        if x.is_multiple_of(7) {
            out.push(b' ');
        }
    }
    out.extend_from_slice(format!(" {mark} ").as_bytes());
    out
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn set(layers: &[Layer]) -> BTreeSet<Layer> {
    layers.iter().copied().collect()
}

// ---------------------------------------------------------------------------------------------

#[test]
fn each_leak_is_seen_by_exactly_the_expected_layers() {
    for c in cases() {
        let r = check(&c.bytes, &Options::default());
        assert_eq!(
            r.needles_hit(),
            BTreeSet::from([c.needle]),
            "{}: wrong needles\n{}",
            c.name,
            show(&r)
        );
        assert_eq!(
            r.hit_layers(),
            set(c.layers),
            "{}: wrong layers\n{}",
            c.name,
            show(&r)
        );
    }
}

#[test]
fn each_expected_layer_finds_the_leak_on_its_own() {
    for c in cases() {
        for &layer in c.layers {
            let r = check(&c.bytes, &Options::only(&[layer]));
            assert_eq!(
                r.needles_hit(),
                BTreeSet::from([c.needle]),
                "{}: layer {layer} alone does not find it\n{}",
                c.name,
                show(&r)
            );
        }
    }
}

#[test]
fn with_the_expected_layers_off_nothing_is_found() {
    for c in cases() {
        let r = check(&c.bytes, &Options::without(c.layers));
        assert!(
            r.hits.is_empty(),
            "{}: found with {:?} off, so another layer sees it too\n{}",
            c.name,
            c.layers,
            show(&r)
        );
    }
}

#[test]
fn these_layers_are_the_only_one_for_some_leak() {
    // For each of these layers there is a file that no other layer sees: remove the layer and a
    // real leak goes through. The other layers have such files in `hard_cases.rs`.
    for layer in [Layer::Raw, Layer::Carve, Layer::Structure, Layer::Images] {
        let sole = cases().into_iter().any(|c| c.layers == [layer]);
        assert!(sole, "no leak in the tests is found by {layer} alone");
    }
}

#[test]
fn every_leak_hit_has_a_layer_and_an_offset_or_a_place() {
    for c in cases() {
        let r = check(&c.bytes, &Options::default());
        for h in &r.hits {
            assert!(!h.location.is_empty(), "{}: a hit without a place", c.name);
            if matches!(
                h.layer,
                Layer::Raw | Layer::Qdf | Layer::Carve | Layer::Text
            ) {
                assert!(
                    h.offset.is_some(),
                    "{}: {} hit without offset",
                    c.name,
                    h.layer
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The switches of the search.

fn only_with(layer: Layer, features: Features) -> Options {
    Options {
        features,
        ..Options::only(&[layer])
    }
}

fn case(name: &str) -> Case {
    cases()
        .into_iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("no case {name}"))
}

#[track_caller]
fn needs_switch(name: &str, layer: Layer, off: Features) {
    let c = case(name);
    let on = check(&c.bytes, &only_with(layer, Features::default()));
    assert!(
        on.needles_hit().contains(&c.needle),
        "{name}: not found with the switch on"
    );
    let r = check(&c.bytes, &only_with(layer, off));
    assert!(
        r.hits.is_empty(),
        "{name}: still found in {layer} with the switch off\n{}",
        show(&r)
    );
}

#[test]
fn utf16_search_is_needed_for_a_utf16_literal() {
    let off = Features {
        utf16: false,
        ..Features::default()
    };
    needs_switch("UTF-16 literal string", Layer::Raw, off);
    needs_switch("UTF-16 hex string", Layer::Raw, off);
}

#[test]
fn string_decoding_is_needed_for_hex_and_escaped_strings() {
    let off = Features {
        decode_strings: false,
        ..Features::default()
    };
    needs_switch("text in a hex string", Layer::Raw, off);
    needs_switch("UTF-16 hex string", Layer::Raw, off);
    needs_switch("escaped literal string", Layer::Raw, off);
    needs_switch("text in a hex string", Layer::Carve, off);
    // Not in the qdf layer: qpdf writes the strings of a content stream in its own way (a hex
    // string comes out as a literal one), so the hex is gone before the verifier reads it.
}

#[test]
fn joining_split_text_is_needed_for_text_over_several_operators() {
    let off = Features {
        split_text: false,
        ..Features::default()
    };
    for layer in [Layer::Raw, Layer::Qdf, Layer::Carve] {
        needs_switch("text split over a TJ array", layer, off);
    }
}

#[test]
fn text_layer_finds_a_leak_that_the_split_switch_does_not_touch() {
    // pdftotext joins the pieces itself: the Text layer does not depend on the switch.
    let c = case("text split over a TJ array");
    let r = check(
        &c.bytes,
        &only_with(
            Layer::Text,
            Features {
                split_text: false,
                ..Features::default()
            },
        ),
    );
    assert!(r.needles_hit().contains(&c.needle));
}

// ---------------------------------------------------------------------------------------------
// Where the image hashes match.

fn image_places(name: &str) -> Vec<String> {
    let c = case(name);
    check(&c.bytes, &Options::default())
        .hits_in(Layer::Images)
        .map(|h| h.location.clone())
        .collect()
}

#[test]
fn image_on_the_page_is_found_stored_and_drawn() {
    let places = image_places("image on the page");
    assert!(
        places.iter().any(|p| p.contains("filters undone")),
        "{places:?}"
    );
    assert!(places.iter().any(|p| p.contains("pdfimages")), "{places:?}");
}

#[test]
fn indexed_image_is_found_only_as_drawn() {
    let places = image_places("indexed image (pixels are palette colours)");
    assert!(!places.is_empty());
    assert!(places.iter().all(|p| p.contains("pdfimages")), "{places:?}");
}

#[test]
fn old_revision_and_thumbnail_images_are_found_only_by_carving() {
    for name in ["image in an old revision", "page thumbnail"] {
        let places = image_places(name);
        assert!(!places.is_empty(), "{name}");
        assert!(
            places.iter().all(|p| !p.contains("pdfimages")),
            "{name}: {places:?}"
        );
        assert!(
            places.iter().all(|p| p.contains("filters undone")),
            "{name}: {places:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Clean files.

fn clean_files() -> Vec<(&'static str, Bytes)> {
    let dir = Dir::new();
    let mut compressed = benign();
    compressed.content_flate = true;
    compressed.xmp = Some((
        "<x:xmpmeta><dc:title>Quarterly report</dc:title></x:xmpmeta>".into(),
        true,
    ));
    // Pieces of the needles that are not the needles: a clean file must not trip on them.
    let mut near = benign();
    near.content = page(
        b"(LEAK-0) Tj 0 -14 Td (PLAIN) Tj 0 -14 Td (LEAK-01-PLAI) Tj 0 -14 Td (KEEP-01-PLAIN) Tj",
    );
    near.info = Some(b"/Title (LEAK 20 NAMEKE) /Author (Q)".to_vec());
    let mut hidden = benign();
    hidden.hidden_layer = true;
    hidden.content = page(
        b"/OC /oc1 BDC BT /F1 12 Tf 72 680 Td (Draft notes) Tj ET EMC BT /F1 12 Tf 72 720 Td (Quarterly report) Tj ET",
    );
    let old = benign().build_with_new_content(&page(b"(Quarterly report) Tj"), true);
    vec![
        ("rich file", benign().build()),
        ("compressed content and XMP", compressed.build()),
        ("near misses", near.build()),
        (
            "object stream",
            object_stream_file(
                b"/Title (Quarterly report)",
                b"BT /F1 12 Tf 72 720 Td (Quarterly report) Tj ET",
            ),
        ),
        ("old revision without a secret", old),
        ("encrypted", encrypt(&dir, &hidden.build())),
        ("bare page", Doc::bare().build()),
    ]
}

#[test]
fn clean_files_report_nothing() {
    for (name, bytes) in clean_files() {
        let r = check(&bytes, &Options::default());
        assert!(r.is_clean(), "{name} is not clean:\n{}", show(&r));
        assert_eq!(
            r.layers_run,
            Layer::DEFAULT.to_vec(),
            "{name}: not every layer ran"
        );
    }
}

#[test]
fn clean_files_stay_clean_in_each_layer() {
    for (name, bytes) in clean_files() {
        for layer in Layer::ALL {
            let r = check(&bytes, &Options::only(&[layer]));
            assert!(r.is_clean(), "{name} in {layer}:\n{}", show(&r));
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Rules of the spec.

#[test]
fn each_scrubbed_key_is_found_on_its_own() {
    let bytes = benign().build();
    for key in redaction_verify::SCRUBBED_KEYS {
        let spec = Spec::default().forbid(&[key]);
        let dir = Dir::new();
        let path = dir.put("rich.pdf", &bytes);
        let r = verify(&path, &spec, &Options::default()).unwrap();
        assert!(
            r.rules_hit().iter().all(|rule| rule.starts_with(key)),
            "{key}: {:?}",
            r.rules_hit()
        );
        assert!(
            !r.rules_hit().is_empty(),
            "{key} is in the file but not found"
        );
        assert_eq!(r.hit_layers(), set(&[Layer::Structure]), "{key}");
    }
}

#[test]
fn a_scrubbed_file_passes_the_scrub_rules() {
    let dir = Dir::new();
    let mut emptied = Doc::bare();
    emptied.info = Some(b"/Title () /Author ()".to_vec());
    for (name, doc) in [("bare", Doc::bare()), ("empty Info", emptied)] {
        let path = dir.put("scrubbed.pdf", &doc.build());
        let r = verify(&path, &Spec::default().scrubbed(), &Options::default()).unwrap();
        assert!(
            r.is_clean(),
            "{name}:\n{}",
            r.render(&Spec::default().scrubbed(), true)
        );
    }
}

#[test]
fn a_rich_file_fails_the_scrub_rules_everywhere() {
    let dir = Dir::new();
    let path = dir.put("rich.pdf", &benign().build());
    let spec = Spec::default().scrubbed();
    let r = verify(&path, &spec, &Options::default()).unwrap();
    let rules = r.rules_hit();
    for key in redaction_verify::SCRUBBED_KEYS {
        assert!(
            rules.iter().any(|x| x.starts_with(key)),
            "{key} not reported: {rules:?}"
        );
    }
}

#[test]
fn a_second_revision_fails_the_single_revision_rule() {
    let dir = Dir::new();
    let spec = Spec {
        single_revision: true,
        ..Spec::default()
    };
    let one = dir.put("one.pdf", &benign().build());
    assert!(verify(&one, &spec, &Options::default()).unwrap().is_clean());
    let two = dir.put(
        "two.pdf",
        &benign().build_with_new_content(&page(b"(Quarterly report) Tj"), false),
    );
    let r = verify(&two, &spec, &Options::default()).unwrap();
    assert_eq!(r.hit_layers(), set(&[Layer::Structure]));
    assert_eq!(r.rules_hit().len(), 2, "{:?}", r.rules_hit());
}

// ---------------------------------------------------------------------------------------------
// Reports and errors.

#[test]
fn a_report_names_the_needle_by_kind_and_digest_not_by_value() {
    let c = case("plain content stream");
    let r = check(&c.bytes, &Options::default());
    let text = r.render(&spec(), false);
    assert!(text.contains("LEAK layer=raw"), "{text}");
    assert!(text.contains("text #1 [sha256 "), "{text}");
    assert!(!text.contains(MARKS[1]), "the value was printed:\n{text}");
    assert!(r.render(&spec(), true).contains(MARKS[1]));
}

#[test]
fn an_empty_needle_and_a_bad_hash_are_refused() {
    let dir = Dir::new();
    let path = dir.put("a.pdf", &benign().build());
    assert!(verify(&path, &Spec::default().text(""), &Options::default()).is_err());
    assert!(verify(
        &path,
        &Spec::default().image_sha256("abc"),
        &Options::default()
    )
    .is_err());
}

#[test]
fn a_file_that_does_not_exist_is_an_error_not_a_clean_report() {
    let dir = Dir::new();
    let missing = dir.0.path().join("missing.pdf");
    assert!(verify(&missing, &spec(), &Options::default()).is_err());
}

#[test]
fn a_file_qpdf_cannot_read_is_a_problem_not_a_clean_report() {
    let dir = Dir::new();
    let path = dir.put("junk.pdf", b"this is not a pdf file at all");
    let r = verify(&path, &spec(), &Options::default()).unwrap();
    assert!(!r.problems.is_empty());
    assert!(!r.is_clean());
}

#[test]
fn the_spec_reads_from_json() {
    let s = Spec::from_json(
        r#"{"text":["a"],"metadata":["b"],"fields":["c"],"bookmarks":["d"],"attachments":["e"],
            "image_sha256":["AB"],"forbidden_keys":["/Thumb"],"single_revision":true}"#,
    )
    .unwrap();
    assert_eq!(s.needles.len(), 6);
    assert_eq!(s.forbidden_keys, ["/Thumb"]);
    assert!(s.single_revision);
    assert!(Spec::from_json(r#"{"nope":[]}"#).is_err());
    assert!(Spec::from_json("[]").is_err());
    assert!(
        Spec::from_json(r#"{"scrubbed":true}"#)
            .unwrap()
            .single_revision
    );
}
