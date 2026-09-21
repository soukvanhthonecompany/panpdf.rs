use std::collections::BTreeSet;

use pdf_paint::{Matrix, PaintAtomKind, PaintGraph, TextShowPaint};

use crate::block_move::PLACEMENT_TOLERANCE;
use crate::plan::{Capability, Effect, MovedRun, Plan, PlannedBody, PlannedWrite, SourceAnchor};
use crate::spike_move_text::SpikeError;

const SIZE_TOLERANCE: f64 = 1e-6;

pub(crate) fn plan_set_text_size(
    program: &pdf_content::PageProgram,
    operations: &[Vec<pdf_content::Operation>],
    graph: &PaintGraph,
    page_index: usize,
    runs: &[SourceAnchor],
    points: f64,
    fonts: crate::Fonts<'_>,
) -> Result<Plan, SpikeError> {
    if !points.is_finite() || points <= 0.0 {
        return Err(SpikeError::SizeNotUsable);
    }
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

    let mut wanted: Vec<Option<f64>> = vec![None; graph.atoms.len()];
    for ordinal in &named {
        let PaintAtomKind::Text(text) = &graph.atoms[*ordinal].kind else {
            continue;
        };
        wanted[*ordinal] = Some(tf_for(text, points)?);
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
    } = insertions_for(graph, &named, &wanted, &offset_of)?;
    if insertions.is_empty() {
        return Err(SpikeError::SizeAlreadySet);
    }

    let decoded = program.streams[stream_index].bytes.as_bytes();
    let mut edited = Vec::with_capacity(decoded.len() + 48 * insertions.len());
    let mut cursor = 0_usize;
    for (at, bytes) in &insertions {
        edited.extend_from_slice(&decoded[cursor..*at]);
        edited.extend_from_slice(bytes);
        cursor = *at;
    }
    edited.extend_from_slice(&decoded[cursor..]);

    let mut anchored = named.clone();
    for ordinal in &named {
        for (follower, _) in crate::spike_move_text::normalization_for(graph, *ordinal) {
            anchored.remove(&follower);
        }
    }

    let rewritten = crate::spike_move_text::interpret_with_insertions_of(
        program,
        stream_index,
        &insertions,
        fonts,
    )?;
    prove_size(graph, &rewritten, &named, &anchored, points)?;

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
            declared_region: declared_region(graph, &rewritten, &named),
        },
    ))
}

fn tf_for(text: &TextShowPaint, points: f64) -> Result<f64, SpikeError> {
    let shape = text.placed_shape().ok_or(SpikeError::ObjectCtmSingular)?;
    let size = points / shape.across;
    if size.is_finite() && size > 0.0 {
        Ok(size)
    } else {
        Err(SpikeError::SizeNotUsable)
    }
}

fn insertions_for(
    graph: &PaintGraph,
    named: &BTreeSet<usize>,
    wanted: &[Option<f64>],
    operator_offset: &dyn Fn(usize) -> Option<usize>,
) -> Result<crate::block_move::BlockRewrite, SpikeError> {
    let mut pin: std::collections::BTreeMap<usize, Matrix> = std::collections::BTreeMap::new();
    for ordinal in named {
        for (follower, matrix) in crate::spike_move_text::normalization_for(graph, *ordinal) {
            if !named.contains(&follower) {
                pin.insert(follower, matrix);
            }
        }
    }

    let mut insertions: Vec<(usize, Vec<u8>)> = Vec::new();
    let mut cancelled = false;
    let mut in_force: Option<(Vec<u8>, f64, pdf_paint::Provenance)> = None;
    for (ordinal, atom) in graph.atoms.iter().enumerate() {
        let PaintAtomKind::Text(text) = &atom.kind else {
            continue;
        };
        let font = text
            .state
            .text
            .font
            .as_ref()
            .ok_or(SpikeError::RunSelectsNoFont)?;
        let size = wanted[ordinal].unwrap_or(text.state.text.font_size.value);
        let selected = (font.value.name.clone(), size);
        let pinned = pin.get(&ordinal).copied();
        let showing = match &in_force {
            Some((name, size, displaced)) if *displaced == text.state.text.font_size.provenance => {
                (name.clone(), *size)
            }
            _ => (font.value.name.clone(), text.state.text.font_size.value),
        };
        let own = (font.value.name.clone(), text.state.text.font_size.value);
        let restate = selected != own || showing != selected;
        if !restate && pinned.is_none() {
            continue;
        }
        let mut bytes = Vec::new();
        if restate {
            if selected.0.first() != Some(&b'/') {
                return Err(SpikeError::RunSelectsNoFont);
            }
            bytes.push(b' ');
            bytes.extend_from_slice(&selected.0);
            bytes.extend_from_slice(format!(" {} Tf ", selected.1).as_bytes());
            in_force = Some((
                selected.0,
                selected.1,
                text.state.text.font_size.provenance.clone(),
            ));
        }
        if let Some(matrix) = pinned {
            if text.matrices.text.value != text.matrices.line.value
                && graph.atoms.iter().skip(ordinal + 1).any(|later| {
                    matches!(&later.kind, PaintAtomKind::Text(after)
                        if after.matrices.line.value == text.matrices.line.value
                            && after.matrices.text.value == after.matrices.line.value)
                })
            {
                return Err(SpikeError::BlockNotChainAligned);
            }
            bytes.extend_from_slice(&crate::spike_move_text::absolute_matrix(matrix));
            cancelled = true;
        }
        let at = operator_offset(ordinal).ok_or(SpikeError::BlockRunNotInTargetStream)?;
        insertions.push((at, bytes));
    }
    insertions.sort_by_key(|(at, _)| *at);
    Ok(crate::block_move::BlockRewrite {
        insertions,
        cancelled,
    })
}

fn declared_region(
    before: &PaintGraph,
    after: &PaintGraph,
    named: &BTreeSet<usize>,
) -> Option<[f64; 4]> {
    let mut region: Option<[f64; 4]> = None;
    for ordinal in named {
        for graph in [before, after] {
            let Some(PaintAtomKind::Text(text)) = graph.atoms.get(*ordinal).map(|atom| &atom.kind)
            else {
                continue;
            };
            let Some(bounds) = text.outline_bounds() else {
                continue;
            };
            let box_of = [
                bounds[0] - PLACEMENT_TOLERANCE,
                bounds[1] - PLACEMENT_TOLERANCE,
                bounds[2] + PLACEMENT_TOLERANCE,
                bounds[3] + PLACEMENT_TOLERANCE,
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
    region
}

fn prove_size(
    before: &PaintGraph,
    after: &PaintGraph,
    named: &BTreeSet<usize>,
    anchored: &BTreeSet<usize>,
    points: f64,
) -> Result<(), SpikeError> {
    if before.atoms.len() != after.atoms.len() {
        return Err(SpikeError::MoveNotIsolated);
    }
    for (ordinal, (one, other)) in before.atoms.iter().zip(&after.atoms).enumerate() {
        if !named.contains(&ordinal) {
            match (&one.kind, &other.kind) {
                (PaintAtomKind::Text(one), PaintAtomKind::Text(other)) => {
                    crate::place_text::prove_run_placed(one, other, Matrix::IDENTITY)?;
                }
                _ => {
                    if pdf_paint::paint_signature(&one.kind)
                        != pdf_paint::paint_signature(&other.kind)
                    {
                        return Err(SpikeError::MoveNotIsolated);
                    }
                }
            }
            continue;
        }
        let (PaintAtomKind::Text(one), PaintAtomKind::Text(other)) = (&one.kind, &other.kind)
        else {
            return Err(SpikeError::MoveNotProvable);
        };
        prove_run_resized(one, other, anchored.contains(&ordinal), points)?;
    }
    Ok(())
}

fn prove_run_resized(
    one: &TextShowPaint,
    other: &TextShowPaint,
    anchored: bool,
    points: f64,
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
    let size = other.size_on_page().ok_or(SpikeError::MoveNotProvable)?;
    if (size - points).abs() > SIZE_TOLERANCE {
        return Err(SpikeError::SizeNotAsAsked);
    }
    let factor = other.state.text.font_size.value / one.state.text.font_size.value;
    if !factor.is_finite() || factor <= 0.0 {
        return Err(SpikeError::MoveNotProvable);
    }
    for (index, (a, b)) in one.glyphs.iter().zip(&other.glyphs).enumerate() {
        if a.code.value != b.code.value || a.code.cid != b.code.cid || a.glyph != b.glyph {
            return Err(SpikeError::MoveNotIsolated);
        }
        for (had, want) in [
            (b.matrix.a, a.matrix.a * factor),
            (b.matrix.b, a.matrix.b * factor),
            (b.matrix.c, a.matrix.c * factor),
            (b.matrix.d, a.matrix.d * factor),
        ] {
            if (had - want).abs() > PLACEMENT_TOLERANCE * want.abs().mul_add(1.0, 1.0) {
                return Err(SpikeError::MoveNotIsolated);
            }
        }
        if index != 0 {
            continue;
        }
        let drift = (b.matrix.e - a.matrix.e, b.matrix.f - a.matrix.f);
        if anchored {
            if drift.0.abs() > PLACEMENT_TOLERANCE || drift.1.abs() > PLACEMENT_TOLERANCE {
                return Err(SpikeError::MoveNotIsolated);
            }
        } else {
            let along = (one.matrices.text.value.a, one.matrices.text.value.b);
            let length = along.0.hypot(along.1);
            if length <= f64::EPSILON {
                return Err(SpikeError::MoveNotProvable);
            }
            let across = drift.1.mul_add(along.0, -(drift.0 * along.1)) / length;
            if across.abs() > PLACEMENT_TOLERANCE {
                return Err(SpikeError::MoveNotIsolated);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use pdf_bytes::ByteStore;
    use pdf_paint::{PaintAtomKind, PaintGraph};

    use super::plan_set_text_size;
    use crate::block_move::tests::{after, anchors, page_with, read, written};
    use crate::plan::{Capability, Plan, SourceAnchor};
    use crate::spike_move_text::SpikeError;

    fn resize(source: &ByteStore, runs: &[usize], points: f64) -> Result<Plan, SpikeError> {
        let page = read(source);
        let all = anchors(&page.graph);
        let named: Vec<SourceAnchor> = runs.iter().map(|index| all[*index].clone()).collect();
        plan_set_text_size(
            &page.program,
            &page.operations,
            &page.graph,
            0,
            &named,
            points,
            None,
        )
    }

    fn sizes(graph: &PaintGraph) -> Vec<f64> {
        graph
            .atoms
            .iter()
            .filter_map(|atom| match &atom.kind {
                PaintAtomKind::Text(text) => text.size_on_page(),
                _ => None,
            })
            .collect()
    }

    fn starts(graph: &PaintGraph) -> Vec<(f64, f64)> {
        graph
            .atoms
            .iter()
            .filter_map(|atom| match &atom.kind {
                PaintAtomKind::Text(text) => {
                    let glyph = text.glyphs.first()?;
                    let at = text.state.ctm.value.transform(pdf_paint::Point {
                        x: glyph.matrix.e,
                        y: glyph.matrix.f,
                    });
                    Some((at.x, at.y))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn typing_fourteen_writes_a_tf_of_fourteen_and_reads_back_fourteen() {
        let source = page_with(b"BT /F1 12 Tf 1 0 0 1 20 150 Tm (AA) Tj ET");
        assert!((sizes(&read(&source).graph)[0] - 12.0).abs() < 1e-9);

        let plan = resize(&source, &[0], 14.0).expect("it plans");
        let text = String::from_utf8(written(&plan)).expect("the stream is text");
        assert!(text.contains("/F1 14 Tf"), "{text}");
        assert_eq!(plan.capability(), Capability::Exact);
        assert!((sizes(&after(&plan, &source))[0] - 14.0).abs() < 1e-9);
    }

    #[test]
    fn the_number_is_the_size_on_the_page_and_not_the_number_in_the_file() {
        let source = page_with(b"BT /F1 12 Tf 2 0 0 2 20 150 Tm (AA) Tj ET");
        assert!((sizes(&read(&source).graph)[0] - 24.0).abs() < 1e-9);

        let plan = resize(&source, &[0], 14.0).expect("it plans");
        let text = String::from_utf8(written(&plan)).expect("the stream is text");
        assert!(text.contains("/F1 7 Tf"), "{text}");
        assert!((sizes(&after(&plan, &source))[0] - 14.0).abs() < 1e-9);
    }

    #[test]
    fn a_run_the_command_did_not_name_keeps_its_own_size() {
        let source = page_with(
            b"BT /F1 12 Tf 1 0 0 1 20 150 Tm (AA) Tj 0 -20 Td (AB) Tj 0 -20 Td (AC) Tj ET",
        );
        let plan = resize(&source, &[0], 24.0).expect("it plans");
        let sizes = sizes(&after(&plan, &source));
        assert!((sizes[0] - 24.0).abs() < 1e-9, "{sizes:?}");
        assert!((sizes[1] - 12.0).abs() < 1e-9, "{sizes:?}");
        assert!((sizes[2] - 12.0).abs() < 1e-9, "{sizes:?}");
        let (was, now) = (starts(&read(&source).graph), starts(&after(&plan, &source)));
        for (had, is) in was.iter().zip(&now) {
            assert!((had.0 - is.0).abs() < 1e-9 && (had.1 - is.1).abs() < 1e-9);
        }
    }

    #[test]
    fn every_named_run_is_resized_and_none_of_them_moves() {
        let source = page_with(
            b"BT /F1 12 Tf 1 0 0 1 20 150 Tm (AA) Tj 0 -20 Td (AB) Tj 0 -20 Td (AC) Tj ET",
        );
        let plan = resize(&source, &[0, 1, 2], 9.0).expect("it plans");
        assert_eq!(plan.capability(), Capability::Exact);
        for size in sizes(&after(&plan, &source)) {
            assert!((size - 9.0).abs() < 1e-9);
        }
        let (was, now) = (starts(&read(&source).graph), starts(&after(&plan, &source)));
        for (had, is) in was.iter().zip(&now) {
            assert!((had.0 - is.0).abs() < 1e-9 && (had.1 - is.1).abs() < 1e-9);
        }
        let text = String::from_utf8(written(&plan)).expect("the stream is text");
        assert_eq!(text.matches("Tf").count(), 4, "one written, one per run");
    }

    #[test]
    fn a_run_further_along_the_same_line_is_pinned_and_the_plan_declares_it() {
        let source = page_with(b"BT /F1 12 Tf 1 0 0 1 20 150 Tm (AA) Tj (BB) Tj ET");
        let plan = resize(&source, &[0], 24.0).expect("it plans");
        assert_eq!(plan.capability(), Capability::Normalized);
        let (was, now) = (starts(&read(&source).graph), starts(&after(&plan, &source)));
        assert!(
            (was[1].0 - now[1].0).abs() < 1e-9 && (was[1].1 - now[1].1).abs() < 1e-9,
            "the run after it moved: {was:?} -> {now:?}"
        );
        let sizes = sizes(&after(&plan, &source));
        assert!((sizes[0] - 24.0).abs() < 1e-9 && (sizes[1] - 12.0).abs() < 1e-9);
    }

    #[test]
    fn two_named_runs_on_one_line_stay_a_line_rather_than_overlapping() {
        let source = page_with(b"BT /F1 12 Tf 1 0 0 1 20 150 Tm (AA) Tj (BB) Tj ET");
        let plan = resize(&source, &[0, 1], 24.0).expect("it plans");
        assert_eq!(plan.capability(), Capability::Exact);
        let (was, now) = (starts(&read(&source).graph), starts(&after(&plan, &source)));
        assert!(
            (was[0].0 - now[0].0).abs() < 1e-9,
            "the line still starts here"
        );
        assert!(
            now[1].0 > was[1].0 + 1.0,
            "the second run moved along with the first's new width: {was:?} -> {now:?}"
        );
    }

    #[test]
    fn a_size_that_is_already_set_is_refused() {
        let source = page_with(b"BT /F1 12 Tf 1 0 0 1 20 150 Tm (AA) Tj ET");
        assert!(matches!(
            resize(&source, &[0], 12.0),
            Err(SpikeError::SizeAlreadySet)
        ));
    }

    #[test]
    fn a_size_of_nothing_is_refused() {
        let source = page_with(b"BT /F1 12 Tf 1 0 0 1 20 150 Tm (AA) Tj ET");
        for points in [0.0, -3.0, f64::NAN, f64::INFINITY] {
            assert!(matches!(
                resize(&source, &[0], points),
                Err(SpikeError::SizeNotUsable)
            ));
        }
    }

    #[test]
    fn the_size_is_read_back_through_the_matrix_that_was_in_force() {
        let source = page_with(b"BT /F1 12 Tf 0.5 0 0 0.5 20 150 Tm (AA) Tj ET");
        assert!((sizes(&read(&source).graph)[0] - 6.0).abs() < 1e-9);
        let plan = resize(&source, &[0], 14.0).expect("it plans");
        let text = String::from_utf8(written(&plan)).expect("the stream is text");
        assert!(text.contains("/F1 28 Tf"), "{text}");
        assert!((sizes(&after(&plan, &source))[0] - 14.0).abs() < 1e-9);
    }

    #[test]
    fn turning_a_run_does_not_change_the_size_it_reports() {
        let (sin, cos) = 15.0_f64.to_radians().sin_cos();
        let source = page_with(
            format!(
                "BT /F1 12 Tf {cos} {sin} {} {cos} 20 150 Tm (AA) Tj ET",
                -sin
            )
            .as_bytes(),
        );
        assert!(
            (sizes(&read(&source).graph)[0] - 12.0).abs() < 1e-9,
            "{:?}",
            sizes(&read(&source).graph)
        );
        let plan = resize(&source, &[0], 20.0).expect("it plans");
        assert!((sizes(&after(&plan, &source))[0] - 20.0).abs() < 1e-9);
    }

    #[test]
    fn a_resized_glyph_is_the_old_glyph_scaled_and_nothing_else() {
        let source = page_with(b"BT /F1 10 Tf 1 0 0 1 20 150 Tm (AAA) Tj ET");
        let plan = resize(&source, &[0], 25.0).expect("it plans");
        let before = read(&source).graph;
        let now = after(&plan, &source);
        let (PaintAtomKind::Text(was), PaintAtomKind::Text(is)) =
            (&before.atoms[0].kind, &now.atoms[0].kind)
        else {
            panic!("both are text")
        };
        for (a, b) in was.glyphs.iter().zip(&is.glyphs) {
            for (had, want) in [
                (b.matrix.a, a.matrix.a * 2.5),
                (b.matrix.b, a.matrix.b * 2.5),
                (b.matrix.c, a.matrix.c * 2.5),
                (b.matrix.d, a.matrix.d * 2.5),
            ] {
                assert!((had - want).abs() < 1e-9, "{had} != {want}");
            }
        }
        let (was_end, is_end) = (
            was.glyphs.last().expect("glyphs").matrix.e,
            is.glyphs.last().expect("glyphs").matrix.e,
        );
        assert!(is_end > was_end + 1.0, "{was_end} -> {is_end}");
    }
}
