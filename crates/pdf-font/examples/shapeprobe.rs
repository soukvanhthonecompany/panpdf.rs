fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = arguments.next().expect("a font path");
    let bytes = std::fs::read(&path).expect("the font reads");
    let font = pdf_font::truetype::TrueTypeFont::parse(bytes).expect("a TrueType face");
    let program = pdf_font::glyph::GlyphProgram::TrueType(font);
    for cluster in arguments {
        match pdf_font::shaping::shape_cluster(&program, 0, &cluster) {
            Some(glyphs) => {
                let shown: Vec<String> = glyphs
                    .iter()
                    .map(|glyph| {
                        format!(
                            "g{} x{} y{} adv{}",
                            glyph.glyph, glyph.x, glyph.y, glyph.advance
                        )
                    })
                    .collect();
                println!("{cluster}: {}", shown.join(" | "));
            }
            None => println!("{cluster}: not shaped"),
        }
    }
}
