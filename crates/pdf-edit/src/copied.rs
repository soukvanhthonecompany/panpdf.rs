use pdf_paint::{Matrix, PaintAtomKind, PaintGraph, PathSegment, Point, TextShowElement};
use pdf_syntax::Reference;

use crate::plan::{PenBlend, PenStep, PenStroke, SourceAnchor};
use crate::spike_move_text::SpikeError;

#[derive(Clone, Debug, PartialEq)]
pub struct Copied {
    pub objects: Vec<CopiedObject>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CopiedObject {
    Picture {
        image: Reference,
        placement: Matrix,
    },
    Drawing {
        steps: Vec<PenStep>,
        closed: bool,
        stroke: Option<PenStroke>,
        fill: Option<[f64; 3]>,
    },
    Text(CopiedRun),
}

#[derive(Clone, Debug, PartialEq)]
pub struct CopiedRun {
    pub font: Reference,
    pub size: f64,
    pub fill: [f64; 3],
    pub matrix: Matrix,
    pub horizontal_scaling: f64,
    pub character_spacing: f64,
    pub word_spacing: f64,
    pub rise: f64,
    pub elements: Vec<CopiedTextElement>,
    pub glyphs: Vec<CopiedGlyph>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CopiedTextElement {
    Codes(Vec<u8>),
    Adjustment(f64),
}

#[derive(Clone, Debug, PartialEq)]
pub struct CopiedGlyph {
    pub code: u32,
    pub cid: Option<u32>,
    pub glyph: Option<u16>,
    pub placed: Matrix,
}

fn refused(reason: &'static str) -> SpikeError {
    SpikeError::CopyUnsupported(reason)
}

pub fn copy_from(graph: &PaintGraph, anchors: &[SourceAnchor]) -> Result<Copied, SpikeError> {
    let mut objects = Vec::with_capacity(anchors.len());
    for anchor in anchors {
        objects.push(copy_one(graph, anchor)?);
    }
    Ok(Copied { objects })
}

pub(crate) fn plain(state: &pdf_paint::GraphicsState) -> bool {
    (state.fill_alpha.value - 1.0).abs() <= 1e-9
        && (state.stroke_alpha.value - 1.0).abs() <= 1e-9
        && normal_blend(&state.blend_mode.value)
        && matches!(state.soft_mask.value, pdf_paint::SoftMask::None)
        && !state.alpha_is_shape.value
}

pub(crate) fn normal_blend(blend: &pdf_paint::BlendMode) -> bool {
    blend.names.is_empty() || blend.names == vec![b"/Normal".to_vec()]
}

fn copy_one(graph: &PaintGraph, anchor: &SourceAnchor) -> Result<CopiedObject, SpikeError> {
    let mut found = graph.atoms.iter().filter(|atom| anchor.names(&atom.id));
    let atom = found.next().ok_or(SpikeError::AnchorNamesNothing)?;
    if found.next().is_some() {
        return Err(SpikeError::ObjectNamedMoreThanOnce);
    }
    if !atom.id.pattern_path.is_empty() {
        return Err(refused(
            "an atom painted through a pattern cell cannot be copied yet",
        ));
    }
    if !atom.id.invocation_path.is_empty() {
        return Err(refused(
            "an atom painted inside a Form cannot be copied yet",
        ));
    }
    match &atom.kind {
        PaintAtomKind::Image(image) => {
            if !plain(&image.state) {
                return Err(refused(
                    "a picture painted see-through or through a mask cannot be copied yet",
                ));
            }
            Ok(CopiedObject::Picture {
                image: image.reference,
                placement: image.state.ctm.value,
            })
        }
        PaintAtomKind::Path(path) => copy_drawing(path),
        PaintAtomKind::Text(text) => copy_run(text).map(CopiedObject::Text),
        PaintAtomKind::Shading(_) => Err(refused("a shading cannot be copied yet")),
        PaintAtomKind::TransparencyGroup(_) => {
            Err(refused("a transparency group cannot be copied yet"))
        }
    }
}

fn device_rgb(colour: &pdf_paint::Color) -> Option<[f64; 3]> {
    match colour {
        pdf_paint::Color::DeviceGray(g) => Some([*g, *g, *g]),
        pdf_paint::Color::DeviceRgb(r, g, b) => Some([*r, *g, *b]),
        _ => None,
    }
}

fn pen_blend(blend: &pdf_paint::BlendMode) -> Result<PenBlend, SpikeError> {
    if normal_blend(blend) {
        Ok(PenBlend::Normal)
    } else if blend.names == vec![b"/Multiply".to_vec()] {
        Ok(PenBlend::Multiply)
    } else {
        Err(refused(
            "a blend mode other than Normal or Multiply cannot be copied yet",
        ))
    }
}

fn copy_stroke(path: &pdf_paint::PathPaint) -> Result<Option<PenStroke>, SpikeError> {
    if !path.stroke {
        return Ok(None);
    }
    let colour = device_rgb(&path.state.stroke_color.value).ok_or_else(|| {
        refused("a stroke in a colour space other than gray or RGB cannot be copied yet")
    })?;
    let blend = pen_blend(&path.state.blend_mode.value)?;
    let width = path.state.line_width.value;
    if !(width.is_finite() && width > 0.0) {
        return Err(refused("a hairline stroke cannot be copied yet"));
    }
    Ok(Some(PenStroke {
        colour,
        width,
        opacity: path.state.stroke_alpha.value,
        blend,
        round_ends: path.state.line_cap.value == pdf_paint::LineCap::Round,
    }))
}

fn copy_fill(path: &pdf_paint::PathPaint) -> Result<Option<[f64; 3]>, SpikeError> {
    match path.fill {
        None => Ok(None),
        Some(_) => device_rgb(&path.state.fill_color.value)
            .map(Some)
            .ok_or_else(|| {
                refused("a fill in a colour space other than gray or RGB cannot be copied yet")
            }),
    }
}

fn copy_drawing(path: &pdf_paint::PathPaint) -> Result<CopiedObject, SpikeError> {
    let unsupported = || refused("a drawing with more than one subpath cannot be copied yet");
    if (path.state.fill_alpha.value - 1.0).abs() > 1e-9
        || !matches!(path.state.soft_mask.value, pdf_paint::SoftMask::None)
        || path.state.alpha_is_shape.value
    {
        return Err(refused(
            "a drawing painted see-through or through a mask cannot be copied yet",
        ));
    }
    let ctm = path.state.ctm.value;
    let at = |point: Point| {
        let moved = ctm.transform(point);
        (moved.x, moved.y)
    };
    let mut steps = Vec::new();
    let mut closed = false;
    let mut moved_pen = false;
    let last = path.path.segments.len().checked_sub(1);
    for (index, segment) in path.path.segments.iter().enumerate() {
        match segment {
            PathSegment::MoveTo { point, .. } => {
                if moved_pen {
                    return Err(unsupported());
                }
                moved_pen = true;
                steps.push(PenStep::Move(at(*point)));
            }
            PathSegment::LineTo { point, .. } => steps.push(PenStep::Line(at(*point))),
            PathSegment::CubicTo {
                control_1,
                control_2,
                end,
                ..
            } => steps.push(PenStep::Curve(at(*control_1), at(*control_2), at(*end))),
            PathSegment::ClosePath { .. } => {
                if Some(index) != last {
                    return Err(unsupported());
                }
                closed = true;
            }
            PathSegment::Rectangle {
                origin,
                width,
                height,
                ..
            } => {
                if moved_pen || Some(index) != last {
                    return Err(unsupported());
                }
                moved_pen = true;
                let corners = [
                    *origin,
                    Point {
                        x: origin.x + width,
                        y: origin.y,
                    },
                    Point {
                        x: origin.x + width,
                        y: origin.y + height,
                    },
                    Point {
                        x: origin.x,
                        y: origin.y + height,
                    },
                ];
                steps.push(PenStep::Move(at(corners[0])));
                for corner in &corners[1..] {
                    steps.push(PenStep::Line(at(*corner)));
                }
                closed = true;
            }
        }
    }
    let stroke = copy_stroke(path)?;
    let fill = copy_fill(path)?;
    let drawing = crate::new_path::NewPath {
        steps: &steps,
        closed,
        stroke,
        fill,
    };
    if let Err(SpikeError::RetypeUnsupported(reason)) = crate::new_path::checked(&drawing) {
        return Err(refused(reason));
    }
    Ok(CopiedObject::Drawing {
        steps,
        closed,
        stroke,
        fill,
    })
}

fn copy_run(text: &pdf_paint::TextShowPaint) -> Result<CopiedRun, SpikeError> {
    if text.type3 {
        return Err(refused("a Type 3 run cannot be copied yet"));
    }
    if text.state.text.rendering_mode.value != pdf_paint::TextRenderingMode::Fill {
        return Err(refused(
            "a run not painted in the ordinary fill mode cannot be copied yet",
        ));
    }
    if !plain(&text.state) {
        return Err(refused(
            "text painted see-through or through a mask cannot be copied yet",
        ));
    }
    let font = text
        .state
        .text
        .font
        .as_ref()
        .and_then(|font| font.value.reference)
        .ok_or_else(|| {
            refused("this run's font is not a resource this file can be asked for again")
        })?;
    let fill = device_rgb(&text.state.fill_color.value).ok_or_else(|| {
        refused("text in a colour space other than gray or RGB cannot be copied yet")
    })?;
    let ctm = text.state.ctm.value;
    let matrix = ctm.multiply(text.matrices.text.value);
    let elements = text
        .elements
        .iter()
        .map(|element| match element {
            TextShowElement::Codes { decoded_bytes, .. } => {
                CopiedTextElement::Codes(decoded_bytes.clone())
            }
            TextShowElement::Adjustment { value, .. } => CopiedTextElement::Adjustment(*value),
        })
        .collect();
    let glyphs = text
        .glyphs
        .iter()
        .map(|glyph| CopiedGlyph {
            code: glyph.code.value,
            cid: glyph.code.cid,
            glyph: glyph.glyph,
            placed: ctm.multiply(glyph.matrix),
        })
        .collect();
    Ok(CopiedRun {
        font,
        size: text.state.text.font_size.value,
        fill,
        matrix,
        horizontal_scaling: text.state.text.horizontal_scaling.value,
        character_spacing: text.state.text.character_spacing.value,
        word_spacing: text.state.text.word_spacing.value,
        rise: text.state.text.rise.value,
        elements,
        glyphs,
    })
}
