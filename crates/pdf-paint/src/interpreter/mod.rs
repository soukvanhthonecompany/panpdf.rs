use std::collections::HashMap;
use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceSpan};
use pdf_content::{
    FontProvider, FontRequest, Operation, PageResources, SubstitutedFace, ToUnicode, Type3Font,
};
use pdf_syntax::{NumberKind, Object, ObjectKind, Reference};

use crate::color::{Color, ColorSpace};
use crate::error::{InterpretError, InterpretErrorKind};
use crate::geometry::{FillRule, Matrix, Path, Point};
use crate::graph::{FormInvocation, InterpretRepair, MarkedContent, PaintGraph, PatternInvocation};
use crate::provenance::Derived;
use crate::state::{GraphicsState, PaintLimits, TextMatrices};

#[cfg(test)]
mod tests;

mod annotation;
mod colour;

pub(crate) use annotation::{AppearanceRun, run_appearance};
mod graphics;
mod image;
mod path;
mod text;
mod xobject;

#[derive(Debug, Default)]
struct ClipAccumulators {
    pending: Option<(FillRule, SourceSpan)>,
    text: Option<(Path, SourceSpan)>,
}

struct SavedGraphicsState {
    state: GraphicsState,
    opening: Option<(Reference, SourceSpan, usize)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaintContext {
    pub page: Reference,
    pub stream: Reference,
    pub invocation_path: Vec<FormInvocation>,
}

impl PaintContext {
    #[must_use]
    pub fn page_stream(page: Reference, stream: Reference) -> Self {
        Self {
            page,
            stream,
            invocation_path: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PaintStream<'a> {
    pub source: &'a ByteStore,
    pub reference: Reference,
    pub operations: &'a [Operation],
}

pub fn interpret_operations(
    source: &ByteStore,
    operations: &[Operation],
    context: &PaintContext,
    limits: PaintLimits,
) -> Result<PaintGraph, InterpretError> {
    interpret_stream_sequence(
        &[PaintStream {
            source,
            reference: context.stream,
            operations,
        }],
        context.page,
        &context.invocation_path,
        limits,
    )
}

pub fn interpret_stream_sequence(
    streams: &[PaintStream<'_>],
    page: Reference,
    invocation_path: &[FormInvocation],
    limits: PaintLimits,
) -> Result<PaintGraph, InterpretError> {
    interpret_stream_sequence_inner(streams, page, invocation_path, None, limits, None)
}

pub fn interpret_stream_sequence_with_resources(
    streams: &[PaintStream<'_>],
    page: Reference,
    invocation_path: &[FormInvocation],
    resources: &PageResources,
    limits: PaintLimits,
) -> Result<PaintGraph, InterpretError> {
    interpret_stream_sequence_inner(
        streams,
        page,
        invocation_path,
        Some(resources),
        limits,
        None,
    )
}

pub fn interpret_stream_sequence_with_fonts(
    streams: &[PaintStream<'_>],
    page: Reference,
    invocation_path: &[FormInvocation],
    resources: &PageResources,
    limits: PaintLimits,
    fonts: Option<Arc<dyn FontProvider>>,
) -> Result<PaintGraph, InterpretError> {
    interpret_stream_sequence_inner(
        streams,
        page,
        invocation_path,
        Some(resources),
        limits,
        fonts,
    )
}

pub fn interpret_stream_sequence_tolerating_unsupported(
    streams: &[PaintStream<'_>],
    page: Reference,
    invocation_path: &[FormInvocation],
    resources: &PageResources,
    limits: PaintLimits,
    fonts: Option<Arc<dyn FontProvider>>,
) -> Result<PaintGraph, InterpretError> {
    let Some(first) = streams.first() else {
        return Ok(PaintGraph::default());
    };
    let provider = fonts.clone();
    let mut interpreter = Interpreter::new(
        first.source,
        page,
        first.reference,
        invocation_path,
        Some(resources),
        limits,
        fonts,
    );
    interpreter.tolerate_unsupported = true;
    let mut graph = interpreter.run(streams)?;
    if invocation_path.is_empty() {
        crate::decipher_fonts::read_lying_fonts(&mut graph, provider.as_ref(), page);
    }
    Ok(graph)
}

fn interpret_stream_sequence_inner(
    streams: &[PaintStream<'_>],
    page: Reference,
    invocation_path: &[FormInvocation],
    resources: Option<&PageResources>,
    limits: PaintLimits,
    fonts: Option<Arc<dyn FontProvider>>,
) -> Result<PaintGraph, InterpretError> {
    let Some(first) = streams.first() else {
        return Ok(PaintGraph::default());
    };
    let provider = fonts.clone();
    let mut graph = Interpreter::new(
        first.source,
        page,
        first.reference,
        invocation_path,
        resources,
        limits,
        fonts,
    )
    .run(streams)?;
    if invocation_path.is_empty() {
        crate::decipher_fonts::read_lying_fonts(&mut graph, provider.as_ref(), page);
    }
    Ok(graph)
}

#[derive(Debug)]
pub(crate) struct ResolvedFont {
    pub(crate) request: Arc<FontRequest>,
    pub(crate) face: Option<SubstitutedFace>,
}

struct Interpreter {
    source: ByteStore,
    sibling_sources: Vec<ByteStore>,
    page: Reference,
    stream: Reference,
    invocation_path: Vec<FormInvocation>,
    resources: Option<PageResources>,
    pattern_base: Matrix,
    repairs: Vec<InterpretRepair>,
    tolerate_unsupported: bool,
    skipped: Vec<InterpretError>,
    limits: PaintLimits,
    state: GraphicsState,
    stack: Vec<SavedGraphicsState>,
    stack_floor: usize,
    path: Path,
    current_point: Option<Point>,
    subpath_start: Option<Point>,
    pending_clip: Option<(FillRule, SourceSpan)>,
    text_clip: Option<(Path, SourceSpan)>,
    text_matrices: Option<TextMatrices>,
    retained_text_matrices: TextMatrices,
    graph: PaintGraph,
    paint_atoms: usize,
    form_invocations: usize,
    pattern_path: Vec<PatternInvocation>,
    pattern_invocations: usize,
    marks: Vec<MarkedContent>,
    hidden_marks: Vec<usize>,
    mark_floor: usize,
    compatibility_depth: usize,
    glyph_depth: usize,
    glyph_shape_only: bool,
    colour_is_fixed: bool,
    type3_fonts: HashMap<Reference, Option<Arc<Type3Font>>>,
    fonts: Option<Arc<dyn FontProvider>>,
    font_faces: HashMap<Reference, Option<Arc<ResolvedFont>>>,
    font_requests: HashMap<Reference, Option<Arc<FontRequest>>>,
    text_maps: HashMap<Reference, Arc<ToUnicode>>,
    parsed_fonts: HashMap<Reference, (pdf_content::ResourceEntry, Arc<pdf_content::Font>)>,
    font_programs: HashMap<Reference, Option<Arc<pdf_content::GlyphProgram>>>,
    outlines_tried: HashMap<Reference, std::collections::HashSet<(u32, usize)>>,
    matched_runs: Vec<(usize, Reference)>,
    type3_procedures: HashMap<Reference, Arc<Vec<Operation>>>,
}

struct ColourState {
    fill: Derived<Color>,
    stroke: Derived<Color>,
    fill_space: Derived<ColorSpace>,
    stroke_space: Derived<ColorSpace>,
}

struct NestedFrame {
    source: ByteStore,
    stream: Reference,
    resources: Option<PageResources>,
    pattern_base: Matrix,
    state: GraphicsState,
    path: Path,
    current_point: Option<Point>,
    subpath_start: Option<Point>,
    clips: ClipAccumulators,
    text_matrices: Option<TextMatrices>,
    retained_text_matrices: TextMatrices,
    stack_floor: usize,
    mark_floor: usize,
    compatibility_depth: usize,
}

fn is_operator_allowed_in_text_object(operator: &[u8]) -> bool {
    matches!(
        operator,
        b"BMC"
            | b"BDC"
            | b"EMC"
            | b"MP"
            | b"DP"
            | b"BX"
            | b"EX"
            | b"BT"
            | b"ET"
            | b"w"
            | b"J"
            | b"j"
            | b"M"
            | b"d"
            | b"ri"
            | b"i"
            | b"gs"
            | b"G"
            | b"g"
            | b"RG"
            | b"rg"
            | b"K"
            | b"k"
            | b"CS"
            | b"cs"
            | b"SC"
            | b"sc"
            | b"SCN"
            | b"scn"
            | b"Tc"
            | b"Tw"
            | b"Tz"
            | b"TL"
            | b"Tf"
            | b"Tr"
            | b"Ts"
            | b"Td"
            | b"TD"
            | b"Tm"
            | b"T*"
            | b"Tj"
            | b"TJ"
            | b"'"
            | b"\""
    )
}

fn is_path_painting_operator(operator: &[u8]) -> bool {
    matches!(
        operator,
        b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n"
    )
}

fn is_colour_operator(operator: &[u8]) -> bool {
    matches!(
        operator,
        b"G" | b"g" | b"RG" | b"rg" | b"K" | b"k" | b"CS" | b"cs" | b"SC" | b"SCN" | b"sc" | b"scn"
    )
}

fn is_known_operator(operator: &[u8]) -> bool {
    is_operator_allowed_in_text_object(operator)
        || matches!(
            operator,
            b"q" | b"Q"
                | b"cm"
                | b"Do"
                | b"EI"
                | b"sh"
                | b"m"
                | b"l"
                | b"c"
                | b"v"
                | b"y"
                | b"h"
                | b"re"
                | b"W"
                | b"W*"
                | b"S"
                | b"s"
                | b"f"
                | b"F"
                | b"f*"
                | b"B"
                | b"B*"
                | b"b"
                | b"b*"
                | b"n"
                | b"d0"
                | b"d1"
        )
}

impl Interpreter {
    pub(crate) fn new(
        source: &ByteStore,
        page: Reference,
        stream: Reference,
        invocation_path: &[FormInvocation],
        resources: Option<&PageResources>,
        limits: PaintLimits,
        fonts: Option<Arc<dyn FontProvider>>,
    ) -> Self {
        Self {
            source: source.clone(),
            sibling_sources: Vec::new(),
            page,
            stream,
            invocation_path: invocation_path.to_vec(),
            resources: resources.cloned(),
            limits,
            state: GraphicsState::default(),
            stack: Vec::new(),
            stack_floor: 0,
            path: Path::default(),
            current_point: None,
            subpath_start: None,
            pending_clip: None,
            text_clip: None,
            text_matrices: None,
            retained_text_matrices: TextMatrices::identity(),
            graph: PaintGraph::default(),
            paint_atoms: 0,
            form_invocations: 0,
            pattern_base: Matrix::IDENTITY,
            repairs: Vec::new(),
            tolerate_unsupported: false,
            skipped: Vec::new(),
            pattern_path: Vec::new(),
            pattern_invocations: 0,
            marks: Vec::new(),
            hidden_marks: Vec::new(),
            mark_floor: 0,
            compatibility_depth: 0,
            glyph_depth: 0,
            glyph_shape_only: false,
            colour_is_fixed: false,
            type3_fonts: HashMap::new(),
            fonts,
            font_faces: HashMap::new(),
            font_requests: HashMap::new(),
            text_maps: HashMap::new(),
            parsed_fonts: HashMap::new(),
            font_programs: HashMap::new(),
            outlines_tried: HashMap::new(),
            matched_runs: Vec::new(),
            type3_procedures: HashMap::new(),
        }
    }

    pub(crate) fn run(mut self, streams: &[PaintStream<'_>]) -> Result<PaintGraph, InterpretError> {
        self.sibling_sources = streams.iter().map(|stream| stream.source.clone()).collect();
        let mut last_operator = None;
        for stream in streams {
            self.source = stream.source.clone();
            self.stream = stream.reference;
            for operation in stream.operations {
                if let Err(error) = self.apply(operation) {
                    if !(self.tolerate_unsupported && error.kind().is_skippable()) {
                        return Err(error);
                    }
                    if operation
                        .operator_bytes(&self.source)
                        .is_ok_and(is_path_painting_operator)
                    {
                        self.abandon_path();
                    }
                    self.skipped.push(error);
                }
                last_operator = Some(operation.operator_span());
            }
        }
        self.close_unbalanced_state(last_operator);
        if !self.marks.is_empty() {
            return Err(InterpretError::new(
                last_operator,
                InterpretErrorKind::UnbalancedMarkedContent {
                    depth: self.marks.len(),
                },
            ));
        }
        if self.compatibility_depth != 0 {
            return Err(InterpretError::new(
                last_operator,
                InterpretErrorKind::UnbalancedCompatibilitySection {
                    depth: self.compatibility_depth,
                },
            ));
        }
        for (ordinal, reference) in std::mem::take(&mut self.matched_runs) {
            if let Some(last) = self.text_maps.get(&reference)
                && let Some(crate::graph::PaintAtomKind::Text(text)) =
                    self.graph.atoms.get_mut(ordinal).map(|atom| &mut atom.kind)
                && !Arc::ptr_eq(&text.text, last)
            {
                text.text = Arc::clone(last);
            }
        }
        self.graph.repairs = std::mem::take(&mut self.repairs);
        self.graph.skipped = std::mem::take(&mut self.skipped);
        Ok(self.graph)
    }

    pub(crate) fn apply(&mut self, operation: &Operation) -> Result<(), InterpretError> {
        let operator = operation
            .operator_bytes(&self.source)
            .map_err(|_| InterpretError::at(operation, InterpretErrorKind::SourceSpanFailure))?;
        let misplaced = self.text_matrices.is_some()
            && is_known_operator(operator)
            && !is_operator_allowed_in_text_object(operator);
        if misplaced {
            self.repairs.push(InterpretRepair {
                kind: crate::graph::RepairKind::OperatorInsideTextObject,
                operator_span: operation.operator_span(),
            });
        }
        let suppressed = ((self.glyph_shape_only || self.colour_is_fixed)
            && is_colour_operator(operator))
        .then(|| self.colour_state());
        let result = match operator {
            b"q" => self.save(operation),
            b"Q" => self.restore(operation),
            b"cm" => self.concatenate_matrix(operation),
            b"w" => self.set_line_width(operation),
            b"J" => self.set_line_cap(operation),
            b"j" => self.set_line_join(operation),
            b"M" => self.set_miter_limit(operation),
            b"d" => self.set_dash(operation),
            b"i" => self.set_flatness(operation),
            b"ri" => self.set_rendering_intent(operation),
            b"gs" => self.apply_ext_gstate(operation),
            b"Do" => self.invoke_xobject(operation),
            b"EI" => self.paint_inline_image(operation),
            b"sh" => self.paint_shading(operation),
            b"BT" => self.begin_text(operation),
            b"ET" => self.end_text(operation),
            b"Tc" => self.set_character_spacing(operation),
            b"Tw" => self.set_word_spacing(operation),
            b"Tz" => self.set_horizontal_scaling(operation),
            b"TL" => self.set_text_leading(operation),
            b"Tf" => self.set_text_font(operation),
            b"Tr" => self.set_text_rendering_mode(operation),
            b"Ts" => self.set_text_rise(operation),
            b"Td" => self.move_text_line(operation, false),
            b"TD" => self.move_text_line(operation, true),
            b"Tm" => self.set_text_matrix(operation),
            b"T*" => self.next_text_line(operation),
            b"Tj" => self.show_text(operation, false),
            b"TJ" => self.show_text(operation, true),
            b"'" => self.show_next_line(operation),
            b"\"" => self.show_next_line_spaced(operation),
            b"G" => self.set_gray(operation, true),
            b"g" => self.set_gray(operation, false),
            b"RG" => self.set_rgb(operation, true),
            b"rg" => self.set_rgb(operation, false),
            b"K" => self.set_cmyk(operation, true),
            b"k" => self.set_cmyk(operation, false),
            b"CS" => self.set_color_space(operation, true),
            b"cs" => self.set_color_space(operation, false),
            b"SC" | b"SCN" => self.set_selected_color(operation, true),
            b"sc" | b"scn" => self.set_selected_color(operation, false),
            b"m" => self.move_to(operation),
            b"l" => self.line_to(operation),
            b"c" => self.curve_to(operation),
            b"v" => self.curve_to_v(operation),
            b"y" => self.curve_to_y(operation),
            b"h" => self.close_path(operation),
            b"re" => self.rectangle(operation),
            b"W" => self.mark_clip(operation, FillRule::Nonzero),
            b"W*" => self.mark_clip(operation, FillRule::EvenOdd),
            b"S" => self.finish_path(operation, true, None, false),
            b"s" => self.finish_path(operation, true, None, true),
            b"f" | b"F" => self.finish_path(operation, false, Some(FillRule::Nonzero), false),
            b"f*" => self.finish_path(operation, false, Some(FillRule::EvenOdd), false),
            b"B" => self.finish_path(operation, true, Some(FillRule::Nonzero), false),
            b"B*" => self.finish_path(operation, true, Some(FillRule::EvenOdd), false),
            b"b" => self.finish_path(operation, true, Some(FillRule::Nonzero), true),
            b"b*" => self.finish_path(operation, true, Some(FillRule::EvenOdd), true),
            b"n" => self.finish_path(operation, false, None, false),
            b"BMC" => self.begin_marked_content(operation, false),
            b"BDC" => self.begin_marked_content(operation, true),
            b"EMC" => self.end_marked_content(operation),
            b"MP" => self.marked_point(operation, false),
            b"DP" => self.marked_point(operation, true),
            b"BX" => self.begin_compatibility(operation),
            b"EX" => self.end_compatibility(operation),
            b"d0" => self.set_glyph_width(operation),
            b"d1" => self.set_glyph_shape(operation),
            _ if self.compatibility_depth != 0 => Ok(()),
            _ => Err(InterpretError::at(
                operation,
                InterpretErrorKind::UnsupportedOperator,
            )),
        };
        if let Some(colour) = suppressed {
            self.restore_colour_state(colour);
        }
        result
    }

    fn take_clip_accumulators(&mut self) -> ClipAccumulators {
        ClipAccumulators {
            pending: self.pending_clip.take(),
            text: self.text_clip.take(),
        }
    }

    fn restore_clip_accumulators(&mut self, parent: ClipAccumulators) {
        self.pending_clip = parent.pending;
        self.text_clip = parent.text;
    }

    fn emit(&mut self, atom: crate::graph::PaintAtom) {
        if self.hidden_marks.is_empty() {
            self.graph.atoms.push(atom);
        }
    }

    fn record_stream_repairs(
        &mut self,
        operation: &Operation,
        repairs: &[pdf_syntax::StreamRepair],
    ) {
        for repair in repairs {
            self.repairs.push(InterpretRepair {
                kind: crate::graph::RepairKind::StreamDecoding { repair: *repair },
                operator_span: operation.operator_span(),
            });
        }
    }

    fn enter_nested(
        &mut self,
        source: ByteStore,
        stream: Reference,
        resources: Option<PageResources>,
    ) -> NestedFrame {
        NestedFrame {
            source: std::mem::replace(&mut self.source, source),
            stream: std::mem::replace(&mut self.stream, stream),
            resources: std::mem::replace(&mut self.resources, resources),
            pattern_base: self.pattern_base,
            state: self.state.clone(),
            path: std::mem::take(&mut self.path),
            current_point: self.current_point.take(),
            subpath_start: self.subpath_start.take(),
            clips: self.take_clip_accumulators(),
            text_matrices: self.text_matrices.take(),
            retained_text_matrices: std::mem::replace(
                &mut self.retained_text_matrices,
                TextMatrices::identity(),
            ),
            stack_floor: std::mem::replace(&mut self.stack_floor, self.stack.len()),
            mark_floor: std::mem::replace(&mut self.mark_floor, self.marks.len()),
            compatibility_depth: std::mem::take(&mut self.compatibility_depth),
        }
    }

    fn leave_nested(&mut self, frame: NestedFrame) {
        self.stack_floor = frame.stack_floor;
        self.mark_floor = frame.mark_floor;
        self.compatibility_depth = frame.compatibility_depth;
        self.source = frame.source;
        self.stream = frame.stream;
        self.resources = frame.resources;
        self.pattern_base = frame.pattern_base;
        self.state = frame.state;
        self.path = frame.path;
        self.current_point = frame.current_point;
        self.subpath_start = frame.subpath_start;
        self.restore_clip_accumulators(frame.clips);
        self.text_matrices = frame.text_matrices;
        self.retained_text_matrices = frame.retained_text_matrices;
    }

    pub(super) fn text_matrices_in_hand(&mut self, operation: &Operation) -> &mut TextMatrices {
        if self.text_matrices.is_none() {
            self.repairs.push(InterpretRepair {
                kind: crate::graph::RepairKind::TextOperatorOutsideTextObject,
                operator_span: operation.operator_span(),
            });
            self.text_matrices = Some(self.retained_text_matrices.clone());
        }
        self.text_matrices
            .as_mut()
            .expect("a text object was just opened")
    }

    pub(super) fn repair(&mut self, operation: &Operation, kind: crate::graph::RepairKind) {
        self.repairs.push(InterpretRepair {
            kind,
            operator_span: operation.operator_span(),
        });
    }

    fn check_nested_balance(
        &mut self,
        last_operator: Option<SourceSpan>,
    ) -> Result<(), InterpretError> {
        self.close_unbalanced_state(last_operator);
        if self.marks.len() != self.mark_floor {
            return Err(InterpretError::new(
                last_operator,
                InterpretErrorKind::UnbalancedMarkedContent {
                    depth: self.marks.len() - self.mark_floor,
                },
            ));
        }
        if self.compatibility_depth != 0 {
            return Err(InterpretError::new(
                last_operator,
                InterpretErrorKind::UnbalancedCompatibilitySection {
                    depth: self.compatibility_depth,
                },
            ));
        }
        Ok(())
    }

    fn close_unbalanced_state(&mut self, last_operator: Option<SourceSpan>) {
        let depth = self.stack.len() - self.stack_floor;
        if depth > 0 {
            self.stack.truncate(self.stack_floor);
            self.repairs.push(InterpretRepair {
                kind: crate::graph::RepairKind::UnclosedSaveState { depth },
                operator_span: last_operator.unwrap_or_else(|| SourceSpan::empty(self.source.id())),
            });
        }
        if self.text_matrices.take().is_some() {
            self.repairs.push(InterpretRepair {
                kind: crate::graph::RepairKind::UnclosedTextObject,
                operator_span: last_operator.unwrap_or_else(|| SourceSpan::empty(self.source.id())),
            });
        }
    }

    fn exact_integer(&self, operation: &Operation) -> Result<i64, InterpretError> {
        Self::expect_count(operation, 1)?;
        let object = &operation.operands()[0];
        if !matches!(object.kind(), ObjectKind::Number(NumberKind::Integer)) {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::OperandType,
            ));
        }
        let bytes = self
            .operand_source(object)
            .resolve(object.span())
            .map_err(|_| InterpretError::at(operation, InterpretErrorKind::SourceSpanFailure))?;
        let text = std::str::from_utf8(bytes)
            .map_err(|_| InterpretError::at(operation, InterpretErrorKind::InvalidNumber))?;
        text.parse::<i64>()
            .map_err(|_| InterpretError::at(operation, InterpretErrorKind::InvalidNumber))
    }

    pub(crate) fn numbers<const N: usize>(
        &self,
        operation: &Operation,
    ) -> Result<[f64; N], InterpretError> {
        Self::expect_count(operation, N)?;
        let mut values = [0.0; N];
        for (slot, object) in values.iter_mut().zip(operation.operands()) {
            *slot = self.number(object, operation)?;
        }
        Ok(values)
    }

    fn operand_source(&self, object: &Object) -> &ByteStore {
        if self.source.id() == object.span().source() {
            return &self.source;
        }
        self.sibling_sources
            .iter()
            .find(|store| store.id() == object.span().source())
            .unwrap_or(&self.source)
    }

    pub(crate) fn number(
        &self,
        object: &Object,
        operation: &Operation,
    ) -> Result<f64, InterpretError> {
        if !matches!(object.kind(), ObjectKind::Number(_)) {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::OperandType,
            ));
        }
        let bytes = self
            .operand_source(object)
            .resolve(object.span())
            .map_err(|_| InterpretError::at(operation, InterpretErrorKind::SourceSpanFailure))?;
        let text = std::str::from_utf8(bytes)
            .map_err(|_| InterpretError::at(operation, InterpretErrorKind::InvalidNumber))?;
        let value = text
            .parse::<f64>()
            .map_err(|_| InterpretError::at(operation, InterpretErrorKind::InvalidNumber))?;
        if !value.is_finite() {
            return Err(InterpretError::at(
                operation,
                InterpretErrorKind::InvalidNumber,
            ));
        }
        Ok(value)
    }

    fn expect_count(operation: &Operation, expected: usize) -> Result<(), InterpretError> {
        let actual = operation.operands().len();
        if actual == expected {
            Ok(())
        } else {
            Err(InterpretError::at(
                operation,
                InterpretErrorKind::OperandCount { expected, actual },
            ))
        }
    }
}
