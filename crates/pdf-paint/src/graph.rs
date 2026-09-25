use std::sync::Arc;

use pdf_bytes::SourceSpan;
use pdf_content::{FontRequest, FontSubstitution, SubstitutedFace, UnresolvedReason};
use pdf_content::{GlyphPath, GlyphProgram, GlyphSegment, SourceCode, ToUnicode};
use pdf_syntax::Reference;

use crate::color::ColorSpace;
use crate::geometry::{FillRule, Matrix, Path, PathSegment, Point, Shape};
use crate::image::ImagePaint;
use crate::provenance::Derived;
use crate::shading::ShadingPaint;
use crate::state::{GraphicsState, SoftMaskSubtype, SoftMaskTransfer, TextMatrices};
use crate::text::code_advance;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct PaintId {
    pub page: Reference,
    pub stream: Reference,
    pub operator_span: SourceSpan,
    pub invocation_path: Vec<FormInvocation>,
    pub pattern_path: Vec<PatternInvocation>,
    pub ordinal: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PaintAtom {
    pub id: PaintId,
    pub kind: PaintAtomKind,
    pub marks: Vec<MarkedContent>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InterpretRepair {
    pub kind: RepairKind,
    pub operator_span: SourceSpan,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepairKind {
    UnmatchedEndMarkedContent,
    AppearanceWithoutBBox,
    StreamDecoding { repair: pdf_syntax::StreamRepair },
    FontNameBoundToStandardFont,
    ExtGStateEntryIgnored { key_span: SourceSpan },
    ShadingEntryIgnored { key_span: SourceSpan },
    ColourOperandCount { expected: usize, actual: usize },
    UncolouredPatternOperandCount { expected: usize, actual: usize },
    JpxCodestream { repair: jpeg2000::Repair },
    NestedTextObject,
    TextOperatorOutsideTextObject,
    UnmatchedEndText,
    UnclosedTextObject,
    OperatorInsideTextObject,
    UnmatchedRestoreState,
    UnclosedSaveState { depth: usize },
}

impl std::fmt::Display for RepairKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnmatchedEndMarkedContent => {
                formatter.write_str("EMC with no marked-content section open")
            }
            Self::AppearanceWithoutBBox => formatter
                .write_str("annotation appearance has no /BBox; the rectangle's size was used"),
            Self::StreamDecoding { repair } => repair.fmt(formatter),
            Self::FontNameBoundToStandardFont => formatter
                .write_str("Tf named a font the page does not have; the standard font was used"),
            Self::ExtGStateEntryIgnored { .. } => {
                formatter.write_str("ExtGState entry this build does not implement; ignored")
            }
            Self::ShadingEntryIgnored { .. } => {
                formatter.write_str("shading dictionary entry this build does not read; ignored")
            }
            Self::ColourOperandCount { expected, actual } => write!(
                formatter,
                "colour space wanted {expected} operands and was given {actual}"
            ),
            Self::UncolouredPatternOperandCount { expected, actual } => write!(
                formatter,
                "uncoloured pattern wanted {expected} colour operands and was given {actual}"
            ),
            Self::JpxCodestream { repair } => write!(formatter, "JPEG 2000 image: {repair}"),
            Self::NestedTextObject => {
                formatter.write_str("BT inside a text object; it opened a new one")
            }
            Self::TextOperatorOutsideTextObject => {
                formatter.write_str("text operator outside BT/ET; the text state in hand was used")
            }
            Self::UnmatchedEndText => formatter.write_str("ET with no text object open"),
            Self::UnclosedTextObject => {
                formatter.write_str("content stream ends inside a text object")
            }
            Self::OperatorInsideTextObject => {
                formatter.write_str("operator not allowed inside a text object; it was carried out")
            }
            Self::UnmatchedRestoreState => {
                formatter.write_str("Q with an empty graphics-state stack; it was ignored")
            }
            Self::UnclosedSaveState { depth } => write!(
                formatter,
                "content stream ends with {depth} unmatched q; they were closed"
            ),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PaintGraph {
    pub atoms: Vec<PaintAtom>,
    pub repairs: Vec<InterpretRepair>,
    pub skipped: Vec<crate::InterpretError>,
    pub object_scopes: Vec<ObjectScope>,
}

const fn right_to_left(character: char) -> bool {
    matches!(
        character as u32,
        0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x1_0800..=0x1_0FFF | 0x1_E800..=0x1_EFFF
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActualText {
    pub text: String,
    pub first: bool,
}

impl PaintGraph {
    #[must_use]
    pub fn actual_texts(&self) -> std::collections::BTreeMap<usize, ActualText> {
        let mut spans: Vec<(SourceSpan, String, Vec<usize>)> = Vec::new();
        for (index, atom) in self.atoms.iter().enumerate() {
            let PaintAtomKind::Text(_) = &atom.kind else {
                continue;
            };
            let Some(mark) = atom
                .marks
                .iter()
                .rev()
                .find(|mark| mark.actual_text.is_some() && mark.written_here)
            else {
                continue;
            };
            match spans
                .iter_mut()
                .find(|(key, ..)| *key == mark.operator_span)
            {
                Some((_, _, atoms)) => atoms.push(index),
                None => spans.push((
                    mark.operator_span,
                    mark.actual_text.clone().unwrap_or_default(),
                    vec![index],
                )),
            }
        }
        let mut out = std::collections::BTreeMap::new();
        for (_, text, atoms) in spans {
            let mut read = String::new();
            for index in &atoms {
                if let PaintAtomKind::Text(run) = &self.atoms[*index].kind {
                    for glyph in &run.glyphs {
                        if let Some(meaning) = run.text.text_of(pdf_content::Code {
                            value: glyph.code.value,
                            byte_len: glyph.code.bytes.len(),
                        }) {
                            read.push_str(&meaning.text);
                        }
                    }
                }
            }
            if read == text && !text.chars().any(right_to_left) {
                continue;
            }
            for (at, index) in atoms.into_iter().enumerate() {
                out.insert(
                    index,
                    ActualText {
                        text: if at == 0 { text.clone() } else { String::new() },
                        first: at == 0,
                    },
                );
            }
        }
        out
    }

    #[must_use]
    pub fn footprint(&self) -> usize {
        let mut counted: std::collections::HashSet<*const u8> = std::collections::HashSet::new();
        let mut bytes = self.atoms.len() * std::mem::size_of::<PaintAtom>();
        for atom in &self.atoms {
            if let PaintAtomKind::Image(image) = &atom.kind {
                bytes += image_footprint(image, &mut counted);
            }
        }
        bytes
    }
}

fn image_footprint(
    image: &crate::ImagePaint,
    counted: &mut std::collections::HashSet<*const u8>,
) -> usize {
    let mut bytes = 0;
    if counted.insert(image.samples.as_ptr()) {
        bytes += image.samples.len();
    }
    if let Some(mask) = &image.soft_mask {
        bytes += image_footprint(mask, counted);
    }
    bytes
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObjectScope {
    pub stream: Reference,
    pub open: SourceSpan,
    pub close: SourceSpan,
    pub atoms: std::ops::Range<usize>,
    pub ctm: Derived<Matrix>,
    pub inherited_clips: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PaintAtomKind {
    Path(PathPaint),
    Text(TextShowPaint),
    TransparencyGroup(Box<TransparencyGroupPaint>),
    Shading(Box<ShadingPaint>),
    Image(Box<ImagePaint>),
}

impl PaintAtomKind {
    #[must_use]
    pub fn user_bounds(&self) -> Option<[f64; 4]> {
        match self {
            Self::Text(text) => text.outline_bounds(),
            Self::Path(paint) => bounds_of_points(path_points(&paint.path), paint.state.ctm.value),
            Self::Image(image) => {
                bounds_of_points(corners_of([0.0, 0.0, 1.0, 1.0]), image.state.ctm.value)
            }
            Self::Shading(shading) => bounds_of_points(
                corners_of(shading.bbox.as_ref()?.value),
                shading.state.ctm.value,
            ),
            Self::TransparencyGroup(group) => bounds_of_points(
                corners_of(group.bbox),
                group.state.ctm.value.multiply(group.matrix.value),
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PathPaint {
    pub path: Path,
    pub stroke: bool,
    pub fill: Option<FillRule>,
    pub state: GraphicsState,
}

impl PathPaint {
    #[must_use]
    pub fn own_bounds(&self) -> Option<[f64; 4]> {
        bounds_of_points(path_points(&self.path), Matrix::IDENTITY)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextShowPaint {
    pub elements: Vec<TextShowElement>,
    pub state: GraphicsState,
    pub matrices: TextMatrices,
    pub glyphs: Vec<PositionedGlyph>,
    pub program: Option<Arc<GlyphProgram>>,
    pub units_per_em: u16,
    pub text: Arc<ToUnicode>,
    pub substitution: Option<Arc<FontSubstitution>>,
    pub font_request: Option<Arc<FontRequest>>,
    pub type3: bool,
    pub family_line: Option<(f64, f64)>,
}

#[must_use]
pub fn usable_line((ascent, descent): (f64, f64)) -> Option<(f64, f64)> {
    (ascent.is_finite() && descent.is_finite() && ascent > descent).then_some((ascent, descent))
}

impl TextShowPaint {
    #[must_use]
    pub fn placed_shape(&self) -> Option<Shape> {
        let shape = self
            .state
            .ctm
            .value
            .multiply(self.matrices.text.value)
            .shape()?;
        Some(Shape {
            across: shape.across.abs(),
            along: shape.along,
            ..shape
        })
    }

    #[must_use]
    pub fn has_unshaped_cluster(&self) -> bool {
        self.substitution.is_some()
            && self.glyphs.iter().any(|glyph| {
                glyph.substituted.len() > 1 && glyph.substituted.iter().any(|drawn| !drawn.shaped)
            })
    }

    #[must_use]
    pub fn size_on_page(&self) -> Option<f64> {
        Some(self.state.text.font_size.value * self.placed_shape()?.across)
    }

    #[must_use]
    pub fn outline_points(&self) -> Option<Vec<Point>> {
        self.outline_points_in(0..self.glyphs.len())
    }

    #[must_use]
    pub fn outline_points_in(&self, range: std::ops::Range<usize>) -> Option<Vec<Point>> {
        if range.end > self.glyphs.len() || range.start > range.end {
            return None;
        }
        if self.type3 {
            let mut points = Vec::new();
            for glyph in &self.glyphs[range] {
                let Some(procedure) = glyph.procedure.as_ref() else {
                    continue;
                };
                for atom in &procedure.atoms {
                    if let Some(bounds) = atom.kind.user_bounds() {
                        points.extend(corners_of(bounds));
                    }
                }
            }
            return (!points.is_empty()).then_some(points);
        }

        let mut points = Vec::new();
        for glyph in &self.glyphs[range] {
            let placed = self.state.ctm.value.multiply(glyph.matrix);
            for (outline, units_per_em) in self.outlines_of(glyph) {
                let scale = 1.0 / f64::from(units_per_em);
                let mut place = |x: f64, y: f64| {
                    points.push(placed.transform(Point {
                        x: x * scale,
                        y: y * scale,
                    }));
                };
                for segment in &outline.segments {
                    match *segment {
                        GlyphSegment::MoveTo { x, y } | GlyphSegment::LineTo { x, y } => {
                            place(x, y);
                        }
                        GlyphSegment::CurveTo {
                            x1,
                            y1,
                            x2,
                            y2,
                            x,
                            y,
                        } => {
                            place(x1, y1);
                            place(x2, y2);
                            place(x, y);
                        }
                        GlyphSegment::Close => {}
                    }
                }
            }
        }
        (!points.is_empty()).then_some(points)
    }

    #[must_use]
    pub fn draws_no_ink(&self) -> bool {
        self.draws_no_ink_in(0..self.glyphs.len())
    }

    #[must_use]
    pub fn draws_no_ink_in(&self, range: std::ops::Range<usize>) -> bool {
        if range.end > self.glyphs.len() || range.start >= range.end {
            return false;
        }
        if self.type3 {
            return self.glyphs[range].iter().all(|glyph| {
                glyph.procedure.as_ref().is_none_or(|procedure| {
                    procedure
                        .atoms
                        .iter()
                        .all(|atom| atom.kind.user_bounds().is_none())
                })
            });
        }
        if self.program.is_none() && self.substitution.is_none() {
            return false;
        }
        self.glyphs[range].iter().all(|glyph| {
            let outlines = self.outlines_of(glyph);
            !outlines.is_empty()
                && outlines
                    .iter()
                    .all(|(outline, _)| outline.segments.is_empty())
        })
    }

    #[must_use]
    pub fn substituted_face(&self, face: u16) -> Option<&SubstitutedFace> {
        let substitution = self.substitution.as_ref()?;
        if face == 0 {
            return Some(&substitution.primary);
        }
        substitution.fallbacks.get(usize::from(face) - 1)
    }

    #[must_use]
    pub fn outlines_of(&self, glyph: &PositionedGlyph) -> Vec<(GlyphPath, u16)> {
        if !glyph.substituted.is_empty() {
            return glyph
                .substituted
                .iter()
                .filter_map(|drawn| {
                    let face = self.substituted_face(drawn.face)?;
                    let mut outline = face.program.path(drawn.glyph)?;
                    outline.translate(f64::from(drawn.offset[0]), f64::from(drawn.offset[1]));
                    Some((outline, face.program.units_per_em()))
                })
                .collect();
        }
        let Some(program) = self.program.as_ref() else {
            return Vec::new();
        };
        let Some(index) = glyph.glyph else {
            return Vec::new();
        };
        program
            .path(index)
            .map(|outline| vec![(outline, self.units_per_em)])
            .unwrap_or_default()
    }

    #[must_use]
    pub fn outline_bounds(&self) -> Option<[f64; 4]> {
        self.outline_bounds_in(0..self.glyphs.len())
    }

    #[must_use]
    pub fn clone_without_glyphs(&self) -> Self {
        Self {
            elements: Vec::new(),
            state: self.state.clone(),
            matrices: self.matrices.clone(),
            glyphs: Vec::new(),
            program: self.program.clone(),
            units_per_em: self.units_per_em,
            text: Arc::clone(&self.text),
            substitution: self.substitution.clone(),
            font_request: self.font_request.clone(),
            type3: self.type3,
            family_line: self.family_line,
        }
    }

    #[must_use]
    pub fn outline_bounds_in(&self, range: std::ops::Range<usize>) -> Option<[f64; 4]> {
        let points = self.outline_points_in(range)?;
        let mut bounds = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        for point in points {
            bounds[0] = bounds[0].min(point.x);
            bounds[1] = bounds[1].min(point.y);
            bounds[2] = bounds[2].max(point.x);
            bounds[3] = bounds[3].max(point.y);
        }
        Some(bounds)
    }

    #[must_use]
    pub fn advance_bounds_in(&self, range: std::ops::Range<usize>) -> Option<[f64; 4]> {
        if range.end > self.glyphs.len() || range.start >= range.end {
            return None;
        }
        let (ascent, descent) = self.line_metrics()?;
        let mut bounds = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        for glyph in &self.glyphs[range] {
            let placed = self.state.ctm.value.multiply(glyph.matrix);
            let across = glyph.code.width * 0.001;
            for (x, y) in [
                (0.0, descent),
                (across, descent),
                (across, ascent),
                (0.0, ascent),
            ] {
                let at = placed.transform(Point { x, y });
                bounds = [
                    bounds[0].min(at.x),
                    bounds[1].min(at.y),
                    bounds[2].max(at.x),
                    bounds[3].max(at.y),
                ];
            }
        }
        Some(bounds)
    }

    #[must_use]
    pub fn line_metrics(&self) -> Option<(f64, f64)> {
        let usable = usable_line;
        self.font_request
            .as_deref()
            .or_else(|| {
                self.substitution
                    .as_deref()
                    .map(|found| found.request.as_ref())
            })
            .and_then(|request| Some((request.ascent? / 1000.0, request.descent? / 1000.0)))
            .and_then(usable)
            .or_else(|| self.program.as_ref()?.vertical_metrics().and_then(usable))
            .or_else(|| self.family_line.and_then(usable))
            .or_else(|| {
                self.substitution
                    .as_ref()?
                    .primary
                    .program
                    .vertical_metrics()
                    .and_then(usable)
            })
    }

    #[must_use]
    pub fn layout_bounds_in(&self, range: std::ops::Range<usize>) -> Option<[f64; 4]> {
        let mut bounds = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        for placed in self.layout_points_in(range)? {
            bounds[0] = bounds[0].min(placed.x);
            bounds[1] = bounds[1].min(placed.y);
            bounds[2] = bounds[2].max(placed.x);
            bounds[3] = bounds[3].max(placed.y);
        }
        bounds
            .iter()
            .all(|value| value.is_finite())
            .then_some(bounds)
    }

    #[must_use]
    pub fn layout_points_in(&self, range: std::ops::Range<usize>) -> Option<Vec<Point>> {
        if self.type3 || range.start >= range.end || range.end > self.glyphs.len() {
            return None;
        }
        let (ascent, descent) = self.line_metrics()?;
        let ctm = self.state.ctm.value;
        let mut points = Vec::with_capacity(4 * range.len());
        for glyph in &self.glyphs[range] {
            let pen = glyph.text_matrix;
            let after = pen.multiply(Matrix {
                e: code_advance(&self.state.text, &glyph.code, 0.001),
                ..Matrix::IDENTITY
            });
            let (along_x, along_y) = (after.e - pen.e, after.f - pen.f);
            for height in [ascent, descent] {
                let start = glyph.matrix.transform(Point { x: 0.0, y: height });
                for point in [
                    start,
                    Point {
                        x: start.x + along_x,
                        y: start.y + along_y,
                    },
                ] {
                    points.push(ctm.transform(point));
                }
            }
        }
        Some(points)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum TextShowElement {
    Codes {
        source_span: SourceSpan,
        decoded_bytes: Vec<u8>,
        codes: Vec<SourceCode>,
    },
    Adjustment {
        source_span: SourceSpan,
        value: f64,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct PositionedGlyph {
    pub code: SourceCode,
    pub glyph: Option<u16>,
    pub matrix: Matrix,
    pub text_matrix: Matrix,
    pub procedure: Option<Arc<Type3Glyph>>,
    pub substituted: Vec<SubstitutedGlyph>,
    pub unresolved: Option<UnresolvedReason>,
    pub silent: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SubstitutedGlyph {
    pub face: u16,
    pub glyph: u16,
    pub offset: [i32; 2],
    pub shaped: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Type3Glyph {
    pub name: Vec<u8>,
    pub reference: Reference,
    pub atoms: Vec<PaintAtom>,
    pub shape_only: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MarkedContent {
    pub tag: Vec<u8>,
    pub tag_span: SourceSpan,
    pub operator_span: SourceSpan,
    pub properties: Option<MarkedProperties>,
    pub actual_text: Option<String>,
    pub written_here: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MarkedProperties {
    Inline(SourceSpan),
    Resource {
        name: Vec<u8>,
        name_span: SourceSpan,
        reference: Option<Reference>,
        dictionary_span: SourceSpan,
    },
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FormInvocation {
    pub form: Reference,
    pub operator_span: SourceSpan,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PatternInvocation {
    pub pattern: Reference,
    pub operator_span: SourceSpan,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TilingPatternPaint {
    pub reference: Reference,
    pub dictionary_span: SourceSpan,
    pub paint_type: Derived<i64>,
    pub bbox: [f64; 4],
    pub bbox_span: SourceSpan,
    pub x_step: Derived<f64>,
    pub y_step: Derived<f64>,
    pub matrix: Derived<Matrix>,
    pub tiling_type: Derived<i64>,
    pub graph: PaintGraph,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShadingPatternPaint {
    pub reference: Option<Reference>,
    pub dictionary_span: SourceSpan,
    pub matrix: Derived<Matrix>,
    pub base: Matrix,
    pub shading: ShadingPaint,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroupBackdrop {
    Transparent,
    Inherited,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TransparencyGroupPaint {
    pub reference: Reference,
    pub dictionary_span: SourceSpan,
    pub group_span: SourceSpan,
    pub group_reference_span: Option<SourceSpan>,
    pub subtype_span: SourceSpan,
    pub bbox: [f64; 4],
    pub bbox_span: SourceSpan,
    pub matrix: Derived<Matrix>,
    pub isolated: Derived<bool>,
    pub knockout: Derived<bool>,
    pub blend_space: Option<Derived<ColorSpace>>,
    pub backdrop: Derived<GroupBackdrop>,
    pub state: GraphicsState,
    pub graph: PaintGraph,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SoftMaskPaint {
    pub dictionary_span: SourceSpan,
    pub dictionary_reference_span: Option<SourceSpan>,
    pub subtype: Derived<SoftMaskSubtype>,
    pub group: Box<TransparencyGroupPaint>,
    pub backdrop_color: Option<Derived<Vec<f64>>>,
    pub transfer: Derived<SoftMaskTransfer>,
}

fn path_points(path: &Path) -> Vec<Point> {
    let mut points = Vec::new();
    for segment in &path.segments {
        match segment {
            PathSegment::MoveTo { point, .. } | PathSegment::LineTo { point, .. } => {
                points.push(*point);
            }
            PathSegment::CubicTo {
                control_1,
                control_2,
                end,
                ..
            } => points.extend_from_slice(&[*control_1, *control_2, *end]),
            PathSegment::Rectangle {
                origin,
                width,
                height,
                ..
            } => points.extend_from_slice(&[
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
            ]),
            PathSegment::ClosePath { .. } => {}
        }
    }
    points
}

fn corners_of(bounds: [f64; 4]) -> Vec<Point> {
    vec![
        Point {
            x: bounds[0],
            y: bounds[1],
        },
        Point {
            x: bounds[2],
            y: bounds[1],
        },
        Point {
            x: bounds[2],
            y: bounds[3],
        },
        Point {
            x: bounds[0],
            y: bounds[3],
        },
    ]
}

fn bounds_of_points(points: Vec<Point>, matrix: Matrix) -> Option<[f64; 4]> {
    let mut bounds = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    let mut any = false;
    for point in points {
        let placed = matrix.transform(point);
        if !placed.x.is_finite() || !placed.y.is_finite() {
            continue;
        }
        any = true;
        bounds[0] = bounds[0].min(placed.x);
        bounds[1] = bounds[1].min(placed.y);
        bounds[2] = bounds[2].max(placed.x);
        bounds[3] = bounds[3].max(placed.y);
    }
    any.then_some(bounds)
}
