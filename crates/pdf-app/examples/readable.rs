use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_paint::{Code, Confidence, PaintAtomKind};

fn percent(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        return 0.0;
    }
    let (part, whole) = (
        u32::try_from(part).unwrap_or(u32::MAX),
        u32::try_from(whole).unwrap_or(u32::MAX),
    );
    100.0 * f64::from(part) / f64::from(whole)
}

fn main() {
    let dir = std::env::args().nth(1).expect("dir");
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .expect("readdir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")))
        .collect();
    paths.sort();

    let mut pages = 0_usize;
    let (mut runs, mut readable_runs, mut declared, mut named) = (0_usize, 0, 0, 0);
    let (mut glyphs, mut read, mut round_trip, mut ambiguous) = (0_usize, 0, 0, 0);
    let (mut digit_runs, mut digit_pages) = (0_usize, 0_usize);
    for path in &paths {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
        let Ok(view) = pdf_session::interpret_page(&source, 0) else {
            continue;
        };
        pages += 1;
        let mut digits_here = false;
        for atom in &view.graph.atoms {
            let PaintAtomKind::Text(text) = &atom.kind else {
                continue;
            };
            runs += 1;
            if !text.text.is_empty() {
                readable_runs += 1;
                declared += text.text.by_confidence(Confidence::Declared);
                named += text.text.by_confidence(Confidence::Named);
            }
            if ('0'..='9').all(|digit| text.text.codes_for(&digit.to_string()).len() == 1) {
                digit_runs += 1;
                digits_here = true;
            }
            for glyph in &text.glyphs {
                glyphs += 1;
                let code = Code {
                    value: glyph.code.value,
                    byte_len: glyph.code.bytes.len(),
                };
                let Some(meaning) = text.text.text_of(code) else {
                    continue;
                };
                read += 1;
                let back = text.text.codes_for(&meaning.text);
                if back.len() > 1 {
                    ambiguous += 1;
                }
                if back.contains(&code) {
                    round_trip += 1;
                }
            }
        }
        if digits_here {
            digit_pages += 1;
        }
    }
    println!("pages: {pages}");
    println!(
        "text runs whose font declares any meaning: {readable_runs} of {runs} ({:.1}%)",
        percent(readable_runs, runs)
    );
    println!("  codes read from a /ToUnicode CMap: {declared}");
    println!("  codes read from a glyph name or the font's own cmap: {named}");
    println!(
        "glyphs whose text is known: {read} of {glyphs} ({:.1}%)",
        percent(read, glyphs)
    );
    println!(
        "glyphs whose text maps back to a code the font already uses: {round_trip} ({:.1}%)",
        percent(round_trip, glyphs)
    );
    println!("glyphs whose text maps back to more than one code, and so are refused: {ambiguous}");
    println!(
        "runs that could retype any digit: {digit_runs} of {runs} ({:.1}%), on {digit_pages} pages",
        percent(digit_runs, runs)
    );
}
