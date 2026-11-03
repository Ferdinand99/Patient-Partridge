//! The strict profile of the geometry and glyph check of the redacted regions.
//!
//! The check of a region gives exit status 0 only when every page (its content, and the forms,
//! annotation appearances, soft mask groups and Type 3 glyph procedures that it draws) lies inside
//! this profile, which the tool understands fully. Whatever is outside it is "could not check"
//! (exit status 2). The profile is an allow-list: a reader that does something else with a feature
//! is of no concern, because the feature is not in it. When in doubt, a feature is left out.
//!
//! What is inside:
//!
//! 1. **Operators.** Every operator of a content stream is one of the operators of the PDF
//!    specification (Table A.1), with exactly the number and the types of operands that the
//!    specification gives: numbers are finite, an integer operand has no fraction, `TJ` shows
//!    only strings and numbers. An operator that is not in the table is refused, except between
//!    `BX` and `EX`. Readers differ on extra operands (Poppler takes the last, and says nothing),
//!    so none are accepted ([`operand_problem`]).
//! 2. **Matrices and boxes.** A `/Matrix`, `/BBox`, `/Rect` or `/FontMatrix` is exactly six (a
//!    matrix) or four (a box) finite numbers, no more, no fewer, nothing else ([`exact_numbers`]).
//!    A form has a `/BBox`.
//! 3. **Fonts.** Text is shown only with
//!    - one of the 14 standard fonts that the file does not embed, with no `/Differences` and an
//!      `/Encoding` that is absent or `WinAnsiEncoding`, `MacRomanEncoding` or `StandardEncoding`,
//!      and with no `/Widths`, or with `/Widths` that agree with the standard metrics to
//!      [`STD_TOLERANCE`] for every code from 32 to 126;
//!    - a simple font (Type 1, MMType1, TrueType) whose program is in the file (TrueType,
//!      OpenType, CFF or Type 1) and has a bounding box that the tool can read, with `/Widths`
//!      that are `/LastChar - /FirstChar + 1` numbers (a code outside that range is not shown),
//!      and, for a TrueType program, widths that are advances of the program;
//!    - a composite font (Type 0) with the encoding `Identity-H` (a name, not a stream), a
//!      descendant font whose program is in the file, and, for a TrueType descendant, widths that
//!      are the advances of the program for the glyph of the code, with the ink of that glyph
//!      within [`INK_SLACK`] em of its advance box;
//!    - a Type 3 font whose `/FontMatrix` has no rotation or skew, whose `/Widths` fit
//!      `/FirstChar`..`/LastChar`, and in which every shown code is named by `/Differences`, has a
//!      procedure in `/CharProcs` that can be read and is walked.
//!
//!    The bounding box of the program must lie within [`PROGRAM_SLACK_Y`] em above and below the
//!    ascent and descent that the glyph cell uses, and within [`PROGRAM_SLACK_X`] em to the left of
//!    the origin and to the right of the widest advance, so that the ink cannot be a long way from
//!    the cell. Writing is horizontal: a `/WMode` of 1 in the font or in a character map, a name
//!    that ends in `-V`, and every character map that is a stream or a name other than
//!    `Identity-H` are refused.
//! 4. **Annotations.** An annotation with the NoRotate flag on a page that is turned is refused.
//! 5. **Strokes.** A miter limit above [`MAX_MITER`], and a dash pattern on a path with joins, are
//!    refused when the stroke is near a region.
//!
//! The glyph rule (the contract with the redaction engine) is in the README and in
//! [`crate::walk`]: the cell of a glyph is its advance box across, from the origin by the width of
//! the glyph (after `Tz`), and from the descent to the ascent of the font, within the bounding box
//! of the font program. A glyph is under a region when its cell overlaps the region with positive
//! area, in every spec.

use crate::content::{Op, Operand};
use crate::pdfobj::{Doc, Obj};

/// How far the width of a standard font in the file may be from the metrics of the standard font,
/// as a fraction.
pub const STD_TOLERANCE: f64 = 0.01;

/// The highest miter limit that a stroke may have.
pub const MAX_MITER: f64 = 10.0;

/// How far the box of the font program may reach above the ascent and below the descent of the
/// glyph cell, in em.
pub const PROGRAM_SLACK_Y: f64 = 0.5;

/// How far the box of the font program may reach left of the origin and right of the widest
/// advance, in em.
pub const PROGRAM_SLACK_X: f64 = 1.2;

/// How far an advance of a TrueType program may be from a width in the file: this fraction of the
/// width, or [`ADVANCE_ABS`] em, whichever is more.
pub const ADVANCE_TOLERANCE: f64 = 0.02;
pub const ADVANCE_ABS: f64 = 0.005;

/// The least width of a shown glyph, in em (a simple font may show a space of no width).
pub const MIN_WIDTH: f64 = 0.01;

/// How far the ink of a glyph of a composite font with a TrueType program may reach left of its
/// origin and right of its advance, in em.
pub const INK_SLACK: f64 = 0.3;

/// The most that the width of a glyph may be, in em.
pub const MAX_WIDTH: f64 = 3.0;

/// The kind of an operand that an operator takes.
#[derive(Clone, Copy, PartialEq)]
enum K {
    /// A finite number.
    N,
    /// An integer in a range.
    I(i64, i64),
    Name,
    Str,
    /// An array of numbers.
    Nums,
    /// An array of strings and numbers.
    Text,
    /// A name or a dictionary.
    NameOrDict,
}

use K::*;

/// The operators of the specification and their operands. `sc`, `SC`, `scn` and `SCN` take a
/// variable number and are checked apart.
const OPERATORS: &[(&[u8], &[K])] = &[
    (b"b", &[]),
    (b"B", &[]),
    (b"b*", &[]),
    (b"B*", &[]),
    (b"BDC", &[Name, NameOrDict]),
    (b"BI", &[]),
    (b"BMC", &[Name]),
    (b"BT", &[]),
    (b"BX", &[]),
    (b"c", &[N, N, N, N, N, N]),
    (b"cm", &[N, N, N, N, N, N]),
    (b"CS", &[Name]),
    (b"cs", &[Name]),
    (b"d", &[Nums, N]),
    (b"d0", &[N, N]),
    (b"d1", &[N, N, N, N, N, N]),
    (b"Do", &[Name]),
    (b"DP", &[Name, NameOrDict]),
    (b"EMC", &[]),
    (b"ET", &[]),
    (b"EX", &[]),
    (b"f", &[]),
    (b"F", &[]),
    (b"f*", &[]),
    (b"G", &[N]),
    (b"g", &[N]),
    (b"gs", &[Name]),
    (b"h", &[]),
    (b"i", &[N]),
    (b"j", &[I(0, 2)]),
    (b"J", &[I(0, 2)]),
    (b"K", &[N, N, N, N]),
    (b"k", &[N, N, N, N]),
    (b"l", &[N, N]),
    (b"m", &[N, N]),
    (b"M", &[N]),
    (b"MP", &[Name]),
    (b"n", &[]),
    (b"q", &[]),
    (b"Q", &[]),
    (b"re", &[N, N, N, N]),
    (b"RG", &[N, N, N]),
    (b"rg", &[N, N, N]),
    (b"ri", &[Name]),
    (b"s", &[]),
    (b"S", &[]),
    (b"sh", &[Name]),
    (b"T*", &[]),
    (b"Tc", &[N]),
    (b"Td", &[N, N]),
    (b"TD", &[N, N]),
    (b"Tf", &[Name, N]),
    (b"Tj", &[Str]),
    (b"TJ", &[Text]),
    (b"TL", &[N]),
    (b"Tm", &[N, N, N, N, N, N]),
    (b"Tr", &[I(0, 7)]),
    (b"Ts", &[N]),
    (b"Tw", &[N]),
    (b"Tz", &[N]),
    (b"v", &[N, N, N, N]),
    (b"w", &[N]),
    (b"W", &[]),
    (b"W*", &[]),
    (b"y", &[N, N, N, N]),
    (b"'", &[Str]),
    (b"\"", &[N, N, Str]),
];

/// The operators that take a number of colour components.
const COLOUR_OPERATORS: [&[u8]; 4] = [b"sc", b"SC", b"scn", b"SCN"];

/// The most components of a colour (a DeviceN colour space has up to 32).
const MAX_COMPONENTS: usize = 32;

/// Whether the operator is one of the specification.
pub fn is_known(name: &[u8]) -> bool {
    OPERATORS.iter().any(|(n, _)| *n == name) || COLOUR_OPERATORS.contains(&name)
}

fn kind_name(k: K) -> &'static str {
    match k {
        N => "a number",
        I(..) => "an integer",
        Name => "a name",
        Str => "a string",
        Nums => "an array of numbers",
        Text => "an array of strings and numbers",
        NameOrDict => "a name or a dictionary",
    }
}

fn fits(k: K, a: &Operand) -> bool {
    match (k, a) {
        (N, Operand::Num(v)) => v.is_finite(),
        (I(lo, hi), Operand::Num(v)) => {
            v.is_finite() && v.fract() == 0.0 && *v >= lo as f64 && *v <= hi as f64
        }
        (Name, Operand::Name(_)) => true,
        (Str, Operand::Str(_)) => true,
        (Nums, Operand::Array(items)) => items
            .iter()
            .all(|x| matches!(x, Operand::Num(v) if v.is_finite())),
        (Text, Operand::Array(items)) => items
            .iter()
            .all(|x| matches!(x, Operand::Str(_)) || matches!(x, Operand::Num(v) if v.is_finite())),
        (NameOrDict, Operand::Name(_) | Operand::Other) => true,
        _ => false,
    }
}

/// Why the operands of a known operator are not the ones that the specification gives, or `None`
/// when they are (and for an operator that is not known: see [`is_known`]).
pub fn operand_problem(op: &Op) -> Option<String> {
    let name = String::from_utf8_lossy(op.name);
    if COLOUR_OPERATORS.contains(&op.name) {
        let (names, nums) = op.args.iter().fold((0, 0), |(a, b), x| match x {
            Operand::Name(_) => (a + 1, b),
            Operand::Num(v) if v.is_finite() => (a, b + 1),
            _ => (a + 1000, b),
        });
        let pattern_name_last = matches!(op.args.last(), Some(Operand::Name(_)));
        let with_name = op.name == b"scn" || op.name == b"SCN";
        let ok = names < 1000
            && nums <= MAX_COMPONENTS
            && if with_name {
                (names == 0 && nums >= 1) || (names == 1 && pattern_name_last)
            } else {
                names == 0 && nums >= 1
            };
        return (!ok).then(|| {
            format!(
                "the operator {name} has operands that are not a colour (1 to {MAX_COMPONENTS} numbers{})",
                if with_name { ", and a name last" } else { "" }
            )
        });
    }
    let (_, kinds) = OPERATORS.iter().find(|(n, _)| *n == op.name)?;
    if op.args.len() != kinds.len() {
        return Some(format!(
            "the operator {name} has {} operands, and the specification gives it {}",
            op.args.len(),
            kinds.len()
        ));
    }
    for (i, (k, a)) in kinds.iter().zip(&op.args).enumerate() {
        if !fits(*k, a) {
            return Some(format!(
                "operand {} of the operator {name} is not {}",
                i + 1,
                kind_name(*k)
            ));
        }
    }
    None
}

/// The numbers of an array of exactly `n` entries that are all finite numbers (references
/// followed), or `None`.
pub fn exact_numbers(o: &Obj, doc: &Doc, n: usize) -> Option<Vec<f64>> {
    let a = doc.resolve(o).array()?;
    if a.len() != n {
        return None;
    }
    a.iter()
        .map(|x| doc.resolve(x).num().filter(|v| v.is_finite()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::for_each_op;

    fn problems(content: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        for_each_op(content, &mut |op| {
            if let Some(p) = operand_problem(op) {
                out.push(p);
            }
        });
        out
    }

    #[test]
    fn the_operands_that_the_specification_gives_are_accepted() {
        let ok = b"q 1 0 0 1 10 20 cm /F1 12 Tf 0 g 1 0 0 rg 0 0 0 1 k [3 2] 0 d 2 w 1 j 0 J 10 M \
                   BT 70 80 Td (x) Tj [(a) -20 (b)] TJ 1 2 (c) \" (d) ' ET /Im1 Do /GS1 gs \
                   0.1 0.2 0.3 sc /P1 scn 0.5 /P1 scn 0 0 1 1 re f Q /OC /oc1 BDC EMC 500 0 d0 \
                   BX /Foo 1 foo EX";
        let p = problems(ok);
        assert!(p.is_empty(), "{p:?}");
    }

    #[test]
    fn an_operator_with_too_many_too_few_or_wrong_operands_is_refused() {
        for bad in [
            "/Blank /Fm1 Do",
            "[] [(x)] TJ",
            "/F1 /F7 12 Tf",
            "0.1 60 w",
            "1 2 3 cm",
            "1 q",
            "1.5 j",
            "9 Tr",
            "[(x) /N] TJ",
            "(x) 1 Tj",
            "1 2 3 4 5 6 7 cm",
            "/A 1 2 3 d",
            "0 0 0 0 rg",
            "/P1 1 sc",
            "1 /P1 /P2 scn",
            "true Tj",
            "1e400000 w",
        ] {
            assert!(
                !problems(bad.as_bytes()).is_empty(),
                "{bad} must be refused"
            );
        }
        assert!(is_known(b"Tj") && is_known(b"scn") && !is_known(b"zork"));
    }

    #[test]
    fn a_matrix_is_exactly_six_numbers() {
        let doc = Doc::scan(b"");
        let arr = |v: Vec<f64>| Obj::Array(v.into_iter().map(Obj::Real).collect());
        assert!(exact_numbers(&arr(vec![1.0, 0.0, 0.0, 1.0, 0.0, 0.0]), &doc, 6).is_some());
        assert!(exact_numbers(&arr(vec![1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]), &doc, 6).is_none());
        assert!(exact_numbers(&arr(vec![1.0, 0.0, 0.0, 1.0, 0.0]), &doc, 6).is_none());
        assert!(exact_numbers(&arr(vec![f64::INFINITY, 0.0, 0.0, 1.0]), &doc, 4).is_none());
        let mixed = Obj::Array(vec![
            Obj::Int(1),
            Obj::Name("x".into()),
            Obj::Int(2),
            Obj::Int(3),
        ]);
        assert!(exact_numbers(&mixed, &doc, 4).is_none());
        assert!(exact_numbers(&Obj::Int(1), &doc, 4).is_none());
    }
}
