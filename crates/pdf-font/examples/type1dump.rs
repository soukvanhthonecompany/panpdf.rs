fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: type1dump <program> [name]");
    let bytes = std::fs::read(&path).expect("read");
    let font = pdf_font::type1::Type1Font::parse(&bytes).expect("a Type 1 program");
    println!(
        "{} glyphs, units per em {}",
        font.glyph_count(),
        font.units_per_em()
    );
    let coded: Vec<u8> = (0..=255u8)
        .filter(|code| font.glyph_for_code(*code).is_some())
        .collect();
    println!("codes the program's own encoding answers: {}", coded.len());
    if let Some(name) = std::env::args().nth(2) {
        let glyph = font.glyph_for_name(name.as_bytes()).expect("that name");
        let path = font.path(glyph).expect("an outline");
        println!(
            "{name} is glyph {glyph} with {} segments",
            path.segments.len()
        );
        for segment in path.segments.iter().take(24) {
            println!("  {segment:?}");
        }
    }
}
