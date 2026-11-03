//! Reading the form of a document: its fields by name, the kind of each, what it holds, and which
//! of them the designer may change.
//!
//! The designer changes a field only when it can write all of it again without losing anything it
//! does not know: the field is at the top of the form's list (not inside another field), has a name
//! without a dot, a kind the designer makes, widgets that are only widgets, and no action or script.
//! Every other field is listed so the window can show it, and left alone.

use std::collections::{HashMap, HashSet};

use partridge_protocol::{DesignKind, DesignProps, DesignedWidget};

use super::syntax::{
    array_items, decode_text, entries, get, parse_box, parse_f32, parse_int, parse_name, parse_ref,
    skip_ws,
};
use super::{odd, rules, Acro, Author, AuthorError, FieldList, PageAnnots, Place};

// Bits of the field flags (`/Ff`).
pub(crate) const FF_READ_ONLY: u32 = 1;
pub(crate) const FF_REQUIRED: u32 = 1 << 1;
pub(crate) const FF_MULTILINE: u32 = 1 << 12;
pub(crate) const FF_PASSWORD: u32 = 1 << 13;
pub(crate) const FF_NO_TOGGLE_TO_OFF: u32 = 1 << 14;
pub(crate) const FF_RADIO: u32 = 1 << 15;
pub(crate) const FF_PUSHBUTTON: u32 = 1 << 16;
pub(crate) const FF_COMBO: u32 = 1 << 17;
pub(crate) const FF_EDIT: u32 = 1 << 18;
pub(crate) const FF_MULTISELECT: u32 = 1 << 21;
pub(crate) const FF_COMB: u32 = 1 << 24;

/// Most field objects followed, and the deepest nesting of them.
const MAX_FIELDS: usize = 200_000;
const MAX_FIELD_DEPTH: usize = 32;

pub(crate) struct Node {
    /// The full name: the names of the parents and the field, joined by dots.
    pub name: String,
    pub top: bool,
    /// The kids that are fields themselves.
    pub fields: Vec<u32>,
}

pub(crate) struct Walk {
    pub nodes: HashMap<u32, Node>,
    /// The fields at the top of the form's list, in its order.
    pub top: Vec<u32>,
    /// Every name a field has, and every part of a name before a dot.
    pub taken: HashSet<String>,
}

impl Walk {
    pub fn take(&mut self, name: &str) {
        self.taken.insert(name.to_string());
        for (i, c) in name.char_indices() {
            if c == '.' {
                self.taken.insert(name[..i].to_string());
            }
        }
    }
}

/// A field, as the designer sees it.
pub(crate) struct View {
    pub obj: u32,
    pub name: String,
    pub kind: DesignKind,
    pub ff: u32,
    pub widgets: Vec<u32>,
    pub editable: bool,
}

fn is_widget_dict(d: &[u8]) -> bool {
    get(d, "Subtype").and_then(parse_name).as_deref() == Some(b"Widget")
}

impl Author {
    // ---- the form ---------------------------------------------------------------------------

    /// Reads the form's dictionary and its list of fields. A document with an XFA form is refused.
    pub(crate) fn load_acro(&mut self) -> Result<(), AuthorError> {
        if self.acro.is_some() {
            return Ok(());
        }
        let catalog = self.catalog_text()?;
        let (place, dict) = match get(&catalog, "AcroForm") {
            None => (Place::Missing, b"<< >>".to_vec()),
            Some(v) if v[skip_ws(v, 0)..].starts_with(b"<<") => (Place::Direct, v.to_vec()),
            Some(v) => {
                let n = parse_ref(v)
                    .map(|r| r.0)
                    .ok_or_else(|| odd("a form that is neither a dictionary nor an object"))?;
                (Place::Object(n), self.revisable(n)?)
            }
        };
        if get(&dict, "XFA").is_some() {
            return Err(AuthorError::Unsupported(
                "this form is an XFA form, which the form designer does not support".to_string(),
            ));
        }
        let fields = match get(&dict, "Fields") {
            None => FieldList {
                items: Vec::new(),
                place: Place::Missing,
            },
            Some(v) if v[skip_ws(v, 0)..].starts_with(b"[") => FieldList {
                items: array_items(v)
                    .ok_or_else(|| odd("a damaged list of fields"))?
                    .into_iter()
                    .map(<[u8]>::to_vec)
                    .collect(),
                place: Place::Direct,
            },
            Some(v) => {
                let n = parse_ref(v)
                    .map(|r| r.0)
                    .ok_or_else(|| odd("a list of fields that is not an array"))?;
                let text = self.revisable(n)?;
                FieldList {
                    items: array_items(&text)
                        .ok_or_else(|| odd("a damaged list of fields"))?
                        .into_iter()
                        .map(<[u8]>::to_vec)
                        .collect(),
                    place: Place::Object(n),
                }
            }
        };
        self.acro = Some(Acro {
            place,
            dict,
            fields: Some(fields),
            touched: false,
        });
        Ok(())
    }

    pub(crate) fn catalog_text(&mut self) -> Result<Vec<u8>, AuthorError> {
        if let Some(c) = &self.catalog {
            return Ok(c.clone());
        }
        let text = self.revisable(self.root)?;
        if !text[skip_ws(&text, 0)..].starts_with(b"<<") {
            return Err(odd("a catalog that is not a dictionary"));
        }
        self.catalog = Some(text.clone());
        Ok(text)
    }

    pub(crate) fn acro_mut(&mut self) -> Result<&mut Acro, AuthorError> {
        self.load_acro()?;
        self.acro.as_mut().ok_or_else(|| odd("no form"))
    }

    // ---- the fields -------------------------------------------------------------------------

    pub(crate) fn walk(&mut self) -> Result<&Walk, AuthorError> {
        if self.walk.is_none() {
            let walk = self.build_walk()?;
            self.walk = Some(walk);
        }
        self.walk.as_ref().ok_or_else(|| odd("no fields"))
    }

    pub(crate) fn invalidate_walk(&mut self) {
        self.walk = None;
    }

    fn build_walk(&self) -> Result<Walk, AuthorError> {
        let roots: Vec<u32> = self
            .acro
            .as_ref()
            .and_then(|a| a.fields.as_ref())
            .map(|f| {
                f.items
                    .iter()
                    .filter_map(|i| parse_ref(i).map(|r| r.0))
                    .collect()
            })
            .unwrap_or_default();
        let mut walk = Walk {
            nodes: HashMap::new(),
            top: Vec::new(),
            taken: HashSet::new(),
        };
        let mut stack: Vec<(u32, Option<String>, usize, bool)> =
            roots.iter().rev().map(|r| (*r, None, 0, true)).collect();
        while let Some((obj, prefix, depth, top)) = stack.pop() {
            if depth > MAX_FIELD_DEPTH || walk.nodes.len() >= MAX_FIELDS {
                return Err(odd("too many fields, or fields nested too deep"));
            }
            if walk.nodes.contains_key(&obj) {
                continue;
            }
            let Some(d) = self.dict(obj) else { continue };
            let partial = get(d, "T").and_then(decode_text);
            let name = match (prefix, partial) {
                (Some(p), Some(t)) => format!("{p}.{t}"),
                (Some(p), None) => p,
                (None, Some(t)) => t,
                (None, None) => String::new(),
            };
            let kids: Vec<u32> = get(d, "Kids")
                .and_then(array_items)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|k| parse_ref(k).map(|r| r.0))
                .filter(|k| {
                    self.dict(*k)
                        .is_some_and(|kd| get(kd, "T").is_some() || get(kd, "Kids").is_some())
                })
                .collect();
            if top {
                walk.top.push(obj);
            }
            walk.take(&name);
            let below = if name.is_empty() {
                None
            } else {
                Some(name.clone())
            };
            for k in kids.iter().rev() {
                stack.push((*k, below.clone(), depth + 1, false));
            }
            walk.nodes.insert(
                obj,
                Node {
                    name,
                    top,
                    fields: kids,
                },
            );
        }
        Ok(walk)
    }

    /// A value of a field, from the field or the nearest parent that has it.
    fn inherited(&self, obj: u32, key: &str) -> Option<Vec<u8>> {
        let mut at = obj;
        for _ in 0..=MAX_FIELD_DEPTH {
            let d = self.dict(at)?;
            if let Some(v) = get(d, key) {
                return Some(v.to_vec());
            }
            at = parse_ref(get(d, "Parent")?)?.0;
        }
        None
    }

    /// The field with this object number, if the walk found it.
    pub(crate) fn view(&self, obj: u32) -> Option<View> {
        let walk = self.walk.as_ref()?;
        let node = walk.nodes.get(&obj)?;
        let d = self.dict(obj)?;
        let ff = self
            .inherited(obj, "Ff")
            .and_then(|v| parse_int(&v))
            .unwrap_or(0) as u32;
        let ft = self.inherited(obj, "FT").and_then(|v| parse_name(&v))?;
        let kind = match ft.as_slice() {
            b"Tx" => DesignKind::Text,
            b"Ch" if ff & FF_COMBO != 0 => DesignKind::Combo,
            b"Ch" => DesignKind::List,
            b"Btn" if ff & FF_PUSHBUTTON != 0 => return None,
            b"Btn" if ff & FF_RADIO != 0 => DesignKind::Radio,
            b"Btn" => DesignKind::Checkbox,
            b"Sig" => DesignKind::Signature,
            _ => return None,
        };
        let widgets: Vec<u32> = match get(d, "Kids") {
            None => {
                if is_widget_dict(d) {
                    vec![obj]
                } else {
                    Vec::new()
                }
            }
            Some(k) => array_items(k)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|i| parse_ref(i).map(|r| r.0))
                .filter(|w| {
                    self.dict(*w)
                        .is_some_and(|wd| get(wd, "T").is_none() && get(wd, "Kids").is_none())
                })
                .collect(),
        };
        let acts = |o: u32| {
            self.dict(o)
                .is_none_or(|x| get(x, "A").is_some() || get(x, "AA").is_some())
        };
        let editable = node.top
            && !node.name.is_empty()
            && !node.name.contains('.')
            && node.fields.is_empty()
            && !widgets.is_empty()
            && !acts(obj)
            && widgets.iter().all(|w| !acts(*w))
            && get(d, "Parent").is_none();
        Some(View {
            obj,
            name: node.name.clone(),
            kind,
            ff,
            widgets,
            editable,
        })
    }

    /// The field with this name that the designer may change.
    pub(crate) fn find_field(&mut self, name: &str) -> Result<View, AuthorError> {
        let walk = self.walk()?;
        let obj = walk
            .top
            .iter()
            .copied()
            .find(|o| walk.nodes.get(o).is_some_and(|n| n.name == name));
        let missing = || {
            rules::refused(format!(
                "There is no field called \u{201c}{name}\u{201d} that can be changed here."
            ))
        };
        let view = obj.and_then(|o| self.view(o)).ok_or_else(missing)?;
        if !view.editable {
            return Err(rules::refused(format!(
                "The field \u{201c}{name}\u{201d} has an action or a structure that cannot be changed here."
            )));
        }
        Ok(view)
    }

    /// The field a widget belongs to: the widget itself when it is the field, else its parent.
    fn field_of(&self, widget: u32) -> Option<u32> {
        let walk = self.walk.as_ref()?;
        if walk.nodes.contains_key(&widget) {
            return Some(widget);
        }
        let parent = parse_ref(get(self.dict(widget)?, "Parent")?)?.0;
        walk.nodes.contains_key(&parent).then_some(parent)
    }

    // ---- what a field holds -----------------------------------------------------------------

    /// The states a button has besides the off state, from its normal appearance.
    pub(crate) fn button_states(&self, widget: u32) -> Vec<Vec<u8>> {
        let Some(d) = self.dict(widget) else {
            return Vec::new();
        };
        let resolve = |v: &[u8]| -> Option<Vec<u8>> {
            match parse_ref(v) {
                Some((n, _)) => self.cur(n).map(<[u8]>::to_vec),
                None => Some(v.to_vec()),
            }
        };
        let Some(ap) = get(d, "AP").and_then(resolve) else {
            return Vec::new();
        };
        let Some(normal) = get(&ap, "N").and_then(resolve) else {
            return Vec::new();
        };
        entries(&normal)
            .unwrap_or_default()
            .into_iter()
            .map(|e| e.key)
            .filter(|k| k != b"Off")
            .collect()
    }

    fn labels_of(&self, obj: u32) -> Vec<(Vec<u8>, String)> {
        let Some(opt) = self.inherited(obj, "Opt") else {
            return Vec::new();
        };
        let resolved = match parse_ref(&opt) {
            Some((n, _)) => self.cur(n).map(<[u8]>::to_vec),
            None => Some(opt),
        };
        let Some(items) = resolved.as_deref().and_then(array_items) else {
            return Vec::new();
        };
        items
            .into_iter()
            .filter_map(|item| {
                if item[skip_ws(item, 0)..].starts_with(b"[") {
                    let pair = array_items(item)?;
                    let export = super::syntax::string_bytes(pair.first()?)?;
                    let label = decode_text(pair.get(1)?)?;
                    Some((export, label))
                } else {
                    let label = decode_text(item)?;
                    Some((super::syntax::string_bytes(item)?, label))
                }
            })
            .collect()
    }

    pub(crate) fn read_props(&self, v: &View) -> DesignProps {
        let obj = v.obj;
        let text = |key: &str| {
            self.inherited(obj, key)
                .and_then(|x| decode_text(&x))
                .unwrap_or_default()
        };
        let mut p = DesignProps {
            tooltip: text("TU"),
            required: v.ff & FF_REQUIRED != 0,
            read_only: v.ff & FF_READ_ONLY != 0,
            ..DesignProps::default()
        };
        let da = v
            .widgets
            .iter()
            .find_map(|w| self.dict(*w).and_then(|d| get(d, "DA")).map(<[u8]>::to_vec))
            .or_else(|| self.inherited(obj, "DA"))
            .or_else(|| {
                self.acro
                    .as_ref()
                    .and_then(|a| get(&a.dict, "DA").map(<[u8]>::to_vec))
            });
        p.font_size = da
            .as_deref()
            .and_then(decode_text)
            .and_then(|s| font_size_of(&s))
            .unwrap_or(0.0);
        match v.kind {
            DesignKind::Text => {
                p.multiline = v.ff & FF_MULTILINE != 0;
                p.password = v.ff & FF_PASSWORD != 0;
                p.comb = v.ff & FF_COMB != 0;
                p.max_len = self
                    .inherited(obj, "MaxLen")
                    .and_then(|x| parse_int(&x))
                    .unwrap_or(0)
                    .clamp(0, i64::from(u32::MAX)) as u32;
                p.default_value = text("DV");
            }
            DesignKind::Combo | DesignKind::List => {
                p.editable = v.ff & FF_EDIT != 0;
                p.multiselect = v.ff & FF_MULTISELECT != 0;
                let labels = self.labels_of(obj);
                p.choices = labels.iter().map(|(_, l)| l.clone()).collect();
                if let Some(dv) = self.inherited(obj, "DV") {
                    let mut picked = Vec::new();
                    let values: Vec<Vec<u8>> = if dv[skip_ws(&dv, 0)..].starts_with(b"[") {
                        array_items(&dv)
                            .unwrap_or_default()
                            .into_iter()
                            .filter_map(super::syntax::string_bytes)
                            .collect()
                    } else {
                        super::syntax::string_bytes(&dv).into_iter().collect()
                    };
                    for value in values {
                        match labels.iter().find(|(e, _)| *e == value) {
                            Some((_, l)) => picked.push(l.clone()),
                            None => picked.push(String::from_utf8_lossy(&value).into_owned()),
                        }
                    }
                    p.default_value = picked.join("\n");
                }
            }
            DesignKind::Checkbox => {
                let states = v
                    .widgets
                    .first()
                    .map(|w| self.button_states(*w))
                    .unwrap_or_default();
                p.export_value = states
                    .first()
                    .map(|s| String::from_utf8_lossy(s).into_owned())
                    .unwrap_or_default();
                if self
                    .inherited(obj, "DV")
                    .and_then(|x| parse_name(&x))
                    .is_some_and(|n| n != b"Off")
                {
                    p.default_value = "on".to_string();
                }
            }
            DesignKind::Radio => {
                if let Some(n) = self
                    .inherited(obj, "DV")
                    .and_then(|x| parse_name(&x))
                    .filter(|n| n != b"Off")
                {
                    p.default_value = String::from_utf8_lossy(&n).into_owned();
                }
            }
            DesignKind::Signature => {}
        }
        p
    }

    // ---- the annotations of a page ----------------------------------------------------------

    fn read_annots(&self, page: usize) -> Result<(Vec<Vec<u8>>, Place), AuthorError> {
        let obj = *self
            .page_objs
            .get(page)
            .ok_or_else(|| rules::refused("That page is not there."))?;
        let d = self
            .dict(obj)
            .ok_or_else(|| odd("a page that is not a dictionary"))?;
        match get(d, "Annots") {
            None => Ok((Vec::new(), Place::Missing)),
            Some(v) if v[skip_ws(v, 0)..].starts_with(b"[") => Ok((
                array_items(v)
                    .ok_or_else(|| odd("a damaged list of annotations"))?
                    .into_iter()
                    .map(<[u8]>::to_vec)
                    .collect(),
                Place::Direct,
            )),
            Some(v) => {
                let n = parse_ref(v)
                    .map(|r| r.0)
                    .ok_or_else(|| odd("a list of annotations that is not an array"))?;
                let text = self
                    .cur(n)
                    .ok_or_else(|| odd("a missing list of annotations"))?;
                Ok((
                    array_items(text)
                        .ok_or_else(|| odd("a damaged list of annotations"))?
                        .into_iter()
                        .map(<[u8]>::to_vec)
                        .collect(),
                    Place::Object(n),
                ))
            }
        }
    }

    /// The annotations of a page, ready to be changed.
    pub(crate) fn page_annots(&mut self, page: usize) -> Result<&mut PageAnnots, AuthorError> {
        if !self.pages.contains_key(&page) {
            let (items, place) = self.read_annots(page)?;
            if let Place::Object(n) = place {
                self.revisable(n)?;
            }
            let map = (0..items.len()).map(Some).collect();
            self.pages.insert(
                page,
                PageAnnots {
                    items,
                    map,
                    place,
                    dirty: false,
                },
            );
        }
        self.pages
            .get_mut(&page)
            .ok_or_else(|| odd("a page that is not there"))
    }

    /// Which page lists each widget, found once and kept up to date as widgets move.
    pub(crate) fn owners(&mut self) -> Result<&mut HashMap<u32, usize>, AuthorError> {
        if self.owners.is_none() {
            let mut owners = HashMap::new();
            for page in 0..self.page_objs.len() {
                let items = match self.pages.get(&page) {
                    Some(p) => p.items.clone(),
                    None => self.read_annots(page)?.0,
                };
                for item in items {
                    if let Some((n, _)) = parse_ref(&item) {
                        owners.entry(n).or_insert(page);
                    }
                }
            }
            self.owners = Some(owners);
        }
        self.owners.as_mut().ok_or_else(|| odd("no pages"))
    }

    /// The widgets of a page the designer lists, in the order of the page's annotations.
    pub(crate) fn widgets_on(&mut self, page: usize) -> Result<Vec<DesignedWidget>, AuthorError> {
        self.walk()?;
        let geo = self.geometry(page)?;
        let items = match self.pages.get(&page) {
            Some(p) => p.items.clone(),
            None => self.read_annots(page)?.0,
        };
        let mut out = Vec::new();
        for item in items {
            if out.len() >= partridge_protocol::MAX_LISTED_WIDGETS {
                break;
            }
            let Some((w, _)) = parse_ref(&item) else {
                continue;
            };
            let Some(wd) = self.dict(w) else { continue };
            if !is_widget_dict(wd) {
                continue;
            }
            let Some(rect) = get(wd, "Rect").and_then(parse_box) else {
                continue;
            };
            let Some(field) = self.field_of(w) else {
                continue;
            };
            let Some(view) = self.view(field) else {
                continue;
            };
            let Some(index) = view.widgets.iter().position(|x| *x == w) else {
                continue;
            };
            let user = [
                rect[0].min(rect[2]),
                rect[1].min(rect[3]),
                rect[0].max(rect[2]),
                rect[1].max(rect[3]),
            ];
            let export = if matches!(view.kind, DesignKind::Radio | DesignKind::Checkbox) {
                self.button_states(w)
                    .first()
                    .map(|s| String::from_utf8_lossy(s).into_owned())
                    .unwrap_or_default()
            } else {
                String::new()
            };
            out.push(DesignedWidget {
                name: view.name.clone(),
                kind: view.kind,
                widget: index as u32,
                widgets: view.widgets.len() as u32,
                rect: geo.to_drawn(user),
                export,
                editable: view.editable,
                props: self.read_props(&view),
            });
        }
        Ok(out)
    }
}

/// The size of the text in a default appearance string such as `/Helv 12 Tf 0 g`.
pub(crate) fn font_size_of(da: &str) -> Option<f32> {
    let tokens: Vec<&str> = da.split_ascii_whitespace().collect();
    let at = tokens.iter().position(|t| *t == "Tf")?;
    let size = tokens.get(at.checked_sub(1)?)?;
    parse_f32(size.as_bytes()).filter(|s| *s >= 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_size_comes_from_the_default_appearance() {
        assert_eq!(font_size_of("/Helv 12 Tf 0 g"), Some(12.0));
        assert_eq!(font_size_of("0 g /Helv 0 Tf"), Some(0.0));
        assert_eq!(font_size_of("/Helv Tf"), None);
        assert_eq!(font_size_of("nothing"), None);
    }
}
