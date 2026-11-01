//! Test support: small PDF files generated in code (so no binary fixtures live in the repo),
//! including a damaged one and a password-protected one, and a way to find the PDFium folder.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use md5::{Digest, Md5};

/// One page of a generated document.
pub struct PageSpec {
    pub width: u32,
    pub height: u32,
    /// Page content stream. `None` gives a blank page without a content stream.
    pub content: Option<String>,
}

/// Content of a page that has a title, a block of text and a coloured bar at the bottom.
pub fn demo_content(number: usize, width: u32, height: u32) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "BT /F1 40 Tf 48 {} Td (Page {number}) Tj ET",
        height - 90
    );
    let _ = writeln!(s, "BT /F1 11 Tf 48 {} Td 14 TL", height - 130);
    for line in 0..30 {
        let _ = writeln!(
            s,
            "(Line {line} of page {number}: the quick brown fox jumps over the lazy dog.) Tj T*"
        );
    }
    let _ = writeln!(s, "ET");
    let _ = writeln!(
        s,
        "0.1 0.3 0.8 rg 36 36 {} 24 re f",
        width.saturating_sub(72)
    );
    s
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Assembles objects 1.. into a file with a correct cross-reference table.
/// `objects[i]` is the body of object `i + 1`.
fn assemble(objects: &[Vec<u8>], trailer_extra: &str) -> Vec<u8> {
    let mut out = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for off in offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R {trailer_extra} >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// Builds a document from page specs. `catalog_extra` goes into the catalog dictionary,
/// `trailer_extra` into the trailer, and `extra_objects` are appended after the pages.
fn build(
    pages: &[PageSpec],
    catalog_extra: &str,
    trailer_extra: &str,
    extra_objects: Vec<Vec<u8>>,
) -> Vec<u8> {
    // 1 catalog, 2 page tree, 3 font, then per page: the page and (optionally) its content.
    let mut objects: Vec<Vec<u8>> = Vec::new();
    let mut kids = String::new();
    let mut body_objects: Vec<Vec<u8>> = Vec::new();
    let mut next = 4;
    for spec in pages {
        let page_id = next;
        next += 1;
        let _ = write!(kids, "{page_id} 0 R ");
        let contents_ref = if spec.content.is_some() {
            let id = next;
            next += 1;
            format!("/Contents {id} 0 R ")
        } else {
            String::new()
        };
        body_objects.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] {contents_ref}/Resources << /Font << /F1 3 0 R >> >> >>",
                spec.width, spec.height
            )
            .into_bytes(),
        );
        if let Some(text) = &spec.content {
            let mut stream = format!("<< /Length {} >>\nstream\n", text.len()).into_bytes();
            stream.extend_from_slice(text.as_bytes());
            stream.extend_from_slice(b"\nendstream");
            body_objects.push(stream);
        }
    }

    objects.push(format!("<< /Type /Catalog /Pages 2 0 R {catalog_extra} >>").into_bytes());
    objects.push(format!("<< /Type /Pages /Kids [{kids}] /Count {} >>", pages.len()).into_bytes());
    objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
    objects.extend(body_objects);
    objects.extend(extra_objects);
    assemble(&objects, trailer_extra)
}

/// A valid document of `pages` Letter-size pages with text and a bar on each.
pub fn simple_pdf(pages: usize) -> Vec<u8> {
    let specs: Vec<PageSpec> = (1..=pages)
        .map(|n| PageSpec {
            width: 612,
            height: 792,
            content: Some(demo_content(n, 612, 792)),
        })
        .collect();
    build(&specs, "", "", Vec::new())
}

/// A valid document whose pages differ in size: Letter, A4 landscape, a small square, a tall strip.
pub fn mixed_size_pdf() -> Vec<u8> {
    let sizes = [(612, 792), (842, 595), (300, 300), (200, 1000)];
    let specs: Vec<PageSpec> = sizes
        .iter()
        .enumerate()
        .map(|(i, &(w, h))| PageSpec {
            width: w,
            height: h,
            content: Some(demo_content(i + 1, w, h)),
        })
        .collect();
    build(&specs, "", "", Vec::new())
}

/// A valid document with a document-level JavaScript open action. The engine must open and draw
/// it like any other file, and must not run the script.
pub fn javascript_pdf() -> Vec<u8> {
    let specs = [PageSpec {
        width: 612,
        height: 792,
        content: Some(demo_content(1, 612, 792)),
    }];
    build(
        &specs,
        "/OpenAction << /S /JavaScript /JS (app.alert\\(\"hello\"\\);) >> \
         /Names << /JavaScript << /Names [(init) << /S /JavaScript /JS (this.print\\(\\);) >>] >> >>",
        "",
        Vec::new(),
    )
}

/// Starts like a PDF but is not one: the structure is cut off in the middle of the catalog.
pub fn corrupt_pdf() -> Vec<u8> {
    b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R\n".to_vec()
}

/// Not a PDF at all.
pub fn not_a_pdf() -> Vec<u8> {
    b"This is a plain text file, not a PDF.\n".repeat(20)
}

/// A real document with the second half cut off. Readers may repair it or refuse it, but they
/// must not crash.
pub fn truncated_pdf() -> Vec<u8> {
    let full = simple_pdf(5);
    full[..full.len() * 6 / 10].to_vec()
}

const PADDING: [u8; 32] = [
    0x28, 0xBF, 0x4E, 0x5E, 0x4E, 0x75, 0x8A, 0x41, 0x64, 0x00, 0x4E, 0x56, 0xFF, 0xFA, 0x01, 0x08,
    0x2E, 0x2E, 0x00, 0xB6, 0xD0, 0x68, 0x3E, 0x80, 0x2F, 0x0C, 0xA9, 0xFE, 0x64, 0x53, 0x69, 0x7A,
];

fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut s: Vec<u8> = (0..=255).collect();
    let mut j = 0usize;
    for i in 0..256 {
        j = (j + s[i] as usize + key[i % key.len()] as usize) % 256;
        s.swap(i, j);
    }
    let (mut i, mut j) = (0usize, 0usize);
    data.iter()
        .map(|b| {
            i = (i + 1) % 256;
            j = (j + s[i] as usize) % 256;
            s.swap(i, j);
            b ^ s[(s[i] as usize + s[j] as usize) % 256]
        })
        .collect()
}

/// A document protected with the standard security handler (RC4, 40 bit, revision 2) that needs
/// `user_password` to open. The pages are blank so nothing has to be encrypted besides the
/// handler's own entries.
pub fn encrypted_pdf(user_password: &str, pages: usize) -> Vec<u8> {
    let mut padded = [0u8; 32];
    let pw = user_password.as_bytes();
    let n = pw.len().min(32);
    padded[..n].copy_from_slice(&pw[..n]);
    padded[n..].copy_from_slice(&PADDING[..32 - n]);

    // The owner entry only matters for the owner password, which these files do not use.
    let owner: Vec<u8> = (0u8..32)
        .map(|i| i.wrapping_mul(7).wrapping_add(3))
        .collect();
    let permissions: i32 = -4;
    let id: [u8; 16] = *b"partridge-test-1";

    // Algorithm 2 for revision 2: the file key.
    let mut md5 = Md5::new();
    md5.update(padded);
    md5.update(&owner);
    md5.update(permissions.to_le_bytes());
    md5.update(id);
    let key = md5.finalize()[..5].to_vec();
    // Algorithm 4: the user entry is the padding string encrypted with the file key.
    let user = rc4(&key, &PADDING);

    let encrypt_object = format!(
        "<< /Filter /Standard /V 1 /R 2 /P {permissions} /O <{}> /U <{}> >>",
        hex(&owner),
        hex(&user)
    )
    .into_bytes();
    // Object numbers: 1 catalog, 2 tree, 3 font, one per page from 4, then the encrypt dictionary.
    let encrypt_id = 4 + pages;
    let specs: Vec<PageSpec> = (0..pages)
        .map(|_| PageSpec {
            width: 612,
            height: 792,
            content: None,
        })
        .collect();
    build(
        &specs,
        "",
        &format!("/Encrypt {encrypt_id} 0 R /ID [<{0}> <{0}>]", hex(&id)),
        vec![encrypt_object],
    )
}

/// Folder name used for a platform in `vendor/pdfium` and in the pinned PDFium list.
pub fn pdfium_platform_id() -> &'static str {
    if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "win-x64"
    } else if cfg!(all(target_os = "windows", target_arch = "aarch64")) {
        "win-arm64"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "mac-arm64"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "mac-x64"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "linux-arm64"
    } else {
        "linux-x64"
    }
}

/// The folder with the PDFium library for tests: `PARTRIDGE_PDFIUM_DIR`, or the folder that
/// `node scripts/fetch-pdfium.mjs` fills.
pub fn pdfium_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("PARTRIDGE_PDFIUM_DIR") {
        return PathBuf::from(dir);
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../vendor/pdfium")
        .join(pdfium_platform_id())
}

/// A file in the temp folder that is removed again when this value is dropped.
pub struct TempFile {
    path: PathBuf,
}

impl TempFile {
    pub fn new(name: &str, bytes: &[u8]) -> TempFile {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let unique = format!(
            "partridge-test-{}-{}-{name}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let path = std::env::temp_dir().join(unique);
        std::fs::write(&path, bytes).expect("write temp file");
        TempFile { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn path_str(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    #[test]
    fn simple_pdf_has_the_right_structure() {
        let pdf = simple_pdf(3);
        let t = text(&pdf);
        assert!(t.starts_with("%PDF-1.4"));
        assert!(t.contains("/Count 3"));
        assert!(t.trim_end().ends_with("%%EOF"));
        // The startxref offset must point at the xref keyword.
        let start: usize = t
            .rsplit("startxref\n")
            .next()
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(pdf[start..].starts_with(b"xref"));
    }

    #[test]
    fn xref_offsets_point_at_their_objects() {
        let pdf = simple_pdf(2);
        let t = text(&pdf);
        let xref = t.find("xref\n").unwrap();
        for (i, line) in t[xref..].lines().skip(3).take(7).enumerate() {
            let off: usize = line[..10].parse().unwrap();
            let expect = format!("{} 0 obj", i + 1);
            assert!(text(&pdf[off..]).starts_with(&expect), "object {}", i + 1);
        }
    }

    #[test]
    fn rc4_matches_a_known_vector() {
        // RFC 6229 style check: key "Key", plaintext "Plaintext".
        let out = rc4(b"Key", b"Plaintext");
        assert_eq!(hex(&out), "bbf316e8d940af0ad3");
    }

    #[test]
    fn the_damaged_files_are_not_valid() {
        assert!(!text(&corrupt_pdf()).contains("%%EOF"));
        assert!(!text(&not_a_pdf()).starts_with("%PDF"));
        assert!(truncated_pdf().len() < simple_pdf(5).len());
    }

    #[test]
    fn temp_files_are_removed() {
        let path;
        {
            let f = TempFile::new("x.pdf", b"x");
            path = f.path().to_path_buf();
            assert!(path.exists());
        }
        assert!(!path.exists());
    }
}
