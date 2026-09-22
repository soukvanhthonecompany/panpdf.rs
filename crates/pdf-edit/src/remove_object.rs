use pdf_paint::{Matrix, PaintAtomKind, PaintGraph};

use crate::plan::{Capability, Effect, MovedRun, Plan, PlannedBody, PlannedWrite, SourceAnchor};
use crate::spike_move_text::SpikeError;

pub(crate) fn plan_remove_object(
    program: &pdf_content::PageProgram,
    operations: &[Vec<pdf_content::Operation>],
    graph: &PaintGraph,
    page_index: usize,
    anchor: &SourceAnchor,
    fonts: crate::Fonts<'_>,
) -> Result<Plan, SpikeError> {
    let ordinal = crate::place_object::resolve(graph, anchor)?;
    let atom = &graph.atoms[ordinal];
    let region = crate::place_object::declared_region(&atom.kind, Some(atom));
    let ctm = crate::place_object::placement_of(&atom.kind)?;

    let (stream_index, operation_index) =
        crate::place_object::written_at(program, operations, atom)?;
    let span = operations[stream_index][operation_index].span();
    let begin = crate::place_object::construction_start(atom).unwrap_or_else(|| span.start());
    if begin < span.start()
        && !only_draws(
            &program.streams[stream_index].bytes,
            &operations[stream_index],
            begin..span.end(),
        )
    {
        return Err(SpikeError::ObjectIsDrawing);
    }

    let decoded = program.streams[stream_index].bytes.as_bytes();
    if span.end() > decoded.len() || begin > span.end() {
        return Err(SpikeError::ObjectNotInPageContent);
    }
    let mut edited = Vec::with_capacity(decoded.len());
    edited.extend_from_slice(&decoded[..begin]);
    edited.push(b' ');
    edited.extend_from_slice(&decoded[span.end()..]);

    let rewritten =
        crate::spike_move_text::interpret_bytes_of(program, stream_index, &edited, fonts)?;
    prove_removed(graph, &rewritten, ordinal)?;

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
            declared_region: region,
        },
    ))
}

pub(crate) fn only_draws(
    source: &pdf_bytes::ByteStore,
    operations: &[pdf_content::Operation],
    range: std::ops::Range<usize>,
) -> bool {
    const DRAWING: &[&[u8]] = &[
        b"m", b"l", b"c", b"v", b"y", b"h", b"re", b"f", b"F", b"f*", b"B", b"B*", b"b", b"b*",
        b"S", b"s", b"n",
    ];
    operations
        .iter()
        .filter(|operation| {
            operation.span().start() >= range.start && operation.span().end() <= range.end
        })
        .all(|operation| {
            operation
                .operator_bytes(source)
                .is_ok_and(|operator| DRAWING.contains(&operator))
        })
}

fn prove_removed(
    before: &PaintGraph,
    after: &PaintGraph,
    ordinal: usize,
) -> Result<(), SpikeError> {
    if before.atoms.len() != after.atoms.len() + 1 {
        return Err(SpikeError::MoveNotIsolated);
    }
    let remaining = before
        .atoms
        .iter()
        .enumerate()
        .filter(|(at, _)| *at != ordinal)
        .map(|(_, atom)| atom);
    for (one, other) in remaining.zip(&after.atoms) {
        match (&one.kind, &other.kind) {
            (PaintAtomKind::Text(one), PaintAtomKind::Text(other)) => {
                crate::place_text::prove_run_placed(one, other, Matrix::IDENTITY)?;
            }
            _ => {
                if pdf_paint::paint_signature(&one.kind) != pdf_paint::paint_signature(&other.kind)
                {
                    return Err(SpikeError::MoveNotIsolated);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use pdf_bytes::ByteStore;
    use pdf_paint::PaintAtomKind;

    use super::plan_remove_object;
    use crate::place_object::tests::{PAGE, after, anchor_of, glyphs, page_with, read};
    use crate::plan::{Plan, SourceAnchor};
    use crate::spike_move_text::SpikeError;

    fn remove(source: &ByteStore, target: &str) -> Result<Plan, SpikeError> {
        let held = read(source);
        let anchor = anchor_of(&held.graph, target);
        plan_remove_object(
            &held.program,
            &held.operations,
            &held.graph,
            0,
            &anchor,
            None,
        )
    }

    #[test]
    fn a_picture_goes_and_the_text_beside_it_stays_exactly_where_it_was() {
        let source = page_with(PAGE);
        let before = read(&source);
        let was = glyphs(&before.graph);
        assert_eq!(before.graph.atoms.len(), 2);

        let plan = remove(&source, "image").expect("a picture on the page can be removed");
        assert_eq!(plan.capability(), crate::plan::Capability::Exact);

        let now = read(&after(&source, &plan));
        assert_eq!(now.graph.atoms.len(), 1);
        assert!(
            !now.graph
                .atoms
                .iter()
                .any(|atom| matches!(atom.kind, PaintAtomKind::Image(_))),
            "the picture is gone"
        );
        let still = glyphs(&now.graph);
        assert_eq!(was.len(), still.len());
        for (one, other) in was.iter().zip(&still) {
            assert!(
                (one.x - other.x).abs() < 1e-9 && (one.y - other.y).abs() < 1e-9,
                "{one:?} became {other:?}"
            );
        }
    }

    #[test]
    fn what_is_written_is_the_page_minus_one_operator() {
        let source = page_with(PAGE);
        let plan = remove(&source, "image").expect("a picture can be removed");
        let written = plan
            .writes()
            .iter()
            .find_map(|write| match &write.body {
                crate::plan::PlannedBody::ReplacedStream { decoded } => Some(decoded.clone()),
                crate::plan::PlannedBody::NewStream { .. }
                | crate::plan::PlannedBody::Direct { .. } => None,
            })
            .expect("a replaced content stream");
        let written = String::from_utf8(written).expect("the fixture is ASCII");
        assert!(!written.contains("/Im1 Do"), "{written}");
        assert!(written.contains("q 40 0 0 30 60 40 cm"), "{written}");
        assert!(written.contains(" Q"), "{written}");
        assert!(written.contains("(AB) Tj"), "{written}");
    }

    #[test]
    fn the_region_it_declares_is_where_the_picture_stood() {
        let source = page_with(PAGE);
        let plan = remove(&source, "image").expect("a picture can be removed");
        let region = plan
            .effect()
            .declared_region
            .expect("a picture has an extent");
        assert!(
            region[0] <= 60.0 + 1e-6 && region[1] <= 40.0 + 1e-6,
            "{region:?}"
        );
        assert!(
            region[2] >= 100.0 - 1e-6 && region[3] >= 70.0 - 1e-6,
            "{region:?}"
        );
    }

    #[test]
    fn text_is_refused_because_a_different_command_owns_it() {
        let source = page_with(PAGE);
        assert!(matches!(
            remove(&source, "text"),
            Err(SpikeError::ObjectIsText)
        ));
    }

    #[test]
    fn a_drawing_goes_from_its_first_construction_operator() {
        let source = page_with(b"BT /F1 12 Tf 1 0 0 1 20 100 Tm (AB) Tj ET\n0 0 10 10 re f\n");
        let plan = remove(&source, "path").expect("a drawing can be removed");
        let written = plan
            .writes()
            .iter()
            .find_map(|write| match &write.body {
                crate::plan::PlannedBody::ReplacedStream { decoded } => Some(decoded.clone()),
                crate::plan::PlannedBody::NewStream { .. }
                | crate::plan::PlannedBody::Direct { .. } => None,
            })
            .expect("a replaced content stream");
        let written = String::from_utf8(written).expect("the fixture is ASCII");
        assert!(!written.contains("re"), "{written}");
        assert!(written.contains("(AB) Tj"), "{written}");
        let now = read(&after(&source, &plan));
        assert_eq!(now.graph.atoms.len(), 1);
    }

    #[test]
    fn a_drawing_that_clips_is_refused() {
        let source = page_with(b"0 0 10 10 re W f\nBT /F1 12 Tf 1 0 0 1 2 2 Tm (AB) Tj ET\n");
        assert!(matches!(
            remove(&source, "path"),
            Err(SpikeError::ObjectIsDrawing)
        ));
    }

    #[test]
    fn an_anchor_naming_nothing_is_refused_rather_than_guessed_at() {
        let source = page_with(PAGE);
        let held = read(&source);
        let anchor = SourceAnchor {
            stream: held.program.streams[0].reference,
            operator_offset: 9_999,
            invocation_path: Vec::new(),
        };
        assert!(matches!(
            plan_remove_object(
                &held.program,
                &held.operations,
                &held.graph,
                0,
                &anchor,
                None
            ),
            Err(SpikeError::AnchorNamesNothing)
        ));
    }
}
