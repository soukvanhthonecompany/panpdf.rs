use std::collections::BTreeSet;

use pdf_paint::{Matrix, PaintAtomKind, PaintGraph};

use crate::block_move::{BlockRewrite, TextInsertions};
use crate::place_object::Placement;
use crate::plan::{Capability, Effect, MovedRun, Plan, PlannedBody, PlannedWrite, SourceAnchor};
use crate::spike_move_text::SpikeError;

pub(crate) fn plan_group_move(
    (program, operations, graph): (
        &pdf_content::PageProgram,
        &[Vec<pdf_content::Operation>],
        &PaintGraph,
    ),
    page_index: usize,
    (runs, objects): (&[SourceAnchor], &[SourceAnchor]),
    (dx, dy): (f64, f64),
    fonts: crate::Fonts<'_>,
) -> Result<Plan, SpikeError> {
    let gathered = gather_insertions((program, operations, graph), (runs, objects), (dx, dy))?;
    let rewritten = crate::spike_move_text::interpret_with_insertions_of(
        program,
        gathered.stream_index,
        &gathered.insertions,
        fonts,
    )?;
    prove_group_move(
        graph,
        &rewritten,
        &gathered.named,
        &gathered.placed,
        (dx, dy),
    )?;

    let decoded = program.streams[gathered.stream_index].bytes.as_bytes();
    let mut edited = Vec::with_capacity(decoded.len() + 32 * gathered.insertions.len());
    let mut cursor = 0_usize;
    for (at, bytes) in &gathered.insertions {
        edited.extend_from_slice(&decoded[cursor..*at]);
        edited.extend_from_slice(bytes);
        cursor = *at;
    }
    edited.extend_from_slice(&decoded[cursor..]);

    let stream_reference = program.streams[gathered.stream_index].reference;
    Ok(Plan::new(
        if gathered.cancelled {
            Capability::Normalized
        } else {
            Capability::Exact
        },
        vec![PlannedWrite {
            reference: stream_reference,
            body: PlannedBody::ReplacedStream { decoded: edited },
        }],
        group_effect(
            graph,
            &rewritten,
            &gathered,
            (page_index, stream_reference),
            (dx, dy),
        ),
    ))
}

struct Gathered {
    stream_index: usize,
    insertions: Vec<(usize, Vec<u8>)>,
    named: BTreeSet<usize>,
    cancelled: bool,
    placed: Vec<Placement>,
}

fn gather_insertions(
    (program, operations, graph): (
        &pdf_content::PageProgram,
        &[Vec<pdf_content::Operation>],
        &PaintGraph,
    ),
    (runs, objects): (&[SourceAnchor], &[SourceAnchor]),
    (dx, dy): (f64, f64),
) -> Result<Gathered, SpikeError> {
    if runs.is_empty() && objects.is_empty() {
        return Err(SpikeError::BlockNamesNoRun);
    }
    let offset = Matrix {
        e: dx,
        f: dy,
        ..Matrix::IDENTITY
    };
    let mut stream_index: Option<usize> = None;
    let mut same_stream = |index: usize| match stream_index {
        None => {
            stream_index = Some(index);
            Ok(())
        }
        Some(had) if had == index => Ok(()),
        Some(_) => Err(SpikeError::BlockSpansSeveralStreams),
    };

    let mut insertions: Vec<(usize, bool, Vec<u8>)> = Vec::new();
    let mut named = BTreeSet::new();
    let mut cancelled = false;
    if !runs.is_empty() {
        let TextInsertions {
            named: text,
            stream_index: index,
            rewrite,
            ..
        } = crate::block_move::text_insertions(program, operations, graph, runs, (dx, dy))?;
        same_stream(index)?;
        let BlockRewrite {
            insertions: text_insertions,
            cancelled: restated,
        } = rewrite;
        insertions.extend(
            text_insertions
                .into_iter()
                .map(|(at, bytes)| (at, false, bytes)),
        );
        named = text;
        cancelled = restated;
    }
    let mut placed: Vec<Placement> = Vec::with_capacity(objects.len());
    for anchor in objects {
        let placement =
            crate::place_object::placement_insertions(program, operations, graph, anchor, offset)?;
        same_stream(placement.stream_index)?;
        if named.contains(&placement.ordinal)
            || placed
                .iter()
                .any(|other| other.ordinal == placement.ordinal)
        {
            return Err(SpikeError::MoveNotIsolated);
        }
        insertions.extend(
            placement
                .insertions
                .iter()
                .map(|(at, bytes)| (*at, bytes.starts_with(b" Q"), bytes.clone())),
        );
        placed.push(placement);
    }
    let stream_index = stream_index.ok_or(SpikeError::BlockNamesNoRun)?;
    insertions.sort_by_key(|(at, closes, _)| (*at, !*closes));
    Ok(Gathered {
        stream_index,
        insertions: insertions
            .into_iter()
            .map(|(at, _, bytes)| (at, bytes))
            .collect(),
        named,
        cancelled,
        placed,
    })
}

fn group_effect(
    graph: &PaintGraph,
    rewritten: &PaintGraph,
    gathered: &Gathered,
    (page_index, stream_reference): (usize, pdf_syntax::Reference),
    (dx, dy): (f64, f64),
) -> Effect {
    let mut moved: Vec<MovedRun> = gathered
        .named
        .iter()
        .map(|ordinal| {
            let atom = &graph.atoms[*ordinal];
            MovedRun {
                anchor: SourceAnchor::of(&atom.id),
                atom_ordinal: *ordinal,
                original_matrix: match &atom.kind {
                    PaintAtomKind::Text(text) => text.matrices.text.value,
                    _ => Matrix::IDENTITY,
                },
            }
        })
        .collect();
    moved.extend(gathered.placed.iter().map(|placement| MovedRun {
        anchor: SourceAnchor::of(&graph.atoms[placement.ordinal].id),
        atom_ordinal: placement.ordinal,
        original_matrix: placement.ctm,
    }));
    moved.sort_by_key(|run| run.atom_ordinal);

    let mut region = crate::block_move::declared_region(graph, &gathered.named, dx, dy);
    let mut bounded = true;
    for placement in &gathered.placed {
        match crate::place_object::declared_region(
            &graph.atoms[placement.ordinal].kind,
            rewritten.atoms.get(placement.ordinal),
        ) {
            Some(extent) => region = Some(region.map_or(extent, |had| union(had, extent))),
            None => bounded = false,
        }
    }
    Effect {
        page_index,
        moved,
        target_stream: stream_reference,
        declared_region: region.filter(|_| bounded),
    }
}

fn prove_group_move(
    before: &PaintGraph,
    after: &PaintGraph,
    named: &BTreeSet<usize>,
    placed: &[Placement],
    (dx, dy): (f64, f64),
) -> Result<(), SpikeError> {
    if before.atoms.len() != after.atoms.len() {
        return Err(SpikeError::MoveNotIsolated);
    }
    for (ordinal, (one, other)) in before.atoms.iter().zip(&after.atoms).enumerate() {
        if let Some(placement) = placed.iter().find(|placement| placement.ordinal == ordinal) {
            crate::place_object::prove_placed(
                &one.kind,
                Some(other),
                (placement.wanted, placement.ctm, placement.travelling_from),
            )?;
            continue;
        }
        if named.contains(&ordinal) {
            let (PaintAtomKind::Text(was), PaintAtomKind::Text(now)) = (&one.kind, &other.kind)
            else {
                return Err(SpikeError::MoveNotIsolated);
            };
            crate::block_move::prove_run_displaced(was, now, dx, dy)?;
            continue;
        }
        crate::place_object::prove_unchanged(&one.kind, &other.kind)?;
    }
    Ok(())
}

fn union(one: [f64; 4], other: [f64; 4]) -> [f64; 4] {
    [
        one[0].min(other[0]),
        one[1].min(other[1]),
        one[2].max(other[2]),
        one[3].max(other[3]),
    ]
}

#[cfg(test)]
mod tests {
    use pdf_paint::{Matrix, PaintAtomKind, PaintGraph};

    use crate::place_object::tests::{after, anchor_of, glyphs, page_with, read};
    use crate::plan::{Capability, Command, Plan, SourceAnchor};
    use crate::spike_move_text::{PlannerPage, SpikeError, plan_command_in};

    fn path_ctm(graph: &PaintGraph) -> Matrix {
        graph
            .atoms
            .iter()
            .find_map(|atom| match &atom.kind {
                PaintAtomKind::Path(path) => Some(path.state.ctm.value),
                _ => None,
            })
            .expect("the page paints a rule")
    }

    fn plan_group(
        source: &pdf_bytes::ByteStore,
        runs: Vec<SourceAnchor>,
        objects: Vec<SourceAnchor>,
    ) -> Result<Plan, SpikeError> {
        let page = read(source);
        plan_command_in(
            source,
            PlannerPage {
                program: &page.program,
                operations: &page.operations,
                graph: &page.graph,
                fonts: None,
                restrictions: crate::Restrictions::Respect,
                credential: b"",
            },
            &Command::MoveGroup {
                page_index: 0,
                runs,
                objects,
                dx: 5.0,
                dy: 7.0,
            },
        )
    }

    #[test]
    fn text_and_a_drawn_rule_move_together_as_one_plan() {
        let source = page_with(b"BT /F1 12 Tf 1 0 0 1 20 100 Tm (AB) Tj ET\n20 90 30 2 re f\n");
        let before = read(&source);
        let text = anchor_of(&before.graph, "text");
        let rule = anchor_of(&before.graph, "path");

        let plan = plan_group(&source, vec![text], vec![rule.clone()]).expect("the group moves");
        assert_eq!(plan.capability(), Capability::Exact);
        let moved = read(&after(&source, &plan));
        let shifted: Vec<(f64, f64)> = glyphs(&before.graph)
            .iter()
            .map(|point| (point.x + 5.0, point.y + 7.0))
            .collect();
        let landed: Vec<(f64, f64)> = glyphs(&moved.graph)
            .iter()
            .map(|point| (point.x, point.y))
            .collect();
        assert_eq!(shifted.len(), landed.len());
        for (want, got) in shifted.iter().zip(&landed) {
            assert!(
                (want.0 - got.0).abs() < 1e-6 && (want.1 - got.1).abs() < 1e-6,
                "{landed:?}"
            );
        }
        let ctm = path_ctm(&moved.graph);
        assert!(
            (ctm.e - 5.0).abs() < 1e-9 && (ctm.f - 7.0).abs() < 1e-9,
            "{ctm:?}"
        );

        let alone = plan_group(&source, Vec::new(), vec![rule]).expect("the rule moves alone");
        let only = read(&after(&source, &alone));
        assert_eq!(glyphs(&before.graph), glyphs(&only.graph));
        let ctm = path_ctm(&only.graph);
        assert!((ctm.e - 5.0).abs() < 1e-9, "{ctm:?}");
    }
}
