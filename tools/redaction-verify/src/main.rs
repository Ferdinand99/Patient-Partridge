//! Command line for the redaction verifier.
//!
//! ```text
//! redaction-verify [options] FILE.pdf
//!   --spec FILE            JSON spec (see Spec::from_json); the options below add to it
//!   --text S               a text that must be gone (also --metadata, --field, --bookmark,
//!   --attachment S         --image-sha256 HEX)
//!   --forbid-key /Key      a key that must be absent or empty
//!   --scrubbed             forbid the keys a redaction scrub empties, and more than one revision
//!   --single-revision      one %%EOF and one startxref
//!   --only L,L / --skip L,L   layers: raw, qdf, carve, text, structure, images
//!   --show-values          print the values of hit needles (default: kind, position, digest)
//! ```
//!
//! Exit status: 0 nothing found, 1 something found, 2 the check could not be done.

use std::path::PathBuf;
use std::process::ExitCode;

use redaction_verify::{verify, Layer, Options, Region, Spec};

fn usage() -> ExitCode {
    eprintln!(
        "usage: redaction-verify [--spec FILE] [--text S].. [--metadata S].. [--field S].. \
         [--bookmark S].. [--attachment S].. [--image-sha256 HEX].. [--forbid-key /K].. \
         [--scrubbed] [--single-revision] [--only LAYERS] [--skip LAYERS] [--show-values] FILE.pdf"
    );
    ExitCode::from(2)
}

fn layers(list: &str) -> Result<Vec<Layer>, String> {
    list.split(',')
        .map(|n| Layer::parse(n.trim()).ok_or_else(|| format!("unknown layer \"{n}\"")))
        .collect()
}

/// `PAGE:X0,Y0,X1,Y1` or `PAGE:X0,Y0,X1,Y1:TOLERANCE`.
fn region(s: &str) -> Result<Region, String> {
    let bad = || format!("--region wants PAGE:X0,Y0,X1,Y1[:TOLERANCE], not \"{s}\"");
    let mut parts = s.split(':');
    let page: usize = parts.next().and_then(|p| p.parse().ok()).ok_or_else(bad)?;
    let rect: Vec<f64> = parts
        .next()
        .ok_or_else(bad)?
        .split(',')
        .map(|n| n.trim().parse().map_err(|_| bad()))
        .collect::<Result<_, _>>()?;
    let rect: [f64; 4] = rect.try_into().map_err(|_| bad())?;
    let tolerance = match parts.next() {
        None => None,
        Some(t) => Some(t.parse().map_err(|_| bad())?),
    };
    Ok(Region {
        page,
        rect,
        tolerance,
    })
}

fn run() -> Result<ExitCode, String> {
    let mut args = std::env::args().skip(1);
    let mut spec = Spec::default();
    let mut file: Option<PathBuf> = None;
    let mut only: Option<Vec<Layer>> = None;
    let mut skip: Vec<Layer> = Vec::new();
    let mut show = false;
    let mut scrubbed = false;
    let mut single = false;
    let mut fonts = false;
    let mut password: Option<String> = None;
    while let Some(a) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} needs a value"));
        match a.as_str() {
            "--spec" => {
                let path = value("--spec")?;
                let json = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
                let from_file = Spec::from_json(&json).map_err(|e| e.to_string())?;
                spec.needles.extend(from_file.needles);
                spec.forbidden_keys.extend(from_file.forbidden_keys);
                single |= from_file.single_revision;
                spec.regions.extend(from_file.regions);
            }
            "--text" => spec = spec.text(&value("--text")?),
            "--metadata" => spec = spec.metadata(&value("--metadata")?),
            "--field" => spec = spec.field(&value("--field")?),
            "--bookmark" => spec = spec.bookmark(&value("--bookmark")?),
            "--attachment" => spec = spec.attachment(&value("--attachment")?),
            "--image-sha256" => spec = spec.image_sha256(&value("--image-sha256")?),
            "--forbid-key" => spec.forbidden_keys.push(value("--forbid-key")?),
            "--scrubbed" => scrubbed = true,
            "--single-revision" => single = true,
            "--only" => only = Some(layers(&value("--only")?)?),
            "--skip" => skip.extend(layers(&value("--skip")?)?),
            "--show-values" => show = true,
            "--fonts" => fonts = true,
            "--region" => spec.regions.push(region(&value("--region")?)?),
            "--password-file" => {
                let path = value("--password-file")?;
                let text = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
                password = Some(
                    text.trim_end_matches(|c: char| c == char::from(13) || c == char::from(10))
                        .to_owned(),
                );
            }
            "-h" | "--help" => return Ok(usage()),
            other if other.starts_with("--") => return Err(format!("unknown option {other}")),
            other => {
                if file.replace(PathBuf::from(other)).is_some() {
                    return Err("only one file".into());
                }
            }
        }
    }
    let Some(file) = file else {
        return Ok(usage());
    };
    if scrubbed {
        spec = spec.scrubbed();
    }
    spec.single_revision |= single;
    let mut opts = match only {
        Some(l) => Options::only(&l),
        None => Options::default(),
    };
    for l in skip {
        opts.layers.remove(&l);
    }
    if fonts {
        opts.layers.insert(Layer::Fonts);
    }
    opts.password = password;
    let report = verify(&file, &spec, &opts).map_err(|e| e.to_string())?;
    print!("{}", report.render(&spec, show));
    Ok(if !report.hits.is_empty() {
        ExitCode::from(1)
    } else if !report.problems.is_empty() {
        ExitCode::from(2)
    } else {
        ExitCode::SUCCESS
    })
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("redaction-verify: {e}");
            ExitCode::from(2)
        }
    }
}
