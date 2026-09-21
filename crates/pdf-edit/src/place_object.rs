use pdf_paint::{Matrix, PaintAtomKind, PaintGraph, Point};

use crate::block_move::PLACEMENT_TOLERANCE;
use crate::plan::{
    Capability, Effect, FixedPoint, MovedRun, Plan, PlannedBody, PlannedWrite, Showing,
    SourceAnchor,
};
use crate::spike_move_text::SpikeError;

#[expect(
    clippy::too_many_arguments,
    reason = "one edit's inputs, the font context among them, threaded as its neighbours are"
)]
pub(crate) fn plan_place_object(
    source: &pdf_bytes::ByteStore,
    program: &pdf_content::PageProgram,
    operations: &[Vec<pdf_content::Operation>],
    graph: &PaintGraph,
    page_index: usize,
    anchor: &SourceAnchor,
    transform: Matrix,
    about: FixedPoint,
    fonts: crate::Fonts<'_>,
) -> Result<Plan, SpikeError> {
    let wanted_here = about.applied_to(transform);
    let inside = graph
        .atoms
        .get(resolve_anywhere(graph, anchor)?)
        .and_then(|atom| atom.id.invocation_path.first().copied());
    if let Some(invocation) = inside {
        return place_in_form(
            source,
            program,
            operations,
            graph,
            page_index,
            anchor,
            invocation,
            wanted_here,
            fonts,
        );
    }
    let Placement {
        ordinal,
        stream_index,
        insertions,
        ctm,
        wanted,
        travelling_from,
        cropping,
    } = placement_insertions(program, operations, graph, anchor, wanted_here)?;
    let atom = &graph.atoms[ordinal];

    let rewritten = crate::spike_move_text::interpret_with_insertions_of(
        program,
        stream_index,
        &insertions,
        fonts,
    )?;
    prove_placement(graph, &rewritten, ordinal, wanted, ctm, travelling_from)?;

    let decoded = program.streams[stream_index].bytes.as_bytes();
    let mut edited = Vec::with_capacity(decoded.len() + 64);
    let mut cursor = 0_usize;
    for (at, bytes) in &insertions {
        edited.extend_from_slice(&decoded[cursor..*at]);
        edited.extend_from_slice(bytes);
        cursor = *at;
    }
    edited.extend_from_slice(&decoded[cursor..]);

    let stream_reference = program.streams[stream_index].reference;
    Ok(Plan::new(
        Capability::Exact,
        vec![PlannedWrite {
            reference: stream_reference,
            body: PlannedBody::ReplacedStream { decoded: edited },
        }],
        Effect {
            page_index,
            moved: vec![MovedRun {
                anchor: SourceAnchor::of(&atom.id),
                atom_ordinal: ordinal,
                original_matrix: ctm,
            }],
            target_stream: stream_reference,
            declared_region: declared_region(&atom.kind, rewritten.atoms.get(ordinal)),
        },
    )
    .with_showing(cropping))
}

#[expect(
    clippy::too_many_arguments,
    reason = "one edit's inputs, threaded as its neighbours are"
)]
fn place_in_form(
    source: &pdf_bytes::ByteStore,
    program: &pdf_content::PageProgram,
    operations: &[Vec<pdf_content::Operation>],
    graph: &PaintGraph,
    page_index: usize,
    anchor: &SourceAnchor,
    invocation: pdf_paint::FormInvocation,
    wanted: Matrix,
    fonts: crate::Fonts<'_>,
) -> Result<Plan, SpikeError> {
    let ordinal = resolve_anywhere(graph, anchor)?;
    let atom = &graph.atoms[ordinal];
    if graph.repairs.iter().any(|repair| {
        matches!(repair.kind, pdf_paint::RepairKind::OperatorInsideTextObject)
            && repair.operator_span == atom.id.operator_span
    }) {
        return Err(SpikeError::ObjectInsideTextObject);
    }
    if wanted.inverse().is_none() {
        return Err(SpikeError::PlacementNotInvertible);
    }
    let ctm = placement_of(&atom.kind)?;
    let delta = ctm
        .inverse()
        .ok_or(SpikeError::ObjectCtmSingular)?
        .multiply(wanted)
        .multiply(ctm);
    if !finite(delta) {
        return Err(SpikeError::PlacementNotInvertible);
    }
    let clips = &state_of(&atom.kind).clip_paths;
    let cropping = clip_admits(&atom.kind, wanted, clips.len())?;

    let scope = crate::form_edit::scope_of(program, operations, invocation)?;
    let span = scope.span_of(atom.id.operator_span)?;
    let begin = construction_start(atom).unwrap_or_else(|| span.start());
    let decoded = scope.bytes();
    if span.end() > decoded.len() || begin > span.start() {
        return Err(SpikeError::ObjectNotInPageContent);
    }
    let mut edited = Vec::with_capacity(decoded.len() + 64);
    edited.extend_from_slice(&decoded[..begin]);
    edited.extend_from_slice(
        format!(
            "q {} {} {} {} {} {} cm ",
            delta.a, delta.b, delta.c, delta.d, delta.e, delta.f
        )
        .as_bytes(),
    );
    edited.extend_from_slice(&decoded[begin..span.end()]);
    edited.extend_from_slice(b" Q");
    edited.extend_from_slice(&decoded[span.end()..]);

    let proved =
        crate::form_edit::prove_isolated(&scope, &edited, &[atom.id.operator_span], fonts)?;
    let at = *proved.windows.first().ok_or(SpikeError::MoveNotProvable)?;
    let was = proved
        .before
        .atoms
        .get(at)
        .ok_or(SpikeError::MoveNotProvable)?;
    prove_placed(
        &was.kind,
        proved.after.atoms.get(at),
        (delta, placement_of(&was.kind)?, usize::MAX),
    )?;

    let written = crate::form_edit::writes_for(source, program, &scope, edited)?;
    Ok(Plan::new(
        Capability::Exact,
        written.writes,
        Effect {
            page_index,
            moved: vec![MovedRun {
                anchor: SourceAnchor::of(&atom.id),
                atom_ordinal: ordinal,
                original_matrix: ctm,
            }],
            target_stream: written.target_stream,
            declared_region: declared_region_of(&atom.kind, wanted),
        },
    )
    .with_showing(cropping))
}

pub(crate) struct Placement {
    pub ordinal: usize,
    pub stream_index: usize,
    pub insertions: Vec<(usize, Vec<u8>)>,
    pub ctm: Matrix,
    pub wanted: Matrix,
    pub travelling_from: usize,
    pub cropping: Showing,
}

pub(crate) fn placement_insertions(
    program: &pdf_content::PageProgram,
    operations: &[Vec<pdf_content::Operation>],
    graph: &PaintGraph,
    anchor: &SourceAnchor,
    wanted: Matrix,
) -> Result<Placement, SpikeError> {
    let ordinal = resolve(graph, anchor)?;
    let atom = &graph.atoms[ordinal];
    if graph.repairs.iter().any(|repair| {
        matches!(repair.kind, pdf_paint::RepairKind::OperatorInsideTextObject)
            && repair.operator_span == atom.id.operator_span
    }) {
        return Err(SpikeError::ObjectInsideTextObject);
    }
    let ctm = placement_of(&atom.kind)?;

    if wanted.inverse().is_none() {
        return Err(SpikeError::PlacementNotInvertible);
    }
    let (stream_index, operation_index) = written_at(program, operations, atom)?;
    let span = operations[stream_index][operation_index].span();
    let begin = construction_start(atom).unwrap_or_else(|| span.start());
    let clips = &state_of(&atom.kind).clip_paths;
    let scope = graph.object_scopes.iter().find(|scope| {
        scope.atoms == (ordinal..ordinal + 1)
            && scope.stream == atom.id.stream
            && scope.open.source() == span.source()
            && scope.open.end() <= begin
            && scope.close.start() >= span.end()
            && scope.inherited_clips < clips.len()
            && clip_admits(&atom.kind, wanted, scope.inherited_clips).is_ok()
    });
    let (start, end, entry, travelling_from, cropping) = if let Some(scope) = scope {
        (
            scope.open.start(),
            scope.close.end(),
            scope.ctm.value,
            scope.inherited_clips,
            clip_admits(&atom.kind, wanted, scope.inherited_clips)?,
        )
    } else {
        let cropping = clip_admits(&atom.kind, wanted, clips.len())?;
        (begin, span.end(), ctm, clips.len(), cropping)
    };
    let delta = entry
        .inverse()
        .ok_or(SpikeError::ObjectCtmSingular)?
        .multiply(wanted)
        .multiply(entry);
    if !finite(delta) {
        return Err(SpikeError::PlacementNotInvertible);
    }
    let insertions = vec![
        (
            start,
            format!(
                "q {} {} {} {} {} {} cm ",
                delta.a, delta.b, delta.c, delta.d, delta.e, delta.f
            )
            .into_bytes(),
        ),
        (end, b" Q".to_vec()),
    ];
    Ok(Placement {
        ordinal,
        stream_index,
        insertions,
        ctm,
        wanted,
        travelling_from,
        cropping,
    })
}

pub(crate) fn resolve(graph: &PaintGraph, anchor: &SourceAnchor) -> Result<usize, SpikeError> {
    let mut found = graph
        .atoms
        .iter()
        .enumerate()
        .filter(|(_, atom)| anchor.names(&atom.id));
    let (ordinal, atom) = found.next().ok_or(SpikeError::AnchorNamesNothing)?;
    if found.next().is_some() {
        return Err(SpikeError::ObjectNamedMoreThanOnce);
    }
    match &atom.kind {
        PaintAtomKind::Text(_) => return Err(SpikeError::ObjectIsText),
        PaintAtomKind::Path(_) if construction_start(atom).is_none() => {
            return Err(SpikeError::ObjectIsDrawing);
        }
        PaintAtomKind::Path(_)
        | PaintAtomKind::Image(_)
        | PaintAtomKind::Shading(_)
        | PaintAtomKind::TransparencyGroup(_) => {}
    }
    if !atom.id.pattern_path.is_empty() {
        return Err(SpikeError::RunNotDirectlyOnPage);
    }
    if atom.id.invocation_path.len() > 1 {
        return Err(SpikeError::RunNestedTooDeep);
    }
    if !atom.id.invocation_path.is_empty() {
        return Err(SpikeError::ObjectInsideForm);
    }
    Ok(ordinal)
}

fn resolve_anywhere(graph: &PaintGraph, anchor: &SourceAnchor) -> Result<usize, SpikeError> {
    match resolve(graph, anchor) {
        Err(SpikeError::ObjectInsideForm) => {
            let (ordinal, _) = graph
                .atoms
                .iter()
                .enumerate()
                .find(|(_, atom)| anchor.names(&atom.id))
                .ok_or(SpikeError::AnchorNamesNothing)?;
            Ok(ordinal)
        }
        other => other,
    }
}

pub(crate) fn construction_start(atom: &pdf_paint::PaintAtom) -> Option<usize> {
    let PaintAtomKind::Path(paint) = &atom.kind else {
        return None;
    };
    let source = atom.id.operator_span.source();
    let mut first: Option<usize> = None;
    for segment in &paint.path.segments {
        let span = match segment {
            pdf_paint::PathSegment::MoveTo { provenance, .. }
            | pdf_paint::PathSegment::LineTo { provenance, .. }
            | pdf_paint::PathSegment::CubicTo { provenance, .. }
            | pdf_paint::PathSegment::ClosePath { provenance }
            | pdf_paint::PathSegment::Rectangle { provenance, .. } => provenance,
        };
        if span.source() != source {
            return None;
        }
        first = Some(first.map_or(span.start(), |had| had.min(span.start())));
    }
    first
}

fn object_quad(kind: &PaintAtomKind) -> Option<pdf_semantics::Quad> {
    pdf_semantics::placed_quad(kind).or_else(|| match kind {
        PaintAtomKind::Path(_) => kind
            .user_bounds()
            .map(|bounds| pdf_semantics::Quad::placed(bounds, Matrix::IDENTITY)),
        _ => None,
    })
}

pub(crate) fn placement_of(kind: &PaintAtomKind) -> Result<Matrix, SpikeError> {
    match kind {
        PaintAtomKind::Image(image) => Ok(image.state.ctm.value),
        PaintAtomKind::Shading(shading) => Ok(shading.state.ctm.value),
        PaintAtomKind::TransparencyGroup(group) => Ok(group.state.ctm.value),
        PaintAtomKind::Text(_) => Err(SpikeError::ObjectIsText),
        PaintAtomKind::Path(path) => Ok(path.state.ctm.value),
    }
}

pub(crate) fn finite(matrix: Matrix) -> bool {
    [matrix.a, matrix.b, matrix.c, matrix.d, matrix.e, matrix.f]
        .iter()
        .all(|value| value.is_finite())
}

pub(crate) fn written_at(
    program: &pdf_content::PageProgram,
    operations: &[Vec<pdf_content::Operation>],
    atom: &pdf_paint::PaintAtom,
) -> Result<(usize, usize), SpikeError> {
    if program
        .streams
        .iter()
        .filter(|stream| stream.reference == atom.id.stream)
        .count()
        != 1
    {
        return Err(SpikeError::SharedPageContentStream);
    }
    let stream_index = program
        .streams
        .iter()
        .position(|stream| stream.reference == atom.id.stream)
        .ok_or(SpikeError::ObjectNotInPageContent)?;
    let operation_index = operations[stream_index]
        .iter()
        .position(|operation| operation.operator_span() == atom.id.operator_span)
        .ok_or(SpikeError::ObjectNotInPageContent)?;
    Ok((stream_index, operation_index))
}

fn clip_admits(
    kind: &PaintAtomKind,
    wanted: Matrix,
    stationary: usize,
) -> Result<Showing, SpikeError> {
    let clips = match kind {
        PaintAtomKind::Image(image) => &image.state.clip_paths,
        PaintAtomKind::Shading(shading) => &shading.state.clip_paths,
        PaintAtomKind::TransparencyGroup(group) => &group.state.clip_paths,
        PaintAtomKind::Path(path) => &path.state.clip_paths,
        PaintAtomKind::Text(_) => return Ok(Showing::Whole),
    };
    let stationary = &clips[..stationary.min(clips.len())];
    if stationary.is_empty() {
        return Ok(Showing::Whole);
    }
    let quad = object_quad(kind).ok_or(SpikeError::ObjectExtentUnknown)?;
    let there: Vec<Point> = quad
        .corners
        .iter()
        .map(|corner| wanted.transform(*corner))
        .collect();
    let mut showing = Showing::Whole;
    for clip in stationary {
        showing = showing.and(crate::spike_move_text::clip_shows(clip, &there));
    }
    Ok(showing)
}

fn ink_signature(kind: &PaintAtomKind) -> Option<String> {
    fn unplace(image: &mut pdf_paint::ImagePaint) {
        image.state.ctm.value = Matrix::IDENTITY;
        if let Some(mask) = image.soft_mask.as_mut() {
            unplace(mask);
        }
    }
    let mut kind = kind.clone();
    match &mut kind {
        PaintAtomKind::Image(image) => unplace(image),
        PaintAtomKind::Shading(shading) => shading.state.ctm.value = Matrix::IDENTITY,
        PaintAtomKind::TransparencyGroup(group) => group.state.ctm.value = Matrix::IDENTITY,
        PaintAtomKind::Path(path) => path.state.ctm.value = Matrix::IDENTITY,
        PaintAtomKind::Text(_) => return None,
    }
    Some(pdf_paint::paint_signature(&kind))
}

fn prove_placement(
    before: &PaintGraph,
    after: &PaintGraph,
    named: usize,
    wanted: Matrix,
    ctm: Matrix,
    travelling_from: usize,
) -> Result<(), SpikeError> {
    if before.atoms.len() != after.atoms.len() {
        return Err(SpikeError::MoveNotIsolated);
    }
    for (ordinal, (one, other)) in before.atoms.iter().zip(&after.atoms).enumerate() {
        if ordinal == named {
            continue;
        }
        prove_unchanged(&one.kind, &other.kind)?;
    }
    prove_placed(
        &before.atoms[named].kind,
        after.atoms.get(named),
        (wanted, ctm, travelling_from),
    )
}

pub(crate) fn prove_unchanged(
    one: &PaintAtomKind,
    other: &PaintAtomKind,
) -> Result<(), SpikeError> {
    prove_clips(one, other, Matrix::IDENTITY, usize::MAX)?;
    if let (PaintAtomKind::Text(was), PaintAtomKind::Text(now)) = (one, other) {
        crate::place_text::prove_run_placed(was, now, Matrix::IDENTITY)?;
    }
    if pdf_paint::paint_signature(one) != pdf_paint::paint_signature(other) {
        return Err(SpikeError::MoveNotIsolated);
    }
    Ok(())
}

pub(crate) fn prove_placed(
    before: &PaintAtomKind,
    after: Option<&pdf_paint::PaintAtom>,
    (wanted, ctm, travelling_from): (Matrix, Matrix, usize),
) -> Result<(), SpikeError> {
    let placed = after.ok_or(SpikeError::MoveNotProvable)?;
    prove_clips(before, &placed.kind, wanted, travelling_from)?;
    let landed = placement_of(&placed.kind)?;
    let expected = wanted.multiply(ctm);
    if !alike(landed, expected) {
        return Err(SpikeError::MoveNotIsolated);
    }
    let was = ink_signature(before).ok_or(SpikeError::MoveNotProvable)?;
    let now = ink_signature(&placed.kind).ok_or(SpikeError::MoveNotProvable)?;
    if was != now {
        return Err(SpikeError::MoveNotIsolated);
    }
    Ok(())
}

fn prove_clips(
    before: &PaintAtomKind,
    after: &PaintAtomKind,
    wanted: Matrix,
    travelling_from: usize,
) -> Result<(), SpikeError> {
    let one = &state_of(before).clip_paths;
    let other = &state_of(after).clip_paths;
    if one.len() != other.len() {
        return Err(SpikeError::MoveNotIsolated);
    }
    for (index, (was, now)) in one.iter().zip(other).enumerate() {
        let expected = if index >= travelling_from {
            wanted.multiply(was.ctm.value)
        } else {
            was.ctm.value
        };
        if !alike(now.ctm.value, expected) || clip_shape(was) != clip_shape(now) {
            return Err(SpikeError::MoveNotIsolated);
        }
    }
    if let (PaintAtomKind::Image(was), PaintAtomKind::Image(now)) = (before, after) {
        match (&was.soft_mask, &now.soft_mask) {
            (Some(was), Some(now)) => prove_clips(
                &PaintAtomKind::Image(was.clone()),
                &PaintAtomKind::Image(now.clone()),
                wanted,
                travelling_from,
            )?,
            (None, None) => {}
            _ => return Err(SpikeError::MoveNotIsolated),
        }
    }
    Ok(())
}

fn clip_shape(clip: &pdf_paint::ClipPath) -> String {
    pdf_paint::paint_signature(&PaintAtomKind::Path(pdf_paint::PathPaint {
        path: clip.path.clone(),
        stroke: false,
        fill: Some(clip.rule),
        state: pdf_paint::GraphicsState::default(),
    }))
}

fn state_of(kind: &PaintAtomKind) -> &pdf_paint::GraphicsState {
    match kind {
        PaintAtomKind::Image(paint) => &paint.state,
        PaintAtomKind::Path(paint) => &paint.state,
        PaintAtomKind::Text(paint) => &paint.state,
        PaintAtomKind::Shading(paint) => &paint.state,
        PaintAtomKind::TransparencyGroup(paint) => &paint.state,
    }
}

pub(crate) fn alike(one: Matrix, other: Matrix) -> bool {
    [
        (one.a, other.a),
        (one.b, other.b),
        (one.c, other.c),
        (one.d, other.d),
        (one.e, other.e),
        (one.f, other.f),
    ]
    .iter()
    .all(|(had, want)| (had - want).abs() <= PLACEMENT_TOLERANCE * want.abs().mul_add(1.0, 1.0))
}

pub(crate) fn declared_region(
    before: &PaintAtomKind,
    after: Option<&pdf_paint::PaintAtom>,
) -> Option<[f64; 4]> {
    let was = object_quad(before)?.bounds();
    let now = object_quad(&after?.kind)?.bounds();
    Some([
        was[0].min(now[0]) - PLACEMENT_TOLERANCE,
        was[1].min(now[1]) - PLACEMENT_TOLERANCE,
        was[2].max(now[2]) + PLACEMENT_TOLERANCE,
        was[3].max(now[3]) + PLACEMENT_TOLERANCE,
    ])
}

fn declared_region_of(before: &PaintAtomKind, wanted: Matrix) -> Option<[f64; 4]> {
    let quad = object_quad(before)?;
    let was = quad.bounds();
    let corners = quad.corners.map(|corner| wanted.transform(corner));
    let now = pdf_semantics::Quad {
        corners,
        evidence: quad.evidence,
    }
    .bounds();
    Some([
        was[0].min(now[0]) - PLACEMENT_TOLERANCE,
        was[1].min(now[1]) - PLACEMENT_TOLERANCE,
        was[2].max(now[2]) + PLACEMENT_TOLERANCE,
        was[3].max(now[3]) + PLACEMENT_TOLERANCE,
    ])
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Arc;

    use pdf_bytes::{ByteStore, SourceId};
    use pdf_content::{
        ContentLimits, PageContentLimits, load_page_program_strict, parse_operation_sequence_strict,
    };
    use pdf_paint::{
        Matrix, PaintAtomKind, PaintGraph, PaintLimits, PaintStream, Point,
        interpret_stream_sequence_with_resources,
    };

    use super::plan_place_object;
    use crate::plan::{FixedPoint, Plan, Showing, SourceAnchor};
    use crate::spike_move_text::SpikeError;

    pub(crate) fn page_with(content: &[u8]) -> ByteStore {
        document(content, false)
    }

    fn page_with_masked_picture(content: &[u8]) -> ByteStore {
        document(content, true)
    }

    fn document(content: &[u8], mask: bool) -> ByteStore {
        let samples: [u8; 12] = [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0];
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        let object = |bytes: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &[u8]| {
            offsets.push(bytes.len());
            bytes.extend_from_slice(body);
        };
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
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /XObject << /Im1 6 0 R >> /Shading << /Sh1 << /ShadingType 2 /ColorSpace /DeviceGray /Coords [0 0 100 0] /Function << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >> >> >> >> >>\nendobj\n",
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
            b"5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            format!(
                "6 0 obj\n<< /Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8{} /Length 12 >>\nstream\n",
                if mask { " /SMask 7 0 R" } else { "" }
            )
            .as_bytes(),
        );
        bytes.extend_from_slice(&samples);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        if mask {
            offsets.push(bytes.len());
            bytes.extend_from_slice(
                b"7 0 obj\n<< /Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 4 >>\nstream\n",
            );
            bytes.extend_from_slice(&[0, 90, 180, 255]);
            bytes.extend_from_slice(b"\nendstream\nendobj\n");
        }
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
        ByteStore::new(SourceId::new(700), Arc::<[u8]>::from(bytes))
    }

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

    pub(crate) fn anchor_of(graph: &PaintGraph, wanted: &str) -> SourceAnchor {
        let atom = graph
            .atoms
            .iter()
            .find(|atom| kind_name(&atom.kind) == wanted)
            .unwrap_or_else(|| panic!("the fixture paints a {wanted}"));
        SourceAnchor::of(&atom.id)
    }

    fn kind_name(kind: &PaintAtomKind) -> &'static str {
        match kind {
            PaintAtomKind::Text(_) => "text",
            PaintAtomKind::Path(_) => "path",
            PaintAtomKind::Image(_) => "image",
            PaintAtomKind::Shading(_) => "shading",
            PaintAtomKind::TransparencyGroup(_) => "group",
        }
    }

    fn image_ctm(graph: &PaintGraph) -> Matrix {
        graph
            .atoms
            .iter()
            .find_map(|atom| match &atom.kind {
                PaintAtomKind::Image(image) => Some(image.state.ctm.value),
                _ => None,
            })
            .expect("the fixture paints an image")
    }

    pub(crate) fn glyphs(graph: &PaintGraph) -> Vec<Point> {
        let mut found = Vec::new();
        for atom in &graph.atoms {
            let PaintAtomKind::Text(text) = &atom.kind else {
                continue;
            };
            for glyph in &text.glyphs {
                found.push(crate::block_move::glyph_origin(
                    glyph.matrix,
                    text.state.ctm.value,
                ));
            }
        }
        found
    }

    fn translate(dx: f64, dy: f64) -> Matrix {
        Matrix {
            e: dx,
            f: dy,
            ..Matrix::IDENTITY
        }
    }

    fn place(
        source: &ByteStore,
        target: &str,
        transform: Matrix,
        about: FixedPoint,
    ) -> Result<(Plan, Read), SpikeError> {
        let read = read(source);
        let anchor = anchor_of(&read.graph, target);
        let plan = plan_place_object(
            source,
            &read.program,
            &read.operations,
            &read.graph,
            0,
            &anchor,
            transform,
            about,
            None,
        )?;
        Ok((plan, read))
    }

    pub(crate) fn after(source: &ByteStore, plan: &Plan) -> ByteStore {
        plan.commit(source, b"").expect("the plan commits")
    }

    fn close(had: f64, want: f64) -> bool {
        (had - want).abs() < 1e-9
    }

    pub(crate) const PAGE: &[u8] =
        b"BT /F1 12 Tf 1 0 0 1 20 100 Tm (AB) Tj ET\nq 40 0 0 30 60 40 cm /Im1 Do Q\n";

    #[test]
    fn a_picture_moves_by_the_distance_asked_for_and_nothing_else_moves() {
        let source = page_with(PAGE);
        let (plan, before) = place(&source, "image", translate(30.0, -20.0), FixedPoint::Origin)
            .expect("a picture on the page moves");
        assert_eq!(plan.capability(), crate::plan::Capability::Exact);

        let committed = after(&source, &plan);
        let read = read(&committed);
        let (was, now) = (image_ctm(&before.graph), image_ctm(&read.graph));
        assert!(close(now.a, was.a) && close(now.d, was.d), "{now:?}");
        assert!(
            close(now.e, was.e + 30.0),
            "{} is not {}",
            now.e,
            was.e + 30.0
        );
        assert!(
            close(now.f, was.f - 20.0),
            "{} is not {}",
            now.f,
            was.f - 20.0
        );
        assert_eq!(glyphs(&before.graph), glyphs(&read.graph));
    }

    #[test]
    fn the_cm_that_placed_it_is_wrapped_rather_than_rewritten() {
        let source = page_with(PAGE);
        let (plan, _) = place(&source, "image", translate(30.0, -20.0), FixedPoint::Origin)
            .expect("a picture on the page moves");
        let crate::plan::PlannedBody::ReplacedStream { decoded } = &plan.writes()[0].body else {
            panic!("a placement replaces one content stream");
        };
        let written = String::from_utf8(decoded.clone()).expect("the fixture is ASCII");
        assert!(
            written.contains("40 0 0 30 60 40 cm"),
            "the original placement was rewritten: {written}"
        );
        let Some((_, wrapped)) = written.split_once("q 1 0 0 1 ") else {
            panic!("not wrapped: {written}")
        };
        assert!(
            wrapped.contains("cm /Im1 Do Q"),
            "the wrap does not close round the Do: {written}"
        );
        let (u, rest) = wrapped.split_once(' ').expect("two numbers and cm");
        let (v, _) = rest.split_once(' ').expect("two numbers and cm");
        assert!(
            (u.parse::<f64>().expect("a number") - 30.0 / 40.0).abs() < 1e-9,
            "{u} is not 30/40"
        );
        assert!(
            (v.parse::<f64>().expect("a number") + 20.0 / 30.0).abs() < 1e-9,
            "{v} is not -20/30"
        );
    }

    #[test]
    fn a_picture_under_a_turned_matrix_still_moves_the_way_the_page_is_read() {
        let source =
            page_with(b"q 0.7071 0.7071 -0.7071 0.7071 100 20 cm 40 0 0 30 0 0 cm /Im1 Do Q\n");
        let (plan, before) = place(&source, "image", translate(25.0, 0.0), FixedPoint::Origin)
            .expect("a turned picture moves");
        let read = read(&after(&source, &plan));
        let (was, now) = (image_ctm(&before.graph), image_ctm(&read.graph));
        assert!(
            close(now.e, was.e + 25.0),
            "{} is not {}",
            now.e,
            was.e + 25.0
        );
        assert!(close(now.f, was.f), "{} is not {}", now.f, was.f);
        assert!(close(now.a, was.a) && close(now.b, was.b), "{now:?}");
        assert!(close(now.c, was.c) && close(now.d, was.d), "{now:?}");
    }

    #[test]
    fn a_scale_about_a_corner_leaves_that_corner_where_it_was() {
        let source = page_with(PAGE);
        let held = Point { x: 60.0, y: 40.0 };
        let doubled = Matrix {
            a: 2.0,
            d: 2.0,
            ..Matrix::IDENTITY
        };
        let (plan, before) =
            place(&source, "image", doubled, FixedPoint::At(held)).expect("a picture scales");
        let read = read(&after(&source, &plan));
        let (was, now) = (image_ctm(&before.graph), image_ctm(&read.graph));
        assert!(
            close(now.a, was.a * 2.0) && close(now.d, was.d * 2.0),
            "{now:?}"
        );
        let corner = now.transform(Point { x: 0.0, y: 0.0 });
        assert!(
            close(corner.x, held.x) && close(corner.y, held.y),
            "{corner:?}"
        );
    }

    #[test]
    fn text_is_refused_and_a_path_is_wrapped_from_its_first_construction_operator() {
        let source = page_with(PAGE);
        assert!(matches!(
            place(&source, "text", translate(1.0, 0.0), FixedPoint::Origin),
            Err(SpikeError::ObjectIsText)
        ));
        let drawn = page_with(b"0 0 100 100 re f\n");
        let (plan, _) = place(&drawn, "path", translate(1.0, 0.0), FixedPoint::Origin)
            .expect("a path on the page moves");
        let committed = read(&after(&drawn, &plan));
        let written =
            String::from_utf8_lossy(committed.program.streams[0].bytes.as_bytes()).into_owned();
        assert!(
            written.starts_with("q 1 0 0 1 1 0 cm 0 0 100 100 re f Q"),
            "{written:?}"
        );
        let ctm = committed
            .graph
            .atoms
            .iter()
            .find_map(|atom| match &atom.kind {
                PaintAtomKind::Path(path) => Some(path.state.ctm.value),
                _ => None,
            })
            .expect("the path is painted");
        assert!(close(ctm.e, 1.0) && close(ctm.f, 0.0), "{ctm:?}");
    }

    #[test]
    fn one_of_two_pictures_moves_and_the_other_does_not() {
        let source =
            page_with(b"q 40 0 0 30 10 10 cm /Im1 Do Q\nq 40 0 0 30 120 150 cm /Im1 Do Q\n");
        let read_before = read(&source);
        let second = SourceAnchor::of(
            &read_before
                .graph
                .atoms
                .iter()
                .filter(|atom| matches!(atom.kind, PaintAtomKind::Image(_)))
                .nth(1)
                .expect("two pictures")
                .id,
        );
        let plan = plan_place_object(
            &source,
            &read_before.program,
            &read_before.operations,
            &read_before.graph,
            0,
            &second,
            translate(-15.0, 5.0),
            FixedPoint::Origin,
            None,
        )
        .expect("the second picture moves");
        let read_after = read(&after(&source, &plan));
        let placements = |graph: &PaintGraph| -> Vec<(f64, f64)> {
            graph
                .atoms
                .iter()
                .filter_map(|atom| match &atom.kind {
                    PaintAtomKind::Image(image) => {
                        Some((image.state.ctm.value.e, image.state.ctm.value.f))
                    }
                    _ => None,
                })
                .collect()
        };
        assert_eq!(
            placements(&read_before.graph),
            [(10.0, 10.0), (120.0, 150.0)]
        );
        let now = placements(&read_after.graph);
        assert_eq!(now.len(), 2);
        assert!(close(now[0].0, 10.0) && close(now[0].1, 10.0), "{now:?}");
        assert!(close(now[1].0, 105.0) && close(now[1].1, 155.0), "{now:?}");
    }

    #[test]
    fn a_picture_painted_inside_a_text_object_is_refused_by_this_planner() {
        let source = page_with(b"BT /F1 12 Tf 1 0 0 1 20 100 Tm (A) Tj /Im1 Do ET\n");
        let program = load_page_program_strict(&source, 0, PageContentLimits::default())
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
        .expect("the page is drawn, and what was wrong with it is recorded");
        assert!(
            graph.repairs.iter().any(|repair| matches!(
                repair.kind,
                pdf_paint::RepairKind::OperatorInsideTextObject
            )),
            "the interpreter says where the picture was painted"
        );

        let image = graph
            .atoms
            .iter()
            .find(|atom| matches!(atom.kind, PaintAtomKind::Image(_)))
            .expect("the fixture paints one picture");
        let refused = plan_place_object(
            &source,
            &program,
            &operations,
            &graph,
            0,
            &SourceAnchor::of(&image.id),
            Matrix {
                a: 2.0,
                d: 2.0,
                ..Matrix::IDENTITY
            },
            FixedPoint::Origin,
            None,
        )
        .expect_err("a picture inside a text object cannot be given a transform");
        assert!(
            matches!(refused, SpikeError::ObjectInsideTextObject),
            "{refused:?}"
        );
    }

    #[test]
    fn a_placement_that_would_leave_the_picture_with_no_area_is_refused() {
        let source = page_with(PAGE);
        let flattened = Matrix {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 0.0,
            e: 0.0,
            f: 0.0,
        };
        assert!(matches!(
            place(&source, "image", flattened, FixedPoint::Origin),
            Err(SpikeError::PlacementNotInvertible)
        ));
    }

    #[test]
    fn a_picture_moves_under_a_clip_whole_cropped_or_out_of_sight_and_says_which() {
        let whole = page_with(b"q 60 40 60 30 re W n 40 0 0 30 60 40 cm /Im1 Do /Im1 Do Q\n");
        let (plan, _) = place(&whole, "image", translate(30.0, 0.0), FixedPoint::Origin)
            .expect("a move that cuts a whole picture is done, not refused");
        assert_eq!(
            plan.showing(),
            Showing::PartlyHidden,
            "the plan says part of it is hidden"
        );
        let cropped = page_with(b"q 60 40 20 30 re W n 40 0 0 30 60 40 cm /Im1 Do /Im1 Do Q\n");
        let (plan, _) = place(&cropped, "image", translate(1.0, 0.0), FixedPoint::Origin)
            .expect("a cropped picture slides under its crop");
        assert_eq!(plan.showing(), Showing::PartlyHidden);
        let (plan, _) = place(&cropped, "image", translate(400.0, 0.0), FixedPoint::Origin)
            .expect("a move out of sight is done, not refused");
        assert_eq!(plan.showing(), Showing::OutOfSight);
        let roomy = page_with(b"q 0 0 200 200 re W n 40 0 0 30 60 40 cm /Im1 Do Q\n");
        let (plan, _) =
            place(&roomy, "image", translate(30.0, 0.0), FixedPoint::Origin).expect("roomy");
        assert_eq!(plan.showing(), Showing::Whole, "nothing is hidden");
    }

    #[test]
    fn a_curved_clip_no_longer_refuses_a_move_and_still_answers_out_of_sight() {
        let curved: &[u8] = b"q 50 30 m 110 30 l 110 50 110 70 90 70 c 50 70 l h W n \
             40 0 0 30 60 35 cm /Im1 Do /Im1 Do Q\n";
        let source = page_with(curved);
        let (plan, _) = place(&source, "image", translate(2.0, 0.0), FixedPoint::Origin)
            .expect("a curved clip no longer refuses");
        assert_eq!(plan.showing(), Showing::Whole);
        let (plan, _) = place(&source, "image", translate(600.0, 0.0), FixedPoint::Origin)
            .expect("a curved clip no longer refuses");
        assert_eq!(
            plan.showing(),
            Showing::OutOfSight,
            "the cover of a curved clip still answers what it can"
        );
    }

    #[test]
    fn an_exclusive_crop_travels_with_the_image_and_survives_undo() {
        let source = page_with_masked_picture(
            b"q 2 0 0 3 10 20 cm q 5 5 10 10 re W n 20 0 0 20 0 0 cm /Im1 Do Q Q\n\
              q 10 0 0 10 150 150 cm /Im1 Do Q\n",
        );
        let (plan, before) = place(&source, "image", translate(30.0, -10.0), FixedPoint::Origin)
            .expect("exclusive crop travels");
        let moved = after(&source, &plan);
        let now = read(&moved);
        let clip = &super::state_of(&now.graph.atoms[0].kind).clip_paths[0];
        let region = crate::clip_region::ClipRegion::of(&clip.path, clip.ctm.value).unwrap();
        let corners = region.parts()[0].clone();
        assert!(
            corners.iter().any(|p| close(p.x, 50.0) && close(p.y, 25.0)),
            "{corners:?}"
        );
        assert!(
            corners.iter().any(|p| close(p.x, 70.0) && close(p.y, 55.0)),
            "{corners:?}"
        );
        assert_eq!(
            pdf_paint::paint_signature(&before.graph.atoms[1].kind),
            pdf_paint::paint_signature(&now.graph.atoms[1].kind)
        );
        let undone = plan
            .inverse(&source, b"")
            .unwrap()
            .commit(&moved, b"")
            .unwrap();
        let back = read(&undone);
        super::prove_clips(
            &before.graph.atoms[0].kind,
            &back.graph.atoms[0].kind,
            Matrix::IDENTITY,
            usize::MAX,
        )
        .unwrap();
    }

    #[test]
    fn curved_and_even_odd_crops_can_be_scaled_and_turned_without_polygon_approximation() {
        let source =
            page_with(b"q 60 40 m 60 80 100 80 100 40 c h W* n 40 0 0 30 60 40 cm /Im1 Do Q\n");
        let transform = Matrix {
            a: 0.0,
            b: 2.0,
            c: -2.0,
            d: 0.0,
            e: 0.0,
            f: 0.0,
        };
        let (plan, before) = place(&source, "image", transform, FixedPoint::Origin).unwrap();
        let now = read(&after(&source, &plan));
        let clip = &super::state_of(&now.graph.atoms[0].kind).clip_paths[0];
        assert!(super::alike(clip.ctm.value, transform));
        assert_eq!(
            super::clip_shape(clip),
            super::clip_shape(&super::state_of(&before.graph.atoms[0].kind).clip_paths[0])
        );
    }

    #[test]
    fn an_inherited_shared_clip_stays_put_and_still_limits_the_move() {
        let source = page_with(
            b"q 0 0 150 150 re W n q 60 40 20 30 re W n 40 0 0 30 60 40 cm /Im1 Do Q\n\
            q 10 0 0 10 10 10 cm /Im1 Do Q Q\n",
        );
        let (plan, before) =
            place(&source, "image", translate(10.0, 0.0), FixedPoint::Origin).unwrap();
        let now = read(&after(&source, &plan));
        let clips = &super::state_of(&now.graph.atoms[0].kind).clip_paths;
        assert!(super::alike(clips[0].ctm.value, Matrix::IDENTITY));
        assert!(super::alike(clips[1].ctm.value, translate(10.0, 0.0)));
        super::prove_clips(
            &before.graph.atoms[1].kind,
            &now.graph.atoms[1].kind,
            Matrix::IDENTITY,
            usize::MAX,
        )
        .unwrap();
        let (far, _) = place(&source, "image", translate(200.0, 0.0), FixedPoint::Origin)
            .expect("leaving the inherited clip is a move, not a refusal");
        assert_eq!(far.showing(), Showing::OutOfSight);
    }

    #[test]
    fn a_path_crossing_q_or_q_restore_is_not_owned_by_that_scope() {
        for content in [
            &b"60 40 20 30 re q W n 40 0 0 30 60 40 cm /Im1 Do Q\n"[..],
            &b"q 60 40 20 30 re W n 40 0 0 30 60 40 cm /Im1 Do 0 0 5 5 re Q f\n"[..],
        ] {
            let source = page_with(content);
            assert!(read(&source).graph.object_scopes.is_empty());
            let (plan, _) = place(&source, "image", translate(1.0, 0.0), FixedPoint::Origin)
                .expect("it slides under the crop the scope does not own");
            assert_eq!(plan.showing(), Showing::PartlyHidden);
        }
    }

    #[test]
    fn clip_proof_rejects_stationary_lost_reshaped_and_wrong_winding_crops() {
        let source = page_with(b"q 60 40 20 30 re W n 40 0 0 30 60 40 cm /Im1 Do Q\n");
        let (plan, before) =
            place(&source, "image", translate(30.0, 0.0), FixedPoint::Origin).unwrap();
        let now = read(&after(&source, &plan));
        let check = |graph: &PaintGraph| {
            super::prove_placement(
                &before.graph,
                graph,
                0,
                translate(30.0, 0.0),
                image_ctm(&before.graph),
                0,
            )
        };
        check(&now.graph).unwrap();
        for mutation in 0..4 {
            let mut broken = now.graph.clone();
            let PaintAtomKind::Image(image) = &mut broken.atoms[0].kind else {
                unreachable!()
            };
            match mutation {
                0 => image.state.clip_paths[0].ctm.value = Matrix::IDENTITY,
                1 => image.state.clip_paths.clear(),
                2 => image.state.clip_paths[0].path.segments.clear(),
                _ => image.state.clip_paths[0].rule = pdf_paint::FillRule::EvenOdd,
            }
            assert!(check(&broken).is_err(), "bad measurement {mutation} passed");
        }
    }

    #[test]
    fn a_shading_with_no_extent_travels_with_the_clip_that_crops_it() {
        let source = page_with(b"q 20 20 50 50 re W n /Sh1 sh Q\n");
        let before = read(&source);
        assert!(matches!(
            before.graph.atoms[0].kind,
            PaintAtomKind::Shading(_)
        ));
        assert_eq!(
            super::state_of(&before.graph.atoms[0].kind)
                .clip_paths
                .len(),
            1
        );
        let (plan, _) = place(&source, "shading", translate(30.0, 0.0), FixedPoint::Origin)
            .expect("an owned crop makes no containment claim");
        let now = read(&after(&source, &plan));
        let clip = &super::state_of(&now.graph.atoms[0].kind).clip_paths[0];
        assert!(super::alike(clip.ctm.value, translate(30.0, 0.0)));

        let shared = page_with(b"q 20 20 50 50 re W n /Sh1 sh /Sh1 sh Q\n");
        assert!(matches!(
            place(&shared, "shading", translate(30.0, 0.0), FixedPoint::Origin),
            Err(SpikeError::ObjectExtentUnknown)
        ));
    }

    #[test]
    fn a_picture_with_a_soft_mask_moves_with_its_mask() {
        let source = page_with_masked_picture(PAGE);
        let (plan, before) = place(&source, "image", translate(30.0, -20.0), FixedPoint::Origin)
            .expect("a masked picture moves");
        let read = read(&after(&source, &plan));
        let (was, now) = (image_ctm(&before.graph), image_ctm(&read.graph));
        assert!(
            close(now.e, was.e + 30.0) && close(now.f, was.f - 20.0),
            "{now:?}"
        );
        let mask_ctm = |graph: &PaintGraph| -> Matrix {
            graph
                .atoms
                .iter()
                .find_map(|atom| match &atom.kind {
                    PaintAtomKind::Image(image) => Some(image.soft_mask.as_ref()?.state.ctm.value),
                    _ => None,
                })
                .expect("the fixture's picture carries a mask")
        };
        let (mask_was, mask_now) = (mask_ctm(&before.graph), mask_ctm(&read.graph));
        assert!(
            close(mask_now.e, mask_was.e + 30.0) && close(mask_now.f, mask_was.f - 20.0),
            "{mask_now:?}"
        );
    }

    #[test]
    fn undoing_a_placement_puts_the_picture_back_where_the_file_had_it() {
        let source = page_with(PAGE);
        let (plan, before) = place(&source, "image", translate(30.0, -20.0), FixedPoint::Origin)
            .expect("a picture on the page moves");
        let moved = after(&source, &plan);
        let undone = plan
            .inverse(&source, b"")
            .expect("a replaced stream can be put back")
            .commit(&moved, b"")
            .expect("the inverse commits");
        let back = image_ctm(&read(&undone).graph);
        let was = image_ctm(&before.graph);
        assert!(close(back.e, was.e) && close(back.f, was.f), "{back:?}");
    }

    #[test]
    fn the_declared_region_covers_where_the_picture_was_and_where_it_went() {
        let source = page_with(PAGE);
        let (plan, _) = place(&source, "image", translate(30.0, 0.0), FixedPoint::Origin)
            .expect("a picture on the page moves");
        let region = plan
            .effect()
            .declared_region
            .expect("a picture has an extent");
        assert!(region[0] <= 60.0 && region[1] <= 40.0, "{region:?}");
        assert!(region[2] >= 130.0 && region[3] >= 70.0, "{region:?}");
        assert!(region[0] > 59.9 && region[2] < 130.1, "{region:?}");
    }
}
