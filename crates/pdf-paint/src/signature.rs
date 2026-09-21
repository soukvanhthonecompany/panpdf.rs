use crate::color::Color;
use crate::color::color_components;
use crate::geometry::PathSegment;
use crate::graph::{PaintAtomKind, PaintGraph, PositionedGlyph, TextShowElement};
use crate::shading::ShadingGeometry;

#[must_use]
pub fn glyph_placement_signature(graph: &PaintGraph) -> Vec<String> {
    let mut placements = Vec::new();
    for atom in &graph.atoms {
        let PaintAtomKind::Text(text) = &atom.kind else {
            continue;
        };
        for glyph in &text.glyphs {
            placements.push(format!(
                "code={} cid={:?} glyph={:?} drawn={} at={:?} ctm={:?} mode={:?}                  fill={} stroke={}",
                glyph.code.value,
                glyph.code.cid,
                glyph.glyph,
                substituted_signature(text, glyph),
                glyph.matrix,
                text.state.ctm.value,
                text.state.text.rendering_mode.value,
                colour_signature(&text.state.fill_color.value),
                colour_signature(&text.state.stroke_color.value),
            ));
        }
    }
    placements
}

fn substituted_signature(text: &crate::graph::TextShowPaint, glyph: &PositionedGlyph) -> String {
    if glyph.silent {
        return "silent".to_owned();
    }
    if glyph.substituted.is_empty() {
        return "-".to_owned();
    }
    let faces = text.substitution.as_ref().map(|substitution| {
        std::iter::once(&substitution.primary)
            .chain(&substitution.fallbacks)
            .collect::<Vec<_>>()
    });
    let named: Vec<String> = glyph
        .substituted
        .iter()
        .map(|drawn| {
            let identity = faces
                .as_ref()
                .and_then(|faces| faces.get(usize::from(drawn.face)))
                .map_or_else(
                    || format!("face{}", drawn.face),
                    |face| {
                        format!(
                            "{}#{}:{}",
                            face.identity.sha256, face.identity.face_index, face.identity.family
                        )
                    },
                );
            format!("{identity}/{}@{:?}", drawn.glyph, drawn.offset)
        })
        .collect();
    format!("[{}]", named.join(" "))
}

#[must_use]
pub fn colour_signature(colour: &Color) -> String {
    match colour {
        Color::ShadingPattern(pattern) => format!(
            "shading-pattern matrix={:?} base={:?} {}",
            pattern.matrix.value,
            pattern.base,
            shading_signature(&pattern.shading),
        ),
        Color::TilingPattern(pattern) => format!(
            "tiling({} {} R paint={} tiling={} bbox={:?} step={},{} matrix={:?} atoms={})",
            pattern.reference.object_number(),
            pattern.reference.generation(),
            pattern.paint_type.value,
            pattern.tiling_type.value,
            pattern.bbox,
            pattern.x_step.value,
            pattern.y_step.value,
            pattern.matrix.value,
            pattern.graph.atoms.len()
        ),
        other => format!("{other:?}"),
    }
}

fn shading_signature(paint: &crate::shading::ShadingPaint) -> String {
    let geometry = match &paint.geometry {
        ShadingGeometry::Axial(coords) => format!("axial{:?}", coords.value),
        ShadingGeometry::Radial(coords) => format!("radial{:?}", coords.value),
    };
    format!(
        "{geometry} space={} domain={:?} extend={:?} background={:?} bbox={:?} aa={} function={} ctm={:?}",
        space_signature(&paint.color_space.value),
        paint.domain.value,
        paint.extend.value,
        paint.background.as_ref().map(|item| &item.value),
        paint.bbox.as_ref().map(|item| &item.value),
        paint.anti_alias.value,
        function_signature(&paint.function),
        paint.state.ctm.value,
    )
}

fn function_signature(function: &crate::function::Function) -> String {
    use crate::function::Function;
    match function {
        Function::Sampled(f) => format!(
            "sampled {:?} {:?} {:?} {} {:?} {:?} samples={}:{:016x}",
            f.domain.value,
            f.range.value,
            f.size.value,
            f.bits_per_sample.value,
            f.encode.value,
            f.decode.value,
            f.samples.len(),
            fnv1a(&f.samples),
        ),
        Function::Exponential(f) => format!(
            "exponential {:?} {:?} {:?} {:?} {}",
            f.domain.value,
            f.range.as_ref().map(|r| &r.value),
            f.c0.value,
            f.c1.value,
            f.exponent.value,
        ),
        Function::Stitching(f) => format!(
            "stitching {:?} {:?} {:?} {:?} {:?}",
            f.domain.value,
            f.range.as_ref().map(|r| &r.value),
            f.bounds.value,
            f.encode.value,
            f.functions
                .iter()
                .map(function_signature)
                .collect::<Vec<_>>(),
        ),
        Function::PostScript(f) => format!(
            "calculator {:?} {:?} {:?}",
            f.domain.value, f.range.value, f.program,
        ),
    }
}

#[must_use]
pub fn paint_signature(kind: &PaintAtomKind) -> String {
    match kind {
        PaintAtomKind::Path(paint) => {
            let points: Vec<String> = paint
                .path
                .segments
                .iter()
                .map(|segment| match segment {
                    PathSegment::MoveTo { point, .. } => {
                        format!("M{},{}", point.x, point.y)
                    }
                    PathSegment::LineTo { point, .. } => {
                        format!("L{},{}", point.x, point.y)
                    }
                    PathSegment::CubicTo {
                        control_1,
                        control_2,
                        end,
                        ..
                    } => format!(
                        "C{},{},{},{},{},{}",
                        control_1.x, control_1.y, control_2.x, control_2.y, end.x, end.y
                    ),
                    PathSegment::ClosePath { .. } => "Z".to_owned(),
                    PathSegment::Rectangle {
                        origin,
                        width,
                        height,
                        ..
                    } => format!("R{},{},{width},{height}", origin.x, origin.y),
                })
                .collect();
            format!(
                "path[{}] fill={:?} stroke={} ctm={:?} fill_colour={} stroke_colour={}",
                points.join(" "),
                paint.fill,
                paint.stroke,
                paint.state.ctm.value,
                colour_signature(&paint.state.fill_color.value),
                colour_signature(&paint.state.stroke_color.value)
            )
        }
        PaintAtomKind::Text(paint) => {
            let shown: Vec<String> = paint
                .elements
                .iter()
                .map(|element| match element {
                    TextShowElement::Codes { decoded_bytes, .. } => {
                        format!("{decoded_bytes:02x?}")
                    }
                    TextShowElement::Adjustment { value, .. } => format!("adj{value}"),
                })
                .collect();
            format!(
                "text[{}] text_matrix={:?} ctm={:?}",
                shown.join(" "),
                paint.matrices.text.value,
                paint.state.ctm.value
            )
        }
        PaintAtomKind::Shading(paint) => {
            let geometry = match &paint.geometry {
                ShadingGeometry::Axial(coords) => format!("axial{:?}", coords.value),
                ShadingGeometry::Radial(coords) => format!("radial{:?}", coords.value),
            };
            format!(
                "shading {geometry} domain={:?} extend={:?} ctm={:?}",
                paint.domain.value, paint.extend.value, paint.state.ctm.value
            )
        }
        PaintAtomKind::Image(paint) => image_signature(paint),
        PaintAtomKind::TransparencyGroup(group) => format!(
            "group {:?} children={} ctm={:?}",
            group.bbox,
            group.graph.atoms.len(),
            group.state.ctm.value
        ),
    }
}

fn image_signature(paint: &crate::ImagePaint) -> String {
    format!(
        "image {}x{} bpc={} mask={} space={} decode={:?} interpolate={} samples={}:{:016x} smask=[{}] matte={:?} ctm={:?}",
        paint.width.value,
        paint.height.value,
        paint.bits_per_component.value,
        paint.image_mask.value,
        paint
            .color_space
            .as_ref()
            .map_or_else(|| "none".to_owned(), |space| space_signature(&space.value)),
        paint.decode.value,
        paint.interpolate.value,
        paint.samples.len(),
        fnv1a(&paint.samples),
        paint
            .soft_mask
            .as_ref()
            .map_or_else(String::new, |mask| image_signature(mask)),
        paint.matte.as_ref().map(|matte| &matte.value),
        paint.state.ctm.value,
    )
}

fn space_signature(space: &crate::ColorSpace) -> String {
    use crate::ColorSpace;
    let family = match space {
        ColorSpace::DeviceGray => "DeviceGray",
        ColorSpace::DeviceRgb => "DeviceRGB",
        ColorSpace::DeviceCmyk => "DeviceCMYK",
        ColorSpace::CalGray(_) => "CalGray",
        ColorSpace::CalRgb(_) => "CalRGB",
        ColorSpace::Lab(_) => "Lab",
        ColorSpace::IccBased(_) => "ICCBased",
        ColorSpace::Indexed(_) => "Indexed",
        ColorSpace::Separation(_) => "Separation",
        ColorSpace::DeviceN(_) => "DeviceN",
        ColorSpace::Pattern(_) => "Pattern",
    };
    let components = color_components(space).unwrap_or(0);
    match space {
        ColorSpace::Indexed(indexed) => format!(
            "{family}/{components}/hival={}/palette={}:{:016x}",
            indexed.hival.value,
            indexed.lookup.value.len(),
            fnv1a(&indexed.lookup.value)
        ),
        _ => format!("{family}/{components}"),
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}
