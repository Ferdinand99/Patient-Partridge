//! The strict profile of the region check.
//!
//! The check of a region gives exit status 0 only when every page (its content, and the forms,
//! annotation appearances, soft mask groups, patterns and Type 3 glyph procedures that it draws)
//! lies inside this profile, which the tool understands fully. Whatever is outside it is "could not
//! check" (exit status 2). The profile is an allow-list: a reader that does something else with a
//! feature is of no concern, because the feature is not in it. When in doubt, a feature is left out.
//!
//! What is inside:
//!
//! 1. **Operators.** Every operator of a content stream is one of the operators of the PDF
//!    specification (Table A.1), with exactly the number and the types of operands that the
//!    specification gives: numbers are finite, an integer operand has no fraction, `TJ` shows
//!    only strings and numbers. An operator that is not in the table is refused, except between
//!    `BX` and `EX` in the walk (the x-ray copy refuses it there too: it cannot be rewritten).
//!    Readers differ on extra operands (Poppler takes the last, and says nothing), so none are
//!    accepted ([`operand_problem`]).
//! 2. **Matrices and boxes.** A `/Matrix`, `/BBox`, `/Rect` or `/FontMatrix` is exactly six (a
//!    matrix) or four (a box) finite numbers, no more, no fewer, nothing else ([`exact_numbers`]).
//!    A form has a `/BBox`.
//! 3. **Type 3 fonts.** The `/FontMatrix` has no rotation, skew or zero scale, the `/Widths` fit
//!    `/FirstChar`..`/LastChar`, and every shown code is named by `/Differences` and has a glyph
//!    procedure that can be read and is walked: a Type 3 glyph paints what its procedure says,
//!    where the pen puts it, and the pen follows the widths that the file gives.
//! 4. **Annotations.** An annotation with the NoRotate flag on a page that is turned is refused
//!    when it lies near a region (the walk cannot place its appearance).
//!
//! Not in the profile, because the render of the x-ray copy decides it: where the glyphs of a font
//! are drawn. Fonts are not read for their metrics, their programs are not parsed, and no font
//! is refused.
//!
//! The contract with the redaction engine is in the README: once the box is taken away, no
//! renderer may draw anything inside the region.

use crate::content::{Op, Operand};
use crate::pdfobj::{Doc, Obj};

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
