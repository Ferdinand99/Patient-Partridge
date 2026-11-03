//! Text that sits where a byte search cannot read it: in fonts that use glyph codes, in
//! single-byte encodings that are not Latin-1, spaced by kerning, in text objects of their own, in
//! places that are not drawn, in rich text and XML, in images that are only partly covered, and in
//! streams with filters or encryption. Each case is a small file that leaks; the tests say which
//! layers must see it, and prove that those layers find it on their own and that no other layer
//! does. Every group has files that are clean.
//!
//! The command line is the interface under test, so a case reads the same as a gate in CI.

mod common;

use std::collections::BTreeSet;

use common::review::*;
use common::*;

fn text_spec(s: &str) -> String {
    format!(r#"{{"text":["{s}"]}}"#)
}

fn scrubbed_spec(s: &str) -> String {
    format!(r#"{{"text":["{s}"],"scrubbed":true}}"#)
}

#[derive(Debug, Clone, Copy)]
enum Expect {
    /// Exactly these layers hit (the default layers), each alone finds it, and without them nothing
    /// is found.
    Leak(&'static [&'static str]),
    Clean,
}

struct Case {
    name: &'static str,
    bytes: Bytes,
    spec: String,
    expect: Expect,
}

fn case(name: &'static str, bytes: Bytes, spec: String, expect: Expect) -> Case {
    Case {
        name,
        bytes,
        spec,
        expect,
    }
}

fn cid_show() -> Bytes {
    let fx = Fx::new(b"");
    format!("BT /F2 12 Tf 72 600 Td {} Tj ET\n", fx.show(NEEDLE)).into_bytes()
}

const TEX: &[u8] = b"BT /F1 12 Tf 72 680 Td [(Ola)-333(Nordmann)] TJ ET\n";
const TWO: &[u8] = b"BT /F1 12 Tf 72 680 Td (Ola) Tj ET BT /F1 12 Tf 95 680 Td (Nordmann) Tj ET\n";

fn plain_with_needle() -> Bytes {
    String::from_utf8_lossy(BENIGN)
        .replace("Quarterly report", NEEDLE)
        .into_bytes()
}

fn ascii85(data: &[u8]) -> Bytes {
    let mut out = b"<~".to_vec();
    for chunk in data.chunks(4) {
        let mut b = [0u8; 4];
        b[..chunk.len()].copy_from_slice(chunk);
        let mut v = u32::from_be_bytes(b);
        if chunk.len() == 4 && v == 0 {
            out.push(b'z');
            continue;
        }
        let mut digits = [0u8; 5];
        for d in digits.iter_mut().rev() {
            *d = (v % 85) as u8 + b'!';
            v /= 85;
        }
        out.extend_from_slice(&digits[..chunk.len() + 1]);
    }
    out.extend_from_slice(b"~>");
    out
}

fn raw_stream(dict: &str, data: &[u8]) -> Bytes {
    stream_obj(dict, data, false)
}

/// A zlib stream made of stored blocks, one for each part: the data is in it as it is, with the
/// five-byte header of the next block between the parts.
fn flate_stored(parts: &[&[u8]]) -> Bytes {
    let (mut a, mut b) = (1u32, 0u32);
    let mut out = vec![0x78, 0x01];
    for (i, p) in parts.iter().enumerate() {
        out.push(u8::from(i + 1 == parts.len()));
        let len = u16::try_from(p.len()).unwrap();
        out.extend(len.to_le_bytes());
        out.extend((!len).to_le_bytes());
        out.extend_from_slice(p);
        for &x in *p {
            a = (a + u32::from(x)) % 65521;
            b = (b + a) % 65521;
        }
    }
    out.extend(((b << 16) | a).to_be_bytes());
    out
}

/// A content stream, Flate with stored blocks, with the word `endstream` in the data before a hex
/// string that a block header cuts in two. A reader that finds the end of a stream by scanning
/// stops at that word; one that uses `/Length` does not. The text is in no layer that scans.
fn endstream_in_data() -> Bytes {
    let mut content = String::from_utf8_lossy(BENIGN).into_owned();
    content
        .push_str("BT /F1 12 Tf 72 640 Td (endstream) Tj ET\n/OC /oc1 BDC BT /F1 12 Tf 72 680 Td");
    let hex: String = NEEDLE.bytes().map(|b| format!("{b:02X}")).collect();
    let start = content.len();
    content.push_str(&format!("<{hex}> Tj ET EMC\n"));
    let cut = start + 10;
    let body = flate_stored(&[&content.as_bytes()[..cut], &content.as_bytes()[cut..]]);
    // The block header inside the hex string holds a byte that a hex string cannot read over.
    let header = &body[2 + 5 + cut..2 + 5 + cut + 5];
    assert!(header
        .iter()
        .any(|b| !b.is_ascii_hexdigit() && !matches!(b, 0 | 9 | 10 | 12 | 13 | 32)));
    let mut fx = Fx::new(b"");
    fx.resources_extra
        .push_str(" /Properties << /oc1 11 0 R >>");
    fx.catalog_extra
        .push_str(" /OCProperties << /OCGs [11 0 R] /D << /OFF [11 0 R] >> >>");
    fx.extra
        .insert(11, b"<< /Type /OCG /Name (Draft) >>".to_vec());
    let mut o = fx.objs();
    o.insert(4, raw_stream("/Filter /FlateDecode", &body));
    file(&o)
}

const XMP: &[u8] = br#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?><x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:creator>Smith &amp; Sons</dc:creator></rdf:Description></rdf:RDF></x:xmpmeta><?xpacket end="w"?>"#;

fn freetext(rc: &str) -> Fx {
    let mut fx = Fx::new(BENIGN);
    fx.page_extra.push_str(" /Annots [12 0 R]");
    fx.extra.insert(
        12,
        format!(
            "<< /Type /Annot /Subtype /FreeText /Rect [72 580 400 640] /DA (/Helv 12 Tf 0 g) \
             /Contents () /RC ({rc}) >>"
        )
        .into_bytes(),
    );
    fx
}

fn cases() -> Vec<Case> {
    use Expect::*;
    let mut v = Vec::new();
    let spec = text_spec(NEEDLE);
    let show = cid_show();
    let benign_plus = |extra: &[u8]| [BENIGN, extra].concat();

    // C1: text as glyph codes, in every place that a viewer does not show.
    v.push(case(
        "CID text drawn on the page",
        Fx::new(&benign_plus(&show)).build(),
        spec.clone(),
        Leak(&["decode", "text"]),
    ));
    let mut fx = Fx::new(BENIGN);
    fx.resources_extra.push_str(" /XObject << /Fm1 10 0 R >>");
    fx.extra.insert(10, Fx::form(&show));
    v.push(case(
        "CID text in a form that is not drawn",
        fx.build(),
        spec.clone(),
        Leak(&["decode"]),
    ));
    v.push(case(
        "CID text in hidden optional content",
        Fx::new(BENIGN).hidden(&show).build(),
        spec.clone(),
        Leak(&["decode"]),
    ));
    let mut fx = Fx::new(BENIGN);
    fx.page_extra.push_str(" /Annots [12 0 R]");
    fx.extra.insert(10, Fx::form(&show));
    fx.extra.insert(
        12,
        b"<< /Type /Annot /Subtype /Stamp /Rect [72 580 400 640] /F 4 /AP << /N 10 0 R >> >>"
            .to_vec(),
    );
    v.push(case(
        "CID text in an annotation appearance",
        fx.build(),
        spec.clone(),
        Leak(&["decode", "text"]),
    ));
    let off = format!(
        "BT /F2 12 Tf -900 -900 Td {} Tj ET\n",
        Fx::new(b"").show(NEEDLE)
    );
    v.push(case(
        "CID text off the page",
        Fx::new(&benign_plus(off.as_bytes())).build(),
        spec.clone(),
        Leak(&["decode"]),
    ));
    let mut fx = Fx::new(BENIGN);
    fx.extra.insert(13, packed("", &show));
    v.push(case(
        "CID text in an object that nothing refers to",
        fx.build(),
        scrubbed_spec(NEEDLE),
        Leak(&["decode"]),
    ));

    // C2: word spaces written as kerning, and words in text objects of their own.
    v.push(case(
        "kerned word space, hidden",
        Fx::new(BENIGN).hidden(TEX).build(),
        spec.clone(),
        Leak(&["qdf", "carve", "decode"]),
    ));
    let mut fx = Fx::new(BENIGN);
    fx.extra.insert(13, packed("", TEX));
    v.push(case(
        "kerned word space, in an object that nothing refers to",
        fx.build(),
        scrubbed_spec(NEEDLE),
        Leak(&["carve", "decode"]),
    ));
    v.push(case(
        "two text objects, hidden",
        Fx::new(BENIGN).hidden(TWO).build(),
        spec.clone(),
        Leak(&["qdf", "carve", "decode"]),
    ));
    v.push(case(
        "kerned word space, drawn",
        Fx::new(&benign_plus(TEX)).build(),
        spec.clone(),
        Leak(&["qdf", "carve", "decode", "text"]),
    ));
    let letters = b"BT /F1 12 Tf 72 680 Td (O) Tj 8 0 Td (l) Tj 6 0 Td (a) Tj 10 0 Td (N) Tj 9 0 Td \
        (o) Tj 8 0 Td (r) Tj 6 0 Td (d) Tj 8 0 Td (m) Tj 12 0 Td (a) Tj 8 0 Td (n) Tj 8 0 Td (n) Tj ET\n";
    v.push(case(
        "text set letter by letter, hidden",
        Fx::new(BENIGN).hidden(letters).build(),
        spec.clone(),
        Leak(&["qdf", "carve", "decode"]),
    ));
    v.push(case(
        "words drawn out of reading order",
        Fx::new(&benign_plus(
            b"BT /F1 12 Tf 102 680 Td (Nordmann) Tj ET BT /F1 12 Tf 72 680 Td (Ola) Tj ET\n",
        ))
        .build(),
        spec.clone(),
        Leak(&["text"]),
    ));

    // C1: single-byte encodings that are not Latin-1.
    let quote = text_spec("O\u{2019}Brien");
    v.push(case(
        "WinAnsi right quote, hidden",
        Fx::new(BENIGN)
            .hidden(b"BT /F1 12 Tf 72 680 Td (O\x92Brien) Tj ET\n")
            .build(),
        quote.clone(),
        Leak(&["decode"]),
    ));
    v.push(case(
        "WinAnsi right quote, drawn",
        Fx::new(&benign_plus(b"BT /F1 12 Tf 72 680 Td (O\x92Brien) Tj ET\n")).build(),
        quote,
        Leak(&["decode", "text"]),
    ));
    let mut fx = Fx::new(BENIGN);
    fx.extra.insert(
        14,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Times-Roman /Encoding /MacRomanEncoding >>"
            .to_vec(),
    );
    fx.extra.insert(
        13,
        stream_obj("", b"BT /F3 12 Tf 72 680 Td (Bl\x8cb\xber) Tj ET\n", true),
    );
    v.push(case(
        "MacRoman text in an object that nothing refers to",
        fx.build(),
        scrubbed_spec("Bl\u{e5}b\u{e6}r"),
        Leak(&["decode"]),
    ));
    let mut fx = Fx::new(BENIGN);
    fx.cid_text = "xy".into();
    fx.maps_to = Some(|c| {
        if c == 'x' {
            "a\u{30A}".into()
        } else {
            "\u{FB01}".into()
        }
    });
    let hidden = format!("BT /F2 12 Tf 72 600 Td {} Tj ET\n", fx.show("xy"));
    v.push(case(
        "decomposed letter and ligature in a font map",
        fx.hidden(hidden.as_bytes()).build(),
        text_spec("\u{e5}fi"),
        Leak(&["decode"]),
    ));

    // I5: rich text and XML.
    v.push(case(
        "rich text with markup inside the phrase",
        freetext(r#"<?xml version="1.0"?><body xmlns="http://www.w3.org/1999/xhtml"><p>Ola <span style="font-weight:bold">Nordmann</span></p></body>"#).build(),
        spec.clone(),
        Leak(&["structure"]),
    ));
    let mut fx = Fx::new(BENIGN);
    fx.catalog_extra.push_str(" /Metadata 15 0 R");
    fx.extra
        .insert(15, stream_obj("/Type /Metadata /Subtype /XML", XMP, true));
    v.push(case(
        "XMP with an entity",
        fx.build(),
        r#"{"metadata":["Smith & Sons"]}"#.into(),
        Leak(&["structure"]),
    ));
    let mut fx = Fx::new(BENIGN);
    fx.catalog_extra
        .push_str(" /Dests << /Bl#E5b#E6r [3 0 R /Fit] >>");
    v.push(case(
        "a name that is not UTF-8",
        fx.build(),
        text_spec("Bl\u{e5}b\u{e6}r"),
        Leak(&["structure"]),
    ));

    // I1: images that are only partly covered.
    let orig = [200u8, 10, 10, 201, 11, 11, 0, 0, 255, 0, 0, 254];
    let bad = [200u8, 10, 10, 201, 11, 11, 0, 0, 0, 0, 0, 0];
    let top_row = r#"{"regions":[{"page":1,"rect":[72,450,172,500]}]}"#;
    v.push(case(
        "the wrong rows of an image were blanked",
        with_image(Fx::image(2, 2, 3, &bad)).build(),
        top_row.into(),
        Leak(&["images"]),
    ));
    let good = [255u8, 255, 255, 255, 255, 255, 0, 0, 255, 0, 0, 254];
    v.push(case(
        "the covered row of an image is blank",
        with_image(Fx::image(2, 2, 3, &good)).build(),
        top_row.into(),
        Clean,
    ));
    let h = sha(&orig);
    v.push(case(
        "an image that was changed is not the original",
        with_image(Fx::image(2, 2, 3, &bad)).build(),
        format!(r#"{{"image_sha256":["{h}"]}}"#),
        Clean,
    ));
    v.push(case(
        "the original image",
        with_image(Fx::image(2, 2, 3, &orig)).build(),
        format!(r#"{{"image_sha256":["{h}"]}}"#),
        Leak(&["images"]),
    ));
    // 4 x 4 gray: the left two columns are blank, the right two are not.
    let mut half = Vec::new();
    for row in 0..4u8 {
        half.extend([255, 255, 10 + row * 30, 90 + row * 30]);
    }
    v.push(case(
        "only the covered pixels of a partly covered image count",
        with_image(Fx::image(4, 4, 1, &half)).build(),
        r#"{"regions":[{"page":1,"rect":[72,400,122,500]}]}"#.into(),
        Clean,
    ));
    v.push(case(
        "pixels outside the blanked part are still there",
        with_image(Fx::image(4, 4, 1, &half)).build(),
        r#"{"regions":[{"page":1,"rect":[72,400,172,500]}]}"#.into(),
        Leak(&["images"]),
    ));
    let mut fx = Fx::new(BENIGN);
    fx.page_extra.push_str(" /Thumb 17 0 R");
    fx.extra.insert(
        17,
        stream_obj(
            "/Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8",
            &orig,
            true,
        ),
    );
    v.push(case(
        "a thumbnail without /Subtype",
        fx.build(),
        format!(r#"{{"image_sha256":["{h}"]}}"#),
        Leak(&["images"]),
    ));

    // I2: filters, indirect filters and encryption.
    let plain = plain_with_needle();
    let a85 = ascii85(&flate(&plain));
    let mut fx = Fx::new(BENIGN);
    fx.extra.insert(
        18,
        raw_stream("/Filter [/ASCII85Decode /FlateDecode]", &a85),
    );
    v.push(case(
        "ASCII85 and Flate, in an object that nothing refers to",
        fx.build(),
        scrubbed_spec(NEEDLE),
        Leak(&["carve", "decode"]),
    ));
    let mut fx = Fx::new(BENIGN);
    fx.extra.insert(
        18,
        raw_stream("/Filter /RunLengthDecode", &run_length(&plain)),
    );
    v.push(case(
        "RunLength, in an object that nothing refers to",
        fx.build(),
        scrubbed_spec(NEEDLE),
        Leak(&["raw", "carve", "decode"]),
    ));
    let mut fx = Fx::new(BENIGN);
    fx.extra
        .insert(19, b"[/ASCII85Decode /FlateDecode]".to_vec());
    fx.extra.insert(18, raw_stream("/Filter 19 0 R", &a85));
    v.push(case(
        "an indirect /Filter, in an object that nothing refers to",
        fx.build(),
        scrubbed_spec(NEEDLE),
        Leak(&["carve", "decode"]),
    ));
    let mut fx = Fx::new(BENIGN);
    fx.extra.insert(13, packed("", &plain));
    let g1 = fx.build();
    v.push(case(
        "Flate, in an object that nothing refers to",
        g1.clone(),
        scrubbed_spec(NEEDLE),
        Leak(&["carve", "decode"]),
    ));
    let dir = Dir::new();
    v.push(case(
        "an encrypted file with a stream that nothing refers to",
        encrypt_keep(&dir, &g1, ""),
        scrubbed_spec(NEEDLE),
        Leak(&["carve", "decode"]),
    ));
    v.push(case(
        "a stream that is cut at the word endstream",
        endstream_in_data(),
        spec,
        Leak(&["qdf"]),
    ));

    // Clean files, one near each fix.
    let near_codes = Fx::new(b"").show("Ola Normann");
    let near_ops = format!(
        "BT /F1 12 Tf 72 680 Td [(Ola)-333(Normann)] TJ ET BT /F2 12 Tf 72 640 Td {near_codes} Tj ET\n"
    );
    let near = Fx::new(&benign_plus(near_ops.as_bytes()));
    v.push(case(
        "near misses of the needle in kerned text and glyph codes",
        near.build(),
        text_spec(NEEDLE),
        Clean,
    ));
    v.push(case(
        "rich text without the phrase",
        freetext("<p>Ola</p><p>Nils</p>").build(),
        text_spec(NEEDLE),
        Clean,
    ));
    v.push(case(
        "a font that maps other characters, drawn with Helvetica",
        {
            let mut fx = Fx::new(BENIGN);
            fx.cid_text = "Quarterly report".into();
            fx.build()
        },
        text_spec("Zxqk Wyv"),
        Clean,
    ));
    v
}

fn run(c: &Case, args: &[&str]) -> Run {
    cli(&c.bytes, Some(&c.spec), args, &[])
}

fn set(layers: &[&str]) -> BTreeSet<String> {
    layers.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn each_leak_is_seen_by_exactly_the_expected_layers() {
    for c in cases() {
        let r = run(&c, &[]);
        match c.expect {
            Expect::Leak(layers) => {
                assert_eq!(r.code, 1, "{}: not reported as a leak\n{}", c.name, r.out);
                assert_eq!(
                    r.layers(),
                    set(layers),
                    "{}: wrong layers\n{}",
                    c.name,
                    r.out
                );
            }
            Expect::Clean => {
                assert_eq!(r.code, 0, "{}: not clean\n{}", c.name, r.out);
                assert!(r.out.contains("clean"), "{}: {}", c.name, r.out);
            }
        }
    }
}

#[test]
fn each_expected_layer_finds_the_leak_on_its_own() {
    for c in cases() {
        let Expect::Leak(layers) = c.expect else {
            continue;
        };
        for layer in layers {
            let r = run(&c, &["--only", layer]);
            assert_eq!(
                r.code, 1,
                "{}: layer {layer} alone does not find it\n{}",
                c.name, r.out
            );
            assert_eq!(r.layers(), set(&[layer]), "{}\n{}", c.name, r.out);
        }
    }
}

#[test]
fn with_the_expected_layers_off_nothing_is_found() {
    for c in cases() {
        let Expect::Leak(layers) = c.expect else {
            continue;
        };
        let r = run(&c, &["--skip", &layers.join(",")]);
        assert!(
            r.layers().is_empty(),
            "{}: found with {layers:?} off, so another layer sees it too\n{}",
            c.name,
            r.out
        );
    }
}

#[test]
fn clean_files_are_clean_in_each_layer() {
    for c in cases() {
        if !matches!(c.expect, Expect::Clean) {
            continue;
        }
        for layer in [
            "raw",
            "qdf",
            "carve",
            "decode",
            "text",
            "structure",
            "images",
        ] {
            let r = run(&c, &["--only", layer]);
            // A layer that has nothing to look for in this spec says so (exit 2); it does not
            // say "clean".
            assert!(
                r.code == 0 || (r.code == 2 && r.out.contains("nothing was checked")),
                "{} in {layer}:\n{}",
                c.name,
                r.out
            );
        }
    }
}

#[test]
fn the_report_names_the_tools_that_ran() {
    let c = &cases()[0];
    let r = run(c, &[]);
    assert!(r.out.contains("tools: qpdf"), "{}", r.out);
    assert!(r.out.contains("pdftotext"), "{}", r.out);
}

// ---------------------------------------------------------------------------------------------
// Text that cannot be read is "could not check", never "clean".

#[test]
fn a_font_that_cannot_be_decoded_is_reported_not_passed() {
    let show = cid_show();
    let mut fx = Fx::new(BENIGN).hidden(&show);
    fx.to_unicode = false;
    let r = cli(
        &fx.build(),
        Some(&text_spec(NEEDLE)),
        &["--only", "decode"],
        &[],
    );
    assert_eq!(r.code, 2, "{}", r.out);
    assert!(
        r.problems()
            .iter()
            .any(|p| p.contains("decode:") && p.contains("font /F2") && p.contains("ToUnicode")),
        "{}",
        r.out
    );
    // The same font drawn on the page: still not clean.
    let mut fx = Fx::new(&[BENIGN, &show].concat());
    fx.to_unicode = false;
    let r = cli(&fx.build(), Some(&text_spec(NEEDLE)), &[], &[]);
    assert_eq!(r.code, 2, "{}", r.out);
}

#[test]
fn a_font_that_is_never_used_does_not_stop_the_check() {
    let mut fx = Fx::new(BENIGN);
    fx.to_unicode = false;
    let r = cli(&fx.build(), Some(&text_spec(NEEDLE)), &[], &[]);
    assert_eq!(r.code, 0, "{}", r.out);
}

// ---------------------------------------------------------------------------------------------
// The font check: removed text that stays in a subset font.

#[test]
fn a_subset_font_keeps_the_characters_of_removed_text() {
    // The text is not drawn any more, but the font still maps its characters.
    let fx = Fx::new(BENIGN);
    let bytes = fx.build();
    let spec = text_spec(NEEDLE);
    let r = cli(&bytes, Some(&spec), &[], &[]);
    assert_eq!(r.code, 0, "no layer but the font check sees it:\n{}", r.out);
    assert!(!r.out.contains("fonts"), "{}", r.out);
    let r = cli(&bytes, Some(&spec), &["--fonts"], &[]);
    assert_eq!(r.code, 1, "{}", r.out);
    assert_eq!(r.layers(), set(&["fonts"]), "{}", r.out);
    assert!(r.out.contains("subset font object 6"), "{}", r.out);
    assert!(!r.out.contains(NEEDLE), "the text was printed\n{}", r.out);
    let r = cli(&bytes, Some(&spec), &["--only", "fonts"], &[]);
    assert_eq!(r.code, 1, "{}", r.out);
}

#[test]
fn a_font_that_does_not_map_the_removed_text_is_clean() {
    let mut fx = Fx::new(BENIGN);
    fx.cid_text = "Quarterly report".into();
    let r = cli(&fx.build(), Some(&text_spec("Zxqk Wyv")), &["--fonts"], &[]);
    assert_eq!(r.code, 0, "{}", r.out);
    // A font that is not a subset keeps all its glyphs by design: not a trace of anything removed.
    let mut fx = Fx::new(BENIGN);
    fx.subset = false;
    let r = cli(&fx.build(), Some(&text_spec(NEEDLE)), &["--fonts"], &[]);
    assert_eq!(r.code, 0, "{}", r.out);
}

#[test]
fn text_that_is_still_drawn_is_not_a_font_trace() {
    // The characters are drawn by the page itself: the font is in use.
    let fx = Fx::new(&[BENIGN, &cid_show()].concat());
    let r = cli(
        &fx.build(),
        Some(&text_spec(NEEDLE)),
        &["--only", "fonts"],
        &[],
    );
    assert_eq!(r.code, 0, "{}", r.out);
}
