//! A small writer and reader for DER, enough for the signature that goes into a PDF.
//!
//! The writer builds the pieces of a CMS `SignedData`. The reader walks the few structures the
//! signer has to look into (a PKCS #8 key, an RSA public key, an EC private key). Both are made
//! for input that is small and known; the reader never allocates and never trusts a length.

/// The encoded length of a value of `n` bytes.
pub fn length(n: usize) -> Vec<u8> {
    if n < 0x80 {
        return vec![n as u8];
    }
    let bytes = n.to_be_bytes();
    let skip = bytes.iter().take_while(|b| **b == 0).count();
    let mut out = vec![0x80 | (bytes.len() - skip) as u8];
    out.extend_from_slice(&bytes[skip..]);
    out
}

/// A tag, a length and the content.
pub fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    out.extend(length(content.len()));
    out.extend_from_slice(content);
    out
}

pub fn cat(parts: &[Vec<u8>]) -> Vec<u8> {
    parts.concat()
}

pub fn seq(parts: &[Vec<u8>]) -> Vec<u8> {
    tlv(0x30, &cat(parts))
}

/// A SET OF in the order DER wants: by the bytes of the elements.
pub fn set_of(parts: &[Vec<u8>]) -> Vec<u8> {
    let mut sorted = parts.to_vec();
    sorted.sort();
    tlv(0x31, &cat(&sorted))
}

/// An object identifier from its dotted form. Only called with constants of this program.
pub fn oid(dotted: &str) -> Vec<u8> {
    let arcs: Vec<u64> = dotted
        .split('.')
        .map(|a| a.parse().expect("an arc of a constant identifier"))
        .collect();
    let mut body = vec![(arcs[0] * 40 + arcs[1]) as u8];
    for arc in &arcs[2..] {
        let mut chunk = vec![(*arc & 0x7f) as u8];
        let mut rest = *arc >> 7;
        while rest > 0 {
            chunk.push(0x80 | (rest & 0x7f) as u8);
            rest >>= 7;
        }
        chunk.reverse();
        body.extend(chunk);
    }
    tlv(0x06, &body)
}

/// A small non-negative INTEGER.
pub fn int(n: u8) -> Vec<u8> {
    debug_assert!(n < 0x80);
    tlv(0x02, &[n])
}

/// An INTEGER whose content bytes are given as they stand (a serial number copied out of a
/// certificate keeps its own encoding).
pub fn integer_raw(content: &[u8]) -> Vec<u8> {
    tlv(0x02, content)
}

pub fn octets(bytes: &[u8]) -> Vec<u8> {
    tlv(0x04, bytes)
}

pub fn null() -> Vec<u8> {
    vec![0x05, 0x00]
}

/// `[n]` constructed, explicit.
pub fn explicit(n: u8, content: &[u8]) -> Vec<u8> {
    tlv(0xA0 | n, content)
}

/// One element read from DER.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Element<'a> {
    pub tag: u8,
    pub content: &'a [u8],
    /// What follows the element.
    pub rest: &'a [u8],
}

/// Reads the element at the start of `input`. Only definite lengths of up to four bytes and tags
/// of one byte are taken. `None` when the input is not that.
pub fn read(input: &[u8]) -> Option<Element<'_>> {
    let tag = *input.first()?;
    if tag & 0x1f == 0x1f {
        return None;
    }
    let first = *input.get(1)?;
    let (len, header) = if first < 0x80 {
        (first as usize, 2)
    } else {
        let count = (first & 0x7f) as usize;
        if count == 0 || count > 4 {
            return None;
        }
        let bytes = input.get(2..2 + count)?;
        let len = bytes.iter().fold(0usize, |n, b| (n << 8) | *b as usize);
        (len, 2 + count)
    };
    let end = header.checked_add(len)?;
    Some(Element {
        tag,
        content: input.get(header..end)?,
        rest: input.get(end..)?,
    })
}

/// The content bytes of an INTEGER without the leading zeros that only keep it positive.
pub fn unsigned(content: &[u8]) -> &[u8] {
    let skip = content.iter().take_while(|b| **b == 0).count();
    &content[skip..]
}

/// The number of bits of an unsigned big-endian number.
pub fn bit_length(number: &[u8]) -> u32 {
    let number = unsigned(number);
    match number.first() {
        None => 0,
        Some(first) => (number.len() as u32 - 1) * 8 + (8 - first.leading_zeros()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_are_written_in_the_short_and_the_long_form() {
        assert_eq!(length(0), vec![0]);
        assert_eq!(length(127), vec![127]);
        assert_eq!(length(128), vec![0x81, 128]);
        assert_eq!(length(255), vec![0x81, 255]);
        assert_eq!(length(256), vec![0x82, 1, 0]);
        assert_eq!(length(70_000), vec![0x83, 1, 17, 112]);
    }

    #[test]
    fn identifiers_are_written_the_way_der_does() {
        assert_eq!(
            oid("1.2.840.113549.1.1.11"),
            vec![0x06, 9, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x0B]
        );
        assert_eq!(
            oid("1.3.132.0.34"),
            vec![0x06, 5, 0x2B, 0x81, 0x04, 0x00, 0x22]
        );
    }

    #[test]
    fn a_set_is_sorted_by_the_bytes_of_its_elements() {
        let set = set_of(&[vec![0x04, 1, 9], vec![0x02, 1, 5], vec![0x04, 1, 3]]);
        assert_eq!(set, vec![0x31, 9, 0x02, 1, 5, 0x04, 1, 3, 0x04, 1, 9]);
    }

    #[test]
    fn what_is_written_can_be_read_back() {
        let value = seq(&[int(1), octets(&[7; 200]), null()]);
        let outer = read(&value).unwrap();
        assert_eq!(outer.tag, 0x30);
        assert!(outer.rest.is_empty());
        let one = read(outer.content).unwrap();
        assert_eq!((one.tag, one.content), (0x02, &[1u8][..]));
        let two = read(one.rest).unwrap();
        assert_eq!((two.tag, two.content.len()), (0x04, 200));
        let three = read(two.rest).unwrap();
        assert_eq!(three.tag, 0x05);
    }

    #[test]
    fn input_that_is_not_der_is_not_read() {
        assert!(read(&[]).is_none());
        assert!(read(&[0x30]).is_none());
        assert!(read(&[0x30, 5, 1, 2]).is_none());
        assert!(read(&[0x30, 0x80, 0, 0]).is_none());
        assert!(read(&[0x30, 0x85, 0, 0, 0, 0, 1, 0]).is_none());
        assert!(read(&[0x1f, 1, 0]).is_none());
        // A length that cannot fit in memory is refused without an allocation.
        assert!(read(&[0x04, 0x84, 0xff, 0xff, 0xff, 0xff]).is_none());
    }

    #[test]
    fn bits_are_counted_without_the_zeros_in_front() {
        assert_eq!(bit_length(&[0, 0x80, 0]), 16);
        assert_eq!(bit_length(&[0x01]), 1);
        assert_eq!(bit_length(&[0x00]), 0);
        let mut n = vec![0u8; 256];
        n[0] = 0x80;
        assert_eq!(bit_length(&n), 2048);
    }
}
