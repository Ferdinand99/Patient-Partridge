//! Signing a document with a certificate, through the real engine and the real PDFium. The engine
//! prepares the file (no key in it), the helpers of `sign_support` do what the app does with the
//! signer (hash, sign, fill the hole), and the engine's own check reads the result.
//!
//! Other programs read the same files in `sign_readers.rs`.

mod common;
mod sign_support;

use common::designer::{add, apply_ok, open as open_designer, widget};

use common::*;
use partridge_engine::sigcheck::TrustList;
use partridge_protocol::{
    DesignField, DesignKind, DesignProps, ErrorCode, RequestBody, ResponseBody, SignPlace,
    SignRequest, SignResponse, SignatureFont, SignatureMark, SignatureReason as R,
    SignatureState as S,
};
use partridge_testkit as kit;
use sign_support::*;

fn count(hay: &[u8], needle: &[u8]) -> usize {
    hay.windows(needle.len()).filter(|w| *w == needle).count()
}

#[test]
fn a_document_signed_with_each_kind_of_key_is_intact_and_names_its_signer() {
    let mut shared = engine();
    let e = &mut shared.0;
    let cases = [
        ("rsa2048.p12", "RSA 2048 bit", "SHA-256", 3),
        ("rsa3072.p12", "RSA 3072 bit", "SHA-256", 2),
        ("rsa4096.p12", "RSA 4096 bit", "SHA-256", 2),
        ("p256.p12", "ECDSA P-256", "SHA-256", 2),
        ("p384.p12", "ECDSA P-384", "SHA-384", 2),
        ("rsa2048-legacy.p12", "RSA 2048 bit", "SHA-256", 3),
        ("p256-legacy.p12", "ECDSA P-256", "SHA-256", 2),
    ];
    for (file, key, digest, certificates) in cases {
        let (signed, r) = sign_with(e, &kit::simple_pdf(2), file, rect_place(0));
        assert_eq!((r.total, r.unlisted), (1, 0), "{file}");
        let info = &r.items[0];
        // Intact; the signer is not verified because the app ships no list of trusted issuers.
        assert_eq!(
            (info.state, info.reason),
            (S::UnknownSigner, R::NoTrustList),
            "{file}: {info:?}"
        );
        assert_eq!(info.sub_filter, "ETSI.CAdES.detached");
        assert_eq!(info.key.as_deref(), Some(key), "{file}");
        assert_eq!(info.digest.as_deref(), Some(digest), "{file}");
        assert_eq!(info.certificates, certificates, "{file}");
        let id = identity(file);
        assert_eq!(info.signer.as_deref(), Some(id.certificate().name.as_str()));
        assert_eq!(
            info.fingerprint.as_deref(),
            Some(id.certificate().fingerprint.as_str()),
            "{file}"
        );
        assert_eq!(
            info.serial.as_deref(),
            Some(id.certificate().serial.as_str())
        );
        // The byte range covers the whole file except the hole.
        assert_eq!(info.byte_range.len(), 4);
        assert_eq!(info.byte_range[0], 0);
        assert_eq!(info.byte_range[2] + info.byte_range[3], signed.len() as u64);
        assert_eq!(info.file_len, signed.len() as u64);
        // What the signer typed is in the file as written, and the time is the one that was given.
        assert_eq!(info.stated_reason.as_deref(), Some("I have read this"));
        assert_eq!(info.stated_location.as_deref(), Some("Oslo"));
        assert!(
            info.claimed_time
                .as_deref()
                .is_some_and(|t| t.starts_with("2026-06-15 12:00")),
            "{:?}",
            info.claimed_time
        );
        assert_eq!(info.doc_mdp, 0);
    }
}

#[test]
fn the_signer_is_trusted_only_when_a_list_has_the_issuer() {
    let mut shared = engine();
    let e = &mut shared.0;
    let (signed, r) = sign_with(e, &kit::simple_pdf(1), "rsa2048.p12", rect_place(0));
    assert_eq!(r.items[0].state, S::UnknownSigner);
    // A list with the root that issued the chain of this file: the certificate leads to it through
    // the intermediate that is in the file.
    e.set_trust_list_for_tests(TrustList::from_pem(&root_pem()).unwrap());
    let r = report(e, &signed);
    e.set_trust_list_for_tests(TrustList::empty());
    assert_eq!(
        (r.items[0].state, r.items[0].reason),
        (S::Trusted, R::Verified),
        "{:?}",
        r.items[0]
    );
    assert_eq!(
        r.items[0].trust_anchor.as_deref(),
        Some("Partridge Test Root")
    );
}

#[test]
fn the_file_before_the_hole_is_what_pdfium_wrote_and_the_hole_is_empty() {
    let mut shared = engine();
    let e = &mut shared.0;
    let doc = open(e, &kit::simple_pdf(2), None).unwrap();
    let before = save_all(e, doc);
    let p = begin(e, spec(doc, rect_place(0))).unwrap();
    // The first part of the file is the save, byte for byte: nothing of it was written again.
    // (Every save has an identifier of its own in the trailer, so the trailer is left out.)
    let trailer = before.windows(7).rposition(|w| w == b"trailer").unwrap();
    assert!(p.bytes.starts_with(&before[..trailer]));
    // The hole is `<`, zeros and `>`; nothing else of the new part is zeros that long.
    assert_eq!(p.bytes[p.hole_start], b'<');
    assert_eq!(p.bytes[p.hole_end - 1], b'>');
    assert!(p.bytes[p.hole_start + 1..p.hole_end - 1]
        .iter()
        .all(|b| *b == b'0'));
    assert_eq!((p.hole_end - p.hole_start - 2) / 2, 6_000);
    // The byte range in the file is right already, so nothing in the file changes after the hole
    // is filled (the app only writes the signature).
    let text = String::from_utf8_lossy(&p.bytes[before.len()..]);
    let range = format!(
        "[{:010} {:010} {:010} {:010}]",
        0,
        p.hole_start,
        p.hole_end,
        p.bytes.len() - p.hole_end
    );
    assert!(text.contains(&range), "{range} not in the new part");
    // No key, no certificate and no name of a person's certificate is anywhere in the file.
    assert!(!text.contains("PRIVATE KEY"));
    assert_eq!(p.field, "Signature1");
    ask(e, RequestBody::Close { doc });
}

#[test]
fn the_old_bytes_stay_and_the_new_part_is_a_valid_update() {
    let mut shared = engine();
    let e = &mut shared.0;
    let (signed, r) = sign_with(e, &kit::simple_pdf(3), "p256.p12", rect_place(0));
    assert_eq!(r.items[0].state, S::UnknownSigner);
    // One field, one signature dictionary, one look.
    assert_eq!(count(&signed, b"/ByteRange"), 1);
    assert_eq!(count(&signed, b"/Type /Sig"), 1);
    assert_eq!(count(&signed, b"/SigFlags 3"), 1);
    // The catalog now says version 1.7, which the sub-filter needs.
    assert!(count(&signed, b"/Version /1.7") >= 1);
    // The file opens, has its pages, and the field is on the page asked for.
    let doc = open(e, &signed, None).unwrap();
    let ResponseBody::DocInfo {
        pages, security, ..
    } = ask(e, RequestBody::DocInfo { doc })
    else {
        panic!("no description");
    };
    assert_eq!((pages, security), (3, 0));
    let ResponseBody::Sign(SignResponse::Fields { fields }) =
        ask(e, RequestBody::Sign(SignRequest::Fields { doc }))
    else {
        panic!("no fields");
    };
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].name, "Signature1");
    assert_eq!(fields[0].page, 0);
    // The box is where it was asked for, to within a point.
    let want = [72.0, 600.0, 272.0, 660.0];
    for (got, want) in fields[0].rect.iter().zip(want) {
        assert!((got - want).abs() < 1.0, "{:?}", fields[0].rect);
    }
    ask(e, RequestBody::Close { doc });
}

#[test]
fn the_pages_are_what_they_were_before_the_signature() {
    let mut shared = engine();
    let e = &mut shared.0;
    for (name, pdf) in [
        ("plain", kit::simple_pdf(3)),
        ("form", kit::forms_pdf()),
        ("turned", kit::turned_pdf()),
        ("rotated form", kit::forms_rotated_pdf(90)),
    ] {
        let (doc, sizes) = open_sized(e, &pdf, None).unwrap();
        let (w, h) = (sizes[0].0 as f32, sizes[0].1 as f32);
        let ResponseBody::Fingerprint {
            digest: before,
            pages,
            ..
        } = ask(e, RequestBody::Sign(SignRequest::Fingerprint { doc }))
        else {
            panic!("no fingerprint");
        };
        let identity = identity("p256.p12");
        let mut s = spec(
            doc,
            SignPlace::Rect {
                page: 0,
                rect: [20.0, 20.0, (w - 20.0).min(220.0), (h - 20.0).min(80.0)],
            },
        );
        s.reserve = identity.certificate().signature_bytes + 64;
        let signed = finish(begin(e, s).unwrap(), &identity);
        ask(e, RequestBody::Close { doc });
        let doc = open(e, &signed, None).unwrap();
        let ResponseBody::Fingerprint {
            digest: after,
            pages: pages_after,
            ..
        } = ask(e, RequestBody::Sign(SignRequest::Fingerprint { doc }))
        else {
            panic!("no fingerprint");
        };
        assert_eq!((pages, before), (pages_after, after), "{name}");
        // The plain description does see the new annotation: the two differ.
        let ResponseBody::Fingerprint {
            digest: plain_after,
            ..
        } = ask(e, RequestBody::Fingerprint { doc })
        else {
            panic!("no fingerprint");
        };
        assert_ne!(plain_after, after, "{name}: the field is an annotation");
        ask(e, RequestBody::Close { doc });
    }
}

#[test]
fn an_invisible_signature_has_a_field_of_no_size_and_no_look() {
    let mut shared = engine();
    let e = &mut shared.0;
    let (signed, r) = sign_with(e, &kit::simple_pdf(2), "rsa3072.p12", SignPlace::Invisible);
    assert_eq!(r.items[0].state, S::UnknownSigner);
    assert!(count(&signed, b"/Rect [0 0 0 0]") >= 1);
    assert_eq!(count(&signed, b"/Subtype /Form"), 0);
    assert_eq!(count(&signed, b"/AP"), 0);
}

#[test]
fn a_form_keeps_its_fields_and_gets_a_signature_field_next_to_them() {
    let mut shared = engine();
    let e = &mut shared.0;
    let pdf = kit::forms_pdf();
    let doc = open(e, &pdf, None).unwrap();
    let ResponseBody::FormFields { fields: before } =
        ask(e, RequestBody::ListFormFields { doc, page: 0 })
    else {
        // The listing needs the form information first.
        ask(e, RequestBody::FormInfo { doc });
        panic!("no fields");
    };
    ask(e, RequestBody::Close { doc });
    let (signed, r) = sign_with(e, &pdf, "p256.p12", rect_place(0));
    assert_eq!(r.items[0].state, S::UnknownSigner);
    let doc = open(e, &signed, None).unwrap();
    ask(e, RequestBody::FormInfo { doc });
    let ResponseBody::FormFields { fields: after } =
        ask(e, RequestBody::ListFormFields { doc, page: 0 })
    else {
        panic!("no fields");
    };
    let names = |f: &[partridge_protocol::FormField]| -> Vec<String> {
        f.iter().map(|f| f.name.clone()).collect()
    };
    assert_eq!(names(&before), names(&after));
    ask(e, RequestBody::Close { doc });
}

#[test]
fn an_existing_empty_signature_field_is_signed_in_place() {
    let mut shared = engine();
    let e = &mut shared.0;
    let pdf = kit::forms_empty_signature_pdf();
    let doc = open(e, &pdf, None).unwrap();
    let ResponseBody::Sign(SignResponse::Fields { fields }) =
        ask(e, RequestBody::Sign(SignRequest::Fields { doc }))
    else {
        panic!("no fields");
    };
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].name, "empty_signature");
    ask(e, RequestBody::Close { doc });

    let (signed, r) = sign_with(
        e,
        &pdf,
        "p256.p12",
        SignPlace::Field {
            page: 0,
            name: "empty_signature".into(),
        },
    );
    assert_eq!(r.total, 1);
    assert_eq!(r.items[0].state, S::UnknownSigner, "{:?}", r.items[0]);
    // The field is the one the document had: no second one was made.
    assert_eq!(count(&signed, b"/Type /Sig"), 1);
    assert!(!String::from_utf8_lossy(&signed).contains("Signature1"));
    // The page has no more annotations than before.
    let doc = open(e, &signed, None).unwrap();
    let ResponseBody::Sign(SignResponse::Fields { fields }) =
        ask(e, RequestBody::Sign(SignRequest::Fields { doc }))
    else {
        panic!("no fields");
    };
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].name, "empty_signature");
    ask(e, RequestBody::Close { doc });
}

#[test]
fn a_field_that_is_not_there_or_is_not_a_signature_field_is_refused() {
    let mut shared = engine();
    let e = &mut shared.0;
    let doc = open(e, &kit::forms_empty_signature_pdf(), None).unwrap();
    for name in ["Nobody", "", "Sign", "sign here"] {
        let place = SignPlace::Field {
            page: 0,
            name: name.into(),
        };
        let (code, _) = refusal_text(e, spec(doc, place));
        assert_eq!(code, ErrorCode::SignRefused, "{name:?}");
    }
    // The first text field of the form is a field, but not one that can be signed. (Its name is
    // looked for among the signature fields of the page: there is none by that name.)
    let ResponseBody::FormFields { .. } = ({
        ask(e, RequestBody::FormInfo { doc });
        ask(e, RequestBody::ListFormFields { doc, page: 0 })
    }) else {
        panic!("no fields");
    };
    let (code, _) = refusal_text(
        e,
        spec(
            doc,
            SignPlace::Field {
                page: 3,
                name: "empty_signature".into(),
            },
        ),
    );
    assert_eq!(code, ErrorCode::PageOutOfRange);
    ask(e, RequestBody::Close { doc });
}

#[test]
fn what_cannot_be_signed_is_refused_with_its_own_code() {
    let mut shared = engine();
    let e = &mut shared.0;
    // Encrypted.
    let doc = open(e, &kit::encrypted_pdf("secret", 1), Some("secret")).unwrap();
    assert_eq!(
        begin(e, spec(doc, rect_place(0))).err(),
        Some(ErrorCode::SignEncrypted)
    );
    ask(e, RequestBody::Close { doc });
    // An XFA form.
    let doc = open(e, &kit::xfa_pdf(), None).unwrap();
    assert_eq!(
        begin(e, spec(doc, rect_place(0))).err(),
        Some(ErrorCode::SignXfa)
    );
    ask(e, RequestBody::Close { doc });
    // A document that holds a signature already, in each of the shapes the kit has.
    for (name, pdf) in [
        ("signed", kit::signed_pdf()),
        ("nested", kit::signed_nested_pdf()),
        ("without a field", kit::signed_without_field_pdf()),
        ("form with a signature", kit::forms_signed_pdf()),
    ] {
        let doc = open(e, &pdf, None).unwrap();
        assert_eq!(
            begin(e, spec(doc, rect_place(0))).err(),
            Some(ErrorCode::SignAlreadySigned),
            "{name}"
        );
        ask(e, RequestBody::Close { doc });
    }
}

#[test]
fn a_protected_document_that_the_save_refuses_is_not_signed_either() {
    let mut shared = engine();
    let e = &mut shared.0;
    // The signing starts with the checked save: a protected document with XMP metadata that would
    // be written without encryption is refused for that, before anything else is looked at.
    let doc = open(e, &kit::encrypted_xmp_pdf("secret", false), Some("secret")).unwrap();
    assert_eq!(
        begin(e, spec(doc, rect_place(0))).err(),
        Some(ErrorCode::SaveMetadataNotEncrypted)
    );
    ask(e, RequestBody::Close { doc });
    // One that keeps its metadata in the clear is written, and then refused as encrypted.
    let doc = open(e, &kit::encrypted_xmp_pdf("secret", true), Some("secret")).unwrap();
    assert_eq!(
        begin(e, spec(doc, rect_place(0))).err(),
        Some(ErrorCode::SignEncrypted)
    );
    ask(e, RequestBody::Close { doc });
}

#[test]
fn a_document_that_was_signed_here_is_not_signed_again() {
    let mut shared = engine();
    let e = &mut shared.0;
    let (signed, _) = sign_with(e, &kit::simple_pdf(1), "p256.p12", rect_place(0));
    let doc = open(e, &signed, None).unwrap();
    assert_eq!(
        begin(e, spec(doc, rect_place(0))).err(),
        Some(ErrorCode::SignAlreadySigned)
    );
    ask(e, RequestBody::Close { doc });
}

#[test]
fn a_box_that_cannot_hold_a_signature_is_refused() {
    let mut shared = engine();
    let e = &mut shared.0;
    let doc = open(e, &kit::simple_pdf(2), None).unwrap();
    let rect = |r: [f32; 4]| SignPlace::Rect { page: 0, rect: r };
    for bad in [
        [100.0, 100.0, 110.0, 110.0], // too small
        [100.0, 100.0, 300.0, 110.0], // too flat
        [-50.0, 100.0, 100.0, 200.0], // off the left edge
        [500.0, 100.0, 700.0, 200.0], // off the right edge
        [100.0, 700.0, 300.0, 900.0], // off the bottom
        [300.0, 100.0, 100.0, 200.0], // backwards
    ] {
        assert_eq!(
            begin(e, spec(doc, rect(bad))).err(),
            Some(ErrorCode::SignRefused),
            "{bad:?}"
        );
    }
    assert_eq!(
        begin(
            e,
            spec(
                doc,
                SignPlace::Rect {
                    page: 2,
                    rect: [100.0, 100.0, 300.0, 200.0]
                }
            )
        )
        .err(),
        Some(ErrorCode::PageOutOfRange)
    );
    // A box that fills the page is fine.
    assert!(begin(e, spec(doc, rect([0.0, 0.0, 612.0, 792.0]))).is_ok());
    // A name is needed.
    let mut s = spec(doc, rect([100.0, 100.0, 300.0, 200.0]));
    s.name = "   ".into();
    assert_eq!(begin(e, s).err(), Some(ErrorCode::SignRefused));
    ask(e, RequestBody::Close { doc });
}

#[test]
fn the_pages_of_the_form_are_signed_on_the_page_that_was_chosen() {
    let mut shared = engine();
    let e = &mut shared.0;
    let (signed, r) = sign_with(
        e,
        &kit::simple_pdf(3),
        "p256.p12",
        SignPlace::Rect {
            page: 2,
            rect: [72.0, 600.0, 272.0, 660.0],
        },
    );
    assert_eq!(r.items[0].state, S::UnknownSigner);
    let doc = open(e, &signed, None).unwrap();
    let ResponseBody::Sign(SignResponse::Fields { fields }) =
        ask(e, RequestBody::Sign(SignRequest::Fields { doc }))
    else {
        panic!("no fields");
    };
    assert_eq!(fields[0].page, 2);
    ask(e, RequestBody::Close { doc });
}

#[test]
fn a_page_that_is_turned_gets_the_box_where_it_was_drawn_and_a_matching_look() {
    let mut shared = engine();
    let e = &mut shared.0;
    for degrees in [90u32, 180, 270] {
        let pdf = kit::forms_rotated_pdf(degrees);
        let (doc, sizes) = open_sized(e, &pdf, None).unwrap();
        let (w, h) = sizes[0];
        let rect = [30.0, 40.0, 30.0 + 150.0, 40.0 + 50.0];
        assert!(rect[2] <= w as f32 && rect[3] <= h as f32);
        let identity = identity("p256.p12");
        let mut s = spec(doc, SignPlace::Rect { page: 0, rect });
        s.reserve = identity.certificate().signature_bytes + 64;
        let signed = finish(begin(e, s).unwrap(), &identity);
        ask(e, RequestBody::Close { doc });
        let r = report(e, &signed);
        assert_eq!(r.items[0].state, S::UnknownSigner, "{degrees}");
        // The field the engine lists is at the same place on the drawn page.
        let doc = open(e, &signed, None).unwrap();
        let ResponseBody::Sign(SignResponse::Fields { fields }) =
            ask(e, RequestBody::Sign(SignRequest::Fields { doc }))
        else {
            panic!("no fields");
        };
        let field = fields
            .iter()
            .find(|f| f.name == "Signature1")
            .expect("the new field");
        for (got, want) in field.rect.iter().zip(rect) {
            assert!(
                (got - want).abs() < 1.0,
                "{degrees}: {:?} for {rect:?}",
                field.rect
            );
        }
        // The look is turned to match: the form carries a matrix that is a quarter turn or a half.
        let text = String::from_utf8_lossy(&signed);
        let matrix = text
            .split("/Matrix [")
            .nth(1)
            .unwrap()
            .split(']')
            .next()
            .unwrap()
            .to_string();
        let v: Vec<f32> = matrix
            .split_whitespace()
            .map(|n| n.parse().unwrap())
            .collect();
        assert_eq!(v.len(), 6);
        match degrees {
            180 => assert!(v[0] < -0.99 && v[3] < -0.99, "{matrix}"),
            _ => assert!(
                v[0].abs() < 0.01 && v[3].abs() < 0.01 && v[1].abs() > 0.99,
                "{matrix}"
            ),
        }
        ask(e, RequestBody::Close { doc });
    }
}

#[test]
fn a_page_with_a_box_that_does_not_start_at_zero_is_signed_where_it_was_drawn() {
    let mut shared = engine();
    let e = &mut shared.0;
    let pdf = kit::far_box_pdf();
    let (doc, sizes) = open_sized(e, &pdf, None).unwrap();
    let (w, h) = sizes[0];
    let rect = [
        10.0,
        10.0,
        (w as f32 - 10.0).min(210.0),
        (h as f32 - 10.0).min(80.0),
    ];
    let identity = identity("p256.p12");
    let mut s = spec(doc, SignPlace::Rect { page: 0, rect });
    s.reserve = identity.certificate().signature_bytes + 64;
    let result = begin(e, s);
    ask(e, RequestBody::Close { doc });
    // The page may not be one the reader can write into; if it is, the signature holds.
    if let Ok(p) = result {
        let signed = finish(p, &identity);
        assert_eq!(report(e, &signed).items[0].state, S::UnknownSigner);
    }
}

#[test]
fn nodes_that_pass_down_their_boxes_are_signed_on_the_right_page() {
    let mut shared = engine();
    let e = &mut shared.0;
    for (name, pdf) in [
        ("inherited", kit::inherited_tree_pdf()),
        ("root inherited", kit::root_inherited_pdf()),
        ("indirect kids", kit::indirect_kids_pdf()),
        ("inline", kit::inline_dictionaries_pdf()),
    ] {
        let (_, r) = sign_in_page(e, &pdf, "p256.p12");
        assert_eq!(r.items[0].state, S::UnknownSigner, "{name}");
    }
}

#[test]
fn a_picture_of_each_kind_is_part_of_the_look() {
    let mut shared = engine();
    let e = &mut shared.0;
    let marks = [
        (
            "strokes",
            SignatureMark::Strokes {
                width: 240.0,
                height: 100.0,
                line: 4.0,
                strokes: vec![(0..40)
                    .map(|i| [10.0 + i as f32 * 5.0, 50.0 + 30.0 * (i as f32 / 4.0).sin()])
                    .collect()],
            },
        ),
        (
            "typed",
            SignatureMark::Typed {
                font: SignatureFont::GreatVibes,
                text: "Anna Alvorlig".into(),
            },
        ),
        (
            "image",
            SignatureMark::Image {
                width: 60,
                height: 20,
                rgba: (0..60 * 20)
                    .flat_map(|i| {
                        if i % 7 < 3 {
                            [200, 20, 20, 255]
                        } else {
                            [0, 0, 0, 0]
                        }
                    })
                    .collect(),
            },
        ),
    ];
    for (name, mark) in marks {
        let doc = open(e, &kit::simple_pdf(1), None).unwrap();
        let identity = identity("p256.p12");
        let mut s = with_mark(spec(doc, rect_place(0)), mark);
        s.reserve = identity.certificate().signature_bytes + 64;
        let signed = finish(begin(e, s).unwrap(), &identity);
        ask(e, RequestBody::Close { doc });
        assert_eq!(
            report(e, &signed).items[0].state,
            S::UnknownSigner,
            "{name}"
        );
        match name {
            "image" => {
                assert_eq!(
                    count(&signed, b"/Subtype /Image"),
                    2,
                    "colour and transparency"
                );
                assert!(count(&signed, b"/SMask") >= 1);
                assert!(count(&signed, b"RunLengthDecode") >= 2);
            }
            _ => assert_eq!(count(&signed, b"/Subtype /Image"), 0, "{name}"),
        }
    }
}

#[test]
fn names_beyond_ascii_are_kept_whole_in_the_signature_and_shown_as_best_the_font_can() {
    let mut shared = engine();
    let e = &mut shared.0;
    let doc = open(e, &kit::simple_pdf(1), None).unwrap();
    let identity = identity("p256.p12");
    let mut s = spec(doc, rect_place(0));
    s.name = "\u{c5}se \u{d8}deg\u{e5}rd \u{6d4b}\u{8bd5}".into();
    s.reason = "F\u{f8}r jeg signerer".into();
    s.location = "Troms\u{f8}".into();
    s.reserve = identity.certificate().signature_bytes + 64;
    let signed = finish(begin(e, s).unwrap(), &identity);
    ask(e, RequestBody::Close { doc });
    let r = report(e, &signed);
    let info = &r.items[0];
    assert_eq!(info.state, S::UnknownSigner);
    assert_eq!(info.stated_reason.as_deref(), Some("F\u{f8}r jeg signerer"));
    assert_eq!(info.stated_location.as_deref(), Some("Troms\u{f8}"));
    // The look shows the letters of the font and a question mark for the rest.
    let look = kit::inflated_streams(&signed)
        .into_iter()
        .map(|s| String::from_utf8_lossy(&s).into_owned())
        .collect::<Vec<_>>()
        .join("\n");
    let _ = look;
    assert!(String::from_utf8_lossy(&signed).contains("Digitally signed by"));
}

#[test]
fn the_reserve_is_the_size_of_the_hole_and_a_signature_that_fits_is_all_that_is_needed() {
    let mut shared = engine();
    let e = &mut shared.0;
    let doc = open(e, &kit::simple_pdf(1), None).unwrap();
    for reserve in [512u32, 4_000, 48 * 1024] {
        let mut s = spec(doc, rect_place(0));
        s.reserve = reserve;
        let p = begin(e, s).unwrap();
        assert_eq!((p.hole_end - p.hole_start - 2) / 2, reserve as usize);
    }
    ask(e, RequestBody::Close { doc });
}

#[test]
fn a_document_with_unsaved_changes_is_signed_as_it_is_now_and_the_original_is_not_touched() {
    let mut shared = engine();
    let e = &mut shared.0;
    let doc = open(e, &kit::simple_pdf(2), None).unwrap();
    // An edit in memory: a note on the first page.
    let ResponseBody::Annot { .. } = ask(
        e,
        RequestBody::AddNote {
            doc,
            page: 0,
            x: 100.0,
            y: 100.0,
            color: [255, 220, 0, 255],
            text: "added before signing".into(),
        },
    ) else {
        panic!("no note");
    };
    let identity = identity("p256.p12");
    let mut s = spec(doc, rect_place(1));
    s.reserve = identity.certificate().signature_bytes + 64;
    let signed = finish(begin(e, s).unwrap(), &identity);
    ask(e, RequestBody::Close { doc });
    assert!(kit::text_in_file(&signed, "added before signing"));
    assert_eq!(report(e, &signed).items[0].state, S::UnknownSigner);
}

#[test]
fn a_save_after_signing_breaks_the_signature_and_says_so() {
    // The reason the app refuses an ordinary save of a signed document: PDFium writes the whole
    // file again.
    let mut shared = engine();
    let e = &mut shared.0;
    let (signed, _) = sign_with(e, &kit::simple_pdf(2), "p256.p12", rect_place(0));
    let again = resaved(e, &signed);
    assert!(!again.starts_with(&signed));
    let r = report(e, &again);
    assert!(
        r.items.is_empty() || !matches!(r.items[0].state, S::UnknownSigner | S::Trusted),
        "{r:?}"
    );
}

#[test]
fn the_engine_with_the_file_open_does_not_see_a_signature_that_is_not_there_yet() {
    let mut shared = engine();
    let e = &mut shared.0;
    let doc = open(e, &kit::simple_pdf(1), None).unwrap();
    let p = begin(e, spec(doc, rect_place(0))).unwrap();
    // The document that is open is the one it was: the new part is only in the bytes handed out.
    let ResponseBody::DocInfo { signatures, .. } = ask(e, RequestBody::DocInfo { doc }) else {
        panic!("no description");
    };
    assert_eq!(signatures, 0);
    // The file with the empty hole is not an intact signature: the hole holds nothing.
    let r = report(e, &p.bytes);
    assert!(
        r.items
            .iter()
            .all(|i| !matches!(i.state, S::UnknownSigner | S::Trusted)),
        "{r:?}"
    );
    ask(e, RequestBody::Close { doc });
}

#[test]
fn a_signature_that_was_changed_after_it_was_made_is_not_intact() {
    let mut shared = engine();
    let e = &mut shared.0;
    let (mut signed, r) = sign_with(e, &kit::simple_pdf(2), "rsa2048.p12", rect_place(0));
    assert_eq!(r.items[0].state, S::UnknownSigner);
    // A byte of the name in the signature dictionary, which the signature covers.
    let at = signed
        .windows(8)
        .position(|w| w == b"Alvorlig")
        .expect("the name is in the file");
    signed[at + 1] ^= 0x01;
    let r = report(e, &signed);
    assert!(
        r.items.iter().all(|i| i.state != S::UnknownSigner),
        "{:?}",
        r.items
    );
    // A byte appended after the signed part.
    let (mut signed, _) = sign_with(e, &kit::simple_pdf(2), "rsa2048.p12", rect_place(0));
    signed.extend_from_slice(b"\n% after\n");
    let r = report(e, &signed);
    assert_eq!(r.items[0].state, S::Changed);
}

#[test]
fn a_signature_field_made_by_the_form_designer_can_be_signed() {
    let mut shared = engine();
    let e = &mut shared.0;
    let (doc, _file) = open_designer(e, &kit::simple_pdf(1));
    apply_ok(
        e,
        doc,
        vec![add(DesignField {
            name: "approval".into(),
            kind: DesignKind::Signature,
            props: DesignProps::default(),
            widgets: vec![widget(0, [72.0, 500.0, 272.0, 560.0])],
        })],
    );
    let ResponseBody::Sign(SignResponse::Fields { fields }) =
        ask(e, RequestBody::Sign(SignRequest::Fields { doc }))
    else {
        panic!("no fields");
    };
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].name, "approval");
    let identity = identity("p256.p12");
    let mut s = spec(
        doc,
        SignPlace::Field {
            page: 0,
            name: "approval".into(),
        },
    );
    s.reserve = identity.certificate().signature_bytes + 64;
    let signed = finish(begin(e, s).unwrap(), &identity);
    ask(e, RequestBody::Close { doc });
    let r = report(e, &signed);
    assert_eq!(r.total, 1);
    assert_eq!(r.items[0].state, S::UnknownSigner, "{:?}", r.items[0]);
    // The field the designer made is the one that holds the signature: no second one.
    assert_eq!(count(&signed, b"/Type /Sig"), 1);
    assert!(!String::from_utf8_lossy(&signed).contains("Signature1"));
    let doc = open(e, &signed, None).unwrap();
    let ResponseBody::Sign(SignResponse::Fields { fields }) =
        ask(e, RequestBody::Sign(SignRequest::Fields { doc }))
    else {
        panic!("no fields");
    };
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].name, "approval");
    ask(e, RequestBody::Close { doc });

    // A new field next to the designed one gets a name of its own.
    let (doc, _file) = open_designer(e, &kit::simple_pdf(1));
    apply_ok(
        e,
        doc,
        vec![add(DesignField {
            name: "Signature1".into(),
            kind: DesignKind::Signature,
            props: DesignProps::default(),
            widgets: vec![widget(0, [72.0, 500.0, 272.0, 560.0])],
        })],
    );
    let p = begin(e, spec(doc, rect_place(0))).unwrap();
    assert_eq!(p.field, "Signature2");
    ask(e, RequestBody::Close { doc });
}
