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

use common::hard_cases::*;
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

    // Text as glyph codes, in every place that a viewer does not show.
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

    // Word spaces written as kerning, and words in text objects of their own.
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
        Leak(&["decode", "text"]),
    ));

    // Words that are drawn in another order than they are read, where nothing draws them.
    let words = |first: &str, second: &str| {
        let fx = Fx::new(b"");
        format!(
            "BT /F2 12 Tf 110 600 Td {} Tj ET
BT /F2 12 Tf 72 600 Td {} Tj ET
",
            fx.show(first),
            fx.show(second)
        )
        .into_bytes()
    };
    let out_of_order = words("Nordmann", "Ola");
    v.push(case(
        "CID words drawn out of reading order, hidden",
        Fx::new(BENIGN).hidden(&out_of_order).build(),
        spec.clone(),
        Leak(&["decode"]),
    ));
    v.push(case(
        "CID words drawn out of reading order, drawn",
        Fx::new(&benign_plus(&out_of_order)).build(),
        spec.clone(),
        Leak(&["decode", "text"]),
    ));
    v.push(case(
        "words drawn out of reading order, hidden",
        Fx::new(BENIGN)
            .hidden(
                b"BT /F1 12 Tf 110 600 Td (Nordmann) Tj ET BT /F1 12 Tf 72 600 Td (Ola) Tj ET
",
            )
            .build(),
        spec.clone(),
        Leak(&["decode"]),
    ));
    // The surname is on the line above and drawn first: a reader reads the line above first.
    v.push(case(
        "words on two lines, drawn from the bottom up, hidden",
        Fx::new(BENIGN)
            .hidden(
                b"BT /F1 12 Tf 72 580 Td (Nordmann) Tj ET BT /F1 12 Tf 72 600 Td (Ola) Tj ET
",
            )
            .build(),
        spec.clone(),
        Leak(&["decode"]),
    ));
    // The surname is shown first and a TJ offset takes the text position back over it, so that the
    // first name is drawn before it on the page; and the same words in text that is turned.
    let kerned_back = b"BT /F1 12 Tf 110 600 Td [(Nordmann) 7834 (Ola)] TJ ET
";
    v.push(case(
        "a run that kerns back over the word before it, hidden",
        Fx::new(BENIGN).hidden(kerned_back).build(),
        spec.clone(),
        Leak(&["decode"]),
    ));
    v.push(case(
        "a run that kerns back over the word before it, drawn",
        Fx::new(&benign_plus(kerned_back)).build(),
        spec.clone(),
        Leak(&["decode", "text"]),
    ));
    let turned = b"BT /F1 12 Tf 0 1 -1 0 300 422 Tm (Nordmann) Tj 0 1 -1 0 300 400 Tm (Ola) Tj ET
";
    v.push(case(
        "words turned 90 degrees, drawn out of reading order, hidden",
        Fx::new(BENIGN).hidden(turned).build(),
        spec.clone(),
        Leak(&["decode"]),
    ));
    v.push(case(
        "words turned 90 degrees, drawn out of reading order, drawn",
        Fx::new(&benign_plus(turned)).build(),
        spec.clone(),
        Leak(&["decode", "text"]),
    ));
    // Another first name (made of letters of the font): not the phrase in either order.
    v.push(case(
        "words that are not the phrase in either order",
        Fx::new(BENIGN).hidden(&words("Nordmann", "Nora")).build(),
        spec.clone(),
        Clean,
    ));

    // A word that is hyphenated at the end of a line.
    let hyphenated = b"BT /F1 12 Tf 14 TL 72 600 Td (Ola Nord-) Tj T* (mann) Tj ET
";
    v.push(case(
        "a word hyphenated over a line break, hidden",
        Fx::new(BENIGN).hidden(hyphenated).build(),
        spec.clone(),
        Leak(&["decode"]),
    ));
    v.push(case(
        "a word hyphenated over a line break, drawn",
        Fx::new(&benign_plus(hyphenated)).build(),
        spec.clone(),
        Leak(&["decode", "text"]),
    ));

    // Single-byte encodings that are not Latin-1.
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

    // Rich text and XML.
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

    // Images that are only partly covered.
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

    // Filters, indirect filters and encryption.
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
        Leak(&["qdf", "carve", "decode"]),
    ));
    v.push(case(
        "a stream that is cut at the word endstream",
        endstream_in_data(),
        spec.clone(),
        Leak(&["qdf"]),
    ));

    // A string outside the streams, in a revision of an encrypted file that was replaced.
    let old_info = |title: &[u8]| {
        let plain = Doc {
            info: Some([b"/Title (".as_slice(), title, b") /Producer (x)"].concat()),
            ..Doc::bare()
        }
        .build();
        let encrypted = encrypt_plain_objects(&dir, &plain);
        append_empty_object(&encrypted, "Info")
    };
    v.push(case(
        "an encrypted file whose old revision has the phrase in its document information",
        old_info(NEEDLE.as_bytes()),
        spec.clone(),
        Leak(&["qdf"]),
    ));
    v.push(case(
        "an encrypted file whose old revision has another title",
        old_info(b"Ola Norman"),
        spec.clone(),
        Clean,
    ));

    // Glyph numbers that are small numbers, written as raw bytes in literal strings. A
    // content stream that is full of control bytes is content all the same.
    let low = fonts::codes(3, 1, NEEDLE);
    let low_font = || {
        let mut fx = Fx::new(BENIGN);
        fx.extra
            .insert(8, packed("", &fonts::cmap(&fonts::bfchar(&low))));
        fx
    };
    let raw_lines: Bytes = ["Nora dam", "an old man", NEEDLE, "Norma lad"]
        .iter()
        .enumerate()
        .flat_map(|(i, t)| {
            let mut l = format!("BT /F2 12 Tf 72 {} Td ", 600 - 14 * i).into_bytes();
            l.extend(fonts::raw_literal(&low, t));
            l.extend(b" Tj ET\n");
            l
        })
        .collect();
    assert!(
        raw_lines.iter().filter(|b| **b < 9).count() * 20 > raw_lines.len(),
        "the content is binary"
    );
    v.push(case(
        "CID codes as raw bytes in literal strings, hidden",
        low_font().hidden(&raw_lines).build(),
        spec.clone(),
        Leak(&["decode"]),
    ));
    let mut fx = low_font();
    fx.extra.insert(13, stream_obj("", &raw_lines, true));
    v.push(case(
        "CID codes as raw bytes in an object that nothing refers to",
        fx.build(),
        scrubbed_spec(NEEDLE),
        Leak(&["decode"]),
    ));
    v.push(case(
        "CID codes as raw bytes in literal strings, drawn",
        {
            let mut fx = Fx::new(&[BENIGN, &raw_lines].concat());
            fx.extra
                .insert(8, packed("", &fonts::cmap(&fonts::bfchar(&low))));
            fx.build()
        },
        spec.clone(),
        Leak(&["decode", "text"]),
    ));
    let octal = [
        b"BT /F2 12 Tf 72 600 Td ".as_slice(),
        &fonts::octal_literal(&low, NEEDLE),
        b" Tj ET\n",
    ]
    .concat();
    v.push(case(
        "CID codes as octal escapes, hidden",
        low_font().hidden(&octal).build(),
        spec.clone(),
        Leak(&["decode"]),
    ));
    let near_raw: Bytes = [
        b"BT /F2 12 Tf 72 600 Td ".as_slice(),
        &fonts::raw_literal(&low, "Ola Norman"),
        b" Tj ET\n",
    ]
    .concat();
    v.push(case(
        "CID codes as raw bytes that spell a near miss",
        low_font().hidden(&near_raw).build(),
        spec.clone(),
        Clean,
    ));
    // A stream of binary data that is no content: only the byte layers look at it.
    let junk: Bytes = (0..3000u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 11) as u8)
        .collect();
    let mut fx = Fx::new(BENIGN);
    fx.extra.insert(13, stream_obj("", &junk, false));
    v.push(case(
        "a stream of binary data that is neither image, font nor content",
        fx.build(),
        scrubbed_spec(NEEDLE),
        Clean,
    ));

    // A single code inside a range of a character map.
    let over = |c: char| -> u16 { 0x0100 + c as u16 };
    let over_table: Vec<(u16, char)> = fonts::codes(0, 1, NEEDLE)
        .into_iter()
        .map(|(_, c)| (over(c), c))
        .collect();
    let range = "1 beginbfrange\n<0100> <01FF> <0000>\nendbfrange\n";
    let single = "1 beginbfchar\n<0130> <0030>\nendbfchar\n";
    let over_font = |body: String| {
        let mut fx = Fx::new(BENIGN);
        fx.extra.insert(8, packed("", &fonts::cmap(&body)));
        fx
    };
    let over_ops = format!(
        "BT /F2 12 Tf 72 600 Td {} Tj ET\n",
        fonts::hex_codes(&over_table, NEEDLE)
    )
    .into_bytes();
    v.push(case(
        "a code inside a range of the character map, hidden",
        over_font(format!("{single}{range}"))
            .hidden(&over_ops)
            .build(),
        spec.clone(),
        Leak(&["decode"]),
    ));
    v.push(case(
        "a code inside a range of the character map, the range comes later",
        over_font(format!("{range}{single}"))
            .hidden(&over_ops)
            .build(),
        spec.clone(),
        Leak(&["decode"]),
    ));
    v.push(case(
        "a code inside a range of the character map, drawn",
        {
            let mut fx = over_font(format!("{single}{range}"));
            fx.content = [BENIGN, &over_ops].concat();
            fx.build()
        },
        spec.clone(),
        Leak(&["decode", "text"]),
    ));

    // Type 3 fonts, which name their glyphs.
    let proc = stream_obj("", b"500 0 d0 0 0 400 400 re f", false);
    let t3 = |names: &[String], tu: Option<Bytes>| {
        let mut fx = Fx::new(BENIGN);
        fx.extra
            .insert(6, fonts::type3_font(65, names, 20, tu.as_ref().map(|_| 8)));
        fx.extra.insert(20, proc.clone());
        if let Some(tu) = tu {
            fx.extra.insert(8, stream_obj("", &tu, true));
        }
        fx
    };
    let std_name = |_: usize, c: char| {
        if c == ' ' {
            "space".to_owned()
        } else {
            c.to_string()
        }
    };
    let (names, shown) = fonts::type3_for(65, NEEDLE, std_name);
    let t3_ops = [b"BT /F2 12 Tf 72 600 Td ".as_slice(), &shown, b" Tj ET\n"].concat();
    v.push(case(
        "Type 3 font with standard glyph names, hidden",
        t3(&names, None).hidden(&t3_ops).build(),
        spec.clone(),
        Leak(&["decode"]),
    ));
    let (gnames, gshown) = fonts::type3_for(65, NEEDLE, |i, _| format!("g{}", i + 3));
    let g_ops = [b"BT /F2 12 Tf 72 600 Td ".as_slice(), &gshown, b" Tj ET\n"].concat();
    let tu_table: Vec<(u16, char)> = fonts::codes(65, 1, NEEDLE);
    let tu = fonts::cmap_one_byte(&fonts::bfchar_width(&tu_table, 2));
    v.push(case(
        "Type 3 font with numbered glyph names and a character map, hidden",
        t3(&gnames, Some(tu)).hidden(&g_ops).build(),
        spec.clone(),
        Leak(&["decode"]),
    ));

    // Two resource names, each the name of two fonts of the file, in a stream that nothing
    // refers to. The phrase is in the first font of the first name and the second font of the
    // second name: a pair that reading all the first fonts, then all the second ones, never makes.
    let a_true = fonts::codes(0x0101, 7, "Ola ");
    let b_true = fonts::codes(0x0301, 5, "Nordmann");
    let scramble = |t: &[(u16, char)], by: &str| -> Vec<(u16, char)> {
        t.iter()
            .zip(by.chars().cycle())
            .map(|((code, _), c)| (*code, c))
            .collect()
    };
    let mut objs = Objs::new();
    objs.insert(1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    objs.insert(
        2,
        b"<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".to_vec(),
    );
    let page_of = |f1: u32, f2: u32, contents: u32| {
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {contents} 0 R \
             /Resources << /Font << /F1 {f1} 0 R /F2 {f2} 0 R >> >> >>"
        )
        .into_bytes()
    };
    objs.insert(3, page_of(10, 20, 4));
    objs.insert(4, stream_obj("", b"q Q", false));
    objs.insert(5, page_of(30, 40, 6));
    objs.insert(6, stream_obj("", b"q Q", false));
    for (base, name, table) in [
        (10, "AAAAAA+First", a_true.clone()),
        (20, "BBBBBB+Third", scramble(&b_true, "xyzwvutsrq")),
        (30, "CCCCCC+Second", scramble(&a_true, "wxyz")),
        (40, "DDDDDD+Fourth", b_true.clone()),
    ] {
        for (n, b) in fonts::cid_font(base, name, &table) {
            objs.insert(n, b);
        }
    }
    let pair = format!(
        "BT /F1 12 Tf {} Tj /F2 12 Tf {} Tj ET\n",
        fonts::hex_codes(&a_true, "Ola "),
        fonts::hex_codes(&b_true, "Nordmann")
    );
    objs.insert(50, stream_obj("", pair.as_bytes(), true));
    v.push(case(
        "a pair of fonts that only one combination of the candidates reads",
        file(&objs),
        scrubbed_spec(NEEDLE),
        Leak(&["decode"]),
    ));

    // The role of a stream comes before what its bytes look like.
    v.push(case(
        "page content that shows the word begincmap, hidden CID text",
        Fx::new(
            b"BT /F1 12 Tf 72 720 Td (How begincmap works) Tj ET
",
        )
        .hidden(&show)
        .build(),
        spec.clone(),
        Leak(&["decode"]),
    ));
    let acsp = [
        b"%".as_slice(),
        &[b'x'; 35],
        b"acsp
",
    ]
    .concat();
    assert_eq!(&acsp[36..40], b"acsp");
    v.push(case(
        "page content with acsp at byte 36, hidden CID text",
        Fx::new(&[acsp.as_slice(), BENIGN].concat())
            .hidden(&show)
            .build(),
        spec.clone(),
        Leak(&["decode"]),
    ));
    let mut fx = Fx::new(BENIGN).hidden(
        b"/Fm1 Do
",
    );
    fx.resources_extra.push_str(" /XObject << /Fm1 10 0 R >>");
    fx.extra.insert(
        10,
        stream_obj(
            "/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Length1 0              /Resources << /Font << /F1 5 0 R /F2 6 0 R >> >>",
            &show,
            true,
        ),
    );
    v.push(case(
        "a drawn form with a /Length1 key, hidden CID text",
        fx.build(),
        spec.clone(),
        Leak(&["decode"]),
    ));
    // A form that carries the dictionary of another kind of stream is still a form.
    let mut fx = Fx::new(BENIGN);
    fx.extra.insert(
        13,
        stream_obj(
            "/Type /Metadata /Subtype /Form /BBox [0 0 612 792]              /Resources << /Font << /F1 5 0 R /F2 6 0 R >> >>",
            &show,
            true,
        ),
    );
    v.push(case(
        "a form in an object that nothing refers to, with the type of metadata",
        fx.build(),
        scrubbed_spec(NEEDLE),
        Leak(&["decode"]),
    ));
    // A font program that holds the text is data: the byte layers still read it.
    let mut fx = Fx::new(BENIGN);
    fx.extra.insert(
        9,
        b"<< /Type /FontDescriptor /FontName /ABCDEF+DejaVuSans /Flags 32 /FontBBox [0 -200 1000 900]           /ItalicAngle 0 /Ascent 900 /Descent -200 /CapHeight 700 /StemV 80 /FontFile2 13 0 R >>"
            .to_vec(),
    );
    fx.extra.insert(
        13,
        stream_obj(
            "",
            &[b"BT (".as_slice(), NEEDLE.as_bytes(), b") Tj ET"].concat(),
            true,
        ),
    );
    v.push(case(
        "text in a font program, which is data and not content",
        fx.build(),
        spec.clone(),
        Leak(&["qdf", "carve"]),
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
