use pdf_font::{glyph::GlyphProgram, shaping::shape_cluster};
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    assert_eq!(args.len(), 3, "usage: shape_cluster FONT FACE_INDEX TEXT");
    let index: u32 = args[1].parse().expect("face index");
    let program =
        GlyphProgram::parse_face(std::fs::read(&args[0]).expect("font"), index).expect("program");
    let shaped = shape_cluster(&program, index, &args[2]).expect("supported bounded cluster");
    for glyph in shaped {
        println!("{} {} {}", glyph.glyph, glyph.x, glyph.y);
    }
}
