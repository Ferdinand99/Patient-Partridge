//! The layers that read bytes and tool output: raw, qdf, carve, text, images. The structure layer
//! has its own file.

use std::ffi::OsStr;
use std::path::Path;

use crate::carve;
use crate::search::{BufHit, NeedleSet};
use crate::tools;
use crate::{Hit, Layer, NeedleKind, Options, Report, Result, Spec, Subject};

pub struct Ctx<'a> {
    pub path: &'a Path,
    pub data: &'a [u8],
    pub spec: &'a Spec,
    pub opts: &'a Options,
    pub set: &'a NeedleSet,
}

impl Ctx<'_> {
    fn on(&self, layer: Layer) -> bool {
        self.opts.layers.contains(&layer)
    }
}

pub fn push(rep: &mut Report, layer: Layer, h: &BufHit, location: String) {
    rep.hits.push(Hit {
        layer,
        subject: Subject::Needle(h.needle),
        offset: Some(h.offset),
        location,
        form: Some(h.form),
    });
}

pub fn run(ctx: &Ctx, rep: &mut Report) -> Result<()> {
    rep.layers_run = Layer::ALL.into_iter().filter(|l| ctx.on(*l)).collect();
    if ctx.on(Layer::Raw) {
        raw(ctx, rep);
    }
    if ctx.on(Layer::Qdf) {
        qdf(ctx, rep)?;
    }
    if ctx.on(Layer::Carve) || ctx.on(Layer::Images) {
        carve_pass(ctx, rep);
    }
    if ctx.on(Layer::Text) {
        text(ctx, rep)?;
    }
    if ctx.on(Layer::Images) {
        pdfimages(ctx, rep)?;
    }
    if ctx.on(Layer::Structure) {
        crate::structure::run(ctx, rep)?;
    }
    rep.hits
        .sort_by_key(|h| (h.layer, h.subject_key(), h.offset));
    Ok(())
}

impl Hit {
    fn subject_key(&self) -> String {
        match &self.subject {
            Subject::Needle(i) => format!("n{i:06}"),
            Subject::Rule(r) => format!("r{r}"),
        }
    }
}

/// The file as written. Streams that have a filter are left to the carve layer: they are
/// compressed, and a scan for strings in them would only find noise.
fn raw(ctx: &Ctx, rep: &mut Report) {
    if ctx.set.is_empty() {
        return;
    }
    let masks: Vec<(usize, usize)> = carve::find_streams(ctx.data)
        .iter()
        .filter(|s| s.has_filter())
        .map(|s| (s.body.start, s.body.end))
        .collect();
    for h in ctx.set.search(ctx.data, &masks) {
        push(rep, Layer::Raw, &h, "file bytes".into());
    }
    for h in ctx.set.search_split(ctx.data) {
        push(rep, Layer::Raw, &h, "file bytes, text object".into());
    }
}

/// A copy normalised by qpdf: no object streams, every stream decompressed. qpdf also decrypts
/// a file whose user password is empty, which the other byte layers cannot.
fn qdf(ctx: &Ctx, rep: &mut Report) -> Result<()> {
    if ctx.set.is_empty() {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let out = dir.path().join("normalised.pdf");
    let r = tools::qpdf(&[
        OsStr::new("--qdf"),
        OsStr::new("--object-streams=disable"),
        OsStr::new("--stream-data=uncompress"),
        ctx.path.as_os_str(),
        out.as_os_str(),
    ])?;
    // 0 is success and 3 is success with warnings.
    let bytes = match std::fs::read(&out) {
        Ok(b) if r.status == 0 || r.status == 3 => b,
        _ => {
            rep.problems.push(format!(
                "qdf: qpdf could not normalise the file (exit {}): {}",
                r.status,
                first_line(&r.stderr)
            ));
            return Ok(());
        }
    };
    for h in ctx.set.search(&bytes, &[]) {
        push(rep, Layer::Qdf, &h, "qpdf-normalised copy".into());
    }
    for h in ctx.set.search_split(&bytes) {
        push(
            rep,
            Layer::Qdf,
            &h,
            "qpdf-normalised copy, text object".into(),
        );
    }
    Ok(())
}

fn first_line(s: &str) -> &str {
    s.lines().find(|l| !l.trim().is_empty()).unwrap_or("")
}

/// Every stream found by scanning. Gives the `carve` hits for text and the `images` hits for
/// image hashes that match the stream as stored or with its filters undone.
fn carve_pass(ctx: &Ctx, rep: &mut Report) {
    let want_images = ctx.on(Layer::Images);
    let image_needles: Vec<(usize, &str)> = ctx
        .spec
        .needles
        .iter()
        .enumerate()
        .filter(|(_, n)| n.kind == NeedleKind::ImageSha256)
        .map(|(i, n)| (i, n.value.as_str()))
        .collect();
    carve::carve(ctx.data, &mut |c| {
        if ctx.on(Layer::Carve) {
            let place = format!("stream {}", c.describe());
            for h in ctx.set.search(&c.decoded.data, &[]) {
                push(rep, Layer::Carve, &h, place.clone());
            }
            for h in ctx.set.search_split(&c.decoded.data) {
                push(rep, Layer::Carve, &h, format!("{place}, text object"));
            }
        }
        if want_images && c.is_image && !image_needles.is_empty() {
            let stored = carve::sha256_hex(c.raw);
            let decoded = carve::sha256_hex(&c.decoded.data);
            for (i, want) in &image_needles {
                let how = if *want == stored {
                    "as stored"
                } else if *want == decoded {
                    "with filters undone"
                } else {
                    continue;
                };
                rep.hits.push(Hit {
                    layer: Layer::Images,
                    subject: Subject::Needle(*i),
                    offset: Some(c.body_offset),
                    location: format!("image {} {how}", c.describe()),
                    form: None,
                });
            }
        }
    });
}

/// What a reader can copy from the pages, in three reading orders.
fn text(ctx: &Ctx, rep: &mut Report) -> Result<()> {
    if ctx.set.is_empty() {
        return Ok(());
    }
    for mode in ["", "-raw", "-layout"] {
        let mut args: Vec<&OsStr> = vec![OsStr::new("-enc"), OsStr::new("UTF-8")];
        if !mode.is_empty() {
            args.push(OsStr::new(mode));
        }
        args.push(ctx.path.as_os_str());
        args.push(OsStr::new("-"));
        let out = tools::pdftotext(&args)?;
        let label = if mode.is_empty() {
            "reading order"
        } else {
            mode
        };
        if out.status != 0 {
            rep.problems.push(format!(
                "text: pdftotext {label} failed (exit {}): {}",
                out.status,
                first_line(&out.stderr)
            ));
            continue;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        for h in ctx.set.search_text(&text) {
            let page = text.as_bytes()[..h.offset.min(text.len())]
                .iter()
                .filter(|b| **b == 0x0c)
                .count()
                + 1;
            push(
                rep,
                Layer::Text,
                &h,
                format!("pdftotext {label}, page {page}"),
            );
        }
    }
    Ok(())
}

/// Pixels as Poppler draws them: `pdfimages` writes each image of the pages as PNM, so an image
/// that was stored in another colour space or with other filters still has the same hash.
fn pdfimages(ctx: &Ctx, rep: &mut Report) -> Result<()> {
    let wanted: Vec<(usize, &str)> = ctx
        .spec
        .needles
        .iter()
        .enumerate()
        .filter(|(_, n)| n.kind == NeedleKind::ImageSha256)
        .map(|(i, n)| (i, n.value.as_str()))
        .collect();
    if wanted.is_empty() {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let prefix = dir.path().join("img");
    let out = tools::pdfimages(&[ctx.path.as_os_str(), prefix.as_os_str()])?;
    if out.status != 0 {
        rep.problems.push(format!(
            "images: pdfimages failed (exit {}): {}",
            out.status,
            first_line(&out.stderr)
        ));
        return Ok(());
    }
    let mut names: Vec<_> = std::fs::read_dir(dir.path())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    names.sort();
    for path in names {
        let bytes = std::fs::read(&path)?;
        let Some(payload) = pnm_payload(&bytes) else {
            continue;
        };
        let hash = carve::sha256_hex(payload);
        for (i, want) in &wanted {
            if *want == hash {
                rep.hits.push(Hit {
                    layer: Layer::Images,
                    subject: Subject::Needle(*i),
                    offset: None,
                    location: format!(
                        "image drawn by pdfimages ({})",
                        path.file_name()
                            .map_or(String::new(), |n| n.to_string_lossy().into_owned())
                    ),
                    form: None,
                });
            }
        }
    }
    Ok(())
}

/// The pixel bytes of a binary PNM file (P4, P5, P6).
pub fn pnm_payload(b: &[u8]) -> Option<&[u8]> {
    let mut pos = 0usize;
    let token = |pos: &mut usize| -> Option<&[u8]> {
        loop {
            while *pos < b.len() && b[*pos].is_ascii_whitespace() {
                *pos += 1;
            }
            if b.get(*pos) == Some(&b'#') {
                while *pos < b.len() && b[*pos] != b'\n' {
                    *pos += 1;
                }
            } else {
                break;
            }
        }
        let s = *pos;
        while *pos < b.len() && !b[*pos].is_ascii_whitespace() {
            *pos += 1;
        }
        (*pos > s).then(|| &b[s..*pos])
    };
    let magic = token(&mut pos)?;
    let numbers = match magic {
        b"P4" => 2,
        b"P5" | b"P6" => 3,
        _ => return None,
    };
    for _ in 0..numbers {
        token(&mut pos)?;
    }
    // One white-space byte separates the header from the pixels.
    b.get(pos + 1..)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pnm_header_is_skipped() {
        assert_eq!(pnm_payload(b"P6\n2 1\n255\nabcdef"), Some(&b"abcdef"[..]));
        assert_eq!(pnm_payload(b"P5\n# comment\n1 1 255\nx"), Some(&b"x"[..]));
        assert_eq!(pnm_payload(b"P4\n8 1\n\xff"), Some(&b"\xff"[..]));
        assert_eq!(pnm_payload(b"junk"), None);
    }
}
