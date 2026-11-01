//! Writes a generated PDF with the given number of pages: `make-pdf <pages> <output.pdf>`.
//! Used by the measurement script to get a large test document without storing one in the repo.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(pages), Some(out)) = (
        args.first().and_then(|p| p.parse::<usize>().ok()),
        args.get(1),
    ) else {
        eprintln!("usage: make-pdf <pages> <output.pdf>");
        std::process::exit(2);
    };
    if let Err(e) = std::fs::write(out, partridge_testkit::simple_pdf(pages)) {
        eprintln!("could not write {out}: {e}");
        std::process::exit(1);
    }
}
