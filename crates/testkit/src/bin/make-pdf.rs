//! Writes a generated PDF with the given number of pages: `make-pdf <pages> <output.pdf>`.
//! Used by the measurement script to get a large test document without storing one in the repo.
//! `make-pdf features <output.pdf>` writes the small document with an outline, links and text to
//! search (see `features_pdf`).

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let features = args.first().is_some_and(|a| a == "features");
    let (Some(pages), Some(out)) = (
        if features {
            Some(0)
        } else {
            args.first().and_then(|p| p.parse::<usize>().ok())
        },
        args.get(1),
    ) else {
        eprintln!("usage: make-pdf <pages> <output.pdf>  or  make-pdf features <output.pdf>");
        std::process::exit(2);
    };
    let bytes = if features {
        partridge_testkit::features_pdf()
    } else {
        partridge_testkit::simple_pdf(pages)
    };
    if let Err(e) = std::fs::write(out, bytes) {
        eprintln!("could not write {out}: {e}");
        std::process::exit(1);
    }
}
