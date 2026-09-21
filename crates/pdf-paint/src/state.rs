use std::sync::Arc;

use pdf_bytes::SourceSpan;
use pdf_syntax::Reference;

use crate::color::{Color, ColorSpace};
use crate::geometry::{FillRule, Matrix, Path};
use crate::graph::SoftMaskPaint;
use crate::provenance::Derived;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PaintLimits {
    pub max_graphics_stack_depth: usize,
    pub max_path_segments: usize,
    pub max_paint_atoms: usize,
    pub max_clip_paths: usize,
    pub max_form_depth: usize,
    pub max_form_invocations: usize,
    pub max_decoded_text_bytes: usize,
    pub max_pattern_depth: usize,
    pub max_pattern_invocations: usize,
    pub max_function_depth: usize,
    pub max_functions: usize,
    pub max_color_space_depth: usize,
    pub max_icc_profile_bytes: usize,
    pub max_indexed_lookup_bytes: usize,
    pub max_function_data_bytes: usize,
    pub max_function_inputs: usize,
    pub max_marked_content_depth: usize,
    pub max_compatibility_depth: usize,
    pub max_image_bytes: usize,
    pub max_image_pixels: usize,
}

impl Default for PaintLimits {
    fn default() -> Self {
        Self {
            max_graphics_stack_depth: 1_024,
            max_path_segments: 1_000_000,
            max_paint_atoms: 1_000_000,
            max_clip_paths: 4_096,
            max_form_depth: 128,
            max_form_invocations: 100_000,
            max_decoded_text_bytes: 64 * 1024 * 1024,
            max_pattern_depth: 32,
            max_pattern_invocations: 100_000,
            max_function_depth: 32,
            max_functions: 100_000,
            max_color_space_depth: 32,
            max_icc_profile_bytes: 64 * 1024 * 1024,
            max_indexed_lookup_bytes: 1024 * 1024,
            max_function_data_bytes: 4 * 1024 * 1024,
            max_function_inputs: 8,
            max_marked_content_depth: 64,
            max_compatibility_depth: 64,
            max_image_bytes: 128 * 1024 * 1024,
            max_image_pixels: 64 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LineCap {
    Butt,
    Round,
    ProjectingSquare,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LineJoin {
    Miter,
    Round,
    Bevel,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DashPattern {
    pub array: Vec<f64>,
    pub phase: f64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlendMode {
    pub names: Vec<Vec<u8>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppliedExtGState {
    pub name: Vec<u8>,
    pub reference: Option<Reference>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppliedFont {
    pub name: Vec<u8>,
    pub reference: Option<Reference>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextRenderingMode {
    Fill,
    Stroke,
    FillStroke,
    Invisible,
    FillClip,
    StrokeClip,
    FillStrokeClip,
    Clip,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextState {
    pub character_spacing: Derived<f64>,
    pub word_spacing: Derived<f64>,
    pub horizontal_scaling: Derived<f64>,
    pub leading: Derived<f64>,
    pub font: Option<Derived<AppliedFont>>,
    pub font_size: Derived<f64>,
    pub rendering_mode: Derived<TextRenderingMode>,
    pub rise: Derived<f64>,
}

impl Default for TextState {
    fn default() -> Self {
        Self {
            character_spacing: Derived::initial(0.0),
            word_spacing: Derived::initial(0.0),
            horizontal_scaling: Derived::initial(100.0),
            leading: Derived::initial(0.0),
            font: None,
            font_size: Derived::initial(0.0),
            rendering_mode: Derived::initial(TextRenderingMode::Fill),
            rise: Derived::initial(0.0),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextMatrices {
    pub text: Derived<Matrix>,
    pub line: Derived<Matrix>,
}

impl TextMatrices {
    pub(crate) fn identity() -> Self {
        Self {
            text: Derived::initial(Matrix::IDENTITY),
            line: Derived::initial(Matrix::IDENTITY),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SoftMask {
    None,
    Dictionary(Arc<SoftMaskPaint>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SoftMaskSubtype {
    Alpha,
    Luminosity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SoftMaskTransfer {
    Identity,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClipPath {
    pub path: Path,
    pub rule: FillRule,
    pub ctm: Derived<Matrix>,
    pub provenance: SourceSpan,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GraphicsState {
    pub ctm: Derived<Matrix>,
    pub line_width: Derived<f64>,
    pub line_cap: Derived<LineCap>,
    pub line_join: Derived<LineJoin>,
    pub miter_limit: Derived<f64>,
    pub dash: Derived<DashPattern>,
    pub stroke_color: Derived<Color>,
    pub fill_color: Derived<Color>,
    pub stroke_color_space: Derived<ColorSpace>,
    pub fill_color_space: Derived<ColorSpace>,
    pub flatness: Derived<f64>,
    pub smoothness: Derived<f64>,
    pub rendering_intent: Derived<Vec<u8>>,
    pub stroke_adjust: Derived<bool>,
    pub stroke_overprint: Derived<bool>,
    pub fill_overprint: Derived<bool>,
    pub overprint_mode: Derived<i64>,
    pub blend_mode: Derived<BlendMode>,
    pub stroke_alpha: Derived<f64>,
    pub fill_alpha: Derived<f64>,
    pub alpha_is_shape: Derived<bool>,
    pub text_knockout: Derived<bool>,
    pub soft_mask: Derived<SoftMask>,
    pub ext_gstate: Option<Derived<AppliedExtGState>>,
    pub text: TextState,
    pub clip_paths: Vec<ClipPath>,
}

impl Default for GraphicsState {
    fn default() -> Self {
        Self {
            ctm: Derived::initial(Matrix::IDENTITY),
            line_width: Derived::initial(1.0),
            line_cap: Derived::initial(LineCap::Butt),
            line_join: Derived::initial(LineJoin::Miter),
            miter_limit: Derived::initial(10.0),
            dash: Derived::initial(DashPattern {
                array: Vec::new(),
                phase: 0.0,
            }),
            stroke_color: Derived::initial(Color::DeviceGray(0.0)),
            fill_color: Derived::initial(Color::DeviceGray(0.0)),
            stroke_color_space: Derived::initial(ColorSpace::DeviceGray),
            fill_color_space: Derived::initial(ColorSpace::DeviceGray),
            flatness: Derived::initial(1.0),
            smoothness: Derived::initial(0.0),
            rendering_intent: Derived::initial(b"/RelativeColorimetric".to_vec()),
            stroke_adjust: Derived::initial(false),
            stroke_overprint: Derived::initial(false),
            fill_overprint: Derived::initial(false),
            overprint_mode: Derived::initial(0),
            blend_mode: Derived::initial(BlendMode {
                names: vec![b"/Normal".to_vec()],
            }),
            stroke_alpha: Derived::initial(1.0),
            fill_alpha: Derived::initial(1.0),
            alpha_is_shape: Derived::initial(false),
            text_knockout: Derived::initial(true),
            soft_mask: Derived::initial(SoftMask::None),
            ext_gstate: None,
            text: TextState::default(),
            clip_paths: Vec::new(),
        }
    }
}

pub(crate) fn is_standard_blend_mode(name: &[u8]) -> bool {
    matches!(
        name,
        b"/Normal"
            | b"/Compatible"
            | b"/Multiply"
            | b"/Screen"
            | b"/Overlay"
            | b"/Darken"
            | b"/Lighten"
            | b"/ColorDodge"
            | b"/ColorBurn"
            | b"/HardLight"
            | b"/SoftLight"
            | b"/Difference"
            | b"/Exclusion"
            | b"/Hue"
            | b"/Saturation"
            | b"/Color"
            | b"/Luminosity"
    )
}
