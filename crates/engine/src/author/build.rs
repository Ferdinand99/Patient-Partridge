//! Making the objects of a new field: the field, its widgets, and the pictures that are ours.

use partridge_protocol::{DesignField, DesignKind, DesignProps};

use super::appearance;
use super::model::{
    FF_COMB, FF_COMBO, FF_EDIT, FF_MULTILINE, FF_MULTISELECT, FF_NO_TOGGLE_TO_OFF, FF_PASSWORD,
    FF_RADIO, FF_READ_ONLY, FF_REQUIRED,
};
use super::syntax::{encode_name, encode_text, get, num, num_box, parse_ref, set};
use super::{odd, Author, AuthorError, Place, Role};

/// A widget on its page: the page, the box in the page's own coordinates and the turn.
#[derive(Debug, Clone)]
pub(crate) struct Placed {
    pub page: usize,
    pub page_obj: u32,
    /// `[left, bottom, right, top]` in the page's own coordinates.
    pub user: [f32; 4],
    pub turns: u8,
    pub export: String,
}

impl Placed {
    /// Width and height of the box as it is drawn.
    pub fn drawn(&self) -> (f32, f32) {
        let w = self.user[2] - self.user[0];
        let h = self.user[3] - self.user[1];
        if self.turns.is_multiple_of(2) {
            (w, h)
        } else {
            (h, w)
        }
    }
}

/// The bits of `/Ff` the designer sets for a kind of field.
pub(crate) fn managed_bits() -> u32 {
    FF_READ_ONLY | FF_REQUIRED | FF_MULTILINE | FF_PASSWORD | FF_COMB | FF_EDIT | FF_MULTISELECT
}

/// The flags of a field of this kind with these properties.
pub(crate) fn flags_of(kind: DesignKind, p: &DesignProps) -> u32 {
    let mut ff = 0;
    if p.read_only {
        ff |= FF_READ_ONLY;
    }
    if p.required {
        ff |= FF_REQUIRED;
    }
    match kind {
        DesignKind::Text => {
            if p.multiline {
                ff |= FF_MULTILINE;
            }
            if p.password {
                ff |= FF_PASSWORD;
            }
            if p.comb {
                ff |= FF_COMB;
            }
        }
        DesignKind::Combo => {
            ff |= FF_COMBO;
            if p.editable {
                ff |= FF_EDIT;
            }
        }
        DesignKind::List => {
            if p.multiselect {
                ff |= FF_MULTISELECT;
            }
        }
        DesignKind::Radio => ff |= FF_RADIO | FF_NO_TOGGLE_TO_OFF,
        DesignKind::Checkbox | DesignKind::Signature => {}
    }
    ff
}

pub(crate) fn field_type(kind: DesignKind) -> &'static str {
    match kind {
        DesignKind::Text => "Tx",
        DesignKind::Combo | DesignKind::List => "Ch",
        DesignKind::Checkbox | DesignKind::Radio => "Btn",
        DesignKind::Signature => "Sig",
    }
}

/// `(/Helv 12 Tf 0 g)`: the default appearance of a text field or a list.
pub(crate) fn default_appearance(kind: DesignKind, font_size: f32) -> String {
    match kind {
        DesignKind::Checkbox | DesignKind::Radio => "(/ZaDb 0 Tf 0 g)".to_string(),
        _ => format!("(/Helv {} Tf 0 g)", num(font_size)),
    }
}

/// The `/Opt` array.
pub(crate) fn options_array(choices: &[String]) -> String {
    let mut s = String::from("[");
    for c in choices {
        s.push_str(&encode_text(c));
        s.push(' ');
    }
    s.push(']');
    s
}

/// The value of a choice field and, for a list, which places are picked.
pub(crate) fn choice_value(p: &DesignProps) -> Option<(String, Vec<usize>)> {
    if p.default_value.is_empty() {
        return None;
    }
    let picked: Vec<&str> = p.default_value.split('\n').collect();
    let places: Vec<usize> = picked
        .iter()
        .filter_map(|l| p.choices.iter().position(|c| c == l))
        .collect();
    let text = if p.multiselect && picked.len() != 1 {
        let mut s = String::from("[");
        for l in &picked {
            s.push_str(&encode_text(l));
            s.push(' ');
        }
        s.push(']');
        s
    } else {
        encode_text(picked[0])
    };
    Some((text, places))
}

fn places_array(places: &[usize]) -> String {
    let mut sorted = places.to_vec();
    sorted.sort_unstable();
    let mut s = String::from("[");
    for p in sorted {
        s.push_str(&format!("{p} "));
    }
    s.push(']');
    s
}

impl Author {
    // ---- the fonts and the form's resources -------------------------------------------------

    fn helv_font(&mut self) -> Result<u32, AuthorError> {
        if let Some(n) = self.helv {
            return Ok(n);
        }
        let n = self.alloc()?;
        self.put(
            n,
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
        );
        self.helv = Some(n);
        Ok(n)
    }

    fn zadb_font(&mut self) -> Result<u32, AuthorError> {
        if let Some(n) = self.zadb {
            return Ok(n);
        }
        let n = self.alloc()?;
        self.put(
            n,
            "<< /Type /Font /Subtype /Type1 /BaseFont /ZapfDingbats >>",
        );
        self.zadb = Some(n);
        Ok(n)
    }

    /// Makes sure the form has the fonts the new fields name in their default appearance (Helvetica
    /// with WinAnsi, which has the Nordic letters, and ZapfDingbats for the marks of buttons), and
    /// a default appearance of its own. A font the form already has under the name is the form's
    /// own and is left as it is; a font object is only made for a name that is not there.
    pub(crate) fn ensure_resources(&mut self) -> Result<(), AuthorError> {
        if self.fonts_in_dr {
            return Ok(());
        }
        let mut dict = self.acro_mut()?.dict.clone();
        if get(&dict, "DA").is_none() {
            dict = set(&dict, "DA", b"(/Helv 0 Tf 0 g)").ok_or_else(|| odd("a damaged form"))?;
        }
        match get(&dict, "DR").map(<[u8]>::to_vec) {
            None => {
                let helv = self.helv_font()?;
                let zadb = self.zadb_font()?;
                let dr = format!("<< /Font << /Helv {helv} 0 R /ZaDb {zadb} 0 R >> >>");
                dict = set(&dict, "DR", dr.as_bytes()).ok_or_else(|| odd("a damaged form"))?;
            }
            Some(dr) => {
                let (dr_obj, dr_text) = match parse_ref(&dr) {
                    Some((n, _)) => {
                        self.claim(n, Role::Resources)?;
                        (Some(n), self.revisable(n)?)
                    }
                    None => (None, dr),
                };
                // The new text of the resources, when they change.
                let new_dr: Option<Vec<u8>> = match get(&dr_text, "Font").map(<[u8]>::to_vec) {
                    None => {
                        let helv = self.helv_font()?;
                        let zadb = self.zadb_font()?;
                        let fonts = format!("<< /Helv {helv} 0 R /ZaDb {zadb} 0 R >>");
                        Some(
                            set(&dr_text, "Font", fonts.as_bytes())
                                .ok_or_else(|| odd("damaged resources"))?,
                        )
                    }
                    Some(font) => {
                        let (font_obj, font_text) = match parse_ref(&font) {
                            Some((n, _)) => {
                                self.claim(n, Role::Fonts)?;
                                (Some(n), self.revisable(n)?)
                            }
                            None => (None, font),
                        };
                        let mut new_font = font_text.clone();
                        for key in ["Helv", "ZaDb"] {
                            match get(&new_font, key).map(<[u8]>::to_vec) {
                                None => {
                                    let obj = if key == "Helv" {
                                        self.helv_font()?
                                    } else {
                                        self.zadb_font()?
                                    };
                                    new_font = set(&new_font, key, format!("{obj} 0 R").as_bytes())
                                        .ok_or_else(|| odd("a damaged list of fonts"))?;
                                }
                                // The picture of a place to sign uses the form's own font.
                                Some(have) if key == "Helv" => {
                                    if let Some((n, _)) = parse_ref(&have) {
                                        self.helv = Some(n);
                                    }
                                }
                                Some(_) => {}
                            }
                        }
                        if new_font == font_text {
                            None
                        } else {
                            match font_obj {
                                Some(n) => {
                                    self.put(n, new_font);
                                    None
                                }
                                None => Some(
                                    set(&dr_text, "Font", &new_font)
                                        .ok_or_else(|| odd("damaged resources"))?,
                                ),
                            }
                        }
                    }
                };
                if let Some(new_dr) = new_dr {
                    match dr_obj {
                        Some(n) => self.put(n, new_dr),
                        None => {
                            dict =
                                set(&dict, "DR", &new_dr).ok_or_else(|| odd("a damaged form"))?;
                        }
                    }
                }
            }
        }
        let acro = self.acro_mut()?;
        acro.dict = dict;
        acro.touched = true;
        self.fonts_in_dr = true;
        Ok(())
    }

    // ---- pieces of a widget -----------------------------------------------------------------

    fn page_ref(&self, page_obj: u32) -> String {
        self.file.reference(page_obj)
    }

    fn widget_common(&self, w: &Placed) -> String {
        format!(
            "/Type /Annot /Subtype /Widget /Rect {} /P {} /F 4",
            num_box(&w.user),
            self.page_ref(w.page_obj)
        )
    }

    pub(crate) fn mk(turns: u8, caption: Option<&str>, border: &str) -> String {
        let mut mk = format!("/MK << /BC [{border}] ");
        if let Some(c) = caption {
            mk.push_str(&format!("/CA ({c}) "));
        }
        if !turns.is_multiple_of(4) {
            mk.push_str(&format!("/R {} ", u32::from(turns % 4) * 90));
        }
        mk.push_str(">> /BS << /W 1 /S /S >>");
        mk
    }

    /// The pictures of a check box or a radio button: one for each state and one for off.
    /// Returns the `/AP` entry.
    pub(crate) fn button_ap(
        &mut self,
        kind: DesignKind,
        w: &Placed,
        state: &[u8],
    ) -> Result<String, AuthorError> {
        let (dw, dh) = w.drawn();
        let (on, off) = if kind == DesignKind::Radio {
            (appearance::radio_on(dw, dh), appearance::radio_off(dw, dh))
        } else {
            (appearance::check_on(dw, dh), appearance::check_off(dw, dh))
        };
        let a = self.alloc()?;
        self.put(a, appearance::form_object(dw, dh, w.turns, "", &on));
        let b = self.alloc()?;
        self.put(b, appearance::form_object(dw, dh, w.turns, "", &off));
        Ok(format!(
            "/AP << /N << /{} {a} 0 R /Off {b} 0 R >> >>",
            encode_name(state)
        ))
    }

    /// The picture of an empty place to sign. Returns the `/AP` entry.
    pub(crate) fn signature_ap(&mut self, w: &Placed) -> Result<String, AuthorError> {
        let helv = self.helv_font()?;
        let (dw, dh) = w.drawn();
        let n = self.alloc()?;
        self.put(
            n,
            appearance::form_object(
                dw,
                dh,
                w.turns,
                &format!("/Font << /Helv {helv} 0 R >>"),
                &appearance::signature(dw, dh),
            ),
        );
        Ok(format!("/AP << /N {n} 0 R >>"))
    }

    // ---- a whole field ----------------------------------------------------------------------

    /// Writes the objects of a new field and returns the object that goes in the form's list.
    /// `widgets` also get the object numbers: the caller puts them in the lists of their pages.
    pub(crate) fn build_field(
        &mut self,
        f: &DesignField,
        placed: &[Placed],
    ) -> Result<(u32, Vec<u32>), AuthorError> {
        let p = &f.props;
        let name = encode_text(&f.name);
        let tu = if p.tooltip.is_empty() {
            String::new()
        } else {
            format!(" /TU {}", encode_text(&p.tooltip))
        };
        let ff = flags_of(f.kind, p);
        let ft = field_type(f.kind);
        let da = default_appearance(f.kind, p.font_size);
        let black = "0 0 0";
        match f.kind {
            DesignKind::Radio => {
                let parent = self.alloc()?;
                let on = p.default_value.as_str();
                let mut kids = Vec::new();
                let mut kid_refs = String::new();
                for w in placed {
                    let n = self.radio_kid(parent, w, on)?;
                    kid_refs.push_str(&format!("{n} 0 R "));
                    kids.push(n);
                }
                let value = if on.is_empty() {
                    "/V /Off".to_string()
                } else {
                    let e = encode_name(on.as_bytes());
                    format!("/V /{e} /DV /{e}")
                };
                self.put(
                    parent,
                    format!("<< /FT /{ft} /Ff {ff} /T {name}{tu} {value} /Kids [{kid_refs}] >>"),
                );
                Ok((parent, kids))
            }
            _ => {
                let w = placed
                    .first()
                    .ok_or_else(|| odd("a field without a place"))?;
                let n = self.alloc()?;
                let mut body = format!(
                    "<< {} /FT /{ft} /T {name}{tu} /Ff {ff}",
                    self.widget_common(w)
                );
                match f.kind {
                    DesignKind::Text => {
                        body.push_str(&format!(" /DA {da}"));
                        if p.max_len > 0 {
                            body.push_str(&format!(" /MaxLen {}", p.max_len));
                        }
                        if !p.default_value.is_empty() {
                            let v = encode_text(&p.default_value);
                            body.push_str(&format!(" /V {v} /DV {v}"));
                        }
                        body.push_str(&format!(" {}", Self::mk(w.turns, None, black)));
                    }
                    DesignKind::Combo | DesignKind::List => {
                        body.push_str(&format!(" /DA {da} /Opt {}", options_array(&p.choices)));
                        if let Some((v, places)) = choice_value(p) {
                            body.push_str(&format!(" /V {v} /DV {v}"));
                            if f.kind == DesignKind::List && !places.is_empty() {
                                body.push_str(&format!(" /I {}", places_array(&places)));
                            }
                        }
                        body.push_str(&format!(" {}", Self::mk(w.turns, None, black)));
                    }
                    DesignKind::Checkbox => {
                        let export = if p.export_value.is_empty() {
                            "Yes"
                        } else {
                            p.export_value.as_str()
                        };
                        let ap = self.button_ap(f.kind, w, export.as_bytes())?;
                        let e = encode_name(export.as_bytes());
                        let on = p.default_value == "on";
                        if on {
                            body.push_str(&format!(" /V /{e} /DV /{e} /AS /{e}"));
                        } else {
                            body.push_str(" /V /Off /AS /Off");
                        }
                        body.push_str(&format!(
                            " {ap} /DA {da} {}",
                            Self::mk(w.turns, Some("4"), black)
                        ));
                    }
                    DesignKind::Signature => {
                        let ap = self.signature_ap(w)?;
                        body.push_str(&format!(" {ap} {}", Self::mk(w.turns, None, "0.4 0.4 0.4")));
                    }
                    DesignKind::Radio => unreachable!("handled above"),
                }
                body.push_str(" >>");
                self.put(n, body);
                Ok((n, vec![n]))
            }
        }
    }

    /// One button of a radio group, on when its word is `on`.
    pub(crate) fn radio_kid(
        &mut self,
        parent: u32,
        w: &Placed,
        on: &str,
    ) -> Result<u32, AuthorError> {
        let n = self.alloc()?;
        let ap = self.button_ap(DesignKind::Radio, w, w.export.as_bytes())?;
        let state = if w.export == on {
            encode_name(w.export.as_bytes())
        } else {
            "Off".to_string()
        };
        let body = format!(
            "<< {} /Parent {parent} 0 R /AS /{state} {ap} {} /DA {} >>",
            self.widget_common(w),
            Self::mk(w.turns, Some("l"), "0 0 0"),
            default_appearance(DesignKind::Radio, 0.0),
        );
        self.put(n, body);
        Ok(n)
    }

    /// Gives the form's list of fields a new entry.
    pub(crate) fn add_to_fields(&mut self, obj: u32) -> Result<(), AuthorError> {
        let acro = self.acro_mut()?;
        let list = acro
            .fields
            .as_mut()
            .ok_or_else(|| odd("no list of fields"))?;
        list.items.push(format!("{obj} 0 R").into_bytes());
        if list.place == Place::Missing {
            list.place = Place::Direct;
        }
        acro.touched = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_follow_the_properties() {
        let mut p = DesignProps {
            required: true,
            multiline: true,
            comb: true,
            ..DesignProps::default()
        };
        assert_eq!(
            flags_of(DesignKind::Text, &p),
            FF_REQUIRED | FF_MULTILINE | FF_COMB
        );
        assert_eq!(flags_of(DesignKind::Checkbox, &p), FF_REQUIRED);
        p.editable = true;
        assert_eq!(
            flags_of(DesignKind::Combo, &p),
            FF_REQUIRED | FF_COMBO | FF_EDIT
        );
        assert_ne!(flags_of(DesignKind::Radio, &p) & FF_RADIO, 0);
        p.multiselect = true;
        assert_ne!(flags_of(DesignKind::List, &p) & FF_MULTISELECT, 0);
    }

    #[test]
    fn choices_are_written_as_the_value_wants_them() {
        let mut p = DesignProps {
            choices: vec!["a".into(), "b".into(), "c".into()],
            default_value: "b".into(),
            ..DesignProps::default()
        };
        assert_eq!(choice_value(&p), Some(("(b)".into(), vec![1])));
        p.multiselect = true;
        p.default_value = "a\nc".into();
        let (text, places) = choice_value(&p).unwrap();
        assert_eq!(text, "[(a) (c) ]");
        assert_eq!(places, vec![0, 2]);
        p.default_value.clear();
        assert!(choice_value(&p).is_none());
        assert_eq!(places_array(&[2, 0]), "[0 2 ]");
    }
}
