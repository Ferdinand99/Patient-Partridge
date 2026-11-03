//! Fonts for the cases that need a code table of their own: glyph numbers that are small numbers
//! (so that a literal string is full of control bytes), a character map that gives a range and a
//! single code inside it, a map that stops after a few entries, and Type 3 fonts with glyph names.

use super::Bytes;

fn cmap_with(space: &str, body: &str) -> Bytes {
    format!(
        "/CIDInit /ProcSet findresource begin
12 dict begin
begincmap
         /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def
         /CMapName /Adobe-Identity-UCS def
/CMapType 2 def
         1 begincodespacerange
{space}
endcodespacerange
{body}         endcmap
CMapName currentdict /CMap defineresource pop
end
end
"
    )
    .into_bytes()
}

/// A character map for two-byte codes around `body` (the `bfchar` and `bfrange` sections).
pub fn cmap(body: &str) -> Bytes {
    cmap_with("<0000> <FFFF>", body)
}

/// The same for one-byte codes.
pub fn cmap_one_byte(body: &str) -> Bytes {
    cmap_with("<00> <FF>", body)
}

/// A code for each distinct character of `text`: `first`, `first + step`, ..
pub fn codes(first: u16, step: u16, text: &str) -> Vec<(u16, char)> {
    let mut seen: Vec<char> = Vec::new();
    for c in text.chars() {
        if !seen.contains(&c) {
            seen.push(c);
        }
    }
    seen.iter()
        .enumerate()
        .map(|(i, c)| (first + step * i as u16, *c))
        .collect()
}

fn code_of(table: &[(u16, char)], c: char) -> u16 {
    table
        .iter()
        .find(|(_, x)| *x == c)
        .expect("character in the table")
        .0
}

/// `n beginbfchar .. endbfchar` for the table.
pub fn bfchar(table: &[(u16, char)]) -> String {
    bfchar_width(table, 4)
}

/// The same with codes of `digits` hex digits (2 for a one-byte code).
pub fn bfchar_width(table: &[(u16, char)], digits: usize) -> String {
    let mut s = format!("{} beginbfchar\n", table.len());
    for (code, c) in table {
        s.push_str(&format!("<{code:0digits$X}> <{:04X}>\n", u32::from(*c)));
    }
    s.push_str("endbfchar\n");
    s
}

/// `<0101..>`: the codes that show `text`, as a hex string.
pub fn hex_codes(table: &[(u16, char)], text: &str) -> String {
    let mut s = String::from("<");
    for c in text.chars() {
        s.push_str(&format!("{:04X}", code_of(table, c)));
    }
    s.push('>');
    s
}

/// `(..)`: the codes that show `text`, written as the raw bytes in a literal string. No code may
/// be a byte that a literal string must escape.
pub fn raw_literal(table: &[(u16, char)], text: &str) -> Bytes {
    let mut out = vec![b'('];
    for c in text.chars() {
        let code = code_of(table, c);
        for b in code.to_be_bytes() {
            assert!(!matches!(b, b'(' | b')' | b'\\' | b'\r'), "escape needed");
            out.push(b);
        }
    }
    out.push(b')');
    out
}

/// `(\000\003..)`: the same codes as octal escapes.
pub fn octal_literal(table: &[(u16, char)], text: &str) -> Bytes {
    let mut out = vec![b'('];
    for c in text.chars() {
        for b in code_of(table, c).to_be_bytes() {
            out.extend(format!("\\{b:03o}").into_bytes());
        }
    }
    out.push(b')');
    out
}

/// A Type 3 font dictionary whose glyph for the code `first + i` has the name `names[i]`, all
/// glyphs drawn by the procedure in object `proc_obj`. With `to_unicode`, it names a map.
pub fn type3_font(first: u8, names: &[String], proc_obj: u32, to_unicode: Option<u32>) -> Bytes {
    let procs: String = names
        .iter()
        .map(|n| format!("/{n} {proc_obj} 0 R "))
        .collect();
    let diffs: String = names.iter().map(|n| format!("/{n} ")).collect();
    let widths: String = names.iter().map(|_| "500 ").collect();
    let tu = to_unicode.map_or(String::new(), |n| format!(" /ToUnicode {n} 0 R"));
    format!(
        "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] \
         /CharProcs << {procs}>> /Encoding << /Type /Encoding /Differences [{first} {diffs}] >> \
         /FirstChar {first} /LastChar {} /Widths [{widths}] /Resources << >>{tu} >>",
        usize::from(first) + names.len() - 1
    )
    .into_bytes()
}

/// The glyph names for the distinct characters of `text` (from `name`, given the number of the
/// glyph and the character), and the literal string that shows `text` with them, the first
/// glyph having the code `first`.
pub fn type3_for(
    first: u8,
    text: &str,
    name: impl Fn(usize, char) -> String,
) -> (Vec<String>, Bytes) {
    let mut seen: Vec<char> = Vec::new();
    for c in text.chars() {
        if !seen.contains(&c) {
            seen.push(c);
        }
    }
    let names = seen.iter().enumerate().map(|(i, c)| name(i, *c)).collect();
    let mut shown = vec![b'('];
    for c in text.chars() {
        let i = seen.iter().position(|x| *x == c).unwrap();
        shown.push(first + u8::try_from(i).unwrap());
    }
    shown.push(b')');
    (names, shown)
}

/// The four objects of a composite font that shows glyph numbers: the Type 0 font at `base`, its
/// descendant, its descriptor and its character map (which maps `table`).
pub fn cid_font(base: u32, name: &str, table: &[(u16, char)]) -> Vec<(u32, Bytes)> {
    use super::hard_cases::packed;
    vec![
        (
            base,
            format!(
                "<< /Type /Font /Subtype /Type0 /BaseFont /{name} /Encoding /Identity-H \
                 /DescendantFonts [{} 0 R] /ToUnicode {} 0 R >>",
                base + 1,
                base + 3
            )
            .into_bytes(),
        ),
        (
            base + 1,
            format!(
                "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /{name} \
                 /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> \
                 /FontDescriptor {} 0 R /DW 600 /CIDToGIDMap /Identity >>",
                base + 2
            )
            .into_bytes(),
        ),
        (
            base + 2,
            format!(
                "<< /Type /FontDescriptor /FontName /{name} /Flags 32 /FontBBox [0 -200 1000 900] \
                 /ItalicAngle 0 /Ascent 900 /Descent -200 /CapHeight 700 /StemV 80 >>"
            )
            .into_bytes(),
        ),
        (base + 3, packed("", &cmap(&bfchar(table)))),
    ]
}
