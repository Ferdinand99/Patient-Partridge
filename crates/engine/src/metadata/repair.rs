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
//! The data comes from a file that is not to be trusted, so the work and the update are bounded.
//! A stream is first decoded for its first kilobyte only, and decoded in full only when that is
//! XML; the filter has to account for all the data (a little white space may follow its end); every
//! full decoding is charged to a budget for the whole file, and so is every byte that is written
//! again (the data and the dictionary). The size of the update is worked out before it is built,
//! checked against the limit, and its memory asked for in one piece that may be refused.
//!
//! A protected file is never touched: its metadata is the business of [`super`]. A file that has
//! an earlier part (`/Prev`) is not a file PDFium wrote, and is left as it is as well.

use std::borrow::Cow;
use std::io::Read;

use super::{
    count_names, is_space, looks_like_xml, parse_dictionary, token, Table, Value, PROGRESS_STEP,
};

/// The most one stream may inflate to, and the most data it may have. An XMP packet is some
/// kilobytes; this leaves room for a large one and stops a stream that is made to swell.
pub const MAX_INFLATED: usize = 4 * 1024 * 1024;

/// The most all the streams of a file may inflate to together, and the most that is written again
/// for them (their data and dictionaries) together.
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
    /// XML, after this chain of filters.
    Xml(Chain),
    /// Not data of any of the chains, not XML when decoded, or data that the filters do not
    /// account for to the end.
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

/// How far a decoder goes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// The first bytes only, to see whether they are XML: the decoder stops at the limit or at the
    /// first thing it cannot read, and says what it has.
    Prefix,
    /// All the data, and nothing may be left over.
    Whole,
}

/// How many decoded bytes are looked at to tell whether a stream is XML before it is decoded in
/// full, and how many bytes of the data are read for them.
const PREFIX_OUT: usize = 1024;
const PREFIX_IN: usize = 64 * 1024;

/// How many bytes of white space may follow the end of the data of a filter.
const TAIL_SPACE: usize = 16;

/// Decodes `data` with the filters of `chain` into no more than `cap` bytes. The second number is
/// how many bytes Flate made, for the budget (the other filters cost about as much as the data).
fn decode(chain: Chain, data: &[u8], cap: usize, mode: Mode) -> (Decoded, usize) {
    let data = if mode == Mode::Prefix {
        &data[..data.len().min(PREFIX_IN)]
    } else {
        data
    };
    // What Flate makes of text is not much bigger than the text, so the bytes between the two
    // filters of a chain are limited too.
    let packed_room = match mode {
        Mode::Prefix => PREFIX_IN / 2,
        Mode::Whole => cap.saturating_add(cap / 16).saturating_add(1024),
    };
    let packed: Cow<'_, [u8]> = match chain {
        Chain::Flate => Cow::Borrowed(data),
        Chain::HexFlate => match ascii_hex(data, packed_room, mode) {
            Decoded::Bytes(bytes) => Cow::Owned(bytes),
            other => return (other, 0),
        },
        Chain::Base85Flate => match ascii_85(data, packed_room, mode) {
            Decoded::Bytes(bytes) => Cow::Owned(bytes),
            other => return (other, 0),
        },
        Chain::Hex => return (ascii_hex(data, cap, mode), 0),
        Chain::Base85 => return (ascii_85(data, cap, mode), 0),
    };
    let mut text = Vec::new();
    let mut slice: &[u8] = &packed;
    let mut decoder = flate2::bufread::ZlibDecoder::new(&mut slice);
    let limit = if mode == Mode::Prefix {
        PREFIX_OUT
    } else {
        cap
    };
    let read = (&mut decoder)
        .take(limit as u64 + u64::from(mode == Mode::Whole))
        .read_to_end(&mut text);
    if mode == Mode::Prefix {
        // Whatever came out before the data stopped making sense is the prefix.
        text.truncate(PREFIX_OUT);
        let made = text.len();
        return (Decoded::Bytes(text), made);
    }
    let made = text.len();
    if read.is_err() {
        return (Decoded::Bad, made);
    }
    if text.len() > cap {
        return (Decoded::TooBig, made);
    }
    // The zlib stream has to end where the data ends, but for a little white space.
    let used = decoder.total_in() as usize;
    let rest = packed.get(used..).unwrap_or(&[]);
    if rest.len() > TAIL_SPACE || !rest.iter().all(|b| is_space(*b)) {
        return (Decoded::Bad, made);
    }
    (Decoded::Bytes(text), made)
}

/// `ASCIIHexDecode`: hex digits and white space, ending at `>` or at the end of the data. Anything
/// else is not this filter's data, and so is more than a little white space after the end.
fn ascii_hex(data: &[u8], cap: usize, mode: Mode) -> Decoded {
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
            _ if mode == Mode::Prefix => return Decoded::Bytes(out),
            _ => return Decoded::Bad,
        };
        match high.take() {
            None => high = Some(digit),
            Some(h) => {
                if out.len() >= cap {
                    return if mode == Mode::Prefix {
                        Decoded::Bytes(out)
                    } else {
                        Decoded::TooBig
                    };
                }
                out.push(h << 4 | digit);
            }
        }
    }
    if let Some(h) = high {
        if out.len() >= cap {
            return if mode == Mode::Prefix {
                Decoded::Bytes(out)
            } else {
                Decoded::TooBig
            };
        }
        out.push(h << 4);
    }
    // Nothing but a little white space after the end.
    let mut seen = 0usize;
    for &b in rest {
        seen += 1;
        if mode == Mode::Whole && (seen > TAIL_SPACE || !is_space(b)) {
            return Decoded::Bad;
        }
    }
    Decoded::Bytes(out)
}

/// `ASCII85Decode`: groups of five characters from `!` to `u` (and `z` for four zeros), white
/// space between them, ending at `~>`. Anything else is not this filter's data, and so is more
/// than a little white space after the end.
fn ascii_85(data: &[u8], cap: usize, mode: Mode) -> Decoded {
    let prefix = mode == Mode::Prefix;
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
                    return if prefix {
                        Decoded::Bytes(out)
                    } else {
                        Decoded::Bad
                    };
                }
                at += 1;
                ended = true;
                break;
            }
            b'z' if held == 0 => {
                if out.len() + 4 > cap {
                    return if prefix {
                        Decoded::Bytes(out)
                    } else {
                        Decoded::TooBig
                    };
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
                        return if prefix {
                            Decoded::Bytes(out)
                        } else {
                            Decoded::Bad
                        };
                    };
                    if out.len() + 4 > cap {
                        return if prefix {
                            Decoded::Bytes(out)
                        } else {
                            Decoded::TooBig
                        };
                    }
                    out.extend_from_slice(&value.to_be_bytes());
                    held = 0;
                }
            }
            b if is_space(b) => {}
            _ if prefix => return Decoded::Bytes(out),
            _ => return Decoded::Bad,
        }
    }
    if prefix {
        return Decoded::Bytes(out);
    }
    // The end mark is required, and only a little white space comes after it.
    let tail = &data[at..];
    if !ended || tail.len() > TAIL_SPACE || !tail.iter().all(|b| is_space(*b)) {
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

/// Finds the chain of filters that makes XML of `data`, and decodes the data with it in full, into
/// no more than `room` bytes (and [`MAX_INFLATED`]). Only the first bytes are decoded for each
/// chain to see whether they are XML; the full decoding is for the stream that passes. The second
/// number is how many bytes Flate made, to be charged to the budget whatever came of it.
fn unpack(data: &[u8], room: usize) -> (Unpacked, usize) {
    let Some(chain) = Chain::ALL.into_iter().find(|chain| {
        matches!(
            decode(*chain, data, PREFIX_OUT, Mode::Prefix).0,
            Decoded::Bytes(text) if looks_like_xml(&text[..text.len().min(256)])
        )
    }) else {
        return (Unpacked::Other, 0);
    };
    let cap = room.min(MAX_INFLATED);
    let (decoded, made) = decode(chain, data, cap, Mode::Whole);
    let unpacked = match decoded {
        Decoded::Bytes(_) => Unpacked::Xml(chain),
        Decoded::Bad => Unpacked::Other,
        Decoded::TooBig => Unpacked::TooBig,
    };
    (unpacked, made)
}

fn reserve_bytes(bytes: &mut Vec<u8>, more: usize) -> bool {
    bytes.try_reserve(more).is_ok()
}

/// Mends the XMP streams of the bytes PDFium wrote for a document, as described above, and gives
/// the bytes back: the same ones when there was nothing to mend. `limit` is the most the file may
/// hold. `progress` is told how far the work has come now and then.
pub fn mend(
    file: Vec<u8>,
    limit: usize,
    progress: &mut dyn FnMut(u32),
) -> Result<Vec<u8>, Refusal> {
    mend_with(file, limit, progress, reserve_bytes)
}

/// As [`mend`], with the function that makes room in the buffer for the update (a test replaces
/// it with one that fails).
fn mend_with(
    mut file: Vec<u8>,
    limit: usize,
    progress: &mut dyn FnMut(u32),
    reserve: fn(&mut Vec<u8>, usize) -> bool,
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
    let (trailer_start, trailer_end) = (trailer.start, trailer.end);

    let mut jobs: Vec<Job> = Vec::new();
    // What the streams may still make when decoded, and what may still be copied into the update.
    let mut room = MAX_INFLATED_TOTAL;
    let mut copy_room = MAX_INFLATED_TOTAL;
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
        let (unpacked, made) = unpack(data_bytes, room);
        room = room.saturating_sub(made);
        let chain = match unpacked {
            Unpacked::Xml(chain) => chain,
            Unpacked::Other => continue,
            Unpacked::TooBig => return Err(Refusal::StreamTooBig),
        };
        // What is written again is bounded as well as what is decoded: the data, and the
        // dictionary that is copied with it.
        let cost = data_bytes.len() + (dict.end - dict.start) + COPY_OVERHEAD;
        if data_bytes.len() > MAX_INFLATED || cost > copy_room {
            return Err(Refusal::StreamTooBig);
        }
        copy_room -= cost;
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

    // The size of the update is worked out first (a few bytes too many for the number of the
    // table), checked against the limit, and made room for with one allocation that may fail.
    let digits = |n: usize| n.to_string().len();
    let eol = usize::from(!file.ends_with(b"\n") && !file.ends_with(b"\r")) * 2;
    let mut size = eol + "xref\r\n".len();
    for job in &jobs {
        size += format!("{} {} obj\r\n", job.number, job.generation).len()
            + (job.dict.1 - job.dict.0)
            + job.chain.entry().len()
            + b"\r\nstream\r\n".len()
            + (job.data.1 - job.data.0)
            + b"\r\nendstream\r\nendobj\r\n".len()
            + digits(job.number as usize)
            + b" 1\r\n".len()
            + 20;
    }
    size += format!("trailer\r\n<</Prev {} ", table.end).len()
        + (trailer_end - trailer_start - 2)
        + b"\r\nstartxref\r\n".len()
        + digits(usize::MAX)
        + b"\r\n%%EOF\r\n".len();
    if size > limit.saturating_sub(file.len()) {
        return Err(Refusal::FileTooBig);
    }
    let before = file.len();
    if !reserve(&mut file, size) {
        return Err(Refusal::FileTooBig);
    }

    // The update is added to the end of the buffer, which has the room for it.
    if eol > 0 {
        file.extend_from_slice(b"\r\n");
    }
    let mut places = Vec::with_capacity(jobs.len());
    for job in &jobs {
        places.push(file.len());
        file.extend_from_slice(format!("{} {} obj\r\n", job.number, job.generation).as_bytes());
        // `/Filter` goes in first: the keys of a dictionary have no order.
        file.extend_from_within(job.dict.0..job.dict.0 + 2);
        file.extend_from_slice(job.chain.entry());
        file.extend_from_within(job.dict.0 + 2..job.dict.1);
        file.extend_from_slice(b"\r\nstream\r\n");
        file.extend_from_within(job.data.0..job.data.1);
        file.extend_from_slice(b"\r\nendstream\r\nendobj\r\n");
    }
    let table_at = file.len();
    file.extend_from_slice(b"xref\r\n");
    for (job, place) in jobs.iter().zip(&places) {
        file.extend_from_slice(
            format!(
                "{} 1\r\n{place:010} {:05} n\r\n",
                job.number, job.generation
            )
            .as_bytes(),
        );
    }
    file.extend_from_slice(format!("trailer\r\n<</Prev {} ", table.end).as_bytes());
    file.extend_from_within(trailer_start + 2..trailer_end);
    file.extend_from_slice(format!("\r\nstartxref\r\n{table_at}\r\n%%EOF\r\n").as_bytes());
    if file.len() > limit {
        file.truncate(before);
        return Err(Refusal::FileTooBig);
    }
    Ok(file)
}

/// What a stream written again costs besides its dictionary and data, counted against the budget:
/// more than the object header, the filter entry and the line of the table.
const COPY_OVERHEAD: usize = 192;

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

/// Checks that `output` is `input` mended as [`mend`] does it: the bytes of `input`, then an update
/// with a table of its own whose trailer has the keys of the trailer of `input` and `/Prev` that
/// names the table of `input`, and whose objects are objects of `input` with the same data, the
/// same length and a `/Filter` added. For the fuzz target and the tests; the answer is the reason.
#[doc(hidden)]
pub fn check_update(input: &[u8], output: &[u8]) -> Result<(), String> {
    if output == input {
        return Ok(());
    }
    if !output.starts_with(input) {
        return Err("the bytes of the input were changed".into());
    }
    let old = Table::read(input).ok_or("the table of the input cannot be read")?;
    let new = Table::read(output).ok_or("the table of the update cannot be read")?;
    let old_trailer = parse_dictionary(input, old.trailer).ok_or("trailer of the input")?;
    let new_trailer = parse_dictionary(output, new.trailer).ok_or("trailer of the update")?;
    if new.end < input.len() || new.sorted.is_empty() {
        return Err("the table of the update is in the wrong place or empty".into());
    }
    if !matches!(new_trailer.get(b"Prev"), Some(Value::Int(p)) if usize::try_from(*p) == Ok(old.end))
    {
        return Err("/Prev is not the table of the input".into());
    }
    let mut want: Vec<&[u8]> = old_trailer
        .entries
        .iter()
        .map(|(k, _)| k.as_ref())
        .chain([&b"Prev"[..]])
        .collect();
    let mut have: Vec<&[u8]> = new_trailer
        .entries
        .iter()
        .map(|(k, _)| k.as_ref())
        .collect();
    want.sort_unstable();
    have.sort_unstable();
    if want != have {
        return Err("the trailer of the update has other keys".into());
    }
    for (index, &(offset, number)) in new.sorted.iter().enumerate() {
        if offset < input.len() {
            return Err("an object of the update lies in the input".into());
        }
        let object = new
            .read_at(output, index)
            .ok_or("an object of the update")?;
        let (Some(dict), Some(data)) = (&object.dict, object.data) else {
            return Err("an object of the update is not a stream".into());
        };
        let Some(Value::Int(length)) = dict.get(b"Length") else {
            return Err("an object of the update has no length".into());
        };
        let old_offset = *old
            .index
            .get(&number)
            .ok_or("an object that was not there")?;
        let old_index = old
            .sorted
            .binary_search_by_key(&old_offset, |&(o, _)| o)
            .map_err(|_| "an object that was not there")?;
        let before = old
            .read_at(input, old_index)
            .ok_or("an object of the input")?;
        let (Some(old_dict), Some(old_data)) = (&before.dict, before.data) else {
            return Err("an object of the input is not a stream".into());
        };
        if dict.get(b"Filter").is_none() || old_dict.get(b"Filter").is_some() {
            return Err("the filter was not added".into());
        }
        if dict.entries.len() != old_dict.entries.len() + 1
            || old_dict.get(b"Length") != Some(&Value::Int(*length))
        {
            return Err("the dictionary is not the old one with a filter".into());
        }
        let length = *length as usize;
        if output.get(data..data + length) != input.get(old_data..old_data + length) {
            return Err("the data of a stream is not the data of the input".into());
        }
    }
    let added = output.len() - input.len();
    let trailer_len = old_trailer.end - old_trailer.start;
    if added > MAX_INFLATED_TOTAL + trailer_len + 4096 {
        return Err("the update is longer than the budget allows".into());
    }
    Ok(())
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
            let Decoded::Bytes(back) = ascii_hex(&hex(sample), usize::MAX, Mode::Whole) else {
                panic!("hex");
            };
            assert_eq!(back, sample);
            let Decoded::Bytes(back) = ascii_85(&base85(sample), usize::MAX, Mode::Whole) else {
                panic!("85");
            };
            assert_eq!(back, sample);
        }
        // Odd digits are padded, white space is skipped, the data after the end is not allowed.
        assert!(
            matches!(ascii_hex(b"3 C 3>", 9, Mode::Whole), Decoded::Bytes(b) if b == [0x3c, 0x30])
        );
        assert!(matches!(ascii_hex(b"3C>3C", 9, Mode::Whole), Decoded::Bad));
        assert!(matches!(ascii_hex(b"3G", 9, Mode::Whole), Decoded::Bad));
        assert!(matches!(
            ascii_hex(b"3C3C3C", 2, Mode::Whole),
            Decoded::TooBig
        ));
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
            assert!(
                matches!(ascii_85(bad, 99, Mode::Whole), Decoded::Bad),
                "{bad:?}"
            );
        }
        assert!(matches!(
            ascii_85(b"zzz~>", 11, Mode::Whole),
            Decoded::TooBig
        ));
        assert!(matches!(ascii_85(b"zzz~>", 12, Mode::Whole), Decoded::Bytes(b) if b.len() == 12));
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

    fn with_tail(mut data: Vec<u8>, tail: &[u8], times: usize) -> Vec<u8> {
        for _ in 0..times {
            data.extend_from_slice(tail);
        }
        data
    }

    #[test]
    fn data_after_the_end_of_the_filter_means_it_is_not_that_filter() {
        let started = std::time::Instant::now();
        let flated = deflate(PACKET.as_bytes());
        // 64 MiB after the zlib stream, after the `>` of hex and after the `~>` of base 85.
        let tail = 64 * 1024 * 1024;
        for data in [
            with_tail(flated.clone(), b"Q", tail),
            with_tail(flated.clone(), b" ", tail),
            with_tail(hex(PACKET.as_bytes()), b" ", tail),
            with_tail(base85(PACKET.as_bytes()), b" ", tail),
            with_tail(base85(PACKET.as_bytes()), b"x", 100),
        ] {
            let file = with_xmp(&data);
            assert_eq!(mend_all(file.clone()).unwrap(), file);
        }
        assert!(
            started.elapsed().as_millis() < 120_000,
            "{:?}",
            started.elapsed()
        );
        // A little white space is the end of a line, and is allowed; more is not.
        for (spaces, mended) in [(0, true), (TAIL_SPACE, true), (TAIL_SPACE + 1, false)] {
            for data in [
                with_tail(flated.clone(), b" ", spaces),
                with_tail(hex(PACKET.as_bytes()), b"\n", spaces),
                with_tail(base85(PACKET.as_bytes()), b" ", spaces),
            ] {
                let file = with_xmp(&data);
                assert_eq!(mend_all(file.clone()).unwrap() != file, mended, "{spaces}");
            }
        }
    }

    #[test]
    fn data_that_is_not_xml_is_not_refused_however_big_it_swells() {
        // Over the limit when inflated, but not XML: the save goes ahead as it did before.
        let big = deflate(&vec![b'a'; MAX_INFLATED + 10]);
        let file = with_xmp(&big);
        assert_eq!(mend_all(file.clone()).unwrap(), file);
        let text = hex(&vec![b'a'; MAX_INFLATED + 10]);
        let file = with_xmp(&text);
        assert_eq!(mend_all(file.clone()).unwrap(), file);
    }

    #[test]
    fn xml_with_more_data_than_the_limit_is_refused() {
        // 5 MiB of white space inside the hex of a packet: the data is over the limit.
        let mut data = b"3C783E".to_vec();
        data.extend(std::iter::repeat_n(b' ', 5 * 1024 * 1024));
        data.push(b'>');
        assert_eq!(mend_all(with_xmp(&data)), Err(Refusal::StreamTooBig));
    }

    #[test]
    fn the_room_for_the_update_is_asked_for_and_a_refusal_is_clean() {
        let file = with_xmp(&deflate(PACKET.as_bytes()));
        let result = mend_with(file.clone(), usize::MAX, &mut |_| {}, |_, _| false);
        assert_eq!(result, Err(Refusal::FileTooBig));
        // The limit is checked before anything is built.
        let result = mend_with(file.clone(), file.len() + 1, &mut |_| {}, |_, _| {
            panic!("room was asked for an update that does not fit")
        });
        assert_eq!(result, Err(Refusal::FileTooBig));
        // The room asked for is what the update takes, give or take the number of the table.
        static ASKED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let mended = mend_with(file.clone(), usize::MAX, &mut |_| {}, |bytes, more| {
            ASKED.store(more, std::sync::atomic::Ordering::SeqCst);
            bytes.try_reserve(more).is_ok()
        })
        .unwrap();
        let asked = ASKED.load(std::sync::atomic::Ordering::SeqCst);
        let used = mended.len() - file.len();
        assert!(asked >= used && asked - used < 32, "{asked} {used}");
    }

    #[test]
    fn many_streams_that_swell_without_being_xml_cost_next_to_nothing() {
        // Each inflates to a little under the limit and is not markup; a full decoding of each
        // would take a second for a few hundred of them, even in a release build.
        let swollen = deflate(&vec![b'a'; MAX_INFLATED - 1]);
        let mut objects = vec![
            b"<</Metadata 3 0 R/Pages 2 0 R/Type/Catalog>>".to_vec(),
            b"<</Count 0/Kids[]/Type/Pages>>".to_vec(),
        ];
        for _ in 0..400 {
            objects.push(stream("/Subtype/XML/Type/Metadata", &swollen));
        }
        let file = file(&objects, "/Root 1 0 R/Size 403");
        let started = std::time::Instant::now();
        assert_eq!(mend_all(file.clone()).unwrap(), file);
        assert!(
            started.elapsed().as_millis() < 1500,
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn the_dictionaries_that_are_written_again_are_counted() {
        // Streams with a big dictionary each: they are copied whole, so they are in the budget.
        let packed = deflate(PACKET.as_bytes());
        let junk = format!("/Junk({})", "x".repeat(1024 * 1024));
        let mut objects = vec![
            b"<</Metadata 3 0 R/Pages 2 0 R/Type/Catalog>>".to_vec(),
            b"<</Count 0/Kids[]/Type/Pages>>".to_vec(),
        ];
        for _ in 0..(MAX_INFLATED_TOTAL / (1024 * 1024)) + 2 {
            objects.push(stream(
                &format!("/Subtype/XML/Type/Metadata{junk}"),
                &packed,
            ));
        }
        let file = file(&objects, "/Root 1 0 R/Size 40");
        assert_eq!(mend_all(file), Err(Refusal::StreamTooBig));
    }

    #[test]
    fn a_mended_file_passes_its_own_check() {
        let packed = deflate(PACKET.as_bytes());
        for data in [
            packed.clone(),
            hex(&packed),
            base85(PACKET.as_bytes()),
            with_tail(packed.clone(), b"\r\n", 1),
        ] {
            let damaged = with_xmp(&data);
            let mended = mend_all(damaged.clone()).unwrap();
            assert_ne!(mended, damaged);
            assert_eq!(check_update(&damaged, &mended), Ok(()));
        }
        // And it says so when the update is not one.
        let damaged = with_xmp(&packed);
        let mut mended = mend_all(damaged.clone()).unwrap();
        assert!(check_update(&damaged, &mended[..mended.len() - 30]).is_err());
        let tail = mended[damaged.len()..]
            .windows(11)
            .position(|w| w == b"\r\nendstream")
            .unwrap();
        let at = damaged.len() + tail - 1;
        mended[at] ^= 1;
        assert!(check_update(&damaged, &mended).is_err());
        assert!(check_update(&damaged, &damaged[1..]).is_err());
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
