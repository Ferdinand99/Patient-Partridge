//! The changes the designer makes: add a field, take one away, move a widget, change what a field
//! is, put the widgets of a page in a new order. Each works on the objects as they are after the
//! changes before it in the batch.

use partridge_protocol::{
    AuthorOp, DesignField, DesignKind, DesignProps, DesignWidget, WidgetRef,
    MAX_FORM_FIELDS_PER_PAGE,
};

use super::build::{
    choice_value, default_appearance, flags_of, managed_bits, options_array, Placed,
};
use super::model::{font_size_of, Node, View};
use super::rules::{self, refused};
use super::syntax::{
    array_items, encode_name, encode_text, get, num, num_box, parse_name, parse_ref, remove, set,
};
use super::{odd, Author, AuthorError, Place};

fn taken(name: &str) -> AuthorError {
    refused(format!(
        "There is already a field called \u{201c}{name}\u{201d}. Every field needs a name of its own."
    ))
}

fn array_text(items: &[Vec<u8>]) -> String {
    let mut s = String::from("[");
    for item in items {
        s.push_str(&String::from_utf8_lossy(item));
        s.push(' ');
    }
    s.push(']');
    s
}

fn ref_number(item: &[u8]) -> Option<u32> {
    parse_ref(item).map(|r| r.0)
}

/// The size of the text in a default appearance string replaced, the rest kept.
fn with_font_size(da: &str, size: f32) -> Option<String> {
    let tokens: Vec<&str> = da.split_ascii_whitespace().collect();
    let at = tokens.iter().position(|t| *t == "Tf")?;
    let slot = at.checked_sub(1)?;
    font_size_of(da)?;
    let mut out = Vec::new();
    for (i, t) in tokens.iter().enumerate() {
        if i == slot {
            out.push(num(size));
        } else {
            out.push((*t).to_string());
        }
    }
    Some(out.join(" "))
}

fn set_or_remove(d: &[u8], key: &str, value: Option<&str>) -> Option<Vec<u8>> {
    match value {
        Some(v) => set(d, key, v.as_bytes()),
        None => remove(d, key),
    }
}

impl Author {
    pub(crate) fn op(&mut self, op: &AuthorOp) -> Result<(), AuthorError> {
        match op {
            AuthorOp::Add(field) => self.add_field(field),
            AuthorOp::Remove { name } => self.remove_field(name),
            AuthorOp::RemoveWidget { name, widget } => self.remove_widget(name, *widget),
            AuthorOp::AddWidget { name, widget } => self.add_widget(name, widget),
            AuthorOp::Move {
                name,
                widget,
                page,
                rect,
            } => self.move_widget(name, *widget, *page as usize, *rect),
            AuthorOp::Update {
                name,
                new_name,
                props,
                exports,
            } => self.update_field(name, new_name, props, exports),
            AuthorOp::Reorder { page, order } => self.reorder(*page as usize, order),
        }
    }

    fn edit(
        &mut self,
        obj: u32,
        f: impl FnOnce(Vec<u8>) -> Option<Vec<u8>>,
    ) -> Result<(), AuthorError> {
        let d = self.revisable(obj)?;
        let nd = f(d).ok_or_else(|| odd("a damaged field"))?;
        self.put(obj, nd);
        Ok(())
    }

    // ---- places -----------------------------------------------------------------------------

    fn place(&self, w: &DesignWidget) -> Result<Placed, AuthorError> {
        let page = w.page as usize;
        let page_obj = *self
            .page_objs
            .get(page)
            .ok_or_else(|| refused("That page is not there."))?;
        let geo = self.geometry(page)?;
        rules::check_rect(w.rect, &geo)?;
        Ok(Placed {
            page,
            page_obj,
            user: geo.to_user(w.rect),
            turns: geo.turns % 4,
            export: w.export.clone(),
        })
    }

    fn widgets_in(&mut self, page: usize) -> Result<usize, AuthorError> {
        let items = self.page_annots(page)?.items.clone();
        Ok(items
            .iter()
            .filter_map(|i| ref_number(i))
            .filter(|n| {
                self.dict(*n).is_some_and(|d| {
                    get(d, "Subtype").and_then(parse_name).as_deref() == Some(b"Widget")
                })
            })
            .count())
    }

    fn check_room(&mut self, placed: &[Placed]) -> Result<(), AuthorError> {
        let mut pages: Vec<usize> = placed.iter().map(|p| p.page).collect();
        pages.sort_unstable();
        pages.dedup();
        for page in pages {
            let adding = placed.iter().filter(|p| p.page == page).count();
            if self.widgets_in(page)? + adding > MAX_FORM_FIELDS_PER_PAGE {
                return Err(refused(format!(
                    "A page can have at most {MAX_FORM_FIELDS_PER_PAGE} fields."
                )));
            }
        }
        Ok(())
    }

    fn push_widget(&mut self, widget: u32, page: usize) -> Result<(), AuthorError> {
        let pa = self.page_annots(page)?;
        pa.items.push(format!("{widget} 0 R").into_bytes());
        pa.map.push(None);
        pa.dirty = true;
        if let Some(owners) = self.owners.as_mut() {
            owners.insert(widget, page);
        }
        Ok(())
    }

    /// Takes a widget out of the list of annotations of the page that has it.
    fn detach_widget(&mut self, widget: u32) -> Result<Option<usize>, AuthorError> {
        let Some(page) = self.owners()?.get(&widget).copied() else {
            return Ok(None);
        };
        let pa = self.page_annots(page)?;
        if let Some(at) = pa.items.iter().position(|i| ref_number(i) == Some(widget)) {
            pa.items.remove(at);
            pa.map.remove(at);
            pa.dirty = true;
        }
        self.owners()?.remove(&widget);
        Ok(Some(page))
    }

    fn remove_from_fields(&mut self, obj: u32) -> Result<(), AuthorError> {
        let acro = self.acro_mut()?;
        if let Some(list) = acro.fields.as_mut() {
            list.items.retain(|i| ref_number(i) != Some(obj));
        }
        acro.touched = true;
        Ok(())
    }

    // ---- add --------------------------------------------------------------------------------

    fn add_field(&mut self, f: &DesignField) -> Result<(), AuthorError> {
        rules::check_name(&f.name)?;
        if self.walk()?.taken.contains(&f.name) {
            return Err(taken(&f.name));
        }
        rules::check_props(f.kind, &f.props)?;
        if f.widgets.is_empty() {
            return Err(refused("A field needs a place on a page."));
        }
        if f.kind != DesignKind::Radio && f.widgets.len() != 1 {
            return Err(refused("Only radio buttons can be in more than one place."));
        }
        if f.kind == DesignKind::Radio {
            let exports: Vec<String> = f.widgets.iter().map(|w| w.export.clone()).collect();
            rules::check_exports(&exports, &f.props.default_value)?;
        }
        let placed = f
            .widgets
            .iter()
            .map(|w| self.place(w))
            .collect::<Result<Vec<_>, _>>()?;
        self.check_room(&placed)?;
        self.ensure_resources()?;
        let (field, widgets) = self.build_field(f, &placed)?;
        self.add_to_fields(field)?;
        for (w, p) in widgets.iter().zip(&placed) {
            self.push_widget(*w, p.page)?;
        }
        if let Some(walk) = self.walk.as_mut() {
            walk.nodes.insert(
                field,
                Node {
                    name: f.name.clone(),
                    top: true,
                    fields: Vec::new(),
                },
            );
            walk.top.push(field);
            walk.take(&f.name);
        }
        Ok(())
    }

    fn add_widget(&mut self, name: &str, w: &DesignWidget) -> Result<(), AuthorError> {
        let view = self.find_field(name)?;
        if view.kind != DesignKind::Radio {
            return Err(refused(
                "Only a group of radio buttons can have another button added.",
            ));
        }
        rules::check_export(&w.export)?;
        let existing: Vec<Vec<u8>> = view
            .widgets
            .iter()
            .flat_map(|x| self.button_states(*x))
            .collect();
        if existing.iter().any(|s| s.as_slice() == w.export.as_bytes()) {
            return Err(refused(format!(
                "Two radio buttons of the group have the value \u{201c}{}\u{201d}. Give each its own.",
                w.export
            )));
        }
        if view.widgets.len() >= partridge_protocol::MAX_AUTHOR_WIDGETS {
            return Err(refused("A group can have at most 64 buttons."));
        }
        let placed = vec![self.place(w)?];
        self.check_room(&placed)?;
        self.ensure_resources()?;
        let on = self.read_props(&view).default_value;
        let kid = self.radio_kid(view.obj, &placed[0], &on)?;
        self.edit(view.obj, |d| {
            let kids = get(&d, "Kids")?;
            let mut items: Vec<Vec<u8>> =
                array_items(kids)?.into_iter().map(<[u8]>::to_vec).collect();
            items.push(format!("{kid} 0 R").into_bytes());
            set(&d, "Kids", array_text(&items).as_bytes())
        })?;
        self.push_widget(kid, placed[0].page)?;
        Ok(())
    }

    // ---- remove -----------------------------------------------------------------------------

    fn remove_field(&mut self, name: &str) -> Result<(), AuthorError> {
        let view = self.find_field(name)?;
        for w in &view.widgets {
            self.detach_widget(*w)?;
        }
        self.remove_from_fields(view.obj)?;
        self.invalidate_walk();
        Ok(())
    }

    fn remove_widget(&mut self, name: &str, widget: u32) -> Result<(), AuthorError> {
        let view = self.find_field(name)?;
        let w = *view
            .widgets
            .get(widget as usize)
            .ok_or_else(|| refused("That button is not there."))?;
        if view.widgets.len() == 1 {
            return self.remove_field(name);
        }
        let states = self.button_states(w);
        self.detach_widget(w)?;
        let default = self.read_props(&view).default_value;
        let was_on =
            !default.is_empty() && states.iter().any(|s| s.as_slice() == default.as_bytes());
        self.edit(view.obj, |d| {
            let kids = get(&d, "Kids")?;
            let items: Vec<Vec<u8>> = array_items(kids)?
                .into_iter()
                .filter(|i| ref_number(i) != Some(w))
                .map(<[u8]>::to_vec)
                .collect();
            let mut d = set(&d, "Kids", array_text(&items).as_bytes())?;
            if was_on {
                d = set(&d, "V", b"/Off")?;
                d = remove(&d, "DV")?;
            }
            Some(d)
        })
    }

    // ---- move -------------------------------------------------------------------------------

    fn move_widget(
        &mut self,
        name: &str,
        widget: u32,
        page: usize,
        rect: [f32; 4],
    ) -> Result<(), AuthorError> {
        let view = self.find_field(name)?;
        let w = *view
            .widgets
            .get(widget as usize)
            .ok_or_else(|| refused("That place is not there."))?;
        let mut placed = self.place(&DesignWidget {
            page: page as u32,
            rect,
            export: String::new(),
        })?;
        let old_page = self.owners()?.get(&w).copied();
        if let Some(p) = old_page {
            self.redraw.insert(p);
        }
        self.redraw.insert(page);
        if old_page != Some(page) {
            self.check_room(std::slice::from_ref(&placed))?;
        }
        self.ensure_resources()?;
        let state = self
            .button_states(w)
            .first()
            .map(|s| String::from_utf8_lossy(s).into_owned());
        placed.export = state.clone().unwrap_or_else(|| "Yes".to_string());
        let page_ref = self.file.reference(placed.page_obj);
        let ap = match view.kind {
            DesignKind::Checkbox | DesignKind::Radio => {
                Some(self.button_ap(view.kind, &placed, &placed.export)?)
            }
            DesignKind::Signature => Some(self.signature_ap(&placed)?),
            _ => None,
        };
        let turns = placed.turns;
        let user = placed.user;
        self.edit(w, |d| {
            let mut d = set(&d, "Rect", num_box(&user).as_bytes())?;
            d = set(&d, "P", page_ref.as_bytes())?;
            if let Some(mk) = get(&d, "MK").map(<[u8]>::to_vec) {
                let mk = if turns.is_multiple_of(4) {
                    remove(&mk, "R")?
                } else {
                    set(&mk, "R", (u32::from(turns % 4) * 90).to_string().as_bytes())?
                };
                d = set(&d, "MK", &mk)?;
            }
            match &ap {
                Some(entry) => {
                    // The entry is `/AP ...`; take the value after the key.
                    let value = entry.strip_prefix("/AP ")?;
                    set(&d, "AP", value.as_bytes())
                }
                None => remove(&d, "AP"),
            }
        })?;
        if old_page != Some(page) {
            self.detach_widget(w)?;
            self.push_widget(w, page)?;
        }
        Ok(())
    }

    // ---- update -----------------------------------------------------------------------------

    fn update_field(
        &mut self,
        name: &str,
        new_name: &str,
        props: &DesignProps,
        exports: &[String],
    ) -> Result<(), AuthorError> {
        let view = self.find_field(name)?;
        let target = if new_name.is_empty() { name } else { new_name };
        if target != name {
            rules::check_name(target)?;
            if self.walk()?.taken.contains(target) {
                return Err(taken(target));
            }
        }
        rules::check_props(view.kind, props)?;
        let old = self.read_props(&view);
        let current_states: Vec<Vec<u8>> = view
            .widgets
            .iter()
            .map(|w| {
                self.button_states(*w)
                    .into_iter()
                    .next()
                    .unwrap_or_default()
            })
            .collect();
        let new_exports: Vec<String> = if view.kind == DesignKind::Radio {
            let list: Vec<String> = if exports.is_empty() {
                current_states
                    .iter()
                    .map(|s| String::from_utf8_lossy(s).into_owned())
                    .collect()
            } else if exports.len() == view.widgets.len() {
                exports.to_vec()
            } else {
                return Err(refused("Give every radio button of the group a value."));
            };
            rules::check_exports(&list, &props.default_value)?;
            list
        } else {
            Vec::new()
        };
        self.ensure_resources()?;
        let changed_default = old.default_value != props.default_value
            || (matches!(view.kind, DesignKind::Combo | DesignKind::List)
                && old.choices != props.choices);
        let ff = (view.ff & !managed_bits()) | flags_of(view.kind, props);
        let tooltip = (!props.tooltip.is_empty()).then(|| encode_text(&props.tooltip));
        let renamed = target != name;
        let kind = view.kind;
        let on = self.checkbox_on(&view);
        // The field itself.
        self.edit(view.obj, |d| {
            let mut d = set(&d, "Ff", ff.to_string().as_bytes())?;
            if renamed {
                d = set(&d, "T", encode_text(target).as_bytes())?;
            }
            d = set_or_remove(&d, "TU", tooltip.as_deref())?;
            match kind {
                DesignKind::Text => {
                    d = if props.max_len > 0 {
                        set(&d, "MaxLen", props.max_len.to_string().as_bytes())?
                    } else {
                        remove(&d, "MaxLen")?
                    };
                    if changed_default {
                        d = if props.default_value.is_empty() {
                            remove(&remove(&d, "V")?, "DV")?
                        } else {
                            let v = encode_text(&props.default_value);
                            set(&set(&d, "V", v.as_bytes())?, "DV", v.as_bytes())?
                        };
                    }
                }
                DesignKind::Combo | DesignKind::List => {
                    d = set(&d, "Opt", options_array(&props.choices).as_bytes())?;
                    d = remove(&d, "I")?;
                    if changed_default {
                        d = match choice_value(props) {
                            Some((v, places)) => {
                                let mut d2 = set(&set(&d, "V", v.as_bytes())?, "DV", v.as_bytes())?;
                                if kind == DesignKind::List && !places.is_empty() {
                                    let mut sorted = places.clone();
                                    sorted.sort_unstable();
                                    let list: Vec<String> =
                                        sorted.iter().map(usize::to_string).collect();
                                    d2 = set(&d2, "I", format!("[{}]", list.join(" ")).as_bytes())?;
                                }
                                d2
                            }
                            None => remove(&remove(&d, "V")?, "DV")?,
                        };
                    }
                }
                DesignKind::Radio => {
                    if changed_default || !exports.is_empty() {
                        d = match radio_value(
                            props,
                            &old.default_value,
                            &current_states,
                            &new_exports,
                        ) {
                            Some(e) => {
                                let e = encode_name(e.as_bytes());
                                set(
                                    &set(&d, "V", format!("/{e}").as_bytes())?,
                                    "DV",
                                    format!("/{e}").as_bytes(),
                                )?
                            }
                            None => remove(&set(&d, "V", b"/Off")?, "DV")?,
                        };
                    }
                }
                DesignKind::Checkbox | DesignKind::Signature => {}
            }
            Some(d)
        })?;
        // The widgets.
        for (i, w) in view.widgets.clone().into_iter().enumerate() {
            if let Some(page) = self.owners()?.get(&w).copied() {
                self.redraw.insert(page);
            }
            let rect = self
                .dict(w)
                .and_then(|d| get(d, "Rect"))
                .and_then(super::syntax::parse_box)
                .ok_or_else(|| odd("a widget without a box"))?;
            let turns = self
                .dict(w)
                .and_then(|d| get(d, "MK"))
                .and_then(|mk| get(mk, "R"))
                .and_then(super::syntax::parse_int)
                .map_or(0, |r| ((r / 90).rem_euclid(4)) as u8);
            let mut placed = Placed {
                page: 0,
                page_obj: 0,
                user: [
                    rect[0].min(rect[2]),
                    rect[1].min(rect[3]),
                    rect[0].max(rect[2]),
                    rect[1].max(rect[3]),
                ],
                turns,
                export: String::new(),
            };
            match kind {
                DesignKind::Text | DesignKind::Combo | DesignKind::List => {
                    let existing_da = self
                        .dict(w)
                        .and_then(|d| get(d, "DA"))
                        .and_then(super::syntax::decode_text);
                    let da = existing_da
                        .and_then(|s| with_font_size(&s, props.font_size))
                        .map(|s| encode_text(&s))
                        .unwrap_or_else(|| default_appearance(kind, props.font_size));
                    self.edit(w, |d| remove(&set(&d, "DA", da.as_bytes())?, "AP"))?;
                }
                DesignKind::Checkbox => {
                    let export = if props.export_value.is_empty() {
                        "Yes".to_string()
                    } else {
                        props.export_value.clone()
                    };
                    placed.export = export.clone();
                    let is_on = if changed_default {
                        props.default_value == "on"
                    } else {
                        on
                    };
                    let ap = self.button_ap(kind, &placed, &export)?;
                    let e = encode_name(export.as_bytes());
                    self.edit(w, |d| {
                        let d = set(&d, "AP", ap.strip_prefix("/AP ")?.as_bytes())?;
                        let state = if is_on {
                            format!("/{e}")
                        } else {
                            "/Off".to_string()
                        };
                        let d = set(&d, "AS", state.as_bytes())?;
                        let d = set(&d, "V", state.as_bytes())?;
                        if is_on {
                            set(&d, "DV", state.as_bytes())
                        } else {
                            remove(&d, "DV")
                        }
                    })?;
                }
                DesignKind::Radio => {
                    let export = new_exports[i].clone();
                    placed.export = export.clone();
                    let value = self.radio_value_now(&view, &new_exports);
                    let ap = self.button_ap(kind, &placed, &export)?;
                    let state = if value.as_deref() == Some(export.as_str()) {
                        format!("/{}", encode_name(export.as_bytes()))
                    } else {
                        "/Off".to_string()
                    };
                    self.edit(w, |d| {
                        let d = set(&d, "AP", ap.strip_prefix("/AP ")?.as_bytes())?;
                        set(&d, "AS", state.as_bytes())
                    })?;
                }
                DesignKind::Signature => {}
            }
        }
        if renamed {
            self.invalidate_walk();
        }
        Ok(())
    }

    /// Whether a check box is on now.
    fn checkbox_on(&self, view: &View) -> bool {
        view.kind == DesignKind::Checkbox
            && view
                .widgets
                .first()
                .and_then(|w| self.dict(*w))
                .and_then(|d| get(d, "AS"))
                .and_then(parse_name)
                .is_some_and(|n| n != b"Off")
    }

    /// The value a radio group has after its field was written: the word of the button that is on.
    fn radio_value_now(&self, view: &View, exports: &[String]) -> Option<String> {
        let v = self
            .dict(view.obj)
            .and_then(|d| get(d, "V"))
            .and_then(parse_name)?;
        let v = String::from_utf8_lossy(&v).into_owned();
        exports.iter().find(|e| **e == v).cloned()
    }

    // ---- reorder ----------------------------------------------------------------------------

    fn widget_named(&mut self, r: &WidgetRef) -> Result<u32, AuthorError> {
        let walk = self.walk()?;
        let obj = walk
            .nodes
            .iter()
            .find(|(_, n)| n.name == r.name)
            .map(|(o, _)| *o);
        let view = obj.and_then(|o| self.view(o)).ok_or_else(|| {
            refused(format!(
                "There is no field called \u{201c}{}\u{201d}.",
                r.name
            ))
        })?;
        view.widgets
            .get(r.widget as usize)
            .copied()
            .ok_or_else(|| refused("That place is not there."))
    }

    fn reorder(&mut self, page: usize, order: &[WidgetRef]) -> Result<(), AuthorError> {
        let objs = order
            .iter()
            .map(|r| self.widget_named(r))
            .collect::<Result<Vec<u32>, _>>()?;
        let pa = self.page_annots(page)?;
        let mut slots = Vec::new();
        let mut moved = Vec::new();
        for obj in &objs {
            let at = pa
                .items
                .iter()
                .position(|i| ref_number(i) == Some(*obj))
                .ok_or_else(|| refused("A field in the order is not on that page."))?;
            if slots.contains(&at) {
                return Err(refused("A field is in the order twice."));
            }
            slots.push(at);
            moved.push((pa.items[at].clone(), pa.map[at]));
        }
        slots.sort_unstable();
        for (slot, (item, map)) in slots.into_iter().zip(moved) {
            pa.items[slot] = item;
            pa.map[slot] = map;
        }
        pa.dirty = true;
        Ok(())
    }

    // ---- writing it out ---------------------------------------------------------------------

    pub(crate) fn flush_pages(&mut self) -> Result<(), AuthorError> {
        let dirty: Vec<usize> = self
            .pages
            .iter()
            .filter(|(_, p)| p.dirty)
            .map(|(i, _)| *i)
            .collect();
        for page in dirty {
            let (arr, place) = {
                let pa = &self.pages[&page];
                (array_text(&pa.items), pa.place)
            };
            match place {
                Place::Object(n) => self.put(n, arr),
                Place::Direct | Place::Missing => {
                    let page_obj = self.page_objs[page];
                    let d = self.revisable(page_obj)?;
                    let nd =
                        set(&d, "Annots", arr.as_bytes()).ok_or_else(|| odd("a damaged page"))?;
                    self.put(page_obj, nd);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn flush_form(&mut self) -> Result<(), AuthorError> {
        let Some(acro) = self.acro.take() else {
            return Ok(());
        };
        if !acro.touched {
            return Ok(());
        }
        let mut dict = acro.dict;
        if let Some(list) = acro.fields {
            let arr = array_text(&list.items);
            match list.place {
                Place::Object(n) => self.put(n, arr),
                Place::Direct | Place::Missing => {
                    dict = set(&dict, "Fields", arr.as_bytes())
                        .ok_or_else(|| odd("a damaged form"))?;
                }
            }
        }
        match acro.place {
            Place::Object(n) => self.put(n, dict),
            Place::Direct | Place::Missing => {
                let catalog = self.catalog_text()?;
                let updated =
                    set(&catalog, "AcroForm", &dict).ok_or_else(|| odd("a damaged catalog"))?;
                self.put(self.root, updated);
            }
        }
        Ok(())
    }
}

/// The word of the button that is on after an update of a radio group, or `None` when none is.
fn radio_value(
    props: &DesignProps,
    old_default: &str,
    old_states: &[Vec<u8>],
    new_exports: &[String],
) -> Option<String> {
    if props.default_value != old_default {
        return (!props.default_value.is_empty()).then(|| props.default_value.clone());
    }
    // The default stayed; its word may have been renamed with the button.
    let at = old_states
        .iter()
        .position(|s| s.as_slice() == old_default.as_bytes())?;
    new_exports.get(at).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_size_changes() {
        assert_eq!(
            with_font_size("/Helv 12 Tf 0 0 1 rg", 9.0).as_deref(),
            Some("/Helv 9 Tf 0 0 1 rg")
        );
        assert_eq!(with_font_size("0 g", 9.0), None);
    }

    #[test]
    fn a_renamed_button_keeps_being_on() {
        let states = vec![b"S".to_vec(), b"M".to_vec()];
        let new = vec!["Small".to_string(), "Medium".to_string()];
        let props = DesignProps {
            default_value: "M".into(),
            ..DesignProps::default()
        };
        assert_eq!(
            radio_value(&props, "M", &states, &new).as_deref(),
            Some("Medium")
        );
        let props = DesignProps::default();
        assert_eq!(radio_value(&props, "", &states, &new), None);
        let props = DesignProps {
            default_value: "Small".into(),
            ..DesignProps::default()
        };
        assert_eq!(
            radio_value(&props, "M", &states, &new).as_deref(),
            Some("Small")
        );
    }
}
