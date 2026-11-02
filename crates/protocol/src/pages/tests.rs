use super::*;
use crate::{ErrorCode, Request, RequestBody, Response, ResponseBody};

fn request(body: PagesRequest) -> Request {
    Request {
        id: 7,
        body: RequestBody::Pages(body),
    }
}

fn response(body: PagesResponse) -> Response {
    Response {
        id: 9,
        body: ResponseBody::Pages(body),
    }
}

fn requests() -> Vec<PagesRequest> {
    vec![
        PagesRequest::Delete {
            doc: 1,
            pages: vec![0, 5, 9],
        },
        PagesRequest::Reorder {
            doc: 1,
            order: vec![2, 0, 1],
        },
        PagesRequest::Rotate {
            doc: 1,
            pages: vec![3],
            turns: 3,
        },
        PagesRequest::Crop {
            doc: 1,
            pages: vec![0, 1],
            margins: [10.0, 20.5, 30.0, 40.0],
        },
        PagesRequest::SetBoxes {
            doc: 1,
            pages: vec![0, 1],
            boxes: vec![[0.0, 0.0, 100.0, 200.0], [1.0, 2.0, 3.0, 4.0]],
        },
        PagesRequest::InsertBlank {
            doc: 1,
            at: 4,
            width: 612.0,
            height: 792.0,
        },
        PagesRequest::Restore {
            doc: 1,
            snapshot: 12,
        },
        PagesRequest::ImportBegin {
            doc: 1,
            size: 123_456,
        },
        PagesRequest::ImportChunk {
            doc: 1,
            offset: 1024,
            data: (0..=255).collect(),
        },
        PagesRequest::ImportEnd { doc: 1, at: 0 },
        PagesRequest::ImportAbort { doc: 1 },
        PagesRequest::ExportBegin {
            doc: 1,
            pages: vec![4, 3, 2],
        },
    ]
}

fn responses() -> Vec<PagesResponse> {
    let size = PageSize {
        width: 612.0,
        height: 792.0,
    };
    vec![
        PagesResponse::Changed {
            pages: vec![size, size],
            snapshot: 3,
            count: 1,
            form_fields: 2,
        },
        PagesResponse::Changed {
            pages: vec![],
            snapshot: 0,
            count: 0,
            form_fields: 0,
        },
        PagesResponse::Cropped {
            pages: vec![size],
            before: vec![[0.0, 0.0, 612.0, 792.0]],
            after: vec![[10.0, 10.0, 600.0, 780.0]],
        },
        PagesResponse::ImportReady,
        PagesResponse::ImportAck { received: 99 },
        PagesResponse::Exported {
            size: 5000,
            pages: 3,
            signatures: 1,
            source_security: 4,
            digest: 0xDEAD_BEEF_0123_4567,
        },
        PagesResponse::Done,
    ]
}

#[test]
fn every_request_survives_the_wire() {
    for body in requests() {
        let sent = request(body);
        let again = Request::decode(&sent.encode()).unwrap();
        assert_eq!(again, sent);
    }
}

#[test]
fn every_answer_survives_the_wire() {
    for body in responses() {
        let sent = response(body);
        let again = Response::decode(sent.encode()).unwrap();
        assert_eq!(again, sent);
    }
}

#[test]
fn every_request_and_answer_tag_is_inside_the_range_the_dispatch_looks_at() {
    for body in requests() {
        let tag = request(body).encode()[0];
        assert!(
            (FIRST_REQUEST_TAG..=LAST_REQUEST_TAG).contains(&tag),
            "{tag}"
        );
    }
    for body in responses() {
        let tag = response(body).encode()[0];
        assert!(
            (FIRST_RESPONSE_TAG..=LAST_RESPONSE_TAG).contains(&tag),
            "{tag}"
        );
    }
    // The ends of the range are real messages, so a tag past them is unknown.
    for tag in [FIRST_REQUEST_TAG - 1, LAST_REQUEST_TAG + 1] {
        let payload = [tag, 1, 0, 0, 0];
        assert!(Request::decode(&payload).is_err(), "{tag}");
    }
    for tag in [FIRST_RESPONSE_TAG - 1, LAST_RESPONSE_TAG + 1] {
        let payload = vec![tag, 1, 0, 0, 0];
        assert!(Response::decode(payload).is_err(), "{tag}");
    }
}

/// A request frame by hand: tag, id, then the fields.
fn raw(tag: u8, fields: &[u8]) -> Vec<u8> {
    let mut out = vec![tag, 1, 0, 0, 0];
    out.extend_from_slice(fields);
    out
}

fn u32s(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

#[test]
fn a_list_of_pages_is_cut_at_the_cap_on_both_sides() {
    // Exactly at the cap it decodes ...
    let at_cap: Vec<u32> = (0..MAX_EDIT_PAGES as u32).collect();
    let sent = request(PagesRequest::Delete {
        doc: 1,
        pages: at_cap.clone(),
    });
    let RequestBody::Pages(PagesRequest::Delete { pages, .. }) =
        Request::decode(&sent.encode()).unwrap().body
    else {
        panic!("not a delete");
    };
    assert_eq!(pages, at_cap);

    // ... one more is refused by the decoder, however many bytes follow ...
    let mut fields = u32s(&[1, MAX_EDIT_PAGES as u32 + 1]);
    fields.extend(u32s(&vec![0; MAX_EDIT_PAGES + 1]));
    assert!(matches!(
        Request::decode(&raw(TAG_DELETE, &fields)),
        Err(ProtocolError::InvalidLength)
    ));

    // ... and the encoder never writes more than the cap, so what it writes always decodes.
    let too_many = vec![7u32; MAX_EDIT_PAGES + 50];
    let sent = request(PagesRequest::Delete {
        doc: 1,
        pages: too_many,
    });
    let RequestBody::Pages(PagesRequest::Delete { pages, .. }) =
        Request::decode(&sent.encode()).unwrap().body
    else {
        panic!("not a delete");
    };
    assert_eq!(pages.len(), MAX_EDIT_PAGES);
}

#[test]
fn a_list_that_claims_more_than_the_frame_holds_is_refused_before_anything_is_reserved() {
    // Four billion entries announced, none sent.
    let fields = u32s(&[1, u32::MAX]);
    assert!(matches!(
        Request::decode(&raw(TAG_DELETE, &fields)),
        Err(ProtocolError::InvalidLength)
    ));
    // A count inside the cap, but with the bytes missing.
    let fields = u32s(&[1, 100, 1, 2, 3]);
    assert!(matches!(
        Request::decode(&raw(TAG_DELETE, &fields)),
        Err(ProtocolError::Truncated)
    ));
    // Boxes: one page, no box.
    let fields = u32s(&[1, 1, 0]);
    assert!(matches!(
        Request::decode(&raw(TAG_SET_BOXES, &fields)),
        Err(ProtocolError::Truncated)
    ));
}

#[test]
fn numbers_that_are_not_finite_or_absurd_are_refused() {
    for bad in [
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        MAX_BOX_VALUE * 2.0,
    ] {
        let sent = request(PagesRequest::Crop {
            doc: 1,
            pages: vec![0],
            margins: [bad, 0.0, 0.0, 0.0],
        });
        assert!(Request::decode(&sent.encode()).is_err(), "{bad}");
        let sent = request(PagesRequest::InsertBlank {
            doc: 1,
            at: 0,
            width: 100.0,
            height: bad,
        });
        assert!(Request::decode(&sent.encode()).is_err(), "{bad}");
        let sent = request(PagesRequest::SetBoxes {
            doc: 1,
            pages: vec![0],
            boxes: vec![[0.0, 0.0, bad, 1.0]],
        });
        assert!(Request::decode(&sent.encode()).is_err(), "{bad}");
    }
    // The largest number allowed still decodes.
    let sent = request(PagesRequest::Crop {
        doc: 1,
        pages: vec![0],
        margins: [MAX_BOX_VALUE, -MAX_BOX_VALUE, 0.0, 0.0],
    });
    assert!(Request::decode(&sent.encode()).is_ok());
}

#[test]
fn a_piece_of_a_document_is_held_to_its_cap() {
    let at_cap = request(PagesRequest::ImportChunk {
        doc: 1,
        offset: 0,
        data: vec![1; MAX_SAVE_CHUNK],
    });
    assert!(Request::decode(&at_cap.encode()).is_ok());
    // A piece that says it is one byte longer than the cap is refused, whatever follows.
    let mut fields = u32s(&[1, 0, MAX_SAVE_CHUNK as u32 + 1]);
    fields.extend(vec![0u8; MAX_SAVE_CHUNK + 1]);
    assert!(matches!(
        Request::decode(&raw(TAG_IMPORT_CHUNK, &fields)),
        Err(ProtocolError::InvalidLength)
    ));
    // A piece that is shorter than it says.
    let mut fields = u32s(&[1, 0, 10]);
    fields.extend([0u8; 9]);
    assert!(matches!(
        Request::decode(&raw(TAG_IMPORT_CHUNK, &fields)),
        Err(ProtocolError::Truncated)
    ));
    // The encoder cuts a longer piece instead of writing one that cannot be read.
    let long = request(PagesRequest::ImportChunk {
        doc: 1,
        offset: 0,
        data: vec![1; MAX_SAVE_CHUNK + 10],
    });
    let RequestBody::Pages(PagesRequest::ImportChunk { data, .. }) =
        Request::decode(&long.encode()).unwrap().body
    else {
        panic!("not a piece");
    };
    assert_eq!(data.len(), MAX_SAVE_CHUNK);
}

#[test]
fn the_size_of_a_document_to_insert_is_held_to_the_save_limit() {
    let ok = request(PagesRequest::ImportBegin {
        doc: 1,
        size: MAX_IMPORT_BYTES as u32,
    });
    assert!(Request::decode(&ok.encode()).is_ok());
    let too_big = request(PagesRequest::ImportBegin {
        doc: 1,
        size: MAX_IMPORT_BYTES as u32 + 1,
    });
    assert!(Request::decode(&too_big.encode()).is_err());
}

#[test]
fn an_answer_with_too_many_pages_is_refused() {
    let size = PageSize {
        width: 1.0,
        height: 1.0,
    };
    // At the cap it decodes ...
    let at_cap = response(PagesResponse::Changed {
        pages: vec![size; MAX_PAGES],
        snapshot: 1,
        count: 1,
        form_fields: 0,
    });
    assert!(Response::decode(at_cap.encode()).is_ok());
    // ... one page more is refused ...
    let mut payload = vec![TAG_CHANGED, 9, 0, 0, 0];
    payload.extend(u32s(&[MAX_PAGES as u32 + 1]));
    payload.extend(vec![0u8; (MAX_PAGES + 1) * 8 + 8]);
    assert!(matches!(
        Response::decode(payload),
        Err(ProtocolError::InvalidLength)
    ));
    // ... and so is a count that the bytes cannot hold.
    let mut payload = vec![TAG_CHANGED, 9, 0, 0, 0];
    payload.extend(u32s(&[1000]));
    payload.extend(vec![0u8; 80]);
    assert!(matches!(
        Response::decode(payload),
        Err(ProtocolError::Truncated)
    ));
}

#[test]
fn opening_a_document_with_more_pages_than_the_cap_is_refused_on_the_wire() {
    let size = PageSize {
        width: 1.0,
        height: 1.0,
    };
    let at_cap = Response {
        id: 1,
        body: ResponseBody::Opened {
            doc: 1,
            pages: vec![size; MAX_PAGES],
        },
    };
    assert!(Response::decode(at_cap.encode()).is_ok());
    let mut payload = at_cap.encode();
    // Pretend one more page: the count is the u32 after the tag, the id and the document number.
    let count = MAX_PAGES as u32 + 1;
    payload[9..13].copy_from_slice(&count.to_le_bytes());
    payload.extend([0u8; 8]);
    assert!(matches!(
        Response::decode(payload),
        Err(ProtocolError::InvalidLength)
    ));
}

#[test]
fn an_exported_document_that_is_too_big_or_has_too_many_pages_is_refused() {
    let ok = response(PagesResponse::Exported {
        size: MAX_SAVE_BYTES as u32,
        pages: MAX_EDIT_PAGES as u32,
        signatures: 0,
        source_security: 0,
        digest: 1,
    });
    assert!(Response::decode(ok.encode()).is_ok());
    for (size, pages) in [
        (MAX_SAVE_BYTES as u32 + 1, 1),
        (1, MAX_EDIT_PAGES as u32 + 1),
    ] {
        let bad = response(PagesResponse::Exported {
            size,
            pages,
            signatures: 0,
            source_security: 0,
            digest: 1,
        });
        assert!(Response::decode(bad.encode()).is_err(), "{size} {pages}");
    }
}

#[test]
fn damaged_frames_never_panic() {
    for body in requests() {
        let bytes = request(body).encode();
        for cut in 0..bytes.len() {
            assert!(Request::decode(&bytes[..cut]).is_err() || cut == bytes.len());
        }
        let mut extra = bytes.clone();
        extra.push(0);
        assert!(Request::decode(&extra).is_err());
    }
    for body in responses() {
        let bytes = response(body).encode();
        for cut in 0..bytes.len() {
            assert!(Response::decode(bytes[..cut].to_vec()).is_err());
        }
        let mut extra = bytes.clone();
        extra.push(0);
        assert!(Response::decode(extra).is_err());
    }
}

#[test]
fn the_new_error_codes_have_names_and_round_trip() {
    for (code, name) in [
        (ErrorCode::TooManyPages, "too_many_pages"),
        (ErrorCode::SnapshotGone, "snapshot_gone"),
        (ErrorCode::ImportFailed, "import_failed"),
        (ErrorCode::PageChangeFailed, "page_change_failed"),
        (ErrorCode::Busy, "busy"),
    ] {
        assert_eq!(code.name(), name);
        assert_eq!(ErrorCode::from_u8(code as u8), Some(code));
    }
}
