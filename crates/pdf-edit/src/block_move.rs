use std::collections::{BTreeMap, BTreeSet, HashMap};

use pdf_bytes::SourceSpan;
use pdf_paint::{Matrix, PaintAtomKind, PaintGraph, Point, TextShowPaint};

use crate::plan::{Capability, Effect, MovedRun, Plan, PlannedBody, PlannedWrite, SourceAnchor};
use crate::spike_move_text::SpikeError;

pub(crate) const PLACEMENT_TOLERANCE: f64 = 1e-6;

pub(crate) struct Chain {
    pub(crate) atoms: Vec<usize>,
}

fn chain_root(text: &TextShowPaint) -> Option<SourceSpan> {
    text.matrices.line.provenance.first().copied()
}

pub(crate) fn chains_of(graph: &PaintGraph) -> BTreeMap<(usize, usize, usize), Chain> {
    let mut chains: BTreeMap<(usize, usize, usize), Chain> = BTreeMap::new();
    for (ordinal, atom) in graph.atoms.iter().enumerate() {
        let PaintAtomKind::Text(text) = &atom.kind else {
            continue;
        };
        let Some(root) = chain_root(text) else {
            continue;
        };
        let key = (
            usize::try_from(root.source().get()).unwrap_or(usize::MAX),
            root.start(),
            root.end(),
        );
        chains
            .entry(key)
            .or_insert_with(|| Chain { atoms: Vec::new() })
            .atoms
            .push(ordinal);
    }
    chains
}

fn text_space_offset(text: &TextShowPaint, dx: f64, dy: f64) -> Option<(f64, f64)> {
    let to_user = text.state.ctm.value.multiply(text.matrices.text.value);
    let determinant = to_user.a.mul_add(to_user.d, -(to_user.b * to_user.c));
    if !determinant.is_finite() || determinant == 0.0 {
        return None;
    }
    let u = to_user.d.mul_add(dx, -(to_user.c * dy)) / determinant;
    let v = to_user.a.mul_add(dy, -(to_user.b * dx)) / determinant;
    if u.is_finite() && v.is_finite() {
        Some((u, v))
    } else {
        None
    }
}

pub(crate) fn linear(matrix: Matrix) -> Matrix {
    Matrix {
        e: 0.0,
        f: 0.0,
        ..matrix
    }
}

pub(crate) fn glyph_origin(matrix: Matrix, ctm: Matrix) -> Point {
    ctm.transform(Point {
        x: matrix.e,
        y: matrix.f,
    })
}

pub(crate) struct BlockRewrite {
    pub insertions: Vec<(usize, Vec<u8>)>,
    pub cancelled: bool,
}

fn insertions_for(
    graph: &PaintGraph,
    named: &BTreeSet<usize>,
    dx: f64,
    dy: f64,
    operator_offset: &dyn Fn(usize) -> Option<usize>,
) -> Result<BlockRewrite, SpikeError> {
    let mut insertions: Vec<(usize, Vec<u8>)> = Vec::new();
    let mut cancelled = false;
    for chain in chains_of(graph).into_values() {
        if !chain.atoms.iter().any(|ordinal| named.contains(ordinal)) {
            continue;
        }
        let first = chain
            .atoms
            .iter()
            .find(|ordinal| named.contains(ordinal))
            .copied()
            .ok_or(SpikeError::BlockNamesNoRun)?;
        let PaintAtomKind::Text(seed) = &graph.atoms[first].kind else {
            return Err(SpikeError::BlockNamesNoRun);
        };
        let (wanted_u, wanted_v) =
            text_space_offset(seed, dx, dy).ok_or(SpikeError::BlockOffsetNotRepresentable)?;

        let (mut carried_u, mut carried_v) = (0.0_f64, 0.0_f64);
        for ordinal in chain.atoms {
            let PaintAtomKind::Text(text) = &graph.atoms[ordinal].kind else {
                continue;
            };
            let (want_u, want_v) = if named.contains(&ordinal) {
                (wanted_u, wanted_v)
            } else {
                (0.0, 0.0)
            };
            let (step_u, step_v) = (want_u - carried_u, want_v - carried_v);
            if step_u == 0.0 && step_v == 0.0 {
                continue;
            }
            if text.matrices.text.value != text.matrices.line.value {
                return Err(SpikeError::BlockNotChainAligned);
            }
            let at = operator_offset(ordinal).ok_or(SpikeError::BlockRunNotInTargetStream)?;
            insertions.push((at, format!(" {step_u} {step_v} Td ").into_bytes()));
            if !named.contains(&ordinal) {
                cancelled = true;
            }
            carried_u = want_u;
            carried_v = want_v;
        }
    }
    insertions.sort_by_key(|(at, _)| *at);
    Ok(BlockRewrite {
        insertions,
        cancelled,
    })
}

pub(crate) fn declared_region(
    graph: &PaintGraph,
    named: &BTreeSet<usize>,
    dx: f64,
    dy: f64,
) -> Option<[f64; 4]> {
    let mut region: Option<[f64; 4]> = None;
    for ordinal in named {
        let PaintAtomKind::Text(text) = &graph.atoms[*ordinal].kind else {
            continue;
        };
        let Some(before) = text.outline_bounds() else {
            continue;
        };
        let box_of = [
            before[0].min(before[0] + dx) - PLACEMENT_TOLERANCE,
            before[1].min(before[1] + dy) - PLACEMENT_TOLERANCE,
            before[2].max(before[2] + dx) + PLACEMENT_TOLERANCE,
            before[3].max(before[3] + dy) + PLACEMENT_TOLERANCE,
        ];
        region = Some(match region {
            None => box_of,
            Some(had) => [
                had[0].min(box_of[0]),
                had[1].min(box_of[1]),
                had[2].max(box_of[2]),
                had[3].max(box_of[3]),
            ],
        });
    }
    region
}

pub(crate) fn prove_block_move(
    before: &PaintGraph,
    after: &PaintGraph,
    named: &BTreeSet<usize>,
    dx: f64,
    dy: f64,
) -> Result<(), SpikeError> {
    if before.atoms.len() != after.atoms.len() {
        return Err(SpikeError::MoveNotIsolated);
    }
    for (ordinal, (one, other)) in before.atoms.iter().zip(&after.atoms).enumerate() {
        let (PaintAtomKind::Text(one), PaintAtomKind::Text(other)) = (&one.kind, &other.kind)
        else {
            if pdf_paint::paint_signature(&one.kind) != pdf_paint::paint_signature(&other.kind) {
                return Err(SpikeError::MoveNotIsolated);
            }
            continue;
        };
        let (wanted_x, wanted_y) = if named.contains(&ordinal) {
            (dx, dy)
        } else {
            (0.0, 0.0)
        };
        prove_run_displaced(one, other, wanted_x, wanted_y)?;
    }
    Ok(())
}

pub(crate) fn prove_run_displaced(
    one: &TextShowPaint,
    other: &TextShowPaint,
    wanted_x: f64,
    wanted_y: f64,
) -> Result<(), SpikeError> {
    match run_change(one, other, wanted_x, wanted_y) {
        Some(_) => Err(SpikeError::MoveNotIsolated),
        None => Ok(()),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RunChange {
    GlyphCount,
    RenderingMode,
    Transform,
    FillColour,
    StrokeColour,
    Glyphs,
    Shape,
    Place,
}

pub(crate) fn run_change(
    one: &TextShowPaint,
    other: &TextShowPaint,
    wanted_x: f64,
    wanted_y: f64,
) -> Option<RunChange> {
    if one.glyphs.len() != other.glyphs.len() {
        return Some(RunChange::GlyphCount);
    }
    if one.state.text.rendering_mode.value != other.state.text.rendering_mode.value {
        return Some(RunChange::RenderingMode);
    }
    if one.state.ctm.value != other.state.ctm.value {
        return Some(RunChange::Transform);
    }
    if one.state.fill_color_space.value != other.state.fill_color_space.value
        || pdf_paint::colour_signature(&one.state.fill_color.value)
            != pdf_paint::colour_signature(&other.state.fill_color.value)
    {
        return Some(RunChange::FillColour);
    }
    if one.state.stroke_color_space.value != other.state.stroke_color_space.value
        || pdf_paint::colour_signature(&one.state.stroke_color.value)
            != pdf_paint::colour_signature(&other.state.stroke_color.value)
    {
        return Some(RunChange::StrokeColour);
    }
    for (a, b) in one.glyphs.iter().zip(&other.glyphs) {
        if a.code.value != b.code.value || a.code.cid != b.code.cid || a.glyph != b.glyph {
            return Some(RunChange::Glyphs);
        }
        if linear(a.matrix) != linear(b.matrix) {
            return Some(RunChange::Shape);
        }
        let from = glyph_origin(a.matrix, one.state.ctm.value);
        let to = glyph_origin(b.matrix, other.state.ctm.value);
        if (to.x - from.x - wanted_x).abs() > PLACEMENT_TOLERANCE
            || (to.y - from.y - wanted_y).abs() > PLACEMENT_TOLERANCE
        {
            return Some(RunChange::Place);
        }
    }
    None
}

pub(crate) fn named_runs(
    graph: &PaintGraph,
    runs: &[SourceAnchor],
) -> Result<BTreeSet<usize>, SpikeError> {
    if runs.is_empty() {
        return Err(SpikeError::BlockNamesNoRun);
    }
    let text = TextAtoms::of(graph);
    let mut named: BTreeSet<usize> = BTreeSet::new();
    for anchor in runs {
        named.insert(
            text.named_by(graph, anchor)
                .ok_or(SpikeError::AnchorNamesNothing)?,
        );
    }
    Ok(named)
}

pub(crate) struct TextAtoms {
    at: HashMap<(pdf_syntax::Reference, usize), Vec<usize>>,
}

impl TextAtoms {
    pub(crate) fn of(graph: &PaintGraph) -> Self {
        let mut at: HashMap<(pdf_syntax::Reference, usize), Vec<usize>> = HashMap::new();
        for (ordinal, atom) in graph.atoms.iter().enumerate() {
            if matches!(atom.kind, PaintAtomKind::Text(_)) {
                at.entry((atom.id.stream, atom.id.operator_span.start()))
                    .or_default()
                    .push(ordinal);
            }
        }
        Self { at }
    }

    pub(crate) fn named_by(&self, graph: &PaintGraph, anchor: &SourceAnchor) -> Option<usize> {
        self.at
            .get(&(anchor.stream, anchor.operator_offset))?
            .iter()
            .copied()
            .find(|ordinal| anchor.names(&graph.atoms[*ordinal].id))
    }
}

pub(crate) fn resolve(
    graph: &PaintGraph,
    runs: &[SourceAnchor],
) -> Result<(BTreeSet<usize>, pdf_syntax::Reference), SpikeError> {
    let named = named_runs(graph, runs)?;
    let mut stream = None;
    for ordinal in &named {
        let atom = &graph.atoms[*ordinal];
        if !atom.id.pattern_path.is_empty() {
            return Err(SpikeError::RunNotDirectlyOnPage);
        }
        if !atom.id.invocation_path.is_empty() {
            return Err(SpikeError::BlockRunInsideForm);
        }
        match stream {
            None => stream = Some(atom.id.stream),
            Some(had) if had == atom.id.stream => {}
            Some(_) => return Err(SpikeError::BlockSpansSeveralStreams),
        }
    }
    Ok((named, stream.ok_or(SpikeError::BlockNamesNoRun)?))
}

pub(crate) struct TextInsertions {
    pub named: BTreeSet<usize>,
    pub showing: crate::plan::Showing,
    pub stream_index: usize,
    pub stream_reference: pdf_syntax::Reference,
    pub rewrite: BlockRewrite,
}

pub(crate) fn text_insertions(
    program: &pdf_content::PageProgram,
    operations: &[Vec<pdf_content::Operation>],
    graph: &PaintGraph,
    runs: &[SourceAnchor],
    (dx, dy): (f64, f64),
) -> Result<TextInsertions, SpikeError> {
    let (named, stream_reference) = resolve(graph, runs)?;
    if program
        .streams
        .iter()
        .filter(|stream| stream.reference == stream_reference)
        .count()
        != 1
    {
        return Err(SpikeError::SharedPageContentStream);
    }
    let stream_index = program
        .streams
        .iter()
        .position(|stream| stream.reference == stream_reference)
        .ok_or(SpikeError::NoTextRun)?;

    let showing = block_showing_of(graph, &named, dx, dy)?;

    let offset_of = |ordinal: usize| -> Option<usize> {
        let atom = graph.atoms.get(ordinal)?;
        if atom.id.stream != stream_reference || !atom.id.invocation_path.is_empty() {
            return None;
        }
        operations[stream_index]
            .iter()
            .find(|operation| operation.operator_span() == atom.id.operator_span)
            .map(|operation| operation.span().start())
    };
    let rewrite = insertions_for(graph, &named, dx, dy, &offset_of)?;
    Ok(TextInsertions {
        named,
        showing,
        stream_index,
        stream_reference,
        rewrite,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "one edit's inputs, the font context among them, threaded as its neighbours are"
)]
pub(crate) fn plan_block_move(
    source: &pdf_bytes::ByteStore,
    program: &pdf_content::PageProgram,
    operations: &[Vec<pdf_content::Operation>],
    graph: &PaintGraph,
    page_index: usize,
    runs: &[SourceAnchor],
    dx: f64,
    dy: f64,
    fonts: crate::Fonts<'_>,
) -> Result<Plan, SpikeError> {
    if let Some(invocation) = crate::form_edit::invocation_of(
        named_runs(graph, runs)?.iter().map(|at| &graph.atoms[*at]),
    )? {
        return move_block_in_form(
            source,
            program,
            operations,
            graph,
            page_index,
            runs,
            invocation,
            (dx, dy),
            fonts,
        );
    }
    let TextInsertions {
        named,
        showing,
        stream_index,
        stream_reference,
        rewrite: BlockRewrite {
            insertions,
            cancelled,
        },
    } = text_insertions(program, operations, graph, runs, (dx, dy))?;
    if insertions.is_empty() {
        return Err(SpikeError::BlockNamesNoRun);
    }

    let decoded = program.streams[stream_index].bytes.as_bytes();
    let mut edited = Vec::with_capacity(decoded.len() + 32 * insertions.len());
    let mut cursor = 0_usize;
    for (at, bytes) in &insertions {
        edited.extend_from_slice(&decoded[cursor..*at]);
        edited.extend_from_slice(bytes);
        cursor = *at;
    }
    edited.extend_from_slice(&decoded[cursor..]);

    let rewritten = crate::spike_move_text::interpret_with_insertions_of(
        program,
        stream_index,
        &insertions,
        fonts,
    )?;
    prove_block_move(graph, &rewritten, &named, dx, dy)?;

    let moved = named
        .iter()
        .map(|ordinal| {
            let atom = &graph.atoms[*ordinal];
            let original_matrix = match &atom.kind {
                PaintAtomKind::Text(text) => text.matrices.text.value,
                _ => Matrix::IDENTITY,
            };
            MovedRun {
                anchor: SourceAnchor::of(&atom.id),
                atom_ordinal: *ordinal,
                original_matrix,
            }
        })
        .collect();

    Ok(Plan::new(
        if cancelled {
            Capability::Normalized
        } else {
            Capability::Exact
        },
        vec![PlannedWrite {
            reference: stream_reference,
            body: PlannedBody::ReplacedStream { decoded: edited },
        }],
        Effect {
            page_index,
            moved,
            target_stream: stream_reference,
            declared_region: declared_region(graph, &named, dx, dy),
        },
    )
    .with_showing(showing))
}

#[expect(
    clippy::too_many_arguments,
    reason = "one edit's inputs, threaded as its neighbours are"
)]
fn move_block_in_form(
    source: &pdf_bytes::ByteStore,
    program: &pdf_content::PageProgram,
    operations: &[Vec<pdf_content::Operation>],
    graph: &PaintGraph,
    page_index: usize,
    runs: &[SourceAnchor],
    invocation: pdf_paint::FormInvocation,
    (dx, dy): (f64, f64),
    fonts: crate::Fonts<'_>,
) -> Result<Plan, SpikeError> {
    let named = named_runs(graph, runs)?;
    let showing = block_showing_of(graph, &named, dx, dy)?;
    let scope = crate::form_edit::scope_of(program, operations, invocation)?;

    let offset_of = |ordinal: usize| -> Option<usize> {
        let atom = graph.atoms.get(ordinal)?;
        if atom.id.invocation_path.first().copied() != Some(invocation) {
            return None;
        }
        Some(scope.span_of(atom.id.operator_span).ok()?.start())
    };
    let BlockRewrite {
        insertions,
        cancelled,
    } = insertions_for(graph, &named, dx, dy, &offset_of)?;
    if insertions.is_empty() {
        return Err(SpikeError::BlockNamesNoRun);
    }

    let decoded = scope.bytes();
    let mut edited = Vec::with_capacity(decoded.len() + 32 * insertions.len());
    let mut cursor = 0_usize;
    for (at, bytes) in &insertions {
        if *at < cursor || *at > decoded.len() {
            return Err(SpikeError::BlockRunNotInTargetStream);
        }
        edited.extend_from_slice(&decoded[cursor..*at]);
        edited.extend_from_slice(bytes);
        cursor = *at;
    }
    edited.extend_from_slice(&decoded[cursor..]);

    let spans: Vec<pdf_bytes::SourceSpan> = named
        .iter()
        .map(|ordinal| graph.atoms[*ordinal].id.operator_span)
        .collect();
    let proved = crate::form_edit::prove_isolated(&scope, &edited, &spans, fonts)?;
    let first = *named.iter().next().ok_or(SpikeError::BlockNamesNoRun)?;
    let inside = *proved.windows.first().ok_or(SpikeError::MoveNotProvable)?;
    let (u, v) = invocation_carry(
        &graph.atoms[first].kind,
        proved.before.atoms.get(inside).map(|atom| &atom.kind),
        (dx, dy),
    )
    .ok_or(SpikeError::MoveNotProvable)?;
    let moved_there: BTreeSet<usize> = proved.windows.iter().copied().collect();
    prove_block_move(&proved.before, &proved.after, &moved_there, u, v)?;

    let written = crate::form_edit::writes_for(source, program, &scope, edited)?;
    let moved = named
        .iter()
        .map(|ordinal| {
            let atom = &graph.atoms[*ordinal];
            let original_matrix = match &atom.kind {
                PaintAtomKind::Text(text) => text.matrices.text.value,
                _ => Matrix::IDENTITY,
            };
            MovedRun {
                anchor: SourceAnchor::of(&atom.id),
                atom_ordinal: *ordinal,
                original_matrix,
            }
        })
        .collect();
    Ok(Plan::new(
        if cancelled {
            Capability::Normalized
        } else {
            Capability::Exact
        },
        written.writes,
        Effect {
            page_index,
            moved,
            target_stream: written.target_stream,
            declared_region: declared_region(graph, &named, dx, dy),
        },
    )
    .with_showing(showing))
}

fn invocation_carry(
    on_page: &PaintAtomKind,
    in_form: Option<&PaintAtomKind>,
    (dx, dy): (f64, f64),
) -> Option<(f64, f64)> {
    let (PaintAtomKind::Text(page), PaintAtomKind::Text(form)) = (on_page, in_form?) else {
        return None;
    };
    let carry = page
        .state
        .ctm
        .value
        .multiply(form.state.ctm.value.inverse()?);
    let back = carry.inverse()?;
    let moved = back.transform(Point { x: dx, y: dy });
    let origin = back.transform(Point { x: 0.0, y: 0.0 });
    Some((moved.x - origin.x, moved.y - origin.y))
}

fn block_showing_of(
    graph: &PaintGraph,
    named: &BTreeSet<usize>,
    dx: f64,
    dy: f64,
) -> Result<crate::plan::Showing, SpikeError> {
    let mut showing = crate::plan::Showing::Whole;
    let mut seen = false;
    for ordinal in named {
        let PaintAtomKind::Text(text) = &graph.atoms[*ordinal].kind else {
            continue;
        };
        let (u, v) =
            text_space_offset(text, dx, dy).ok_or(SpikeError::BlockOffsetNotRepresentable)?;
        let run = crate::spike_move_text::clip_allows_move_of(text, u, v)?;
        showing = if seen {
            block_showing(showing, run)
        } else {
            run
        };
        seen = true;
    }
    Ok(showing)
}

fn block_showing(so_far: crate::plan::Showing, run: crate::plan::Showing) -> crate::plan::Showing {
    use crate::plan::Showing;
    match (so_far, run) {
        (Showing::OutOfSight, Showing::OutOfSight) => Showing::OutOfSight,
        (Showing::Whole, Showing::Whole) => Showing::Whole,
        _ => Showing::PartlyHidden,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Arc;

    use pdf_bytes::{ByteStore, SourceId};
    use pdf_content::{
        ContentLimits, PageContentLimits, load_page_program_strict, parse_operation_sequence_strict,
    };
    use pdf_paint::{
        PaintAtomKind, PaintGraph, PaintLimits, PaintStream, Point,
        interpret_stream_sequence_with_resources,
    };

    use super::{PLACEMENT_TOLERANCE, glyph_origin, prove_block_move};
    use crate::plan::{Capability, Command, Plan, SourceAnchor};
    use crate::spike_move_text::{PlannerPage, SpikeError, plan_command_in};

    pub(crate) const SQUARE_CFF: &str = "0100040100010101055465737400010101131d00000030111d000000540f1d0000\
        0059100001010106616c70686100000003010102101e0e8b8b15f8888b8bf888fc888b050e8b8b15f8888b8bf888fc\
        888b050e0000220187000141";

    pub(crate) fn hex(text: &str) -> Vec<u8> {
        text.bytes()
            .filter(u8::is_ascii_hexdigit)
            .collect::<Vec<u8>>()
            .chunks_exact(2)
            .map(|pair| {
                u8::from_str_radix(std::str::from_utf8(pair).expect("hex digits"), 16)
                    .expect("hex byte")
            })
            .collect()
    }

    fn object(bytes: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &[u8]) {
        offsets.push(bytes.len());
        bytes.extend_from_slice(body);
    }

    pub(crate) fn page_with(content: &[u8]) -> ByteStore {
        let program = hex(SQUARE_CFF);
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        object(
            &mut bytes,
            &mut offsets,
            b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
        );
        object(
            &mut bytes,
            &mut offsets,
            b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 200] /Kids [3 0 R] /Count 1 >>\nendobj\n",
        );
        object(
            &mut bytes,
            &mut offsets,
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        );
        bytes.extend_from_slice(content);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        object(
            &mut bytes,
            &mut offsets,
            b"5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Test /FirstChar 65 /LastChar 67 /Widths [600 600 600] /FontDescriptor 6 0 R >>\nendobj\n",
        );
        object(
            &mut bytes,
            &mut offsets,
            b"6 0 obj\n<< /Type /FontDescriptor /FontName /Test /Flags 4 /FontFile3 7 0 R >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            format!(
                "7 0 obj\n<< /Subtype /Type1C /Length {} >>\nstream\n",
                program.len()
            )
            .as_bytes(),
        );
        bytes.extend_from_slice(&program);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        let size = offsets.len() + 1;
        let xref = bytes.len();
        bytes.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n").as_bytes(),
        );
        bytes.extend_from_slice(xref.to_string().as_bytes());
        bytes.extend_from_slice(b"\n%%EOF\n");
        ByteStore::new(SourceId::new(511), Arc::<[u8]>::from(bytes))
    }

    #[derive(Debug)]
    pub(crate) struct Read {
        pub(crate) program: pdf_content::PageProgram,
        pub(crate) operations: Vec<Vec<pdf_content::Operation>>,
        pub(crate) graph: PaintGraph,
    }

    pub(crate) fn read(source: &ByteStore) -> Read {
        let program = load_page_program_strict(source, 0, PageContentLimits::default())
            .expect("the fixture is a page");
        let sources: Vec<&ByteStore> = program.streams.iter().map(|s| &s.bytes).collect();
        let operations = parse_operation_sequence_strict(&sources, ContentLimits::default())
            .expect("the fixture parses");
        let streams: Vec<_> = program
            .streams
            .iter()
            .zip(&sources)
            .zip(&operations)
            .map(|((stream, source), operations)| PaintStream {
                source,
                reference: stream.reference,
                operations,
            })
            .collect();
        let graph = interpret_stream_sequence_with_resources(
            &streams,
            program.page,
            &[],
            &program.resources,
            PaintLimits::default(),
        )
        .expect("the fixture interprets");
        Read {
            program,
            operations,
            graph,
        }
    }

    pub(crate) fn anchors(graph: &PaintGraph) -> Vec<SourceAnchor> {
        graph
            .atoms
            .iter()
            .filter(|atom| matches!(atom.kind, PaintAtomKind::Text(_)))
            .map(|atom| SourceAnchor::of(&atom.id))
            .collect()
    }

    fn origins(graph: &PaintGraph) -> Vec<Point> {
        let mut found = Vec::new();
        for atom in &graph.atoms {
            let PaintAtomKind::Text(text) = &atom.kind else {
                continue;
            };
            for glyph in &text.glyphs {
                found.push(glyph_origin(glyph.matrix, text.state.ctm.value));
            }
        }
        found
    }

    fn plan_block(
        source: &ByteStore,
        runs: &[usize],
        dx: f64,
        dy: f64,
    ) -> Result<(Plan, Read), SpikeError> {
        let read = read(source);
        let all = anchors(&read.graph);
        let named: Vec<SourceAnchor> = runs
            .iter()
            .map(|index| all[*index].clone())
            .collect::<Vec<_>>();
        let plan = plan_command_in(
            source,
            PlannerPage {
                program: &read.program,
                operations: &read.operations,
                graph: &read.graph,
                fonts: None,
                restrictions: crate::Restrictions::Respect,
                credential: b"",
            },
            &Command::MoveTextBlock {
                page_index: 0,
                runs: named,
                dx,
                dy,
            },
        )?;
        Ok((plan, read))
    }

    pub(crate) fn written(plan: &Plan) -> Vec<u8> {
        match &plan.writes()[0].body {
            crate::plan::PlannedBody::ReplacedStream { decoded } => decoded.clone(),
            other => panic!("a block move replaces a stream, not {other:?}"),
        }
    }

    pub(crate) fn after(plan: &Plan, source: &ByteStore) -> PaintGraph {
        let committed = plan.commit(source, b"").expect("the plan commits");
        read(&committed).graph
    }

    fn td_count(bytes: &[u8]) -> usize {
        bytes.windows(3).filter(|window| *window == b" Td").count()
    }

    #[test]
    fn one_insertion_moves_a_whole_chain() {
        let source = page_with(b"BT /F1 12 Tf 10 20 Td (A) Tj (B) Tj (C) Tj ET");
        let (plan, read) = plan_block(&source, &[0, 1, 2], 7.0, -3.0).expect("a chain is movable");

        let before = origins(&read.graph);
        assert_eq!(before.len(), 3, "three shows, three glyphs");
        assert!(
            before[0].x < before[1].x && before[1].x < before[2].x,
            "the instrument: the three glyphs start apart, at {before:?}"
        );

        assert_eq!(
            td_count(&written(&plan)),
            2,
            "the file's own `Td` plus exactly one inserted: {}",
            String::from_utf8_lossy(&written(&plan))
        );
        assert_eq!(
            plan.capability(),
            Capability::Exact,
            "nothing the command did not name was restated"
        );
        assert_eq!(plan.effect().moved.len(), 3);

        let moved = origins(&after(&plan, &source));
        for (was, now) in before.iter().zip(&moved) {
            assert!(
                (now.x - was.x - 7.0).abs() < PLACEMENT_TOLERANCE
                    && (now.y - was.y + 3.0).abs() < PLACEMENT_TOLERANCE,
                "every glyph moves by the same offset: {was:?} -> {now:?}"
            );
        }
    }

    #[test]
    fn lines_under_different_matrices_move_the_same_distance() {
        let source = page_with(b"BT /F1 12 Tf 1 0 0 1 10 50 Tm (A) Tj 2 0 0 2 10 20 Tm (B) Tj ET");
        let (plan, read) = plan_block(&source, &[0, 1], 8.0, -4.0).expect("two lines are movable");

        let before = origins(&read.graph);
        assert_eq!(before.len(), 2);

        let stream = written(&plan);
        let text = String::from_utf8_lossy(&stream);
        assert!(
            text.contains(" 8 -4 Td "),
            "the unscaled line takes the offset as given: {text}"
        );
        assert!(
            text.contains(" 4 -2 Td "),
            "the doubled line takes half of it, so it travels the same way: {text}"
        );

        let moved = origins(&after(&plan, &source));
        for (was, now) in before.iter().zip(&moved) {
            assert!(
                (now.x - was.x - 8.0).abs() < PLACEMENT_TOLERANCE
                    && (now.y - was.y + 4.0).abs() < PLACEMENT_TOLERANCE,
                "both lines travel 8 by -4 in the page's own space: {was:?} -> {now:?}"
            );
        }
    }

    #[test]
    fn a_run_the_block_does_not_name_is_put_back() {
        let source = page_with(b"BT /F1 12 Tf 10 20 Td (A) Tj (B) Tj 0 -20 Td (C) Tj ET");
        let (plan, read) =
            plan_block(&source, &[0, 1], 5.0, 0.0).expect("a partial chain is movable");

        let before = origins(&read.graph);
        assert_eq!(before.len(), 3);
        assert_eq!(
            td_count(&written(&plan)),
            4,
            "the file's two `Td` plus one to shift and one to put back: {}",
            String::from_utf8_lossy(&written(&plan))
        );
        assert_eq!(
            plan.capability(),
            Capability::Normalized,
            "a run the command did not name has had its placement restated"
        );

        let moved = origins(&after(&plan, &source));
        assert!((moved[0].x - before[0].x - 5.0).abs() < PLACEMENT_TOLERANCE);
        assert!((moved[1].x - before[1].x - 5.0).abs() < PLACEMENT_TOLERANCE);
        assert!(
            (moved[2].x - before[2].x).abs() < PLACEMENT_TOLERANCE
                && (moved[2].y - before[2].y).abs() < PLACEMENT_TOLERANCE,
            "the run the block does not name stays exactly where it was: {:?} -> {:?}",
            before[2],
            moved[2]
        );
    }

    #[test]
    fn a_block_that_starts_mid_line_is_refused() {
        let source = page_with(b"BT /F1 12 Tf 10 20 Td (A) Tj (B) Tj (C) Tj ET");
        let refused = plan_block(&source, &[1, 2], 5.0, 0.0).expect_err("mid-line is refused");
        assert!(
            matches!(refused, SpikeError::BlockNotChainAligned),
            "refused for the right reason, not by accident: {refused:?}"
        );
    }

    #[test]
    fn a_block_across_two_text_objects_moves_as_one_plan() {
        let source = page_with(b"BT /F1 12 Tf 10 60 Td (A) Tj ET BT /F1 12 Tf 10 40 Td (B) Tj ET");
        let (plan, read) = plan_block(&source, &[0, 1], 0.0, 6.0).expect("two objects are movable");
        let before = origins(&read.graph);
        assert_eq!(
            td_count(&written(&plan)),
            4,
            "one `Td` of the file and one inserted, in each object"
        );
        assert_eq!(plan.capability(), Capability::Exact);
        assert_eq!(plan.effect().moved.len(), 2);
        let moved = origins(&after(&plan, &source));
        for (was, now) in before.iter().zip(&moved) {
            assert!((now.y - was.y - 6.0).abs() < PLACEMENT_TOLERANCE);
            assert!((now.x - was.x).abs() < PLACEMENT_TOLERANCE);
        }
    }

    #[test]
    fn the_declared_region_contains_the_ink_at_both_ends() {
        let source = page_with(b"BT /F1 12 Tf 10 20 Td (A) Tj (B) Tj ET");
        let (plan, read) = plan_block(&source, &[0, 1], 9.0, 11.0).expect("movable");
        let region = plan
            .effect()
            .declared_region
            .expect("an embedded font gives the run an outline");

        let extent = |graph: &PaintGraph| {
            let mut found: Option<[f64; 4]> = None;
            for atom in &graph.atoms {
                let PaintAtomKind::Text(text) = &atom.kind else {
                    continue;
                };
                let Some(box_of) = text.outline_bounds() else {
                    continue;
                };
                found = Some(match found {
                    None => box_of,
                    Some(had) => [
                        had[0].min(box_of[0]),
                        had[1].min(box_of[1]),
                        had[2].max(box_of[2]),
                        had[3].max(box_of[3]),
                    ],
                });
            }
            found.expect("the page paints text")
        };
        for (name, box_of) in [
            ("before", extent(&read.graph)),
            ("after", extent(&after(&plan, &source))),
        ] {
            assert!(
                region[0] <= box_of[0]
                    && region[1] <= box_of[1]
                    && region[2] >= box_of[2]
                    && region[3] >= box_of[3],
                "the declared region {region:?} must contain the ink {name} the move, {box_of:?}"
            );
        }
    }

    #[test]
    fn a_block_naming_no_run_is_refused() {
        let source = page_with(b"BT /F1 12 Tf 10 20 Td (A) Tj ET");
        let read = read(&source);
        let refused = plan_command_in(
            &source,
            PlannerPage {
                program: &read.program,
                operations: &read.operations,
                graph: &read.graph,
                fonts: None,
                restrictions: crate::Restrictions::Respect,
                credential: b"",
            },
            &Command::MoveTextBlock {
                page_index: 0,
                runs: Vec::new(),
                dx: 1.0,
                dy: 1.0,
            },
        )
        .expect_err("an empty block is refused");
        assert!(matches!(refused, SpikeError::BlockNamesNoRun));
    }

    #[test]
    fn the_proof_accepts_the_right_move_and_refuses_two_wrong_ones() {
        let source = page_with(b"BT /F1 12 Tf 10 20 Td (A) Tj 0 -20 Td (B) Tj ET");
        let before = read(&source).graph;
        let named: std::collections::BTreeSet<usize> = [0].into_iter().collect();

        let right = page_with(b"BT /F1 12 Tf 10 20 Td 5 0 Td (A) Tj 0 -20 Td -5 0 Td (B) Tj ET");
        prove_block_move(&before, &read(&right).graph, &named, 5.0, 0.0)
            .expect("the move it was built to be");

        let dragged = page_with(b"BT /F1 12 Tf 10 20 Td 5 0 Td (A) Tj 0 -20 Td (B) Tj ET");
        assert!(
            matches!(
                prove_block_move(&before, &read(&dragged).graph, &named, 5.0, 0.0),
                Err(SpikeError::MoveNotIsolated)
            ),
            "a run the command did not name must not be allowed to move"
        );

        let short = page_with(b"BT /F1 12 Tf 10 20 Td 4 0 Td (A) Tj 0 -20 Td -4 0 Td (B) Tj ET");
        assert!(
            matches!(
                prove_block_move(&before, &read(&short).graph, &named, 5.0, 0.0),
                Err(SpikeError::MoveNotIsolated)
            ),
            "moving by the wrong distance is as wrong as moving the wrong run"
        );
    }

    #[test]
    fn a_block_move_is_one_undo() {
        use crate::History;

        let source = page_with(b"BT /F1 12 Tf 10 20 Td (A) Tj (B) Tj (C) Tj ET");
        let before = origins(&read(&source).graph);
        let (plan, _) = plan_block(&source, &[0, 1, 2], 6.0, 0.0).expect("movable");

        let mut history = History::new(source.clone(), b"");
        history.apply(plan).expect("it commits");
        assert_eq!(history.undo_depth(), 1, "one gesture, one step");
        let moved = origins(&read(history.source()).graph);
        for (was, now) in before.iter().zip(&moved) {
            assert!((now.x - was.x - 6.0).abs() < PLACEMENT_TOLERANCE);
        }

        assert!(history.undo().expect("it undoes"), "there was a step");
        let back = origins(&read(history.source()).graph);
        for (was, now) in before.iter().zip(&back) {
            assert!(
                (now.x - was.x).abs() < PLACEMENT_TOLERANCE
                    && (now.y - was.y).abs() < PLACEMENT_TOLERANCE,
                "one undo puts the whole block back: {was:?} -> {now:?}"
            );
        }
    }
}
