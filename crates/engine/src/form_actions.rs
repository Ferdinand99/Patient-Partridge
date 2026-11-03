//! What the actions of a form can do to the form, read from the document as PDFium writes it.
//!
//! # What PDFium runs
//!
//! With no JavaScript platform, PDFium still follows the other actions of a field itself, and two of
//! them change a document: a reset of the form (`ResetForm`) clears fields, and a hide (`Hide`)
//! writes the hidden flag into the widgets of other fields. Which events of a field do that was read
//! in PDFium's source (`chromium/8125`, `fpdfsdk`):
//!
//! - A widget's own mouse and focus events (`E`, `X`, `D`, `U`, `Fo`, `Bl` of its `/AA`, and `/A` for
//!   the release of a button and for the keystroke when `/AA` has no entry) reach the action code
//!   through `CPDFSDK_Widget::OnAAction`, which hands them to `ExecuteFieldAction`. That function
//!   runs a script only when there is a JavaScript platform, and passes every other action, a reset
//!   or a hide among them, to `DoActionNoJs`. It then follows `/Next`, a single action or an array,
//!   one after the other.
//! - The keystroke (`K`) and validate (`V`) events go the same way, but PDFium looks them up in the
//!   `/AA` of the field the widget belongs to (`CPDFSDK_Widget::GetAAction`, then
//!   `CPDF_FormField::GetAdditionalAction`, which looks through `/Parent` as far as 32 levels), and
//!   only after that in the widget. The field is built from the field down through `/Kids`, and a
//!   second dictionary with the same full name is joined to the first by name alone, so a widget can
//!   be tied to a dictionary that no `/Parent` and no `/Kids` of the file leads to.
//! - The format (`F`) and calculate (`C`) events are run by `CPDFSDK_InteractiveForm::OnFormat` and
//!   `OnCalculate`, which take the action's JavaScript and nothing else, so they cannot reset or hide
//!   anything. They are counted with the other three anyway, so that a different build of PDFium
//!   cannot make them matter without this search noticing.
//! - The events of a page and of a document are run by functions that this engine never calls.
//!
//! So only `K` and `V` can reach a reset or a hide from a dictionary that is not the widget itself,
//! and no link between objects tells which widget they belong to. The search therefore does not look
//! at links between fields. It reads what the actions do.
//!
//! # The rule
//!
//! Every object of the written document is read, with all the dictionaries inside it, and for each
//! dictionary its `/A` and `/AA` are followed through indirect references and `/Next` chains (a
//! single action or an array) to see whether the chain reaches a `ResetForm` or a `Hide`. A
//! reference is followed as far as the limits below allow, which is further than PDFium follows
//! one.
//!
//! - If the `K`, `V`, `F` or `C` entry of any `/AA`, in any dictionary at all, reaches one, the whole
//!   form is left alone.
//! - For every dictionary that has an `/A` or an `/AA`, the search records which of the two it has,
//!   its box (`/Rect`) and whether its own actions reach one. A widget that PDFium lists with an
//!   action is left alone when a record with the same keys and the same box reaches one, when no
//!   record has them (the file and PDFium do not agree about the widget), and when a record that
//!   reaches one has a `/Rect` that cannot be read and has the same keys. A dictionary with no
//!   `/Rect` key at all (a page, the catalog, a bookmark, a field above its widgets) is not
//!   recorded: PDFium gives a widget such as that an empty box, and the reader never fills one in.
//!   A harmless widget with the box and keys of a harmful one is left alone too. Nothing depends on
//!   where a widget sits in a page,
//!   on the page tree or on the trailer, so none of them can be made to point at another widget.
//! - A reset button that reaches a reset itself blocks nothing else, and a script that does not
//!   reach a reset or a hide through `/Next` blocks nothing, because scripts never run.
//! - A missing or unreadable target, a loop, a chain or nesting that is too deep, more work than the
//!   limits allow, or a document that holds a compressed object stream (whose content is not read)
//!   all count as reaching one, for the whole form.
//!
//! The work is limited, not strictly bounded by the size of the file. Most of it is: each object is
//! read once for each way it is reached, every token read costs one step of a budget that grows with
//! the size of the document, and the search reports progress while it runs. Two things are not
//! counted as they deserve. A string costs one step whatever its length, and a definition with no
//! content costs none. The `/AA` of every dictionary is also read again each time it is met, where
//! an action is read once. So a file made to be slow, with many `/AA` entries that all point at a
//! large string, can keep the search busy for minutes, and the window then stops the engine and
//! closes the document. This is a known limit; it is not a way to change a form.

use std::collections::HashMap;

/// Most steps (tokens read and objects looked up) for a document of no size, and the steps allowed
/// for each byte of the document on top of that.
const BASE_STEPS: u64 = 2_000_000;
const STEPS_PER_BYTE: u64 = 4;

/// Most objects of a written document that are looked at.
const MAX_OBJECTS: usize = 4_000_000;

/// Deepest nesting of arrays and dictionaries inside one object.
const MAX_NEST: usize = 48;

/// Longest chain of actions (`/Next`, arrays of them) that is followed.
const MAX_CHAIN: usize = 32;

/// Most dictionaries with an action that are recorded.
const MAX_RECORDS: usize = 2_000_000;

/// The names of the two actions that change a document when they fire.
const STATE_ACTIONS: [&[u8]; 2] = [b"ResetForm", b"Hide"];

/// The events that PDFium reads from the field above a widget (see the top of this file).
const FIELD_EVENTS: [&[u8]; 4] = [b"K", b"V", b"F", b"C"];

/// What a search found out about a document.
#[derive(Debug, Clone)]
pub struct Scan {
    /// No widget of the form is filled in.
    pub whole_form: bool,
    /// The dictionaries that have actions, when some of them reach a reset or a hide.
    own: Option<Records>,
}

/// The dictionaries with an `/A` or an `/AA`, by which of the two they have and their box.
#[derive(Debug, Default, Clone)]
struct Records {
    /// For each pair of keys and box (in steps of [`CELL`]): whether one of them reaches a reset or
    /// a hide.
    boxes: HashMap<(bool, bool, [i64; 4]), bool>,
    /// For each pair of keys: a dictionary that reaches one has no box that can be read.
    unplaced: [bool; 4],
    count: usize,
}

/// The size of a step in the box of a widget, in points, and how far PDFium's box may be from the
/// file's. A box is looked for in every step that is within the tolerance.
const CELL: f64 = 0.01;
const TOLERANCE: f64 = 0.01;

fn cell(v: f64) -> i64 {
    (v / CELL).floor() as i64
}

fn key_slot(has_a: bool, has_aa: bool) -> usize {
    usize::from(has_a) * 2 + usize::from(has_aa)
}

impl Records {
    fn add(&mut self, has_a: bool, has_aa: bool, rect: Option<[f64; 4]>, reaches: bool) -> R<()> {
        self.count += 1;
        if self.count > MAX_RECORDS {
            return Err(Stop::Whole);
        }
        match rect {
            None => self.unplaced[key_slot(has_a, has_aa)] |= reaches,
            Some(r) => {
                *self
                    .boxes
                    .entry((has_a, has_aa, r.map(cell)))
                    .or_insert(false) |= reaches;
            }
        }
        Ok(())
    }

    fn blocks(&self, has_a: bool, has_aa: bool, rect: [f32; 4]) -> bool {
        if self.unplaced[key_slot(has_a, has_aa)] || !rect.iter().all(|v| v.is_finite()) {
            return true;
        }
        let steps = |v: f32| {
            let v = f64::from(v);
            cell(v - TOLERANCE)..=cell(v + TOLERANCE)
        };
        let mut found = false;
        for left in steps(rect[0]) {
            for bottom in steps(rect[1]) {
                for right in steps(rect[2]) {
                    for top in steps(rect[3]) {
                        match self.boxes.get(&(has_a, has_aa, [left, bottom, right, top])) {
                            Some(true) => return true,
                            Some(false) => found = true,
                            None => {}
                        }
                    }
                }
            }
        }
        // A widget with an action that no record has: the file and PDFium do not agree about it.
        !found
    }
}

impl Scan {
    /// A form that holds nothing that resets or hides.
    pub fn nothing() -> Scan {
        Scan {
            whole_form: false,
            own: None,
        }
    }

    /// A form that is left alone as a whole.
    pub fn everything() -> Scan {
        Scan {
            whole_form: true,
            own: None,
        }
    }

    /// Whether a widget is left alone for what its own actions do. `has_a`, `has_aa` and `rect`
    /// (`[left, bottom, right, top]`) are what PDFium says about the widget.
    pub fn blocks_widget(&self, has_a: bool, has_aa: bool, rect: [f32; 4]) -> bool {
        (has_a || has_aa)
            && self
                .own
                .as_ref()
                .is_some_and(|records| records.blocks(has_a, has_aa, rect))
    }
}

// ---- reading the written document --------------------------------------------------------------

/// The search cannot go on, or found that the whole form is to be left alone.
#[derive(Debug, Clone, Copy)]
enum Stop {
    /// Every widget of the form is left alone.
    Whole,
}

type R<T> = Result<T, Stop>;

#[derive(Debug)]
enum Val {
    Null,
    Bool,
    Num(f64),
    Name(Vec<u8>),
    /// A string. Only a short one is kept, to compare with a name.
    Str(Vec<u8>),
    Array(Vec<Val>),
    Dict(Vec<(Vec<u8>, Val)>),
    Ref(u64),
}

impl Val {
    /// The last entry with this key (the one PDFium keeps when a key is repeated).
    fn get(&self, key: &[u8]) -> Option<&Val> {
        match self {
            Val::Dict(pairs) => pairs.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn is_name(&self, wanted: &[u8]) -> bool {
        matches!(self, Val::Name(n) | Val::Str(n) if n == wanted)
    }
}

/// How much work is left.
struct Budget<'p> {
    steps: u64,
    limit: u64,
    progress: &'p mut dyn FnMut(u32),
}

impl Budget<'_> {
    fn tick(&mut self) -> R<()> {
        self.steps += 1;
        if self.steps > self.limit {
            return Err(Stop::Whole);
        }
        if self.steps.is_multiple_of(1 << 16) {
            (self.progress)((self.steps >> 16) as u32);
        }
        Ok(())
    }
}

/// Whether `b` can be part of a name or a number (it is not white space or a delimiter).
pub(crate) fn is_regular(b: u8) -> bool {
    !matches!(
        b,
        0 | 9
            | 10
            | 12
            | 13
            | 32
            | b'('
            | b')'
            | b'<'
            | b'>'
            | b'['
            | b']'
            | b'{'
            | b'}'
            | b'/'
            | b'%'
    )
}

fn is_space(b: u8) -> bool {
    matches!(b, 0 | 9 | 10 | 12 | 13 | 32)
}

fn skip_space(bytes: &[u8], mut at: usize) -> usize {
    while bytes.get(at).is_some_and(|b| is_space(*b)) {
        at += 1;
    }
    at
}

/// White space and comments.
fn skip_ws(bytes: &[u8], mut at: usize) -> usize {
    loop {
        at = skip_space(bytes, at);
        if bytes.get(at) == Some(&b'%') {
            while bytes.get(at).is_some_and(|b| !matches!(b, 10 | 13)) {
                at += 1;
            }
        } else {
            return at;
        }
    }
}

/// True when `bytes` holds the PDF name `name` (such as `/Hide`) as a whole name: the byte after it
/// is white space or a delimiter, so `/HideToolbar` is not `/Hide`.
pub(crate) fn has_name(bytes: &[u8], name: &[u8]) -> bool {
    let mut from = 0;
    while let Some(at) = bytes[from..]
        .windows(name.len())
        .position(|window| window == name)
    {
        let end = from + at + name.len();
        if bytes.get(end).is_none_or(|b| !is_regular(*b)) {
            return true;
        }
        from = from + at + 1;
    }
    false
}

/// Whether a document written out holds an action that changes a document when it fires.
pub(crate) fn holds_state_action(bytes: &[u8]) -> bool {
    [&b"/ResetForm"[..], &b"/Hide"[..]]
        .iter()
        .any(|name| has_name(bytes, name))
}

/// Whether a written document needs the search at all: it holds a reset or a hide, or a compressed
/// object stream (whose content is not read, so it may hold either).
pub(crate) fn needs_scan(bytes: &[u8]) -> bool {
    holds_state_action(bytes) || has_name(bytes, b"/ObjStm")
}

/// The unsigned number that starts at `at`, and where it ends. At most ten digits.
fn number_at(bytes: &[u8], at: usize) -> Option<(u64, usize)> {
    let digits = bytes[at.min(bytes.len())..]
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .count();
    if digits == 0 || digits > 10 {
        return None;
    }
    let text = std::str::from_utf8(&bytes[at..at + digits]).ok()?;
    Some((text.parse().ok()?, at + digits))
}

/// The number of the object whose header (`12 0 obj`) starts at `at`, and where its body starts.
fn header_at(bytes: &[u8], at: usize) -> Option<(u64, usize)> {
    let (number, end) = number_at(bytes, at)?;
    let next = skip_space(bytes, end);
    if next == end {
        return None;
    }
    let (_, end) = number_at(bytes, next)?;
    let next = skip_space(bytes, end);
    if next == end || !bytes[next..].starts_with(b"obj") {
        return None;
    }
    bytes
        .get(next + 3)
        .is_none_or(|b| !is_regular(*b))
        .then_some((number, next + 3))
}

/// The place and number of the first object header at or after `from` that starts a line.
fn next_header(bytes: &[u8], from: usize) -> Option<(usize, u64)> {
    (from..bytes.len()).find_map(|at| {
        let line_start = at == 0 || matches!(bytes[at - 1], b'\r' | b'\n');
        if line_start && bytes[at].is_ascii_digit() {
            header_at(bytes, at).map(|(number, _)| (at, number))
        } else {
            None
        }
    })
}

/// The objects of a document as PDFium writes it: each with its number and the place of its bytes
/// (up to the header of the next one). A writer puts every object on lines of its own: a string in
/// a written dictionary has its line breaks and brackets escaped, so a header at the start of a line
/// is the start of an object, whatever the strings in the objects before it say. A stream can hold
/// any bytes, and a header-like line in one only cuts the stream into more parts and adds objects
/// that are read as well; a number that is defined more than once is followed through every
/// definition, so a fake one cannot hide the real one.
///
/// The objects are found one at a time, so a file of nothing but headers costs no memory beyond the
/// bytes themselves (the list that is kept is capped by [`MAX_OBJECTS`]).
fn written_objects(bytes: &[u8]) -> WrittenObjects<'_> {
    WrittenObjects {
        bytes,
        next: next_header(bytes, 0),
    }
}

struct WrittenObjects<'a> {
    bytes: &'a [u8],
    /// The header of the object that comes next.
    next: Option<(usize, u64)>,
}

impl Iterator for WrittenObjects<'_> {
    /// Number, start and end.
    type Item = (u64, usize, usize);

    fn next(&mut self) -> Option<Self::Item> {
        let (start, number) = self.next.take()?;
        self.next = next_header(self.bytes, start + 1);
        let end = self.next.map_or(self.bytes.len(), |(next, _)| next);
        Some((number, start, end))
    }
}

/// Whether the bytes of an object name something this search looks at: an action (`/A`, `/AA`) or a
/// compressed object stream. A name with a `#` escape in it is looked at as well.
fn mentions_actions(body: &[u8]) -> bool {
    let mut at = 0;
    while let Some(p) = body[at..].iter().position(|b| *b == b'/') {
        let start = at + p + 1;
        let len = body[start..].iter().take_while(|b| is_regular(**b)).count();
        let name = &body[start..start + len];
        if matches!(name, b"A" | b"AA" | b"ObjStm") || name.contains(&b'#') {
            return true;
        }
        at = start;
    }
    false
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// The name that starts after the slash at `at - 1`, with `#xx` escapes undone.
fn parse_name(bytes: &[u8], at: &mut usize) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(&b) = bytes.get(*at) {
        if !is_regular(b) {
            break;
        }
        if b == b'#' {
            if let (Some(h), Some(l)) = (
                bytes.get(*at + 1).and_then(|b| hex(*b)),
                bytes.get(*at + 2).and_then(|b| hex(*b)),
            ) {
                out.push(h * 16 + l);
                *at += 3;
                continue;
            }
        }
        out.push(b);
        *at += 1;
    }
    out
}

fn parse_value(bytes: &[u8], at: &mut usize, depth: usize, budget: &mut Budget<'_>) -> R<Val> {
    budget.tick()?;
    if depth > MAX_NEST {
        return Err(Stop::Whole);
    }
    *at = skip_ws(bytes, *at);
    let Some(&c) = bytes.get(*at) else {
        return Err(Stop::Whole);
    };
    match c {
        b'<' if bytes.get(*at + 1) == Some(&b'<') => {
            *at += 2;
            let mut pairs = Vec::new();
            loop {
                *at = skip_ws(bytes, *at);
                if bytes[(*at).min(bytes.len())..].starts_with(b">>") {
                    *at += 2;
                    return Ok(Val::Dict(pairs));
                }
                if bytes.get(*at) != Some(&b'/') {
                    return Err(Stop::Whole);
                }
                *at += 1;
                let key = parse_name(bytes, at);
                let value = parse_value(bytes, at, depth + 1, budget)?;
                pairs.push((key, value));
            }
        }
        b'<' => {
            // A hexadecimal string.
            let end = bytes[*at..]
                .iter()
                .position(|b| *b == b'>')
                .ok_or(Stop::Whole)?;
            *at += end + 1;
            Ok(Val::Str(Vec::new()))
        }
        b'[' => {
            *at += 1;
            let mut items = Vec::new();
            loop {
                *at = skip_ws(bytes, *at);
                if bytes.get(*at) == Some(&b']') {
                    *at += 1;
                    return Ok(Val::Array(items));
                }
                items.push(parse_value(bytes, at, depth + 1, budget)?);
            }
        }
        b'(' => {
            let start = *at + 1;
            let mut level = 1usize;
            let mut i = start;
            while level > 0 {
                match bytes.get(i) {
                    None => return Err(Stop::Whole),
                    Some(b'\\') => i += 1,
                    Some(b'(') => level += 1,
                    Some(b')') => level -= 1,
                    Some(_) => {}
                }
                i += 1;
            }
            let text = &bytes[start..i - 1];
            *at = i;
            Ok(Val::Str(if text.len() <= 64 {
                text.to_vec()
            } else {
                Vec::new()
            }))
        }
        b'/' => {
            *at += 1;
            Ok(Val::Name(parse_name(bytes, at)))
        }
        _ => {
            let len = bytes[*at..].iter().take_while(|b| is_regular(**b)).count();
            if len == 0 {
                return Err(Stop::Whole);
            }
            let token = &bytes[*at..*at + len];
            let after = *at + len;
            match token {
                b"true" | b"false" => {
                    *at = after;
                    Ok(Val::Bool)
                }
                b"null" => {
                    *at = after;
                    Ok(Val::Null)
                }
                _ => {
                    // Only what a PDF number is made of: `inf`, `nan` and `1e3` are not numbers.
                    if !token
                        .iter()
                        .all(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.'))
                    {
                        return Err(Stop::Whole);
                    }
                    let text = std::str::from_utf8(token).map_err(|_| Stop::Whole)?;
                    let number: f64 = text.parse().map_err(|_| Stop::Whole)?;
                    // `12 0 R`: a reference.
                    if token.iter().all(|b| b.is_ascii_digit()) && len <= 10 {
                        let second = skip_space(bytes, after);
                        if second > after {
                            if let Some((_, end)) = number_at(bytes, second) {
                                let third = skip_space(bytes, end);
                                if third > end
                                    && bytes.get(third) == Some(&b'R')
                                    && bytes.get(third + 1).is_none_or(|b| !is_regular(*b))
                                {
                                    *at = third + 1;
                                    return Ok(Val::Ref(number as u64));
                                }
                            }
                        }
                    }
                    *at = after;
                    Ok(Val::Num(number))
                }
            }
        }
    }
}

enum Memo {
    Busy,
    Done(bool),
}

struct Scanner<'a, 'p> {
    bytes: &'a [u8],
    /// Number, start and end of every object, by number.
    index: Vec<(u64, u32, u32)>,
    budget: Budget<'p>,
    memo: HashMap<u64, Memo>,
    /// The dictionaries that have actions.
    records: Records,
}

/// Searches a written document. Everything that cannot be settled counts against the form.
pub fn scan(bytes: &[u8], progress: &mut dyn FnMut(u32)) -> Scan {
    let limit = BASE_STEPS + STEPS_PER_BYTE * bytes.len() as u64;
    let mut scanner = Scanner {
        bytes,
        index: Vec::new(),
        budget: Budget {
            steps: 0,
            limit,
            progress,
        },
        memo: HashMap::new(),
        records: Records::default(),
    };
    match scanner.run() {
        Ok(scan) => scan,
        Err(_) => Scan::everything(),
    }
}

impl Scanner<'_, '_> {
    fn run(&mut self) -> R<Scan> {
        if self.bytes.len() >= u32::MAX as usize {
            return Err(Stop::Whole);
        }
        for (number, start, end) in written_objects(self.bytes) {
            if self.index.len() >= MAX_OBJECTS {
                return Err(Stop::Whole);
            }
            self.index.push((number, start as u32, end as u32));
            if self.index.len().is_multiple_of(4096) {
                self.budget.tick()?;
            }
        }
        self.index.sort_by_key(|(number, _, _)| *number);

        // Every object that has an action or is a compressed stream, with every dictionary inside
        // it.
        let mut any_own = false;
        for i in 0..self.index.len() {
            let (_, start, end) = self.index[i];
            let body = &self.bytes[start as usize..end as usize];
            if !mentions_actions(body) {
                continue;
            }
            let value = self.load(start, end)?;
            if let Val::Dict(_) = &value {
                if value.get(b"Type").is_some_and(|t| t.is_name(b"ObjStm")) {
                    // Its content is compressed and is not read: it may hold anything.
                    return Err(Stop::Whole);
                }
            }
            any_own |= self.walk(&value)?;
        }
        if !any_own {
            return Ok(Scan::nothing());
        }
        Ok(Scan {
            whole_form: false,
            own: Some(std::mem::take(&mut self.records)),
        })
    }

    /// The definitions of an object number (more than one when a number is defined twice).
    fn defs(&self, number: u64) -> Vec<(u32, u32)> {
        let first = self.index.partition_point(|(n, _, _)| *n < number);
        self.index[first..]
            .iter()
            .take_while(|(n, _, _)| *n == number)
            .map(|(_, s, e)| (*s, *e))
            .collect()
    }

    /// The first value of an object: its dictionary for a stream.
    fn load(&mut self, start: u32, end: u32) -> R<Val> {
        let body = &self.bytes[start as usize..end as usize];
        let (_, mut at) = header_at(body, 0).ok_or(Stop::Whole)?;
        let after = skip_ws(body, at);
        if body[after.min(body.len())..].starts_with(b"endobj") {
            return Ok(Val::Null);
        }
        parse_value(body, &mut at, 0, &mut self.budget)
    }

    /// Every value that a reference stands for. A number with no definition is an error.
    fn load_ref(&mut self, number: u64) -> R<Vec<Val>> {
        let defs = self.defs(number);
        if defs.is_empty() {
            return Err(Stop::Whole);
        }
        let mut out = Vec::with_capacity(defs.len());
        for (start, end) in defs {
            out.push(self.load(start, end)?);
        }
        Ok(out)
    }

    /// Looks at every dictionary inside `value`. Returns whether the own actions of one of them
    /// (`/A`, or an event of `/AA`) reach a reset or a hide. Stops the search when one of the events
    /// that PDFium reads from the field above a widget does.
    fn walk(&mut self, value: &Val) -> R<bool> {
        let mut own = false;
        let mut stack = vec![value];
        while let Some(v) = stack.pop() {
            self.budget.tick()?;
            match v {
                Val::Dict(pairs) => {
                    let (has_a, has_aa) = (v.get(b"A").is_some(), v.get(b"AA").is_some());
                    if has_a || has_aa {
                        let reaches = self.own_actions(v)?;
                        own |= reaches;
                        // A dictionary with no `/Rect` key is no widget that PDFium gives a box: it
                        // gives such a widget an empty one, and the reader never fills it in. A
                        // `/Rect` that is there and cannot be read is recorded as a box nobody
                        // has, so that it still fails closed.
                        if v.get(b"Rect").is_some() {
                            let rect = self.rect_of(v)?;
                            self.records.add(has_a, has_aa, rect, reaches)?;
                        }
                    }
                    stack.extend(pairs.iter().map(|(_, v)| v));
                }
                Val::Array(items) => stack.extend(items.iter()),
                _ => {}
            }
        }
        Ok(own)
    }

    /// Whether `/A` or an event of `/AA` of this dictionary reaches a reset or a hide. An event of
    /// the kinds that are read from the field above a widget stops the search instead.
    fn own_actions(&mut self, dict: &Val) -> R<bool> {
        let mut own = false;
        if let Some(a) = dict.get(b"A") {
            own |= self.reaches(a, 0)?;
        }
        if let Some(aa) = dict.get(b"AA") {
            own |= self.events(aa, 0)?;
        }
        Ok(own)
    }

    /// The events of an `/AA`, which is a dictionary or a reference to one, as many references deep
    /// as [`MAX_CHAIN`] allows.
    fn events(&mut self, aa: &Val, depth: usize) -> R<bool> {
        let pairs = match aa {
            Val::Dict(pairs) => pairs,
            Val::Ref(number) => {
                if depth > MAX_CHAIN {
                    return Err(Stop::Whole);
                }
                let mut own = false;
                for target in self.load_ref(*number)? {
                    own |= self.events(&target, depth + 1)?;
                }
                return Ok(own);
            }
            _ => return Ok(false),
        };
        let mut own = false;
        for (key, action) in pairs {
            if self.reaches(action, 0)? {
                if FIELD_EVENTS.contains(&key.as_slice()) {
                    return Err(Stop::Whole);
                }
                own = true;
            }
        }
        Ok(own)
    }

    /// Whether an action (a dictionary, a reference to one, or an array of them) reaches a reset or a
    /// hide, itself or through `/Next`.
    fn reaches(&mut self, value: &Val, depth: usize) -> R<bool> {
        if depth > MAX_CHAIN {
            return Err(Stop::Whole);
        }
        self.budget.tick()?;
        match value {
            Val::Dict(_) => {
                let mut found = false;
                if let Some(s) = value.get(b"S") {
                    found |= self.is_state_action(s, depth)?;
                }
                if let Some(next) = value.get(b"Next") {
                    found |= self.reaches(next, depth + 1)?;
                }
                Ok(found)
            }
            Val::Array(items) => {
                let mut found = false;
                for item in items {
                    found |= self.reaches(item, depth + 1)?;
                }
                Ok(found)
            }
            Val::Ref(number) => {
                match self.memo.get(number) {
                    Some(Memo::Done(found)) => return Ok(*found),
                    Some(Memo::Busy) => return Err(Stop::Whole),
                    None => {}
                }
                self.memo.insert(*number, Memo::Busy);
                let mut found = false;
                for target in self.load_ref(*number)? {
                    found |= self.reaches(&target, depth)?;
                }
                self.memo.insert(*number, Memo::Done(found));
                Ok(found)
            }
            _ => Ok(false),
        }
    }

    /// Whether the type of an action (`/S`) is a reset or a hide. It can be given as a reference.
    fn is_state_action(&mut self, s: &Val, depth: usize) -> R<bool> {
        match s {
            Val::Name(_) | Val::Str(_) => Ok(STATE_ACTIONS.iter().any(|n| s.is_name(n))),
            Val::Ref(number) => {
                if depth > MAX_CHAIN {
                    return Err(Stop::Whole);
                }
                let mut found = false;
                for target in self.load_ref(*number)? {
                    found |= self.is_state_action(&target, depth + 1)?;
                }
                Ok(found)
            }
            _ => Ok(false),
        }
    }
}

impl Scanner<'_, '_> {
    /// `[left, bottom, right, top]` of the `/Rect` of a dictionary: four numbers, given as an array
    /// or as a reference to one, where each number may also be a reference. A reference is followed
    /// one level, as PDFium reads a box; a reference to a reference, a missing or twice defined
    /// object, or anything that is not a number gives `None`.
    fn rect_of(&mut self, annot: &Val) -> R<Option<[f64; 4]>> {
        let Some(rect) = annot.get(b"Rect") else {
            return Ok(None);
        };
        match rect {
            Val::Array(items) => self.box_of(items),
            Val::Ref(number) => match self.only_value(*number)? {
                Some(Val::Array(items)) => self.box_of(&items),
                _ => Ok(None),
            },
            _ => Ok(None),
        }
    }

    fn box_of(&mut self, items: &[Val]) -> R<Option<[f64; 4]>> {
        if items.len() != 4 {
            return Ok(None);
        }
        let mut n = [0f64; 4];
        for (slot, item) in n.iter_mut().zip(items) {
            let value = match item {
                Val::Num(v) => Some(*v),
                Val::Ref(number) => match self.only_value(*number)? {
                    Some(Val::Num(v)) => Some(v),
                    _ => None,
                },
                _ => None,
            };
            let Some(value) = value else {
                return Ok(None);
            };
            *slot = value;
        }
        Ok(Some([
            n[0].min(n[2]),
            n[1].min(n[3]),
            n[0].max(n[2]),
            n[1].max(n[3]),
        ]))
    }

    /// The value of an object that is defined exactly once, or `None`.
    fn only_value(&mut self, number: u64) -> R<Option<Val>> {
        let defs = self.defs(number);
        match defs.as_slice() {
            [(start, end)] => Ok(Some(self.load(*start, *end)?)),
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A written document of the given object bodies (object `i + 1` is `bodies[i]`).
    fn doc(bodies: &[&str]) -> Vec<u8> {
        let mut out = String::from("%PDF-1.5\r\n");
        for (i, body) in bodies.iter().enumerate() {
            out.push_str(&format!("{} 0 obj\r\n{body}\r\nendobj\r\n", i + 1));
        }
        out.push_str("trailer\r\n<< /Root 1 0 R >>\r\nstartxref\r\n0\r\n%%EOF\r\n");
        out.into_bytes()
    }

    fn run(bytes: &[u8]) -> Scan {
        scan(bytes, &mut |_| {})
    }

    const RECT: [f32; 4] = [10.0, 10.0, 110.0, 30.0];
    const WIDGET: &str = "/Type /Annot /Subtype /Widget /Rect [10 10 110 30]";

    /// Catalog, tree and one page with the given annotations (object 4 and up).
    fn one_page(annots: &[&str]) -> Vec<u8> {
        let refs: String = (0..annots.len())
            .map(|i| format!("{} 0 R ", 4 + i))
            .collect();
        let mut bodies = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            format!("<< /Type /Page /Parent 2 0 R /Annots [{refs}] >>"),
        ];
        bodies.extend(annots.iter().map(|s| s.to_string()));
        let bodies: Vec<&str> = bodies.iter().map(String::as_str).collect();
        doc(&bodies)
    }

    #[test]
    fn only_the_widget_with_a_reaching_action_is_blocked() {
        let bytes = one_page(&[
            &format!("<< {WIDGET} /A << /S /ResetForm >> >>"),
            &format!("<< {WIDGET} /AA << /Fo << /S /JavaScript /JS (1;) >> >> >>"),
            &format!("<< {WIDGET} >>"),
        ]);
        let scan = run(&bytes);
        assert!(!scan.whole_form);
        assert!(scan.blocks_widget(true, false, RECT));
        assert!(!scan.blocks_widget(false, true, RECT));
        assert!(!scan.blocks_widget(false, false, RECT));
    }

    #[test]
    fn a_document_with_nothing_that_reaches_blocks_nothing() {
        let bytes = one_page(&[&format!(
            "<< {WIDGET} /AA << /F << /S /JavaScript /JS (1;) >> /K << /S /JavaScript /JS (2;) >> >> >>"
        )]);
        let scan = run(&bytes);
        assert!(!scan.whole_form);
        assert!(!scan.blocks_widget(false, true, RECT));
    }

    #[test]
    fn an_event_of_the_field_kind_that_reaches_blocks_the_whole_form_in_any_dictionary() {
        for event in ["K", "V", "F", "C"] {
            // On a dictionary that is no widget and that no page shows.
            let bytes = doc(&[
                "<< /Type /Catalog >>",
                &format!("<< /FT /Tx /T (t) /AA << /{event} << /S /ResetForm >> >> >>"),
            ]);
            assert!(run(&bytes).whole_form, "{event}");
            // Nested inside another dictionary.
            let bytes = doc(&[&format!(
                "<< /Outer << /Inner << /AA << /{event} << /S /Hide >> >> >> >> >>"
            )]);
            assert!(run(&bytes).whole_form, "{event} nested");
        }
        // Another event of a dictionary that is no widget is not a block.
        let bytes = doc(&["<< /AA << /Fo << /S /ResetForm >> >> >>"]);
        assert!(!run(&bytes).whole_form);
    }

    #[test]
    fn an_action_is_followed_through_references_next_and_arrays() {
        let whole = |bytes: Vec<u8>| run(&bytes).whole_form;
        assert!(whole(doc(&[
            "<< /AA << /K 2 0 R >> >>",
            "<< /S /JavaScript /Next 3 0 R >>",
            "<< /S /ResetForm >>"
        ])));
        assert!(whole(doc(&[
            "<< /AA 2 0 R >>",
            "<< /V << /S /JavaScript /Next [ << /S /URI >> 3 0 R ] >> >>",
            "<< /S /Hide /T (a) >>"
        ])));
        assert!(whole(doc(&[
            "<< /AA << /K << /S /Named /Next 2 0 R >> >> >>",
            "[ << /S /URI >> 3 0 R ]",
            "<< /S /ResetForm >>"
        ])));
        // A stream's dictionary stands for the stream.
        assert!(whole(doc(&[
            "<< /AA << /K 2 0 R >> >>",
            "<< /S /ResetForm /Length 0 >>\r\nstream\r\n\r\nendstream"
        ])));
        // Names that are only like the two.
        assert!(!whole(doc(&[
            "<< /AA << /K << /S /ResetFormFlags /Next << /S /HideToolbar >> >> >> >>"
        ])));
        // Names written with escapes are read as the name they are.
        assert!(whole(doc(&["<< /A#41 << /K << /S /Reset#46orm >> >> >>"])));
    }

    #[test]
    fn what_cannot_be_settled_counts_against_the_whole_form() {
        let whole = |bytes: Vec<u8>| run(&bytes).whole_form;
        // A loop.
        assert!(whole(doc(&[
            "<< /AA << /K 2 0 R >> >>",
            "<< /S /JavaScript /Next 3 0 R >>",
            "<< /S /JavaScript /Next 2 0 R >>"
        ])));
        // A reference to nothing, as the action and as `/Next`.
        assert!(whole(doc(&["<< /AA << /K 9 0 R >> >>"])));
        assert!(whole(doc(&[
            "<< /AA << /K << /S /JavaScript /Next 9 0 R >> >> >>"
        ])));
        // A chain that is too long.
        let mut bodies = vec!["<< /AA << /K 2 0 R >> >>".to_string()];
        for i in 0..MAX_CHAIN + 8 {
            bodies.push(format!("<< /S /JavaScript /Next {} 0 R >>", i + 3));
        }
        bodies.push("<< /S /JavaScript >>".to_string());
        let refs: Vec<&str> = bodies.iter().map(String::as_str).collect();
        assert!(whole(doc(&refs)));
        // Nesting that is too deep, and a value that does not end.
        let deep = format!(
            "<< /X {}1{} >>",
            "[".repeat(MAX_NEST + 5),
            "]".repeat(MAX_NEST + 5)
        );
        assert!(whole(doc(&[&format!("<< /AA << >> /Y {deep} >>")])));
        assert!(whole(doc(&["<< /AA << /K << /S /ResetForm "])));
        // A compressed object stream may hold anything.
        assert!(whole(doc(&[
            "<< /Type /ObjStm /N 1 /First 4 /Length 0 >>\r\nstream\r\n\r\nendstream"
        ])));
    }

    #[test]
    fn a_number_defined_twice_is_followed_through_every_definition() {
        // A header-like line in a stream cannot hide the real action behind a fake one, whichever
        // of the two comes first.
        let real = "2 0 obj\r\n<< /S /ResetForm >>\r\nendobj\r\n";
        let fake = "2 0 obj\r\n<< /S /Named >>\r\nendobj\r\n";
        for objects in [format!("{real}{fake}"), format!("{fake}{real}")] {
            let bytes = format!(
                "1 0 obj\r\n<< /AA << /K 2 0 R >> >>\r\nendobj\r\n{objects}trailer\r\n<< /Root 1 0 R >>\r\n"
            );
            assert!(run(bytes.as_bytes()).whole_form);
        }
    }

    #[test]
    fn a_widget_is_matched_by_its_keys_and_its_box_and_nothing_else() {
        // Nothing here says which page a widget is on or where in a list: an entry that is no
        // widget, a decoy with the same box, and a hole in the page tree change nothing.
        let bytes = doc(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [99 0 R 3 0 R 98 0 R] /Count 3 >>",
            "<< /Type /Page /Annots [4 0 R 5 0 R 6 0 R] >>",
            // A decoy with the box and keys of the harmful widget, and a string for a subtype.
            "<< /Type /Annot /Subtype (Widget) /Rect [10 10 110 30] /A << /S /URI >> >>",
            "<< /Type /Annot /Subtype /Widget /Rect [10 10 110 30] /A << /S /Hide >> >>",
            "<< /Type /Annot /Subtype /Widget /Rect [10 40 110 60] /A << /S /URI >> >>",
            "<< /Type /Annot /Subtype /Widget /Rect [0 0 0 0] /A << /S /URI >> >>",
        ]);
        let scan = run(&bytes);
        assert!(!scan.whole_form);
        // The harmful one, and the decoy that looks like it.
        assert!(scan.blocks_widget(true, false, RECT));
        // Another box, with an action that does not reach one: free.
        assert!(!scan.blocks_widget(true, false, [10.0, 40.0, 110.0, 60.0]));
        // A box within a hundredth of a point is the same box, for either of them.
        assert!(scan.blocks_widget(true, false, [10.004, 9.996, 110.0, 30.0]));
        assert!(!scan.blocks_widget(true, false, [10.004, 40.004, 109.996, 60.0]));
        // What PDFium says has to agree with the file: another box, another set of keys, or a box
        // that is not a number is a widget no record has, and it is left alone.
        assert!(scan.blocks_widget(true, false, [0.0, 0.0, 5.0, 5.0]));
        assert!(scan.blocks_widget(false, true, [10.0, 40.0, 110.0, 60.0]));
        assert!(scan.blocks_widget(true, false, [f32::NAN, 0.0, 5.0, 5.0]));
        assert!(scan.blocks_widget(true, false, [f32::NAN; 4]));
        // No action, nothing to settle.
        assert!(!scan.blocks_widget(false, false, [0.0, 0.0, 5.0, 5.0]));
    }

    #[test]
    fn a_harmful_dictionary_with_a_box_that_cannot_be_read_blocks_every_widget_with_its_keys() {
        for rect in [
            "/Rect [10 10 110]",
            "/Rect [10 10 110 (x)]",
            "/Rect 7 0 R",
            "/Rect 8 0 R",
            "/Rect [10 10 110 7 0 R]",
            "/Rect (box)",
            "/Rect null",
        ] {
            let bytes = one_page(&[
                &format!(
                    "<< /Type /Annot /Subtype /Widget {rect} /AA << /Fo << /S /ResetForm >> >> >>"
                ),
                &format!("<< {WIDGET} /AA << /Fo << /S /URI >> >> >>"),
                &format!("<< {WIDGET} /A << /S /URI >> >>"),
                // Object 7 is a name, 8 a reference to a reference.
                "/Name",
                "7 0 R",
            ]);
            let scan = run(&bytes);
            assert!(!scan.whole_form, "{rect}");
            assert!(scan.blocks_widget(false, true, RECT), "{rect}");
            // The same box, the other key: no harmful dictionary is without a box there.
            assert!(!scan.blocks_widget(true, false, RECT), "{rect}");
        }
    }

    #[test]
    fn a_harmful_dictionary_with_no_box_key_blocks_no_widget() {
        // A page, the catalog, a bookmark, a field above its widgets: none of them is a widget, and
        // PDFium gives no box to a widget that has no `/Rect`.
        for (what, objects) in [
            (
                "page",
                vec![
                    "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
                    "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
                    "<< /Type /Page /Parent 2 0 R /Annots [4 0 R] /AA << /O << /S /Hide >> >> >>"
                        .to_string(),
                    format!("<< {WIDGET} /AA << /Fo << /S /URI >> >> >>"),
                ],
            ),
            (
                "catalog",
                vec![
                    "<< /Type /Catalog /AA << /WC << /S /ResetForm >> >> >>".to_string(),
                    format!("<< {WIDGET} /AA << /Fo << /S /URI >> >> >>"),
                ],
            ),
            (
                "bookmark",
                vec![
                    "<< /Title (b) /A << /S /ResetForm >> >>".to_string(),
                    format!("<< {WIDGET} /A << /S /URI >> >>"),
                ],
            ),
            (
                "parent",
                vec![
                    "<< /FT /Tx /T (t) /AA << /Fo << /S /ResetForm >> >> /Kids [2 0 R] >>"
                        .to_string(),
                    format!("<< {WIDGET} /Parent 1 0 R /AA << /Fo << /S /URI >> >> >>"),
                ],
            ),
        ] {
            let refs: Vec<&str> = objects.iter().map(String::as_str).collect();
            let scan = run(&doc(&refs));
            assert!(!scan.whole_form, "{what}");
            let (has_a, has_aa) = (what == "bookmark", what != "bookmark");
            assert!(!scan.blocks_widget(has_a, has_aa, RECT), "{what}");
        }
    }

    #[test]
    fn a_box_given_by_reference_is_the_box_of_the_widget() {
        for rect in [
            "/Rect 6 0 R",
            "/Rect [7 0 R 8 0 R 9 0 R 10 0 R]",
            "/Rect [10 10 110 10 0 R]",
        ] {
            let bytes = one_page(&[
                &format!("<< /Type /Annot /Subtype /Widget {rect} /AA << /Fo << /S /URI >> >> >>"),
                &format!("<< {WIDGET} /A << /S /ResetForm >> >>"),
                "[10 10 110 30]",
                "10",
                "10",
                "110",
                "30",
            ]);
            let scan = run(&bytes);
            assert!(!scan.whole_form, "{rect}");
            assert!(!scan.blocks_widget(false, true, RECT), "{rect}");
            assert!(scan.blocks_widget(true, false, RECT), "{rect}");
        }
    }

    #[test]
    fn a_number_that_is_not_a_pdf_number_cannot_be_settled() {
        for number in ["inf", "nan", "1e3", "--", "1.2.3"] {
            let bytes = one_page(&[&format!(
                "<< /Type /Annot /Subtype /Widget /Rect [10 10 110 {number}] /A << /S /ResetForm >> >>"
            )]);
            assert!(run(&bytes).whole_form, "{number}");
        }
    }

    #[test]
    fn too_many_records_cannot_be_settled() {
        let mut records = Records {
            count: MAX_RECORDS,
            ..Records::default()
        };
        assert!(records.add(true, false, None, false).is_err());
    }

    #[test]
    fn an_aa_given_as_a_reference_to_a_reference_is_read() {
        let bytes = doc(&[
            "<< /Type /Annot /Subtype /Widget /Rect [10 10 110 30] /AA 2 0 R >>",
            "3 0 R",
            "<< /Fo << /S /ResetForm >> >>",
        ]);
        let scan = run(&bytes);
        assert!(!scan.whole_form);
        assert!(scan.blocks_widget(false, true, RECT));
        // The events that PDFium reads from the field above a widget are found the same way.
        for event in ["K", "V"] {
            let bytes = doc(&[
                "<< /Type /Annot /Subtype /Widget /Rect [10 10 110 30] /AA 2 0 R >>",
                "3 0 R",
                &format!("<< /{event} << /S /Hide >> >>"),
            ]);
            assert!(run(&bytes).whole_form, "{event}");
        }
        // A reference that comes back to itself cannot be settled.
        let bytes = doc(&["<< /AA 2 0 R >>", "3 0 R", "2 0 R"]);
        assert!(run(&bytes).whole_form);
    }

    #[test]
    fn a_trailer_that_is_not_read_makes_no_difference() {
        // The trailer of a written document plays no part: a fake one inside a stream, a real one
        // that cannot be read and no trailer at all give the same answer.
        let deep = format!("{}1{}", "[".repeat(MAX_NEST + 7), "]".repeat(MAX_NEST + 7));
        let objects =
            "1 0 obj\r\n<< /Type /Annot /Subtype /Widget /Rect [10 10 110 30] /A << /S /ResetForm >> >>\r\nendobj\r\n\
             2 0 obj\r\n<< /Type /Metadata /Length 40 >>\r\nstream\r\ntrailer << /Root 900 0 R >>\r\n\
             900 0 obj\r\n<< /Type /Catalog >>\r\nendobj\r\nendstream\r\nendobj\r\n"
            ;
        for trailer in [
            format!("trailer\r\n<< /Root 1 0 R /Z {deep} >>\r\n"),
            "trailer\r\n<< /Root".to_string(),
            String::new(),
        ] {
            let bytes = format!("{objects}{trailer}");
            let scan = run(bytes.as_bytes());
            assert!(!scan.whole_form);
            assert!(scan.blocks_widget(true, false, RECT));
        }
    }

    #[test]
    fn a_stream_full_of_the_word_trailer_costs_no_more_than_any_other_bytes() {
        // A reset is there so that the search runs; the bytes that follow are a stream.
        let mut bytes = doc(&["<< /Type /Annot /Rect [1 1 2 2] /A << /S /ResetForm >> >>"]);
        bytes.extend_from_slice(&b"trailer(".repeat(1 << 20));
        let started = std::time::Instant::now();
        let scan = run(&bytes);
        assert!(!scan.whole_form);
        assert!(started.elapsed().as_secs() < 5, "{:?}", started.elapsed());
    }

    #[test]
    fn long_strings_and_odd_escapes_do_not_confuse_the_reading() {
        let long = "x".repeat(5000);
        let bytes = one_page(&[&format!(
            "<< {WIDGET} /TU ({long}) /A << /S /JavaScript /JS (\\(1\\) \\\\ ) >> /Q [1 0 R 2 3 4 R] >>"
        )]);
        let scan = run(&bytes);
        assert!(!scan.whole_form);
        assert!(!scan.blocks_widget(true, false, RECT));
    }

    #[test]
    fn the_names_of_a_reset_and_a_hide_are_found_whole_and_only_whole() {
        assert!(holds_state_action(b"<</S/ResetForm>>"));
        assert!(holds_state_action(b"<< /S /ResetForm /Fields [] >>"));
        assert!(holds_state_action(b"<</S/Hide/T(a)>>"));
        assert!(!holds_state_action(
            b"<</ViewerPreferences<</HideToolbar true/HideMenubar true>>>>"
        ));
        assert!(!holds_state_action(b"/ResetFormFlags /HideWindowUI"));
        assert!(!holds_state_action(b"nothing"));
        assert!(needs_scan(b"<< /Type /ObjStm >>"));
    }
}
