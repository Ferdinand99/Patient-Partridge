//! The structure layer: the objects of the document as qpdf reads them (`qpdf --json`), with
//! strings, names and dictionary keys decoded. This finds what the byte layers cannot match
//! because the bytes are written another way: a name with `#2D` for `-`, a string in
//! PDFDocEncoding or UTF-16 with any escape. It also checks the rules of the spec: forbidden
//! keys (`/Metadata`, `/Thumb`, ..) and the number of revisions, and it reads the XMP packets.

use std::collections::BTreeSet;
use std::ffi::OsStr;

use serde_json::{Map, Value};

use crate::layers::{push, Ctx};
use crate::search::find_all;
use crate::tools;
use crate::{Hit, Layer, Report, Result, Subject};

pub fn run(ctx: &Ctx, rep: &mut Report) -> Result<()> {
    if ctx.opts.check_valid {
        check(ctx, rep)?;
    }
    if ctx.spec.single_revision {
        revisions(ctx, rep);
    }
    let out = tools::qpdf(&[
        OsStr::new("--json=2"),
        OsStr::new("--json-key=qpdf"),
        OsStr::new("--json-stream-data=none"),
        ctx.path.as_os_str(),
    ])?;
    if out.status != 0 && out.status != 3 {
        rep.problems.push(format!(
            "structure: qpdf could not read the structure (exit {}): {}",
            out.status,
            out.stderr.lines().next().unwrap_or("")
        ));
        return Ok(());
    }
    let doc: Value = match serde_json::from_slice(&out.stdout) {
        Ok(v) => v,
        Err(e) => {
            rep.problems
                .push(format!("structure: the qpdf JSON cannot be read: {e}"));
            return Ok(());
        }
    };
    let Some(objects) = doc.get("qpdf").and_then(Value::as_array).and_then(|a| {
        a.iter()
            .filter_map(Value::as_object)
            .find(|m| m.contains_key("trailer") || m.keys().any(|k| k.starts_with("obj:")))
    }) else {
        rep.problems
            .push("structure: the qpdf JSON has no objects".into());
        return Ok(());
    };
    let mut w = Walk {
        ctx,
        objects,
        metadata: BTreeSet::new(),
        hits: Vec::new(),
    };
    for (key, entry) in objects {
        let (label, value) = if key == "trailer" {
            ("trailer".to_owned(), entry.get("value"))
        } else if let Some(id) = key.strip_prefix("obj:") {
            let id = id.strip_suffix(" R").unwrap_or(id);
            let v = entry
                .get("value")
                .or_else(|| entry.get("stream").and_then(|s| s.get("dict")));
            (format!("object {id}"), v)
        } else {
            continue;
        };
        if let Some(v) = value {
            w.walk(v, &label);
        }
    }
    let metadata = std::mem::take(&mut w.metadata);
    rep.hits.append(&mut w.hits);
    for id in metadata {
        xmp(ctx, rep, &id)?;
    }
    Ok(())
}

struct Walk<'a> {
    ctx: &'a Ctx<'a>,
    objects: &'a Map<String, Value>,
    /// Object numbers that a `/Metadata` entry points to.
    metadata: BTreeSet<String>,
    hits: Vec<Hit>,
}

/// `5 0 R` as a string value of the JSON.
fn reference(s: &str) -> Option<&str> {
    let t = s.strip_suffix(" R")?;
    let (num, gen) = t.split_once(' ')?;
    (num.bytes().all(|b| b.is_ascii_digit())
        && !num.is_empty()
        && gen.bytes().all(|b| b.is_ascii_digit())
        && !gen.is_empty())
    .then_some(t)
}

impl Walk<'_> {
    fn walk(&mut self, v: &Value, path: &str) {
        match v {
            Value::Object(m) => {
                for (k, val) in m {
                    let name = k.strip_prefix('/').unwrap_or(k);
                    let here = format!("{path} {k}");
                    self.text(name, &format!("{here} (key)"));
                    if self.ctx.spec.forbidden_keys.iter().any(|f| f == k) && !self.is_empty(val, 0)
                    {
                        self.hits.push(Hit {
                            layer: Layer::Structure,
                            subject: Subject::Rule(format!("{k} is present and not empty")),
                            offset: None,
                            location: here.clone(),
                            form: None,
                        });
                    }
                    if k == "/Metadata" {
                        if let Some(r) = val.as_str().and_then(reference) {
                            self.metadata.insert(r.to_owned());
                        }
                    }
                    self.walk(val, &here);
                }
            }
            Value::Array(a) => {
                for (i, val) in a.iter().enumerate() {
                    self.walk(val, &format!("{path}[{i}]"));
                }
            }
            Value::String(s) => {
                if let Some(t) = s.strip_prefix("u:") {
                    self.text(t, path);
                } else if let Some(h) = s.strip_prefix("b:") {
                    self.binary(&unhex(h), path);
                } else if let Some(n) = s.strip_prefix('/') {
                    self.text(n, &format!("{path} (name)"));
                }
            }
            _ => {}
        }
    }

    fn text(&mut self, value: &str, location: &str) {
        for h in self.ctx.set.search_value(value) {
            self.hits.push(Hit {
                layer: Layer::Structure,
                subject: Subject::Needle(h.needle),
                offset: Some(h.offset),
                location: location.to_owned(),
                form: Some(h.form),
            });
        }
    }

    fn binary(&mut self, value: &[u8], location: &str) {
        for h in self.ctx.set.search_value_bytes(value) {
            self.hits.push(Hit {
                layer: Layer::Structure,
                subject: Subject::Needle(h.needle),
                offset: Some(h.offset),
                location: location.to_owned(),
                form: Some(h.form),
            });
        }
    }

    /// Whether a value holds nothing: null, an empty string, array or dictionary, or a
    /// reference to such a value. A stream is never empty.
    fn is_empty(&self, v: &Value, depth: usize) -> bool {
        if depth > 8 {
            return false;
        }
        match v {
            Value::Null => true,
            Value::String(s) => {
                if let Some(t) = s.strip_prefix("u:") {
                    t.is_empty()
                } else if let Some(h) = s.strip_prefix("b:") {
                    h.is_empty()
                } else if let Some(r) = reference(s) {
                    match self.objects.get(&format!("obj:{r} R")) {
                        Some(e) => match e.get("value") {
                            Some(inner) => self.is_empty(inner, depth + 1),
                            None => false,
                        },
                        None => true,
                    }
                } else {
                    false
                }
            }
            Value::Array(a) => a.iter().all(|x| self.is_empty(x, depth + 1)),
            Value::Object(m) => m.values().all(|x| self.is_empty(x, depth + 1)),
            Value::Bool(_) | Value::Number(_) => false,
        }
    }
}

fn unhex(s: &str) -> Vec<u8> {
    let nibble = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    };
    s.as_bytes()
        .chunks(2)
        .filter_map(|p| Some(nibble(p[0])? << 4 | nibble(*p.get(1)?)?))
        .collect()
}

/// The XMP packet that `/Metadata` points to, with its filters undone by qpdf.
fn xmp(ctx: &Ctx, rep: &mut Report, id: &str) -> Result<()> {
    let number = id.split(' ').next().unwrap_or(id);
    let out = tools::qpdf(&[
        OsStr::new(&format!("--show-object={number}")),
        OsStr::new("--filtered-stream-data"),
        ctx.path.as_os_str(),
    ])?;
    if out.status != 0 && out.status != 3 {
        rep.problems.push(format!(
            "structure: qpdf cannot show the metadata stream {id} (exit {})",
            out.status
        ));
        return Ok(());
    }
    for h in ctx.set.search(&out.stdout, &[]) {
        push(
            rep,
            Layer::Structure,
            &h,
            format!("XMP metadata, object {id}"),
        );
    }
    Ok(())
}

/// `qpdf --check`: the file must be one that other readers accept.
fn check(ctx: &Ctx, rep: &mut Report) -> Result<()> {
    let out = tools::qpdf(&[OsStr::new("--check"), ctx.path.as_os_str()])?;
    if out.status == 0 {
        return Ok(());
    }
    let text = format!("{}\n{}", out.stderr, String::from_utf8_lossy(&out.stdout));
    let line = text
        .lines()
        .find(|l| l.contains("WARNING") || l.contains("ERROR") || l.contains("error"))
        .or_else(|| text.lines().find(|l| !l.trim().is_empty()))
        .unwrap_or("");
    rep.problems.push(format!(
        "structure: qpdf --check exit {}: {}",
        out.status,
        line.trim()
    ));
    Ok(())
}

/// One `%%EOF` and one `startxref`: no old revision hides behind the last one.
fn revisions(ctx: &Ctx, rep: &mut Report) {
    for marker in ["%%EOF", "startxref"] {
        let at = find_all(ctx.data, marker.as_bytes());
        if at.len() > 1 {
            rep.hits.push(Hit {
                layer: Layer::Structure,
                subject: Subject::Rule(format!(
                    "{} {marker} markers: the file has more than one revision",
                    at.len()
                )),
                offset: Some(at[1]),
                location: "file bytes".into(),
                form: None,
            });
        }
    }
}
