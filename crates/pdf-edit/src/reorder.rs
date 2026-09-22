use std::collections::BTreeMap;
use std::ops::Range;

use pdf_paint::{Matrix, PaintAtomKind, PaintGraph};

use crate::plan::{Capability, Effect, MovedRun, Plan, PlannedBody, PlannedWrite, SourceAnchor};
use crate::spike_move_text::SpikeError;
use crate::stacking::{Stacking, is_unchanged, reordered};

fn refused(reason: &'static str) -> SpikeError {
    SpikeError::StackingUnsupported(reason)
}

struct Unit {
    bytes: Range<usize>,
    atoms: Vec<usize>,
}

pub(crate) fn plan_reorder_objects(
    program: &pdf_content::PageProgram,
    operations: &[Vec<pdf_content::Operation>],
    graph: &PaintGraph,
    page_index: usize,
    targets: &[SourceAnchor],
    command: Stacking,
    fonts: crate::Fonts<'_>,
) -> Result<Plan, SpikeError> {
    if targets.is_empty() {
        return Err(refused("nothing was chosen to move in the order"));
    }
    let chosen_atoms = resolve_all(graph, targets)?;
    let stream_index = one_stream(program, operations, graph, &chosen_atoms)?;
    let source = &program.streams[stream_index].bytes;
    let decoded = source.as_bytes();

    let units = units_of(source, &operations[stream_index], graph, decoded.len())?;
    if units.len() < 2 {
        return Err(refused(
            "there is nothing else on this page to be in front of or behind",
        ));
    }
    let mut chosen: Vec<usize> = Vec::with_capacity(chosen_atoms.len());
    for ordinal in &chosen_atoms {
        let at = units
            .iter()
            .position(|unit| unit.atoms.contains(ordinal))
            .ok_or_else(|| {
                refused("this is painted among the page's own instructions, not as a thing of its own, so it has no place of its own in the order")
            })?;
        if !chosen.contains(&at) {
            chosen.push(at);
        }
    }
    let order = reordered(units.len(), &chosen, command)
        .map_err(|_| refused("that is not a selection this page can put in an order"))?;
    if is_unchanged(&order) {
        return Err(SpikeError::StackingUnchanged);
    }

    let edited = written(decoded, &units, &order);
    let expected = expected_atoms(graph, &units, &order, stream_index, program)?;
    let rewritten =
        crate::spike_move_text::interpret_bytes_of(program, stream_index, &edited, fonts)?;
    prove_reordered(graph, &rewritten, &expected)?;

    let moved: Vec<usize> = order
        .iter()
        .enumerate()
        .filter(|(now, was)| *now != **was)
        .flat_map(|(_, was)| units[*was].atoms.iter().copied())
        .collect();
    let stream_reference = program.streams[stream_index].reference;
    Ok(Plan::new(
        Capability::Exact,
        vec![PlannedWrite {
            reference: stream_reference,
            body: PlannedBody::ReplacedStream { decoded: edited },
        }],
        Effect {
            page_index,
            moved: moved
                .iter()
                .map(|ordinal| MovedRun {
                    anchor: SourceAnchor::of(&graph.atoms[*ordinal].id),
                    atom_ordinal: *ordinal,
                    original_matrix: placed_by(&graph.atoms[*ordinal].kind),
                })
                .collect(),
            target_stream: stream_reference,
            declared_region: region_of(graph, &moved),
        },
    )
    .with_correspondence(correspondence(graph, &expected)))
}

fn resolve_all(graph: &PaintGraph, targets: &[SourceAnchor]) -> Result<Vec<usize>, SpikeError> {
    let mut found = Vec::with_capacity(targets.len());
    for anchor in targets {
        let mut naming = graph
            .atoms
            .iter()
            .enumerate()
            .filter(|(_, atom)| anchor.names(&atom.id));
        let (ordinal, _) = naming.next().ok_or(SpikeError::AnchorNamesNothing)?;
        if naming.next().is_some() {
            return Err(SpikeError::ObjectNamedMoreThanOnce);
        }
        if !found.contains(&ordinal) {
            found.push(ordinal);
        }
    }
    found.sort_unstable();
    Ok(found)
}

fn one_stream(
    program: &pdf_content::PageProgram,
    operations: &[Vec<pdf_content::Operation>],
    graph: &PaintGraph,
    chosen: &[usize],
) -> Result<usize, SpikeError> {
    let mut only: Option<usize> = None;
    for ordinal in chosen {
        let atom = &graph.atoms[*ordinal];
        let (stream_index, _) = crate::place_object::written_at(program, operations, atom)?;
        match only {
            None => only = Some(stream_index),
            Some(had) if had == stream_index => {}
            Some(_) => return Err(SpikeError::SelectionSpansSeveralStreams),
        }
    }
    only.ok_or_else(|| refused("nothing was chosen to move in the order"))
}

fn written_in(atom: &pdf_paint::PaintAtom, source: &pdf_bytes::ByteStore) -> Option<usize> {
    let span = atom.id.invocation_path.first().map_or_else(
        || atom.id.operator_span,
        |invocation| invocation.operator_span,
    );
    (span.source() == source.id()).then(|| span.start())
}

fn units_of(
    source: &pdf_bytes::ByteStore,
    operations: &[pdf_content::Operation],
    graph: &PaintGraph,
    length: usize,
) -> Result<Vec<Unit>, SpikeError> {
    let mut groups: Vec<Range<usize>> = Vec::new();
    let mut depth = 0_usize;
    let mut opened = 0_usize;
    for operation in operations {
        let operator = operation
            .operator_bytes(source)
            .map_err(|_| refused("this page's instructions cannot be read back"))?;
        match operator {
            b"q" => {
                if depth == 0 {
                    opened = operation.span().start();
                }
                depth += 1;
            }
            b"Q" => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| refused("this page closes a group it never opened"))?;
                if depth == 0 {
                    let end = operation.span().end();
                    if opened >= end || end > length {
                        return Err(refused("this page's groups are not where it says they are"));
                    }
                    groups.push(opened..end);
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err(refused("this page opens a group it never closes"));
    }
    let mut units: Vec<Unit> = groups
        .into_iter()
        .map(|bytes| Unit {
            bytes,
            atoms: Vec::new(),
        })
        .collect();
    for (ordinal, atom) in graph.atoms.iter().enumerate() {
        let Some(at) = written_in(atom, source) else {
            continue;
        };
        if let Some(unit) = units
            .iter_mut()
            .find(|unit| unit.bytes.start <= at && at < unit.bytes.end)
        {
            unit.atoms.push(ordinal);
        }
    }
    units.retain(|unit| !unit.atoms.is_empty());
    Ok(units)
}

fn written(decoded: &[u8], units: &[Unit], order: &[usize]) -> Vec<u8> {
    let mut out = Vec::with_capacity(decoded.len() + units.len() * 2);
    let mut at = 0;
    for (now, was) in order.iter().enumerate() {
        out.extend_from_slice(&decoded[at..units[now].bytes.start]);
        out.push(b'\n');
        out.extend_from_slice(&decoded[units[*was].bytes.clone()]);
        out.push(b'\n');
        at = units[now].bytes.end;
    }
    out.extend_from_slice(&decoded[at..]);
    out
}

fn expected_atoms(
    graph: &PaintGraph,
    units: &[Unit],
    order: &[usize],
    stream_index: usize,
    program: &pdf_content::PageProgram,
) -> Result<Vec<usize>, SpikeError> {
    let source = &program.streams[stream_index].bytes;
    let mine: Vec<usize> = graph
        .atoms
        .iter()
        .enumerate()
        .filter(|(_, atom)| written_in(atom, source).is_some())
        .map(|(ordinal, _)| ordinal)
        .collect();
    let (first, last) = match (mine.first(), mine.last()) {
        (Some(first), Some(last)) => (*first, *last),
        _ => return Err(refused("nothing on this page is painted from this stream")),
    };
    if mine.len() != last - first + 1 {
        return Err(refused(
            "this page's instructions and what it paints are not in the same order",
        ));
    }
    let loose = |range: Range<usize>| -> Vec<usize> {
        mine.iter()
            .copied()
            .filter(|ordinal| {
                written_in(&graph.atoms[*ordinal], source)
                    .is_some_and(|at| range.start <= at && at < range.end)
            })
            .filter(|ordinal| !units.iter().any(|unit| unit.atoms.contains(ordinal)))
            .collect()
    };
    let mut sequence: Vec<usize> = (0..first).collect();
    let mut at = 0;
    for (now, was) in order.iter().enumerate() {
        sequence.extend(loose(at..units[now].bytes.start));
        sequence.extend(units[*was].atoms.iter().copied());
        at = units[now].bytes.end;
    }
    sequence.extend(loose(at..usize::MAX));
    sequence.extend((last + 1)..graph.atoms.len());
    if sequence.len() != graph.atoms.len() {
        return Err(refused(
            "this page's groups do not account for everything it paints",
        ));
    }
    Ok(sequence)
}

fn prove_reordered(
    before: &PaintGraph,
    after: &PaintGraph,
    expected: &[usize],
) -> Result<(), SpikeError> {
    if after.atoms.len() != expected.len() {
        return Err(SpikeError::MoveNotIsolated);
    }
    for (now, was) in after.atoms.iter().zip(expected) {
        let then = &before.atoms[*was];
        match (&then.kind, &now.kind) {
            (PaintAtomKind::Text(then), PaintAtomKind::Text(now)) => {
                crate::place_text::prove_run_placed(then, now, Matrix::IDENTITY)?;
            }
            _ => {
                if pdf_paint::paint_signature(&then.kind) != pdf_paint::paint_signature(&now.kind) {
                    return Err(SpikeError::MoveNotIsolated);
                }
            }
        }
    }
    Ok(())
}

fn correspondence(
    graph: &PaintGraph,
    expected: &[usize],
) -> BTreeMap<pdf_semantics::ClusterKey, pdf_semantics::ClusterKey> {
    let mut mapping = BTreeMap::new();
    for (now, was) in expected.iter().enumerate() {
        let PaintAtomKind::Text(text) = &graph.atoms[*was].kind else {
            continue;
        };
        for glyph in 0..text.glyphs.len().max(1) {
            mapping.insert(
                pdf_semantics::ClusterKey { atom: *was, glyph },
                pdf_semantics::ClusterKey { atom: now, glyph },
            );
        }
    }
    mapping
}

fn placed_by(kind: &PaintAtomKind) -> Matrix {
    match kind {
        PaintAtomKind::Text(text) => text.matrices.text.value,
        PaintAtomKind::Image(image) => image.state.ctm.value,
        PaintAtomKind::Shading(shading) => shading.state.ctm.value,
        PaintAtomKind::TransparencyGroup(group) => group.state.ctm.value,
        PaintAtomKind::Path(path) => path.state.ctm.value,
    }
}

fn region_of(graph: &PaintGraph, moved: &[usize]) -> Option<[f64; 4]> {
    let mut region: Option<[f64; 4]> = None;
    for ordinal in moved {
        let bounds = graph.atoms[*ordinal].kind.user_bounds()?;
        region = Some(match region {
            None => bounds,
            Some(had) => [
                had[0].min(bounds[0]),
                had[1].min(bounds[1]),
                had[2].max(bounds[2]),
                had[3].max(bounds[3]),
            ],
        });
    }
    region
}

#[cfg(test)]
mod tests {
    use pdf_bytes::ByteStore;
    use pdf_paint::{PaintAtomKind, PaintGraph};

    use super::plan_reorder_objects;
    use crate::place_object::tests::{after, page_with, read};
    use crate::plan::{Capability, Plan, SourceAnchor};
    use crate::spike_move_text::SpikeError;
    use crate::stacking::Stacking;

    const THREE: &[u8] = b"q 40 0 0 30 60 40 cm /Im1 Do Q\nq 1 0 0 rg 50 30 40 30 re f Q\nq BT /F1 12 Tf 1 0 0 1 55 45 Tm (AB) Tj ET Q\n";

    fn painted(graph: &PaintGraph) -> Vec<&'static str> {
        graph
            .atoms
            .iter()
            .map(|atom| match &atom.kind {
                PaintAtomKind::Text(_) => "text",
                PaintAtomKind::Path(_) => "path",
                PaintAtomKind::Image(_) => "image",
                PaintAtomKind::Shading(_) => "shading",
                PaintAtomKind::TransparencyGroup(_) => "group",
            })
            .collect()
    }

    fn anchors(graph: &PaintGraph, wanted: &[&str]) -> Vec<SourceAnchor> {
        let order = painted(graph);
        wanted
            .iter()
            .map(|name| {
                let at = order
                    .iter()
                    .position(|had| had == name)
                    .unwrap_or_else(|| panic!("the fixture paints a {name}"));
                SourceAnchor::of(&graph.atoms[at].id)
            })
            .collect()
    }

    fn reorder(source: &ByteStore, wanted: &[&str], command: Stacking) -> Result<Plan, SpikeError> {
        let held = read(source);
        let targets = anchors(&held.graph, wanted);
        plan_reorder_objects(
            &held.program,
            &held.operations,
            &held.graph,
            0,
            &targets,
            command,
            None,
        )
    }

    fn order_after(source: &ByteStore, wanted: &[&str], command: Stacking) -> Vec<&'static str> {
        let plan = reorder(source, wanted, command).expect("the fixture can be reordered");
        painted(&read(&after(source, &plan)).graph)
    }

    #[test]
    fn the_page_starts_in_the_order_the_stream_writes_it() {
        let source = page_with(THREE);
        assert_eq!(painted(&read(&source).graph), ["image", "path", "text"]);
    }

    #[test]
    fn each_of_the_four_leaves_the_page_in_its_own_order() {
        let source = page_with(THREE);
        assert_eq!(
            order_after(&source, &["image"], Stacking::ToFront),
            ["path", "text", "image"]
        );
        assert_eq!(
            order_after(&source, &["image"], Stacking::Forward),
            ["path", "image", "text"]
        );
        assert_eq!(
            order_after(&source, &["text"], Stacking::Backward),
            ["image", "text", "path"]
        );
        assert_eq!(
            order_after(&source, &["text"], Stacking::ToBack),
            ["text", "image", "path"]
        );
    }

    #[test]
    fn forward_moves_one_place_and_to_front_moves_all_the_way() {
        let source = page_with(THREE);
        let one = order_after(&source, &["image"], Stacking::Forward);
        let all = order_after(&source, &["image"], Stacking::ToFront);
        assert_eq!(one, ["path", "image", "text"]);
        assert_eq!(all, ["path", "text", "image"]);
        assert_ne!(one, all, "a page of three tells the two commands apart");
    }

    #[test]
    fn sent_behind_and_brought_back_is_where_it_started() {
        let source = page_with(THREE);
        let was = painted(&read(&source).graph);
        let plan = reorder(&source, &["text"], Stacking::ToBack).expect("text goes behind");
        let behind = after(&source, &plan);
        assert_eq!(painted(&read(&behind).graph), ["text", "image", "path"]);
        let plan = reorder(&behind, &["text"], Stacking::ToFront).expect("text comes back");
        assert_eq!(painted(&read(&after(&behind, &plan)).graph), was);
    }

    #[test]
    fn two_chosen_together_go_together() {
        let source = page_with(THREE);
        assert_eq!(
            order_after(&source, &["image", "path"], Stacking::ToFront),
            ["text", "image", "path"]
        );
    }

    #[test]
    fn what_is_written_is_the_page_with_its_groups_in_another_order() {
        let source = page_with(THREE);
        let plan =
            reorder(&source, &["image"], Stacking::ToFront).expect("a picture goes in front");
        assert_eq!(plan.capability(), Capability::Exact);
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
        assert!(
            written.contains("q 40 0 0 30 60 40 cm /Im1 Do Q"),
            "{written}"
        );
        assert!(written.contains("50 30 40 30 re f"), "{written}");
        let picture = written.find("/Im1 Do").expect("the picture is still there");
        let drawing = written.find("re f").expect("the drawing is still there");
        assert!(picture > drawing, "the picture is written last: {written}");
    }

    #[test]
    fn an_order_that_would_change_nothing_is_refused_by_name() {
        let source = page_with(THREE);
        assert!(matches!(
            reorder(&source, &["text"], Stacking::ToFront),
            Err(SpikeError::StackingUnchanged)
        ));
        assert!(matches!(
            reorder(&source, &["image"], Stacking::ToBack),
            Err(SpikeError::StackingUnchanged)
        ));
    }

    #[test]
    fn something_painted_loose_in_the_stream_is_refused() {
        let source = page_with(
            b"BT /F1 12 Tf 1 0 0 1 20 100 Tm (AB) Tj ET\nq 40 0 0 30 60 40 cm /Im1 Do Q\nq 1 0 0 rg 50 30 40 30 re f Q\n",
        );
        let held = read(&source);
        let targets = anchors(&held.graph, &["text"]);
        assert!(matches!(
            plan_reorder_objects(
                &held.program,
                &held.operations,
                &held.graph,
                0,
                &targets,
                Stacking::ToFront,
                None,
            ),
            Err(SpikeError::StackingUnsupported(_))
        ));
    }

    #[test]
    fn one_thing_alone_has_nothing_to_be_in_front_of() {
        let source = page_with(b"q 40 0 0 30 60 40 cm /Im1 Do Q\n");
        assert!(matches!(
            reorder(&source, &["image"], Stacking::ToFront),
            Err(SpikeError::StackingUnsupported(_))
        ));
    }

    #[test]
    fn an_anchor_naming_nothing_is_refused_rather_than_guessed_at() {
        let source = page_with(THREE);
        let held = read(&source);
        let anchor = SourceAnchor {
            stream: held.program.streams[0].reference,
            operator_offset: 9_999,
            invocation_path: Vec::new(),
        };
        assert!(matches!(
            plan_reorder_objects(
                &held.program,
                &held.operations,
                &held.graph,
                0,
                &[anchor],
                Stacking::ToFront,
                None,
            ),
            Err(SpikeError::AnchorNamesNothing)
        ));
    }

    #[test]
    fn choosing_nothing_is_refused() {
        let source = page_with(THREE);
        let held = read(&source);
        assert!(matches!(
            plan_reorder_objects(
                &held.program,
                &held.operations,
                &held.graph,
                0,
                &[],
                Stacking::ToFront,
                None,
            ),
            Err(SpikeError::StackingUnsupported(_))
        ));
    }

    #[test]
    fn a_group_that_would_paint_differently_where_it_lands_is_refused() {
        let source = page_with(b"q 10 10 20 20 re f Q\n0 0 1 rg\nq 50 30 40 30 re f Q\n");
        let held = read(&source);
        let targets = vec![SourceAnchor::of(&held.graph.atoms[1].id)];
        assert!(matches!(
            plan_reorder_objects(
                &held.program,
                &held.operations,
                &held.graph,
                0,
                &targets,
                Stacking::ToBack,
                None,
            ),
            Err(SpikeError::MoveNotIsolated)
        ));
    }
}
