//! What a field may be called, hold and look like. The window checks the same things to tell the
//! reader early, but the window is not trusted: these are the rules.
//!
//! The messages are for the reader of the document. They say what is wrong and what to do.

use partridge_protocol::{
    DesignKind, DesignProps, MAX_COMB_LENGTH, MAX_EXPORT_VALUE, MAX_FIELD_LENGTH, MAX_FORM_NAME,
    MAX_FORM_OPTIONS, MAX_FORM_OPTION_LABEL, MAX_FORM_TOOLTIP, MAX_FORM_VALUE, MIN_WIDGET_SIDE,
};

use super::geometry::PageGeo;
use super::AuthorError;

pub fn refused(message: impl Into<String>) -> AuthorError {
    AuthorError::Refused(message.into())
}

/// Longest field name the designer accepts, in bytes.
pub const MAX_NAME_BYTES: usize = 255;

/// Largest font size, in points.
pub const MAX_FONT_SIZE: f32 = 96.0;

/// Characters the standard font can draw: Latin-1 (which has \u{e6}, \u{f8} and \u{e5}), and the
/// line break where a field may hold more than one line.
pub fn drawable(c: char, newline: bool) -> bool {
    matches!(c, ' '..='~' | '\u{a0}'..='\u{ff}') || (newline && c == '\n')
}

/// Says which character cannot be drawn, if one cannot.
pub fn check_drawable(what: &str, text: &str, newline: bool) -> Result<(), AuthorError> {
    match text.chars().find(|c| !drawable(*c, newline)) {
        None => Ok(()),
        Some(c) => Err(refused(format!(
            "{what} has a character that cannot be drawn in a field yet ({}). Use letters from the Latin alphabet, such as \u{e6}, \u{f8} and \u{e5}.",
            show(c)
        ))),
    }
}

fn show(c: char) -> String {
    if c.is_control() {
        format!("U+{:04X}", c as u32)
    } else {
        format!("\u{201c}{c}\u{201d}")
    }
}

/// A name that can stand in a form. Names are compared as written, so two names that differ only
/// in case are two names.
pub fn check_name(name: &str) -> Result<(), AuthorError> {
    if name.trim().is_empty() {
        return Err(refused("Give the field a name."));
    }
    if name.contains('.') {
        return Err(refused(
            "A field name cannot contain a dot, because a dot makes the field a part of another field. Use a dash or an underscore instead.",
        ));
    }
    if name != name.trim() {
        return Err(refused("A field name cannot start or end with a space."));
    }
    if name.len() > MAX_NAME_BYTES || name.len() > MAX_FORM_NAME {
        return Err(refused(format!(
            "The field name is too long. Keep it under {MAX_NAME_BYTES} bytes."
        )));
    }
    if let Some(c) = name.chars().find(|c| is_unsafe_in_text(*c)) {
        return Err(refused(format!(
            "The field name has a character that cannot be used ({}).",
            show(c)
        )));
    }
    Ok(())
}

/// Controls, and the characters that change the direction of the text around them or hide.
pub fn is_unsafe_in_text(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{200E}'
                | '\u{200F}'
                | '\u{061C}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
                | '\u{200B}'
                | '\u{FEFF}'
                | '\u{2028}'
                | '\u{2029}'
        )
}

pub fn check_tooltip(text: &str) -> Result<(), AuthorError> {
    if text.len() > MAX_FORM_TOOLTIP {
        return Err(refused(format!(
            "The tooltip is too long. Keep it under {MAX_FORM_TOOLTIP} bytes."
        )));
    }
    if let Some(c) = text.chars().find(|c| is_unsafe_in_text(*c) && *c != '\n') {
        return Err(refused(format!(
            "The tooltip has a character that cannot be used ({}).",
            show(c)
        )));
    }
    Ok(())
}

/// An export value: the word a check box or a radio button stands for.
pub fn check_export(value: &str) -> Result<(), AuthorError> {
    if value.is_empty() {
        return Err(refused("Give each radio button a value."));
    }
    if value == "Off" {
        return Err(refused(
            "\u{201c}Off\u{201d} means a button that is not picked, so it cannot be the value of a button. Use another word.",
        ));
    }
    if value.len() > MAX_EXPORT_VALUE {
        return Err(refused("A button value is too long."));
    }
    // A value is written as a name, which is bytes, and the readers that show it read the bytes in
    // their own encoding. Plain ASCII is the one spelling that every reader shows the same.
    if let Some(c) = value.chars().find(|c| !(' '..='~').contains(c)) {
        return Err(refused(format!(
            "A button value can only use the letters a to z, digits and ordinary signs, so {} cannot be used. Use a word without accents.",
            show(c)
        )));
    }
    Ok(())
}

/// A box on a page as it is drawn: inside the page (a hair of slack for rounding), not too small.
pub fn check_rect(rect: [f32; 4], geo: &PageGeo) -> Result<(), AuthorError> {
    if rect.iter().any(|v| !v.is_finite()) {
        return Err(refused(
            "The place of the field is not a place on the page.",
        ));
    }
    let (w, h) = geo.drawn_size();
    let slack = 0.5;
    if rect[0] < -slack || rect[1] < -slack || rect[2] > w + slack || rect[3] > h + slack {
        return Err(refused("The field has to be inside the page."));
    }
    if rect[2] - rect[0] < MIN_WIDGET_SIDE || rect[3] - rect[1] < MIN_WIDGET_SIDE {
        return Err(refused(format!(
            "The field is too small. Make it at least {MIN_WIDGET_SIDE} points on each side."
        )));
    }
    Ok(())
}

/// The choices of a drop-down or a list: how many, how long, and not the same twice.
pub fn check_choices(choices: &[String]) -> Result<(), AuthorError> {
    if choices.is_empty() {
        return Err(refused("Give the list at least one choice."));
    }
    if choices.len() > MAX_FORM_OPTIONS {
        return Err(refused(format!(
            "A list can have at most {MAX_FORM_OPTIONS} choices."
        )));
    }
    for (i, c) in choices.iter().enumerate() {
        if c.trim().is_empty() {
            return Err(refused("A choice cannot be empty."));
        }
        if c.len() > MAX_FORM_OPTION_LABEL {
            return Err(refused("A choice is too long."));
        }
        check_drawable("A choice", c, false)?;
        if choices[..i].contains(c) {
            return Err(refused(format!(
                "The choice \u{201c}{c}\u{201d} is there twice."
            )));
        }
    }
    Ok(())
}

/// What a kind of field may carry. The tooltip and the two flags every kind has are checked for all.
pub fn check_props(kind: DesignKind, p: &DesignProps) -> Result<(), AuthorError> {
    check_tooltip(&p.tooltip)?;
    if !(p.font_size == 0.0 || (1.0..=MAX_FONT_SIZE).contains(&p.font_size)) {
        return Err(refused(format!(
            "The size of the text has to be between 1 and {MAX_FONT_SIZE} points, or automatic."
        )));
    }
    if p.default_value.len() > MAX_FORM_VALUE {
        return Err(refused("The default value is too long."));
    }
    match kind {
        DesignKind::Text => {
            if p.max_len > MAX_FIELD_LENGTH {
                return Err(refused(format!(
                    "The longest text can be at most {MAX_FIELD_LENGTH} characters."
                )));
            }
            if p.comb {
                if p.max_len == 0 || p.max_len > MAX_COMB_LENGTH {
                    return Err(refused(format!(
                        "A field with a box for each character needs a longest text between 1 and {MAX_COMB_LENGTH}."
                    )));
                }
                if p.multiline || p.password {
                    return Err(refused(
                        "A field with a box for each character cannot also be a password or have several lines.",
                    ));
                }
            }
            if p.password && p.multiline {
                return Err(refused(
                    "A password field has one line. Turn off several lines or the password.",
                ));
            }
            check_drawable("The default value", &p.default_value, p.multiline)?;
            if p.max_len > 0 && p.default_value.chars().count() as u32 > p.max_len {
                return Err(refused(
                    "The default value is longer than the longest text the field takes.",
                ));
            }
        }
        DesignKind::Checkbox => {
            if !p.export_value.is_empty() {
                check_export(&p.export_value)?;
            }
            if !(p.default_value.is_empty() || p.default_value == "on") {
                return Err(refused("A check box starts either on or off."));
            }
        }
        DesignKind::Radio => {}
        DesignKind::Combo | DesignKind::List => {
            check_choices(&p.choices)?;
            let picked: Vec<&str> = if p.default_value.is_empty() {
                Vec::new()
            } else {
                p.default_value.split('\n').collect()
            };
            let many = kind == DesignKind::List && p.multiselect;
            if picked.len() > 1 && !many {
                return Err(refused(
                    "Only a list that takes several choices can start with several picked.",
                ));
            }
            for label in &picked {
                let known = p.choices.iter().any(|c| c == label);
                if !known && !(kind == DesignKind::Combo && p.editable) {
                    return Err(refused(
                        "The choice that starts picked is not one of the choices.",
                    ));
                }
                check_drawable("The choice that starts picked", label, false)?;
            }
        }
        DesignKind::Signature => {}
    }
    Ok(())
}

/// The export values of a radio group: all given, none empty, none the same twice.
pub fn check_exports(exports: &[String], default: &str) -> Result<(), AuthorError> {
    for (i, e) in exports.iter().enumerate() {
        check_export(e)?;
        if exports[..i].contains(e) {
            return Err(refused(format!(
                "Two radio buttons of the group have the value \u{201c}{e}\u{201d}. Give each its own."
            )));
        }
    }
    if !default.is_empty() && !exports.iter().any(|e| e == default) {
        return Err(refused(
            "The button that starts picked is not one of the buttons.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text() -> DesignProps {
        DesignProps::default()
    }

    #[test]
    fn names_with_a_dot_or_nothing_are_refused() {
        assert!(check_name("first_name").is_ok());
        assert!(check_name("Navn \u{e6}\u{f8}\u{e5}").is_ok());
        assert!(check_name("").is_err());
        assert!(check_name("   ").is_err());
        assert!(check_name("a.b").is_err());
        assert!(check_name(".").is_err());
        assert!(check_name(" a").is_err());
        assert!(check_name("a ").is_err());
        assert!(check_name("a\u{0}b").is_err());
        assert!(check_name("a\u{202e}b").is_err());
        assert!(check_name(&"x".repeat(256)).is_err());
        assert!(check_name(&"x".repeat(255)).is_ok());
    }

    #[test]
    fn only_latin_1_is_drawn() {
        assert!(check_drawable(
            "x",
            "Bl\u{e5}b\u{e6}rsyltet\u{f8}y \u{c6}\u{d8}\u{c5} caf\u{e9}",
            false
        )
        .is_ok());
        assert!(check_drawable("x", "\u{20ac}", false).is_err());
        assert!(check_drawable("x", "\u{141}\u{f3}d\u{17a}", false).is_err());
        assert!(check_drawable("x", "\u{65e5}\u{672c}", false).is_err());
        assert!(check_drawable("x", "a\nb", false).is_err());
        assert!(check_drawable("x", "a\nb", true).is_ok());
        assert!(check_drawable("x", "\u{201c}q\u{201d}", false).is_err());
    }

    #[test]
    fn a_comb_needs_a_length() {
        let mut p = text();
        p.comb = true;
        assert!(check_props(DesignKind::Text, &p).is_err());
        p.max_len = 8;
        assert!(check_props(DesignKind::Text, &p).is_ok());
        p.max_len = MAX_COMB_LENGTH + 1;
        assert!(check_props(DesignKind::Text, &p).is_err());
        p.max_len = 8;
        p.password = true;
        assert!(check_props(DesignKind::Text, &p).is_err());
    }

    #[test]
    fn choices_must_be_there_and_differ() {
        let mut p = DesignProps::default();
        assert!(check_props(DesignKind::Combo, &p).is_err());
        p.choices = vec!["a".into(), "b".into()];
        assert!(check_props(DesignKind::Combo, &p).is_ok());
        p.choices.push("a".into());
        assert!(check_props(DesignKind::List, &p).is_err());
        p.choices.pop();
        p.default_value = "a\nb".into();
        assert!(check_props(DesignKind::List, &p).is_err());
        p.multiselect = true;
        assert!(check_props(DesignKind::List, &p).is_ok());
        p.default_value = "zzz".into();
        assert!(check_props(DesignKind::List, &p).is_err());
        p.editable = true;
        p.multiselect = false;
        assert!(check_props(DesignKind::Combo, &p).is_ok());
    }

    #[test]
    fn exports_are_all_different() {
        let e = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(check_exports(&e(&["S", "M", "L"]), "M").is_ok());
        assert!(check_exports(&e(&["S", "S"]), "").is_err());
        assert!(check_exports(&e(&["S", ""]), "").is_err());
        assert!(check_exports(&e(&["S", "Off"]), "").is_err());
        assert!(check_exports(&e(&["S", "M"]), "XL").is_err());
        // A value is plain ASCII: the same bytes in every reader.
        assert!(check_export("Yes please-2_x.y").is_ok());
        assert!(check_export("Ja \u{f8}").is_err());
        assert!(check_export("\u{65e5}").is_err());
        assert!(check_export("a\tb").is_err());
    }

    #[test]
    fn a_box_has_to_fit_the_page() {
        let geo = PageGeo {
            turns: 0,
            visible: [0.0, 0.0, 200.0, 300.0],
        };
        assert!(check_rect([10.0, 10.0, 100.0, 40.0], &geo).is_ok());
        assert!(check_rect([-5.0, 10.0, 100.0, 40.0], &geo).is_err());
        assert!(check_rect([10.0, 10.0, 201.0, 40.0], &geo).is_err());
        assert!(check_rect([10.0, 10.0, 12.0, 40.0], &geo).is_err());
        assert!(check_rect([10.0, f32::NAN, 100.0, 40.0], &geo).is_err());
        let turned = PageGeo { turns: 1, ..geo };
        assert!(check_rect([10.0, 10.0, 250.0, 40.0], &turned).is_ok());
        assert!(check_rect([10.0, 10.0, 250.0, 250.0], &turned).is_err());
    }

    /// The text of a line of `formrules.cases.txt`: `\uXXXX` is a character, `\n` a line break.
    fn unescape(text: &str) -> String {
        let mut out = String::new();
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    out.push(
                        char::from_u32(u32::from_str_radix(&hex, 16).expect("four hex digits"))
                            .expect("a character"),
                    );
                }
                Some('n') => out.push('\n'),
                other => panic!("an escape that is not known: {other:?}"),
            }
        }
        out
    }

    /// The window checks the same cases (src/lib/formdesign.test.ts reads the same file): the two
    /// sides have one set of rules, and a change to one that is not made to the other fails here or
    /// there.
    #[test]
    fn the_engine_agrees_with_the_cases_the_window_is_held_to() {
        let file = include_str!("../../../../src/lib/formrules.cases.txt");
        let mut seen = 0;
        for line in file.lines() {
            let line = line.trim_end_matches('\r');
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.splitn(3, '\t');
            let kind = parts.next().expect("a kind");
            let verdict = parts.next().expect("a verdict");
            let text = unescape(parts.next().unwrap_or(""));
            let props = |f: &dyn Fn(&mut DesignProps)| {
                let mut p = DesignProps::default();
                f(&mut p);
                p
            };
            let result = match kind {
                "name" => check_name(&text),
                "tooltip" => check_tooltip(&text),
                "export" => check_export(&text),
                "drawn" => check_drawable("x", &text, false),
                "multiline" => check_drawable("x", &text, true),
                "fontsize" => check_props(
                    DesignKind::Text,
                    &props(&|p| p.font_size = text.parse().expect("a number")),
                ),
                "maxlen" => check_props(
                    DesignKind::Text,
                    &props(&|p| p.max_len = text.parse().expect("a number")),
                ),
                other => panic!("a kind of case that is not known: {other}"),
            };
            assert_eq!(
                result.is_ok(),
                verdict == "ok",
                "{kind} {text:?}: {result:?}"
            );
            seen += 1;
        }
        assert!(seen > 40, "{seen} cases");
    }
}
