use crate::glyph::GlyphProgram;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShapedGlyph {
    pub glyph: u16,
    pub x: i32,
    pub y: i32,
    pub advance: i32,
}

#[must_use]
pub fn shape_cluster(
    program: &GlyphProgram,
    face_index: u32,
    text: &str,
) -> Option<Vec<ShapedGlyph>> {
    if text.is_empty() || text.len() > 256 || text.chars().count() > 64 {
        return None;
    }
    let face = rustybuzz::Face::from_slice(program.program_bytes(), face_index)?;
    let mut buffer = rustybuzz::UnicodeBuffer::new();
    buffer.push_str(text);
    buffer.guess_segment_properties();
    let shaped = rustybuzz::shape(&face, &[], buffer);
    if shaped.is_empty() || shaped.len() > 256 {
        return None;
    }
    let mut pen = (0_i32, 0_i32);
    let mut output = Vec::with_capacity(shaped.len());
    for (info, position) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
        let glyph = u16::try_from(info.glyph_id).ok()?;
        if glyph == 0 || program.path(glyph).is_none() {
            return None;
        }
        output.push(ShapedGlyph {
            glyph,
            x: pen.0.checked_add(position.x_offset)?,
            y: pen.1.checked_add(position.y_offset)?,
            advance: position.x_advance,
        });
        pen.0 = pen.0.checked_add(position.x_advance)?;
        pen.1 = pen.1.checked_add(position.y_advance)?;
    }
    Some(output)
}
