use std::collections::BTreeSet;

use pdf_paint::{Matrix, PaintAtomKind, PaintGraph, Point, TextShowPaint};

use crate::block_move::PLACEMENT_TOLERANCE;
use crate::place_object::{alike, finite};
use crate::plan::{
    Capability, Effect, FixedPoint, MovedRun, Plan, PlannedBody, PlannedWrite, SourceAnchor,
};
use crate::spike_move_text::SpikeError;

#[expect(
    clippy::too_many_arguments,
    reason = "one edit's inputs, the font context among them, threaded as its neighbours are"
)]
pub(crate) fn plan_place_text(
    program: &pdf_content::PageProgram,
    operations: &[Vec<pdf_content::Operation>],
    graph: &PaintGraph,
    page_index: usize,
    runs: &[SourceAnchor],
    transform: Matrix,
    about: FixedPoint,
    fonts: crate::Fonts<'_>,
) -> Result<Plan, SpikeError> {
    let (named, stream_reference) = crate::block_move::resolve(graph, runs)?;
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

    let wanted = about.applied_to(transform);
    if wanted.inverse().is_none() || !finite(wanted) {
        return Err(SpikeError::PlacementNotInvertible);
    }

    for ordinal in &named {
        let PaintAtomKind::Text(text) = &graph.atoms[*ordinal].kind else {
            continue;
        };
        clip_admits(text, wanted)?;
    }

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
    let crate::block_move::BlockRewrite {
        insertions,
        cancelled,
    } = insertions_for(graph, &named, wanted, &offset_of)?;
    if insertions.is_empty() {
        return Err(SpikeError::BlockNamesNoRun);
    }

    let decoded = program.streams[stream_index].bytes.as_bytes();
    let mut edited = Vec::with_capacity(decoded.len() + 64 * insertions.len());
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
    prove_text_placement(graph, &rewritten, &named, wanted)?;

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
            declared_region: declared_region(graph, &named, wanted),
        },
    ))
}

fn insertions_for(
    graph: &PaintGraph,
    named: &BTreeSet<usize>,
    wanted: Matrix,
    operator_offset: &dyn Fn(usize) -> Option<usize>,
) -> Result<crate::block_move::BlockRewrite, SpikeError> {
    let mut insertions: Vec<(usize, Vec<u8>)> = Vec::new();
    let mut cancelled = false;
    for chain in crate::block_move::chains_of(graph).into_values() {
        if !chain.atoms.iter().any(|ordinal| named.contains(ordinal)) {
            continue;
        }
        let mut carried = Matrix::IDENTITY;
        for ordinal in chain.atoms {
            let PaintAtomKind::Text(text) = &graph.atoms[ordinal].kind else {
                continue;
            };
            let want = if named.contains(&ordinal) {
                delta_for(text, wanted)?
            } else {
                Matrix::IDENTITY
            };
            if want == carried {
                continue;
            }
            if text.matrices.text.value != text.matrices.line.value {
                return Err(SpikeError::BlockNotChainAligned);
            }
            let placement = want.multiply(text.matrices.line.value);
            if !finite(placement) {
                return Err(SpikeError::PlacementNotInvertible);
            }
            let at = operator_offset(ordinal).ok_or(SpikeError::BlockRunNotInTargetStream)?;
            insertions.push((
                at,
                format!(
                    " {} {} {} {} {} {} Tm ",
                    placement.a, placement.b, placement.c, placement.d, placement.e, placement.f
                )
                .into_bytes(),
            ));
            if !named.contains(&ordinal) {
                cancelled = true;
            }
            carried = want;
        }
    }
    insertions.sort_by_key(|(at, _)| *at);
    Ok(crate::block_move::BlockRewrite {
        insertions,
        cancelled,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "one edit's inputs, the font context among them, threaded as its neighbours are"
)]
pub(crate) fn plan_set_text_shape(
    program: &pdf_content::PageProgram,
    operations: &[Vec<pdf_content::Operation>],
    graph: &PaintGraph,
    page_index: usize,
    runs: &[SourceAnchor],
    turn: Option<f64>,
    slant: Option<f64>,
    fonts: crate::Fonts<'_>,
) -> Result<Plan, SpikeError> {
    if turn.is_none() && slant.is_none() {
        return Err(SpikeError::BlockNamesNoRun);
    }
    if [turn, slant]
        .iter()
        .flatten()
        .any(|angle| !angle.is_finite())
    {
        return Err(SpikeError::PlacementNotInvertible);
    }
    let (named, _) = crate::block_move::resolve(graph, runs)?;
    let seed = named
        .iter()
        .find_map(|ordinal| match &graph.atoms[*ordinal].kind {
            PaintAtomKind::Text(text) => Some(text),
            _ => None,
        })
        .ok_or(SpikeError::BlockNamesNoRun)?;
    let placement = seed.state.ctm.value.multiply(seed.matrices.text.value);
    let shape = placement.shape().ok_or(SpikeError::ObjectCtmSingular)?;
    let wanted = pdf_paint::Shape {
        turn: turn.unwrap_or(shape.turn),
        slant: slant.unwrap_or(shape.slant),
        ..shape
    };
    let transform = wanted.matrix().multiply(
        linear(placement)
            .inverse()
            .ok_or(SpikeError::ObjectCtmSingular)?,
    );
    if !finite(transform) {
        return Err(SpikeError::PlacementNotInvertible);
    }
    let about = middle_of(graph, &named).map_or(FixedPoint::Origin, FixedPoint::At);
    plan_place_text(
        program, operations, graph, page_index, runs, transform, about, fonts,
    )
}

fn linear(matrix: Matrix) -> Matrix {
    Matrix {
        e: 0.0,
        f: 0.0,
        ..matrix
    }
}

fn middle_of(graph: &PaintGraph, named: &BTreeSet<usize>) -> Option<Point> {
    let mut box_of: Option<[f64; 4]> = None;
    for ordinal in named {
        let PaintAtomKind::Text(text) = &graph.atoms.get(*ordinal)?.kind else {
            continue;
        };
        let Some(bounds) = text.outline_bounds() else {
            continue;
        };
        box_of = Some(match box_of {
            None => bounds,
            Some(had) => [
                had[0].min(bounds[0]),
                had[1].min(bounds[1]),
                had[2].max(bounds[2]),
                had[3].max(bounds[3]),
            ],
        });
    }
    box_of.map(|at| Point {
        x: f64::midpoint(at[0], at[2]),
        y: f64::midpoint(at[1], at[3]),
    })
}

fn delta_for(text: &TextShowPaint, wanted: Matrix) -> Result<Matrix, SpikeError> {
    let ctm = text.state.ctm.value;
    let delta = ctm
        .inverse()
        .ok_or(SpikeError::ObjectCtmSingular)?
        .multiply(wanted)
        .multiply(ctm);
    if finite(delta) {
        Ok(delta)
    } else {
        Err(SpikeError::PlacementNotInvertible)
    }
}

fn clip_admits(text: &TextShowPaint, wanted: Matrix) -> Result<(), SpikeError> {
    if text.state.clip_paths.is_empty() {
        return Ok(());
    }
    let Some(points) = text.outline_points() else {
        return if text.draws_no_ink() {
            Ok(())
        } else {
            Err(SpikeError::RunExtentUnknown)
        };
    };
    for clip in &text.state.clip_paths {
        let region = crate::clip_region::ClipRegion::of(&clip.path, clip.ctm.value)?;
        let inside = |at: &dyn Fn(Point) -> Point| {
            let moved: Vec<Point> = points.iter().map(|point| at(*point)).collect();
            region.admits(&moved)
        };
        if !inside(&|point| point) {
            return Err(SpikeError::ObjectIsCropped);
        }
        if !inside(&|point| wanted.transform(point)) {
            return Err(SpikeError::ObjectLeavesClip);
        }
    }
    Ok(())
}

fn declared_region(
    graph: &PaintGraph,
    named: &BTreeSet<usize>,
    wanted: Matrix,
) -> Option<[f64; 4]> {
    let mut region: Option<[f64; 4]> = None;
    for ordinal in named {
        let PaintAtomKind::Text(text) = &graph.atoms[*ordinal].kind else {
            continue;
        };
        let Some(points) = text.outline_points() else {
            continue;
        };
        for point in points {
            for at in [point, wanted.transform(point)] {
                let box_of = [
                    at.x - PLACEMENT_TOLERANCE,
                    at.y - PLACEMENT_TOLERANCE,
                    at.x + PLACEMENT_TOLERANCE,
                    at.y + PLACEMENT_TOLERANCE,
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
        }
    }
    region
}

fn prove_text_placement(
    before: &PaintGraph,
    after: &PaintGraph,
    named: &BTreeSet<usize>,
    wanted: Matrix,
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
        let expected = if named.contains(&ordinal) {
            wanted
        } else {
            Matrix::IDENTITY
        };
        prove_run_placed(one, other, expected)?;
    }
    Ok(())
}

pub(crate) fn prove_run_placed(
    one: &TextShowPaint,
    other: &TextShowPaint,
    wanted: Matrix,
) -> Result<(), SpikeError> {
    if one.glyphs.len() != other.glyphs.len() {
        return Err(SpikeError::MoveNotIsolated);
    }
    if one.state.text.rendering_mode.value != other.state.text.rendering_mode.value
        || one.state.ctm.value != other.state.ctm.value
        || pdf_paint::colour_signature(&one.state.fill_color.value)
            != pdf_paint::colour_signature(&other.state.fill_color.value)
        || pdf_paint::colour_signature(&one.state.stroke_color.value)
            != pdf_paint::colour_signature(&other.state.stroke_color.value)
    {
        return Err(SpikeError::MoveNotIsolated);
    }
    for (a, b) in one.glyphs.iter().zip(&other.glyphs) {
        if a.code.value != b.code.value || a.code.cid != b.code.cid || a.glyph != b.glyph {
            return Err(SpikeError::MoveNotIsolated);
        }
        let was = one.state.ctm.value.multiply(a.matrix);
        let now = other.state.ctm.value.multiply(b.matrix);
        if !alike(now, wanted.multiply(was)) {
            return Err(SpikeError::MoveNotIsolated);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use pdf_bytes::ByteStore;
    use pdf_paint::{Matrix, PaintAtomKind, PaintGraph, Point};

    use super::{plan_place_text, plan_set_text_shape, prove_text_placement};
    use crate::block_move::tests::{after, anchors, page_with, read, written};
    use crate::plan::{Capability, FixedPoint, Plan, SourceAnchor};
    use crate::spike_move_text::SpikeError;

    fn turn(degrees: f64) -> Matrix {
        let (sin, cos) = degrees.to_radians().sin_cos();
        Matrix {
            a: cos,
            b: sin,
            c: -sin,
            d: cos,
            e: 0.0,
            f: 0.0,
        }
    }

    fn scale(factor: f64) -> Matrix {
        Matrix {
            a: factor,
            d: factor,
            ..Matrix::IDENTITY
        }
    }

    fn place(
        source: &ByteStore,
        runs: &[usize],
        transform: Matrix,
        about: FixedPoint,
    ) -> Result<Plan, SpikeError> {
        let page = read(source);
        let all = anchors(&page.graph);
        let named: Vec<SourceAnchor> = runs.iter().map(|index| all[*index].clone()).collect();
        plan_place_text(
            &page.program,
            &page.operations,
            &page.graph,
            0,
            &named,
            transform,
            about,
            None,
        )
    }

    fn placements(graph: &PaintGraph) -> Vec<Matrix> {
        let mut found = Vec::new();
        for atom in &graph.atoms {
            let PaintAtomKind::Text(text) = &atom.kind else {
                continue;
            };
            for glyph in &text.glyphs {
                found.push(text.state.ctm.value.multiply(glyph.matrix));
            }
        }
        found
    }

    fn a_level_line_and_a_turned_one() -> ByteStore {
        let (sin, cos) = 15.0_f64.to_radians().sin_cos();
        page_with(
            format!(
                "BT /F1 24 Tf 1 0 0 1 20 150 Tm (AAA) Tj ET\n\
                 BT /F1 24 Tf {cos} {sin} {} {cos} 20 60 Tm (AAA) Tj ET",
                -sin
            )
            .as_bytes(),
        )
    }

    #[test]
    fn the_line_set_at_an_angle_is_turned_back_to_level() {
        let source = a_level_line_and_a_turned_one();
        let before = read(&source).graph;
        let leaning = placements(&before);
        assert!(leaning[0].b.abs() < 1e-12, "the level line is level");
        assert!(
            leaning[3].b > 0.2,
            "the turned line leans: {:?}",
            leaning[3]
        );

        let held = Point { x: 20.0, y: 60.0 };
        let plan = place(&source, &[1], turn(-15.0), FixedPoint::At(held))
            .expect("a turned line turns back");
        let placed = placements(&after(&plan, &source));

        for (index, matrix) in placed.iter().enumerate() {
            assert!(
                matrix.b.abs() < 1e-9 && matrix.c.abs() < 1e-9,
                "glyph {index} is still leaning: {matrix:?}"
            );
        }
        for (had, now) in leaning.iter().take(3).zip(&placed) {
            assert!((had.e - now.e).abs() < 1e-9 && (had.f - now.f).abs() < 1e-9);
        }
        assert!((placed[3].e - 20.0).abs() < 1e-9 && (placed[3].f - 60.0).abs() < 1e-9);
        assert!((placed[3].a - 24.0).abs() < 1e-9, "{:?}", placed[3]);
    }

    #[test]
    fn the_placement_is_written_as_one_tm_and_removes_no_byte() {
        let source = a_level_line_and_a_turned_one();
        let plan = place(&source, &[1], turn(-15.0), FixedPoint::Origin).expect("it plans");
        let bytes = written(&plan);
        let text = String::from_utf8(bytes.clone()).expect("the stream is text");
        assert_eq!(text.matches(" Tm ").count(), 3, "two written, one inserted");
        assert_eq!(plan.capability(), Capability::Exact);
        let original = read(&source).program.streams[0].bytes.as_bytes().to_vec();
        let mut kept = bytes.iter().copied();
        for byte in original {
            assert!(
                kept.any(|had| had == byte),
                "a byte of the original is gone"
            );
        }
    }

    #[test]
    fn one_instruction_places_every_run_standing_on_one_placement() {
        let source = page_with(
            b"BT /F1 10 Tf 1 0 0 1 20 150 Tm (AA) Tj 0 -12 Td (AB) Tj 0 -12 Td (AC) Tj ET",
        );
        let plan = place(&source, &[0, 1, 2], scale(2.0), FixedPoint::Origin).expect("it plans");
        let text = String::from_utf8(written(&plan)).expect("the stream is text");
        assert_eq!(
            text.matches(" Tm ").count(),
            2,
            "one written, one inserted: {text}"
        );
        let placed = placements(&after(&plan, &source));
        let before = placements(&read(&source).graph);
        assert_eq!(placed.len(), before.len());
        for (had, now) in before.iter().zip(&placed) {
            assert!((now.e - had.e * 2.0).abs() < 1e-9 && (now.f - had.f * 2.0).abs() < 1e-9);
            assert!((now.a - had.a * 2.0).abs() < 1e-9, "the glyphs grew too");
        }
    }

    #[test]
    fn a_run_the_block_does_not_name_is_put_back_and_the_plan_declares_it() {
        let source = page_with(
            b"BT /F1 10 Tf 1 0 0 1 20 150 Tm (AA) Tj 0 -12 Td (AB) Tj 0 -12 Td (AC) Tj ET",
        );
        let plan = place(&source, &[0], scale(2.0), FixedPoint::Origin).expect("it plans");
        assert_eq!(plan.capability(), Capability::Normalized);
        let before = placements(&read(&source).graph);
        let placed = placements(&after(&plan, &source));
        for (had, now) in before.iter().zip(&placed).take(2) {
            assert!((now.e - had.e * 2.0).abs() < 1e-9 && (now.f - had.f * 2.0).abs() < 1e-9);
        }
        for (had, now) in before.iter().zip(&placed).skip(2) {
            assert!(
                (now.e - had.e).abs() < 1e-9 && (now.f - had.f).abs() < 1e-9,
                "a run the command did not name moved"
            );
        }
    }

    #[test]
    fn a_run_that_has_advanced_past_its_line_matrix_is_refused() {
        let source = page_with(b"BT /F1 10 Tf 1 0 0 1 20 150 Tm (AA) Tj (BB) Tj ET");
        assert!(matches!(
            place(&source, &[1], scale(2.0), FixedPoint::Origin),
            Err(SpikeError::BlockNotChainAligned)
        ));
    }

    #[test]
    fn a_placement_with_no_area_is_refused() {
        let source = a_level_line_and_a_turned_one();
        let flat = Matrix {
            a: 1.0,
            d: 0.0,
            ..Matrix::IDENTITY
        };
        assert!(matches!(
            place(&source, &[0], flat, FixedPoint::Origin),
            Err(SpikeError::PlacementNotInvertible)
        ));
    }

    #[test]
    fn a_block_that_names_nothing_is_refused() {
        let source = a_level_line_and_a_turned_one();
        assert!(matches!(
            place(&source, &[], turn(5.0), FixedPoint::Origin),
            Err(SpikeError::BlockNamesNoRun)
        ));
    }

    #[test]
    fn the_proof_can_tell_one_angle_from_another() {
        let source = a_level_line_and_a_turned_one();
        let plan = place(&source, &[1], turn(-15.0), FixedPoint::Origin).expect("it plans");
        let before = read(&source).graph;
        let placed = after(&plan, &source);
        let named = std::iter::once(
            before
                .atoms
                .iter()
                .enumerate()
                .filter(|(_, atom)| matches!(atom.kind, PaintAtomKind::Text(_)))
                .map(|(ordinal, _)| ordinal)
                .nth(1)
                .expect("two runs"),
        )
        .collect();
        prove_text_placement(&before, &placed, &named, turn(-15.0)).expect("the truth passes");
        assert!(matches!(
            prove_text_placement(&before, &placed, &named, turn(-14.9)),
            Err(SpikeError::MoveNotIsolated)
        ));
        assert!(matches!(
            prove_text_placement(&before, &placed, &named, Matrix::IDENTITY),
            Err(SpikeError::MoveNotIsolated)
        ));
    }

    fn shape_of(graph: &PaintGraph, which: usize) -> pdf_paint::Shape {
        let text = graph
            .atoms
            .iter()
            .filter_map(|atom| match &atom.kind {
                PaintAtomKind::Text(text) => Some(text),
                _ => None,
            })
            .nth(which)
            .expect("a run");
        text.state
            .ctm
            .value
            .multiply(text.matrices.text.value)
            .shape()
            .expect("it has an area")
    }

    fn set_shape(
        source: &ByteStore,
        runs: &[usize],
        turn: Option<f64>,
        slant: Option<f64>,
    ) -> Result<Plan, SpikeError> {
        let page = read(source);
        let all = anchors(&page.graph);
        let named: Vec<SourceAnchor> = runs.iter().map(|index| all[*index].clone()).collect();
        plan_set_text_shape(
            &page.program,
            &page.operations,
            &page.graph,
            0,
            &named,
            turn,
            slant,
            None,
        )
    }

    #[test]
    fn an_angle_typed_is_the_angle_that_reads_back() {
        let source = a_level_line_and_a_turned_one();
        assert!(shape_of(&read(&source).graph, 1).turn.to_degrees() - 15.0 < 1e-9);

        let plan = set_shape(&source, &[1], Some(0.0), None).expect("it plans");
        let placed = shape_of(&after(&plan, &source), 1);
        assert!(placed.turn.abs() < 1e-9, "{placed:?}");
        let was = shape_of(&read(&source).graph, 1);
        assert!((placed.across - was.across).abs() < 1e-9);
        assert!((placed.along - was.along).abs() < 1e-9);
        assert!((placed.slant - was.slant).abs() < 1e-9);

        for degrees in [30.0_f64, -12.5, 0.0, 180.0] {
            let plan =
                set_shape(&source, &[1], Some(degrees.to_radians()), None).expect("it plans");
            let placed = shape_of(&after(&plan, &source), 1);
            let round = (placed.turn.to_degrees() - degrees).rem_euclid(360.0);
            assert!(
                round < 1e-9 || (360.0 - round) < 1e-9,
                "{degrees} came back as {}",
                placed.turn.to_degrees()
            );
        }
    }

    #[test]
    fn a_slant_leans_the_strokes_and_leaves_the_rest_alone() {
        let source = a_level_line_and_a_turned_one();
        let was = shape_of(&read(&source).graph, 0);
        assert!(was.slant.abs() < 1e-12, "the level line stands upright");

        let plan = set_shape(&source, &[0], None, Some(12.0_f64.to_radians())).expect("it plans");
        let placed = shape_of(&after(&plan, &source), 0);
        assert!(
            (placed.slant.to_degrees() - 12.0).abs() < 1e-9,
            "{placed:?}"
        );
        assert!((placed.across - was.across).abs() < 1e-9, "the size held");
        assert!((placed.along - was.along).abs() < 1e-9, "the size held");
        assert!(placed.turn.abs() < 1e-9, "the baseline did not turn");
    }

    #[test]
    fn a_slant_on_a_turned_line_is_about_its_own_baseline() {
        let source = a_level_line_and_a_turned_one();
        let plan = set_shape(&source, &[1], None, Some(20.0_f64.to_radians())).expect("it plans");
        let placed = shape_of(&after(&plan, &source), 1);
        assert!(
            (placed.slant.to_degrees() - 20.0).abs() < 1e-9,
            "{placed:?}"
        );
        assert!(
            (placed.turn.to_degrees() - 15.0).abs() < 1e-9,
            "the line is still at fifteen degrees: {placed:?}"
        );
    }

    #[test]
    fn setting_neither_angle_is_refused() {
        let source = a_level_line_and_a_turned_one();
        assert!(set_shape(&source, &[0], None, None).is_err());
        assert!(set_shape(&source, &[0], Some(f64::NAN), None).is_err());
    }

    #[test]
    fn a_placement_that_leaves_a_clip_is_refused() {
        let source =
            page_with(b"q 10 140 120 30 re W n BT /F1 10 Tf 1 0 0 1 20 150 Tm (AA) Tj ET Q");
        assert!(matches!(
            place(&source, &[0], scale(4.0), FixedPoint::Origin),
            Err(SpikeError::ObjectLeavesClip)
        ));
        place(
            &source,
            &[0],
            turn(1.0),
            FixedPoint::At(Point { x: 20.0, y: 150.0 }),
        )
        .expect("a small turn stays inside");
    }
}
