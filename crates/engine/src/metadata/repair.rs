//! Mending a compressed XMP metadata stream that PDFium wrote without its filter.
//!
//! PDFium writes a stream with `/Type /Metadata` and `/Subtype /XML` through
//! `CPDF_FlateEncoder` with the flag for compressing turned off. For a stream that has a filter
//! the encoder then keeps the bytes as they are in the file (still encoded) and takes `/Filter`
//! out of the dictionary. So a document whose XMP stream is Flate compressed, as some writers make
//! it, has the stream written as compressed bytes with no filter, and every reader sees noise
//! where the metadata should be. The same happens to a stream in hexadecimal or base 85 text, with
//! or without Flate compression. The engine cannot change what PDFium writes, so the bytes are
//! mended after they are written, before they leave the engine.
//!
//! The mend is an incremental update, appended to the bytes: the damaged stream is written again
//! with the same data, the same `/Length` and its `/Filter` added to its dictionary, in a new
//! cross-reference section that points to it, with a trailer that names the old table as `/Prev`.
//! Nothing before the update changes, so no offset moves and the work is one pass over the file
//! plus the size of the streams that are written again. A stream is mended only when it is not XML
//! as it stands and its data decodes, within a limit, with one of a few chains of filters (Flate,
//! ASCIIHex, ASCII85, and the last two followed by Flate) to text that starts like XML. The filter
//! that is put back is the chain that did it. Anything else is left as it is.
//!
//! A protected file is never touched: its metadata is the business of [`super`]. A file that has
//! an earlier part (`/Prev`) is not a file PDFium wrote, and is left as it is as well.

use std::borrow::Cow;
use std::io::Read;

use super::{
    count_names, is_space, looks_like_xml, parse_dictionary, token, Table, Value, PROGRESS_STEP,
};

/// The most one stream may inflate to. An XMP packet is some kilobytes; this leaves room for a
/// large one and stops a stream that is made to swell.
pub const MAX_INFLATED: usize = 4 * 1024 * 1024;

/// The most all the streams of a file may inflate to together.
pub const MAX_INFLATED_TOTAL: usize = 32 * 1024 * 1024;

/// Why a file was not mended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// A stream inflates to more than [`MAX_INFLATED`], or all of them to more than
    /// [`MAX_INFLATED_TOTAL`]: the file is not written rather than written with damaged metadata.
    StreamTooBig,
    /// The mended file would be longer than the limit, or there is no memory for it.
    FileTooBig,
}

impl Refusal {
    pub fn message(self) -> &'static str {
        match self {
            Refusal::StreamTooBig => {
                "an XMP metadata stream of the document is too big to be written safely, so it was not written"
            }
            Refusal::FileTooBig => "the document is too big to be saved",
        }
    }
}

/// What is known about a stream that is to be written again.
struct Job {
    number: u32,
    generation: u16,
    /// The dictionary, from its `<<` to after its `>>`, as places in the file.
    dict: (usize, usize),
    /// The data, as a place in the file.
    data: (usize, usize),
    /// The filters the data has to be given back.
    chain: Chain,
}

/// The filters that a stream may have had before PDFium took them off: the ones that can be told
/// from other data by decoding, and so can be put back without a guess about the document. (LZW
/// and run length are not here: their data cannot be checked without keeping what they decode to,
/// and they are rare for a metadata stream.)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Chain {
    Flate,
    Hex,
    Base85,
    HexFlate,
    Base85Flate,
}

impl Chain {
    const ALL: [Chain; 5] = [
        Chain::Flate,
        Chain::Hex,
        Chain::Base85,
        Chain::HexFlate,
        Chain::Base85Flate,
    ];

    /// The entry for the dictionary, with a space after it.
    fn entry(self) -> &'static [u8] {
        match self {
            Chain::Flate => b"/Filter/FlateDecode ",
            Chain::Hex => b"/Filter/ASCIIHexDecode ",
            Chain::Base85 => b"/Filter/ASCII85Decode ",
            Chain::HexFlate => b"/Filter[/ASCIIHexDecode/FlateDecode] ",
            Chain::Base85Flate => b"/Filter[/ASCII85Decode/FlateDecode] ",
        }
    }
}

/// What the data of a stream is, once decoded.
enum Unpacked {
    /// XML, after this chain of filters; the number of bytes.
    Xml(Chain, usize),
    /// Not data of any of the chains, or not XML when decoded.
    Other,
    /// More than the room there is.
    TooBig,
}

/// What a decoder made of its input.
enum Decoded {
    Bytes(Vec<u8>),
    Bad,
    TooBig,
}

/// Decodes `data` with the filters of `chain` into no more than `cap` bytes.
fn decode(chain: Chain, data: &[u8], cap: usize) -> Decoded {
    // What Flate makes of text is not much bigger than the text, so the bytes between the two
    // filters of a chain are limited too.
    let packed_room = cap.saturating_add(cap / 16).saturating_add(1024);
    let text = match chain {
        Chain::Flate | Chain::HexFlate | Chain::Base85Flate => {
            let packed = match chain {
                Chain::HexFlate => match ascii_hex(data, packed_room) {
                    Decoded::Bytes(bytes) => Cow::Owned(bytes),
                    other => return other,
                },
                Chain::Base85Flate => match ascii_85(data, packed_room) {
                    Decoded::Bytes(bytes) => Cow::Owned(bytes),
                    other => return other,
                },
                _ => Cow::Borrowed(data),
            };
            let mut text = Vec::new();
            let mut decoder = flate2::read::ZlibDecoder::new(&packed[..]).take(cap as u64 + 1);
            if decoder.read_to_end(&mut text).is_err() {
                return Decoded::Bad;
            }
            text
        }
        Chain::Hex => return ascii_hex(data, cap),
        Chain::Base85 => return ascii_85(data, cap),
    };
    if text.len() > cap {
        Decoded::TooBig
    } else {
        Decoded::Bytes(text)
    }
}

/// `ASCIIHexDecode`: hex digits and white space, ending at `>` or at the end of the data. Anything
/// else is not this filter's data.
fn ascii_hex(data: &[u8], cap: usize) -> Decoded {
    let mut out = Vec::new();
    let mut high: Option<u8> = None;
    let mut rest = data.iter();
    for &b in rest.by_ref() {
        let digit = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            b'>' => break,
            b if is_space(b) => continue,
            _ => return Decoded::Bad,
        };
        match high.take() {
            None => high = Some(digit),
            Some(h) => {
                if out.len() >= cap {
                    return Decoded::TooBig;
                }
                out.push(h << 4 | digit);
            }
        }
    }
    if let Some(h) = high {
        if out.len() >= cap {
            return Decoded::TooBig;
        }
        out.push(h << 4);
    }
    // Nothing but white space after the end.
    if rest.all(|b| is_space(*b)) {
        Decoded::Bytes(out)
    } else {
        Decoded::Bad
    }
}

/// `ASCII85Decode`: groups of five characters from `!` to `u` (and `z` for four zeros), white
/// space between them, ending at `~>`. Anything else is not this filter's data.
fn ascii_85(data: &[u8], cap: usize) -> Decoded {
    let mut out = Vec::new();
    let mut group = [0u8; 5];
    let mut held = 0usize;
    let mut at = 0usize;
    let mut ended = false;
    while at < data.len() {
        let b = data[at];
        at += 1;
        match b {
            b'~' => {
                if data.get(at) != Some(&b'>') {
                    return Decoded::Bad;
                }
                at += 1;
                ended = true;
                break;
            }
            b'z' if held == 0 => {
                if out.len() + 4 > cap {
                    return Decoded::TooBig;
                }
                out.extend_from_slice(&[0; 4]);
            }
            b'!'..=b'u' => {
                group[held] = b - b'!';
                held += 1;
                if held == 5 {
                    let Some(value) = group
                        .iter()
                        .try_fold(0u32, |v, d| v.checked_mul(85)?.checked_add(u32::from(*d)))
                    else {
                        return Decoded::Bad;
                    };
                    if out.len() + 4 > cap {
                        return Decoded::TooBig;
                    }
                    out.extend_from_slice(&value.to_be_bytes());
                    held = 0;
                }
            }
            b if is_space(b) => {}
            _ => return Decoded::Bad,
        }
    }
    // The end mark is required, and nothing but white space comes after it.
    if !ended || !data[at..].iter().all(|b| is_space(*b)) {
        return Decoded::Bad;
    }
    // A last group of two to four characters stands for one to three bytes.
    match held {
        0 => {}
        1 => return Decoded::Bad,
        n => {
            for slot in &mut group[n..] {
                *slot = 84;
            }
            let Some(value) = group
                .iter()
                .try_fold(0u32, |v, d| v.checked_mul(85)?.checked_add(u32::from(*d)))
            else {
                return Decoded::Bad;
            };
            if out.len() + n - 1 > cap {
                return Decoded::TooBig;
            }
            out.extend_from_slice(&value.to_be_bytes()[..n - 1]);
        }
    }
    Decoded::Bytes(out)
}

/// Decodes `data` with each of the chains of filters in turn, into no more than `room` bytes (and
/// [`MAX_INFLATED`]) each, and gives the first that makes XML.
fn unpack(data: &[u8], room: usize) -> Unpacked {
    let cap = room.min(MAX_INFLATED);
    for chain in Chain::ALL {
        match decode(chain, data, cap) {
            Decoded::Bytes(text) if looks_like_xml(&text[..text.len().min(256)]) => {
                return Unpacked::Xml(chain, text.len());
            }
            Decoded::TooBig => return Unpacked::TooBig,
            _ => {}
        }
    }
    Unpacked::Other
}

/// Mends the XMP streams of the bytes PDFium wrote for a document, as described above, and gives
/// the bytes back: the same ones when there was nothing to mend. `limit` is the most the file may
/// hold. `progress` is told how far the work has come now and then.
pub fn mend(
    mut file: Vec<u8>,
    limit: usize,
    progress: &mut dyn FnMut(u32),
) -> Result<Vec<u8>, Refusal> {
    // Most files have no metadata stream, and are done here (the name is written as it is).
    if count_names(&file, progress).is_none_or(|typed| typed == 0) {
        return Ok(file);
    }
    let Some(table) = Table::read(&file) else {
        return Ok(file);
    };
    let Some(trailer) = parse_dictionary(&file, table.trailer) else {
        return Ok(file);
    };
    if trailer.get(b"Encrypt").is_some() || trailer.get(b"Prev").is_some() {
        return Ok(file);
    }

    let mut jobs: Vec<Job> = Vec::new();
    let mut room = MAX_INFLATED_TOTAL;
    let mut since_progress = 0usize;
    for (index, &(offset, number)) in table.sorted.iter().enumerate() {
        let size = table.length_of(index);
        // An object that cannot be read is not one that this mends.
        let Some(object) = table.read_at(&file, index) else {
            continue;
        };
        since_progress += size;
        if since_progress >= PROGRESS_STEP {
            since_progress = 0;
            progress(offset.min(u32::MAX as usize) as u32);
        }
        let (Some(dict), Some(data)) = (&object.dict, object.data) else {
            continue;
        };
        if !(dict.is_name(b"Type", b"Metadata")
            && dict.is_name(b"Subtype", b"XML")
            && dict.get(b"Filter").is_none())
        {
            continue;
        }
        let Some(Value::Int(length)) = dict.get(b"Length") else {
            continue;
        };
        let Some(end) = data.checked_add(*length as usize) else {
            continue;
        };
        if !stream_ends_at(&file[..offset + size], end) {
            continue;
        }
        let data_bytes = &file[data..end];
        if looks_like_xml(&data_bytes[..data_bytes.len().min(256)]) {
            continue;
        }
        let chain = match unpack(data_bytes, room) {
            Unpacked::Xml(chain, decoded) => {
                room -= decoded;
                chain
            }
            Unpacked::Other => continue,
            Unpacked::TooBig => return Err(Refusal::StreamTooBig),
        };
        let mut at = offset;
        let generation = token(&file, &mut at)
            .and_then(|_| token(&file, &mut at))
            .and_then(super::number::<u16>);
        let Some(generation) = generation else {
            continue;
        };
        jobs.push(Job {
            number,
            generation,
            dict: (offset + dict.start, offset + dict.end),
            data: (data, end),
            chain,
        });
    }
    if jobs.is_empty() {
        return Ok(file);
    }
    jobs.sort_by_key(|job| job.number);

    // The update is made whole before it is added, so that a refusal leaves nothing half done.
    let mut update: Vec<u8> = Vec::new();
    let start = file.len() + usize::from(!file.ends_with(b"\n") && !file.ends_with(b"\r")) * 2;
    let mut places = Vec::with_capacity(jobs.len());
    for job in &jobs {
        places.push(start + update.len());
        update.extend_from_slice(format!("{} {} obj\r\n", job.number, job.generation).as_bytes());
        // `/Filter` goes in first: the keys of a dictionary have no order.
        update.extend_from_slice(&file[job.dict.0..job.dict.0 + 2]);
        update.extend_from_slice(job.chain.entry());
        update.extend_from_slice(&file[job.dict.0 + 2..job.dict.1]);
        update.extend_from_slice(b"\r\nstream\r\n");
        update.extend_from_slice(&file[job.data.0..job.data.1]);
        update.extend_from_slice(b"\r\nendstream\r\nendobj\r\n");
    }
    let table_at = start + update.len();
    update.extend_from_slice(b"xref\r\n");
    for (job, place) in jobs.iter().zip(&places) {
        update.extend_from_slice(
            format!(
                "{} 1\r\n{place:010} {:05} n\r\n",
                job.number, job.generation
            )
            .as_bytes(),
        );
    }
    update.extend_from_slice(format!("trailer\r\n<</Prev {} ", table.end).as_bytes());
    update.extend_from_slice(&file[trailer.start + 2..trailer.end]);
    update.extend_from_slice(format!("\r\nstartxref\r\n{table_at}\r\n%%EOF\r\n").as_bytes());

    let total = start + update.len();
    if total > limit {
        return Err(Refusal::FileTooBig);
    }
    file.try_reserve(total - file.len())
        .map_err(|_| Refusal::FileTooBig)?;
    if start > file.len() {
        file.extend_from_slice(b"\r\n");
    }
    file.extend_from_slice(&update);
    Ok(file)
}

/// Whether the stream whose data ends at `end` is closed there: an end of line (or none) and
/// `endstream`, inside `object`, the bytes of the object up to the next one.
fn stream_ends_at(object: &[u8], end: usize) -> bool {
    let mut at = end;
    if object.get(at) == Some(&b'\r') {
        at += 1;
    }
    if object.get(at) == Some(&b'\n') {
        at += 1;
    }
    end <= object.len()
        && object
            .get(at..)
            .is_some_and(|rest| rest.starts_with(b"endstream"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const PACKET: &str = "<?xpacket begin=\"\"?><x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><title>Quokka</title></x:xmpmeta>";

    fn deflate(data: &[u8]) -> Vec<u8> {
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    /// A file in the form PDFium writes: the objects in the open, a classic table, a trailer.
    /// Each object is given as its text (a stream with its data).
    fn file(objects: &[Vec<u8>], trailer: &str) -> Vec<u8> {
        let mut out = b"%PDF-1.7\r\n%\xA1\xB3\xC5\xD7\r\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            out.extend_from_slice(b"\r\n");
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\r\n", i + 1).as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\r\nendobj\r\n");
        }
        let xref = out.len();
        out.extend_from_slice(
            format!("xref\r\n0 {}\r\n0000000000 65535 f\r\n", objects.len() + 1).as_bytes(),
        );
        for offset in offsets {
            out.extend_from_slice(format!("{offset:010} 00000 n\r\n").as_bytes());
        }
        out.extend_from_slice(
            format!("trailer\r\n<<{trailer}>>\r\nstartxref\r\n{xref}\r\n%%EOF\r\n").as_bytes(),
        );
        out
    }

    fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
        let mut out = format!("<<{dict}/Length {}>>stream\r\n", data.len()).into_bytes();
        out.extend_from_slice(data);
        out.extend_from_slice(b"\r\nendstream");
        out
    }

    const TRAILER: &str = "/Root 1 0 R/Size 4";

    /// Catalog, pages, and the XMP stream (object 3) with `data`.
    fn with_xmp(data: &[u8]) -> Vec<u8> {
        file(
            &[
                b"<</Metadata 3 0 R/Pages 2 0 R/Type/Catalog>>".to_vec(),
                b"<</Count 0/Kids[]/Type/Pages>>".to_vec(),
                stream("/Subtype/XML/Type/Metadata", data),
            ],
            TRAILER,
        )
    }

    fn mend_all(bytes: Vec<u8>) -> Result<Vec<u8>, Refusal> {
        mend(bytes, usize::MAX, &mut |_| {})
    }

    #[test]
    fn a_compressed_stream_without_its_filter_is_written_again_with_it() {
        let packed = deflate(PACKET.as_bytes());
        let damaged = with_xmp(&packed);
        let mended = mend_all(damaged.clone()).unwrap();
        assert!(mended.starts_with(&damaged));
        let update = &mended[damaged.len()..];
        let text = String::from_utf8_lossy(update);
        assert!(
            text.starts_with("3 0 obj\r\n<</Filter/FlateDecode /"),
            "{text}"
        );
        assert!(
            text.contains(&format!("/Prev {}", find_xref(&damaged))),
            "{text}"
        );
        assert!(update.windows(packed.len()).any(|w| w == &packed[..]));
        // The new table points at the new object, and the file ends the way a PDF ends.
        let table = Table::read(&mended).expect("the table of the update is read");
        assert_eq!(table.sorted.len(), 1);
        let (offset, number) = table.sorted[0];
        assert_eq!(number, 3);
        assert_eq!(offset, damaged.len());
        assert!(mended.ends_with(b"%%EOF\r\n"));
        // The entry is 20 bytes.
        assert!(
            text.contains(&format!("3 1\r\n{offset:010} 00000 n\r\n")),
            "{text}"
        );
    }

    fn hex(data: &[u8]) -> Vec<u8> {
        let mut text: String = data.iter().map(|b| format!("{b:02X}")).collect();
        text.push('>');
        text.into_bytes()
    }

    fn base85(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for chunk in data.chunks(4) {
            let mut block = [0u8; 4];
            block[..chunk.len()].copy_from_slice(chunk);
            let mut value = u32::from_be_bytes(block);
            if chunk.len() == 4 && value == 0 {
                out.push(b'z');
                continue;
            }
            let mut digits = [0u8; 5];
            for slot in digits.iter_mut().rev() {
                *slot = (value % 85) as u8 + b'!';
                value /= 85;
            }
            out.extend_from_slice(&digits[..chunk.len() + 1]);
            if out.len() % 60 < 5 {
                out.push(b'\n');
            }
        }
        out.extend_from_slice(b"~>");
        out
    }

    #[test]
    fn text_filters_are_put_back_as_they_were() {
        let flate = deflate(PACKET.as_bytes());
        let cases: [(Vec<u8>, &str); 4] = [
            (hex(PACKET.as_bytes()), "/Filter/ASCIIHexDecode "),
            (base85(PACKET.as_bytes()), "/Filter/ASCII85Decode "),
            (hex(&flate), "/Filter[/ASCIIHexDecode/FlateDecode] "),
            (base85(&flate), "/Filter[/ASCII85Decode/FlateDecode] "),
        ];
        for (data, entry) in cases {
            let damaged = with_xmp(&data);
            let mended = mend_all(damaged.clone()).unwrap();
            let update = String::from_utf8_lossy(&mended[damaged.len()..]).into_owned();
            assert!(
                update.starts_with(&format!("3 0 obj\r\n<<{entry}")),
                "{entry}: {update}"
            );
        }
    }

    #[test]
    fn the_decoders_agree_with_the_filters() {
        let samples: [&[u8]; 6] = [
            b"",
            b"a",
            b"ab",
            b"abc",
            b"abcd",
            b"<x>\0\0\0\0 and more text</x>",
        ];
        for sample in samples {
            let Decoded::Bytes(back) = ascii_hex(&hex(sample), usize::MAX) else {
                panic!("hex");
            };
            assert_eq!(back, sample);
            let Decoded::Bytes(back) = ascii_85(&base85(sample), usize::MAX) else {
                panic!("85");
            };
            assert_eq!(back, sample);
        }
        // Odd digits are padded, white space is skipped, the data after the end is not allowed.
        assert!(matches!(ascii_hex(b"3 C 3>", 9), Decoded::Bytes(b) if b == [0x3c, 0x30]));
        assert!(matches!(ascii_hex(b"3C>3C", 9), Decoded::Bad));
        assert!(matches!(ascii_hex(b"3G", 9), Decoded::Bad));
        assert!(matches!(ascii_hex(b"3C3C3C", 2), Decoded::TooBig));
        // Base 85: no end mark, a character out of range, a group that is too big, a single
        // character at the end, a `z` inside a group, and data after the end mark are all not it.
        for bad in [
            &b"87cURD]i"[..],
            b"87cUR\x7fD]~>",
            b"uuuuu~>",
            b"8~>",
            b"87z~>",
            b"87cURD]~>x",
        ] {
            assert!(matches!(ascii_85(bad, 99), Decoded::Bad), "{bad:?}");
        }
        assert!(matches!(ascii_85(b"zzz~>", 11), Decoded::TooBig));
        assert!(matches!(ascii_85(b"zzz~>", 12), Decoded::Bytes(b) if b.len() == 12));
    }

    #[test]
    fn plain_words_are_not_mistaken_for_a_filter() {
        // Valid hex digits, and valid base 85 characters, but not XML when decoded.
        for data in [
            &b"DEADBEEF>"[..],
            b"ABCD",
            b"hello world~>",
            b"87cURD]i,\"Ebo80~>",
        ] {
            let file = with_xmp(data);
            assert_eq!(mend_all(file.clone()).unwrap(), file, "{data:?}");
        }
    }

    fn find_xref(file: &[u8]) -> usize {
        let at = super::super::rfind(file, b"startxref").unwrap();
        let text = String::from_utf8_lossy(&file[at + 9..]);
        text.split_whitespace().next().unwrap().parse().unwrap()
    }

    #[test]
    fn a_second_mend_changes_nothing() {
        let once = mend_all(with_xmp(&deflate(PACKET.as_bytes()))).unwrap();
        assert_eq!(mend_all(once.clone()).unwrap(), once);
    }

    #[test]
    fn xml_that_is_not_compressed_is_left_alone() {
        for data in [PACKET.as_bytes(), b"  \r\n<x/>", b"\xEF\xBB\xBF<x/>"] {
            let file = with_xmp(data);
            assert_eq!(mend_all(file.clone()).unwrap(), file);
        }
    }

    #[test]
    fn data_that_is_neither_xml_nor_flate_is_left_alone() {
        for data in [&b"\x00\x01\x02garbage"[..], b"", b"789C", b"not xml at all"] {
            let file = with_xmp(data);
            assert_eq!(mend_all(file.clone()).unwrap(), file);
        }
        // Flate data that is not XML when inflated.
        let file = with_xmp(&deflate(b"plain words, no markup"));
        assert_eq!(mend_all(file.clone()).unwrap(), file);
        // Flate data that stops short.
        let packed = deflate(PACKET.as_bytes());
        let file = with_xmp(&packed[..packed.len() - 6]);
        assert_eq!(mend_all(file.clone()).unwrap(), file);
    }

    #[test]
    fn a_stream_that_has_its_filter_or_another_type_is_left_alone() {
        let packed = deflate(PACKET.as_bytes());
        for dict in [
            "/Filter/FlateDecode/Subtype/XML/Type/Metadata",
            "/Subtype/Form/Type/Metadata",
            "/Subtype/XML/Type/XObject",
            "/Subtype/XML",
        ] {
            let file = file(
                &[
                    b"<</Metadata 3 0 R/Pages 2 0 R/Type/Catalog>>".to_vec(),
                    b"<</Count 0/Kids[]/Type/Pages>>".to_vec(),
                    stream(dict, &packed),
                ],
                TRAILER,
            );
            assert_eq!(mend_all(file.clone()).unwrap(), file, "{dict}");
        }
    }

    #[test]
    fn a_protected_file_and_a_file_with_an_earlier_part_are_left_alone() {
        let packed = deflate(PACKET.as_bytes());
        for trailer in [
            "/Root 1 0 R/Encrypt 2 0 R/Size 4",
            "/Root 1 0 R/Prev 9/Size 4",
        ] {
            let file = file(
                &[
                    b"<</Metadata 3 0 R/Pages 2 0 R/Type/Catalog>>".to_vec(),
                    b"<</Filter/Standard/V 4>>".to_vec(),
                    stream("/Subtype/XML/Type/Metadata", &packed),
                ],
                trailer,
            );
            assert_eq!(mend_all(file.clone()).unwrap(), file, "{trailer}");
        }
    }

    #[test]
    fn a_file_without_a_table_or_without_metadata_is_left_alone() {
        for bytes in [
            Vec::new(),
            b"%PDF-1.7 /Type/Metadata".to_vec(),
            file(&[b"<</Type/Catalog>>".to_vec()], "/Root 1 0 R"),
        ] {
            assert_eq!(mend_all(bytes.clone()).unwrap(), bytes);
        }
    }

    #[test]
    fn a_length_that_does_not_fit_is_not_trusted() {
        let packed = deflate(PACKET.as_bytes());
        for length in [packed.len() + 40, packed.len() - 1, usize::MAX / 2] {
            let mut data =
                format!("<</Subtype/XML/Type/Metadata/Length {length}>>stream\r\n").into_bytes();
            data.extend_from_slice(&packed);
            data.extend_from_slice(b"\r\nendstream");
            let file = file(
                &[
                    b"<</Metadata 3 0 R/Pages 2 0 R/Type/Catalog>>".to_vec(),
                    b"<</Count 0/Kids[]/Type/Pages>>".to_vec(),
                    data,
                ],
                TRAILER,
            );
            assert_eq!(mend_all(file.clone()).unwrap(), file, "{length}");
        }
    }

    #[test]
    fn a_stream_that_swells_past_the_limit_refuses_the_file() {
        // Some megabytes of white space inside a comment: small when packed, big when inflated.
        let mut text = String::from("<x><!--");
        text.push_str(&" ".repeat(MAX_INFLATED + 1));
        text.push_str("--></x>");
        let packed = deflate(text.as_bytes());
        assert!(packed.len() < 100_000, "{}", packed.len());
        assert_eq!(mend_all(with_xmp(&packed)), Err(Refusal::StreamTooBig));
        // One byte less is within the limit.
        let mut text = String::from("<x><!--");
        text.push_str(&" ".repeat(MAX_INFLATED - 7 - 7));
        text.push_str("--></x>");
        assert_eq!(text.len(), MAX_INFLATED);
        assert!(mend_all(with_xmp(&deflate(text.as_bytes()))).is_ok());
    }

    #[test]
    fn many_streams_that_swell_together_refuse_the_file() {
        let mut text = String::from("<x><!--");
        text.push_str(&" ".repeat(MAX_INFLATED - 14));
        text.push_str("--></x>");
        let packed = deflate(text.as_bytes());
        let mut objects = vec![
            b"<</Metadata 3 0 R/Pages 2 0 R/Type/Catalog>>".to_vec(),
            b"<</Count 0/Kids[]/Type/Pages>>".to_vec(),
        ];
        for _ in 0..(MAX_INFLATED_TOTAL / MAX_INFLATED) + 1 {
            objects.push(stream("/Subtype/XML/Type/Metadata", &packed));
        }
        let bytes = file(&objects, TRAILER);
        assert_eq!(mend_all(bytes), Err(Refusal::StreamTooBig));
    }

    #[test]
    fn a_mended_file_that_passes_the_limit_refuses_the_file() {
        let file = with_xmp(&deflate(PACKET.as_bytes()));
        assert_eq!(
            mend(file.clone(), file.len() + 10, &mut |_| {}),
            Err(Refusal::FileTooBig)
        );
        assert!(mend(file.clone(), file.len() + 4000, &mut |_| {}).is_ok());
    }

    #[test]
    fn two_streams_are_written_again_in_order_of_their_numbers() {
        let packed = deflate(PACKET.as_bytes());
        let bytes = file(
            &[
                b"<</Metadata 4 0 R/Pages 2 0 R/Type/Catalog>>".to_vec(),
                b"<</Count 0/Kids[]/Type/Pages>>".to_vec(),
                stream("/Subtype/XML/Type/Metadata", &packed),
                stream("/Subtype/XML/Type/Metadata", &packed),
            ],
            "/Root 1 0 R/Size 5",
        );
        let mended = mend_all(bytes.clone()).unwrap();
        let text = String::from_utf8_lossy(&mended[bytes.len()..]).into_owned();
        let first = text.find("3 0 obj").unwrap();
        let second = text.find("4 0 obj").unwrap();
        assert!(first < second);
        assert!(text.contains("xref\r\n3 1\r\n"));
        assert!(text.contains("4 1\r\n"));
        let table = Table::read(&mended).unwrap();
        assert_eq!(table.sorted.len(), 2);
    }

    #[test]
    fn a_file_with_no_end_of_line_at_its_end_gets_one() {
        let mut bytes = with_xmp(&deflate(PACKET.as_bytes()));
        while bytes.ends_with(b"\r") || bytes.ends_with(b"\n") {
            bytes.pop();
        }
        let mended = mend_all(bytes.clone()).unwrap();
        assert_eq!(&mended[bytes.len()..bytes.len() + 2], b"\r\n");
        let table = Table::read(&mended).expect("the table of the update is read");
        assert_eq!(table.sorted[0].0, bytes.len() + 2);
    }

    #[test]
    fn the_work_is_one_pass_over_a_file_with_many_objects() {
        let packed = deflate(PACKET.as_bytes());
        let mut objects = vec![
            b"<</Metadata 3 0 R/Pages 2 0 R/Type/Catalog>>".to_vec(),
            b"<</Count 0/Kids[]/Type/Pages>>".to_vec(),
            stream("/Subtype/XML/Type/Metadata", &packed),
        ];
        for i in 0..100_000 {
            objects.push(format!("<</A {i}/B[1 2 3]/C(text)>>").into_bytes());
        }
        let bytes = file(&objects, "/Root 1 0 R/Size 100004");
        let started = std::time::Instant::now();
        let mended = mend_all(bytes.clone()).unwrap();
        assert!(mended.len() > bytes.len());
        assert!(
            started.elapsed().as_millis() < 5000,
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn progress_is_reported_for_a_big_file() {
        let packed = deflate(PACKET.as_bytes());
        let mut objects = vec![
            b"<</Metadata 3 0 R/Pages 2 0 R/Type/Catalog>>".to_vec(),
            b"<</Count 0/Kids[]/Type/Pages>>".to_vec(),
            stream("/Subtype/XML/Type/Metadata", &packed),
        ];
        objects.push(stream("/Subtype/Image", &vec![7u8; 9 * 1024 * 1024]));
        let bytes = file(&objects, "/Root 1 0 R/Size 5");
        let mut calls = 0;
        mend(bytes, usize::MAX, &mut |_| calls += 1).unwrap();
        assert!(calls >= 1);
    }
}
