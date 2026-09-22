use std::ops::Range;

use pdf_bytes::{ByteStore, SourceId};
use pdf_content::{ContentLimits, parse_operation_sequence_strict};
use pdf_paint::{Matrix, PaintLimits, PaintStream, TextShowElement, TextShowPaint};

use crate::plan::{
    Capability, Effect, GlyphChange, MovedRun, Plan, PlannedBody, PlannedWrite, RunRewrite,
    SourceAnchor,
};
use crate::spike_move_text::SpikeError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClusterEdit {
    Move,
    Delete,
}

struct SplitPiece {
    matrix: Matrix,
    body: Vec<u8>,
    glyphs: usize,
    moved: bool,
}

struct Rendered {
    retyped: Option<crate::retype::Prepared>,
    bytes: Vec<u8>,
    expected: Vec<(f64, f64)>,
    produced: usize,
}

struct RunSplice {
    retyped: Option<crate::retype::Prepared>,
    ordinal: usize,
    start: usize,
    end: usize,
    replacement: Vec<u8>,
    removed: Option<Range<usize>>,
    expected: Vec<(f64, f64)>,
    produced: usize,
    moved: MovedRun,
    region: Ink,
}

enum Ink {
    Within([f64; 4]),
    None,
    Unknown,
}

#[expect(
    clippy::too_many_arguments,
    reason = "threaded rather than given a struct it would not outlive, as its neighbour is"
)]
pub(crate) fn rewrite_cluster(
    program: &pdf_content::PageProgram,
    operations: &[Vec<pdf_content::Operation>],
    graph: &pdf_paint::PaintGraph,
    normalization: &[(usize, Matrix)],
    atom: &pdf_paint::PaintAtom,
    text: &TextShowPaint,
    ordinal: usize,
    page_index: usize,
    glyphs: &Range<usize>,
    edit: ClusterEdit,
    dx: f64,
    dy: f64,
    fonts: crate::Fonts<'_>,
) -> Result<Plan, SpikeError> {
    let replacement = replacement_for(text, glyphs, edit, dx, dy)?;

    let stream_index = program
        .streams
        .iter()
        .position(|stream| stream.reference == atom.id.stream)
        .ok_or(SpikeError::NoTextRun)?;
    let stream = &program.streams[stream_index];
    let instruction = operations[stream_index]
        .iter()
        .find(|operation| operation.operator_span() == atom.id.operator_span)
        .ok_or(SpikeError::NoTextRun)?;
    let span = instruction.span();

    let decoded = stream.bytes.as_bytes();
    let neutral = replace_span(decoded, span.start(), span.end(), &replacement.neutral);
    prove_split_neutral(program, stream_index, &neutral, graph, fonts)?;

    let mut edited = replace_span(decoded, span.start(), span.end(), &replacement.edited);
    if !normalization.is_empty() {
        edited = apply_pins(
            decoded,
            span.start(),
            span.end(),
            &replacement.edited,
            &operations[stream_index],
            graph,
            normalization,
            atom,
        )?;
    }

    Ok(Plan::new(
        Capability::Normalized,
        vec![PlannedWrite {
            reference: stream.reference,
            body: PlannedBody::ReplacedStream { decoded: edited },
        }],
        Effect {
            page_index,
            moved: vec![crate::plan::MovedRun {
                anchor: SourceAnchor::of(&atom.id),
                atom_ordinal: ordinal,
                original_matrix: text.matrices.text.value,
            }],
            target_stream: stream.reference,
            declared_region: match edit {
                ClusterEdit::Move => cluster_region(text, glyphs, dx, dy),
                ClusterEdit::Delete => text.outline_bounds_in(glyphs.clone()),
            },
        },
    ))
}

pub(crate) fn rewrite_runs(
    source: &ByteStore,
    program: &pdf_content::PageProgram,
    operations: &[Vec<pdf_content::Operation>],
    graph: &pdf_paint::PaintGraph,
    page_index: usize,
    runs: &[RunRewrite],
    fonts: crate::Fonts<'_>,
) -> Result<Plan, SpikeError> {
    rewrite_group(
        source,
        program,
        operations,
        graph,
        page_index,
        (runs, &[]),
        fonts,
    )
}

#[expect(
    clippy::too_many_lines,
    reason = "resolution, one-stream rewrite, proof, and plan assembly are one fail-closed transaction"
)]
pub(crate) fn rewrite_group(
    source: &ByteStore,
    program: &pdf_content::PageProgram,
    operations: &[Vec<pdf_content::Operation>],
    graph: &pdf_paint::PaintGraph,
    page_index: usize,
    (runs, objects): (&[RunRewrite], &[SourceAnchor]),
    fonts: crate::Fonts<'_>,
) -> Result<Plan, SpikeError> {
    if runs.is_empty() && objects.is_empty() {
        return Err(SpikeError::SelectionNamesNoRun);
    }
    if let Some(invocation) = crate::form_edit::invocation_of(
        runs.iter()
            .filter_map(|run| graph.atoms.iter().find(|atom| run.anchor.names(&atom.id))),
    )? {
        if !objects.is_empty() {
            return Err(SpikeError::ObjectInsideForm);
        }
        return rewrite_runs_in_form(
            source, program, operations, graph, page_index, runs, invocation, fonts,
        );
    }

    let mut stream_reference = None;
    let mut stream_index = None;
    let mut seen = Vec::new();
    let mut rewrites = Vec::with_capacity(runs.len());
    for run in runs {
        let (ordinal, atom, text) = graph
            .atoms
            .iter()
            .enumerate()
            .find_map(|(ordinal, atom)| match &atom.kind {
                pdf_paint::PaintAtomKind::Text(text) if run.anchor.names(&atom.id) => {
                    Some((ordinal, atom, text))
                }
                _ => None,
            })
            .ok_or(SpikeError::SelectionNamesNoRun)?;
        if seen.contains(&ordinal) {
            return Err(SpikeError::SelectionNamesNoRun);
        }
        seen.push(ordinal);
        if !atom.id.invocation_path.is_empty() || !atom.id.pattern_path.is_empty() {
            return Err(SpikeError::SelectionNotDirectlyOnPage);
        }
        match stream_reference {
            None => stream_reference = Some(atom.id.stream),
            Some(reference) if reference == atom.id.stream => {}
            Some(_) => return Err(SpikeError::SelectionSpansSeveralStreams),
        }
        let removed = match &run.glyphs {
            Some(GlyphChange::Remove { glyphs, .. }) => {
                whole_clusters(text, glyphs)?;
                Some(glyphs.clone())
            }
            Some(GlyphChange::Replace { glyphs, .. }) => {
                on_cluster_boundaries(text, glyphs)?;
                Some(glyphs.clone())
            }
            None => None,
        };

        let index = program
            .streams
            .iter()
            .position(|stream| stream.reference == atom.id.stream)
            .ok_or(SpikeError::SelectionNamesNoRun)?;
        match stream_index {
            None => stream_index = Some(index),
            Some(had) if had == index => {}
            Some(_) => return Err(SpikeError::SelectionSpansSeveralStreams),
        }
        let instruction = operations[index]
            .iter()
            .find(|operation| operation.operator_span() == atom.id.operator_span)
            .ok_or(SpikeError::SelectionNamesNoRun)?;
        let rendered = rendered_run(&program.resources, text, run)?;
        let region = if let Some(prepared) = &rendered.retyped {
            let mut expected = text.clone();
            expected.glyphs.clone_from(&prepared.glyphs);
            retyped_region(text, &expected)
        } else {
            region_of(text, run, removed.as_ref())
        };
        rewrites.push(RunSplice {
            retyped: rendered.retyped,
            ordinal,
            start: instruction.span().start(),
            end: instruction.span().end(),
            replacement: rendered.bytes,
            removed: removed.clone(),
            expected: rendered.expected,
            produced: rendered.produced,
            moved: MovedRun {
                anchor: run.anchor.clone(),
                atom_ordinal: ordinal,
                original_matrix: text.matrices.text.value,
            },
            region,
        });
    }

    let mut removals = Vec::with_capacity(objects.len());
    for anchor in objects {
        let ordinal = crate::place_object::resolve(graph, anchor)?;
        if seen.contains(&ordinal) {
            return Err(SpikeError::ObjectNamedMoreThanOnce);
        }
        seen.push(ordinal);
        let atom = &graph.atoms[ordinal];
        let region = crate::place_object::declared_region(&atom.kind, Some(atom));
        let placed = crate::place_object::placement_of(&atom.kind)?;
        let (index, operation_index) = crate::place_object::written_at(program, operations, atom)?;
        match stream_index {
            None => {
                stream_index = Some(index);
                stream_reference = Some(atom.id.stream);
            }
            Some(had) if had == index => {}
            Some(_) => return Err(SpikeError::SelectionSpansSeveralStreams),
        }
        let span = operations[index][operation_index].span();
        let begin = crate::place_object::construction_start(atom).unwrap_or_else(|| span.start());
        if begin < span.start()
            && !crate::remove_object::only_draws(
                &program.streams[index].bytes,
                &operations[index],
                begin..span.end(),
            )
        {
            return Err(SpikeError::ObjectIsDrawing);
        }
        if span.end() > program.streams[index].bytes.as_bytes().len() || begin > span.end() {
            return Err(SpikeError::ObjectNotInPageContent);
        }
        removals.push(ObjectRemoval {
            ordinal,
            start: begin,
            end: span.end(),
            region,
            moved: MovedRun {
                anchor: anchor.clone(),
                atom_ordinal: ordinal,
                original_matrix: placed,
            },
        });
    }

    let reference = stream_reference.ok_or(SpikeError::SelectionNamesNoRun)?;
    if program
        .streams
        .iter()
        .filter(|stream| stream.reference == reference)
        .count()
        != 1
    {
        return Err(SpikeError::SharedPageContentStream);
    }
    let index = stream_index.ok_or(SpikeError::SelectionNamesNoRun)?;
    rewrites.sort_by_key(|splice| splice.start);
    let decoded = program.streams[index].bytes.as_bytes();
    let mut spliced: Vec<(usize, usize, &[u8])> = rewrites
        .iter()
        .map(|splice| (splice.start, splice.end, splice.replacement.as_slice()))
        .chain(
            removals
                .iter()
                .map(|removal| (removal.start, removal.end, b" ".as_slice())),
        )
        .collect();
    spliced.sort_by_key(|(start, _, _)| *start);
    let mut edited = Vec::with_capacity(decoded.len());
    let mut cursor = 0_usize;
    for (start, end, replacement) in spliced {
        if start < cursor || end < start || end > decoded.len() {
            return Err(SpikeError::SelectionNamesNoRun);
        }
        edited.extend_from_slice(&decoded[cursor..start]);
        edited.extend_from_slice(replacement);
        cursor = end;
    }
    edited.extend_from_slice(&decoded[cursor..]);

    let rewritten = interpret_candidate(program, index, &edited, fonts)
        .map_err(|()| SpikeError::DeleteNotIsolated)?;
    let gone: Vec<usize> = removals.iter().map(|removal| removal.ordinal).collect();
    let (correspondence, inserted) = prove_rewrite(graph, &rewritten, &rewrites, &gone)?;

    let declared_region = declared_over(&rewrites, &removals)?;
    let capability = if rewrites.is_empty() {
        Capability::Exact
    } else {
        Capability::Normalized
    };
    let moved = rewrites
        .into_iter()
        .map(|splice| splice.moved)
        .chain(removals.into_iter().map(|removal| removal.moved))
        .collect();
    Ok(Plan::new(
        capability,
        vec![PlannedWrite {
            reference,
            body: PlannedBody::ReplacedStream { decoded: edited },
        }],
        Effect {
            page_index,
            moved,
            target_stream: reference,
            declared_region,
        },
    )
    .with_correspondence(correspondence)
    .with_inserted(inserted))
}

struct ObjectRemoval {
    ordinal: usize,
    start: usize,
    end: usize,
    region: Option<[f64; 4]>,
    moved: MovedRun,
}

fn declared_over(
    rewrites: &[RunSplice],
    removals: &[ObjectRemoval],
) -> Result<Option<[f64; 4]>, SpikeError> {
    let text = combine_regions(rewrites.iter().map(|splice| &splice.region))?;
    if removals.is_empty() {
        return Ok(text);
    }
    let mut bounds: Option<[f64; 4]> = None;
    for removal in removals {
        let Some(region) = removal.region else {
            return Ok(None);
        };
        bounds = Some(bounds.map_or(region, |old| union(old, region)));
    }
    if rewrites.is_empty() {
        return Ok(bounds);
    }
    Ok(match (text, bounds) {
        (Some(text), Some(objects)) => Some(union(text, objects)),
        _ => None,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "one edit's inputs, threaded as its neighbours are"
)]
#[expect(
    clippy::too_many_lines,
    reason = "the splice, the proof in the Form's scope and the carry back to the page's numbering are one fail-closed transaction"
)]
fn rewrite_runs_in_form(
    source: &ByteStore,
    program: &pdf_content::PageProgram,
    operations: &[Vec<pdf_content::Operation>],
    graph: &pdf_paint::PaintGraph,
    page_index: usize,
    runs: &[RunRewrite],
    invocation: pdf_paint::FormInvocation,
    fonts: crate::Fonts<'_>,
) -> Result<Plan, SpikeError> {
    let scope = crate::form_edit::scope_of(program, operations, invocation)?;
    let mut seen = Vec::new();
    let mut rewrites = Vec::with_capacity(runs.len());
    for run in runs {
        let (ordinal, atom, text) = graph
            .atoms
            .iter()
            .enumerate()
            .find_map(|(ordinal, atom)| match &atom.kind {
                pdf_paint::PaintAtomKind::Text(text) if run.anchor.names(&atom.id) => {
                    Some((ordinal, atom, text))
                }
                _ => None,
            })
            .ok_or(SpikeError::SelectionNamesNoRun)?;
        if seen.contains(&ordinal) {
            return Err(SpikeError::SelectionNamesNoRun);
        }
        seen.push(ordinal);
        let removed = match &run.glyphs {
            Some(GlyphChange::Remove { glyphs, .. }) => {
                whole_clusters(text, glyphs)?;
                Some(glyphs.clone())
            }
            Some(GlyphChange::Replace { glyphs, .. }) => {
                on_cluster_boundaries(text, glyphs)?;
                Some(glyphs.clone())
            }
            None => None,
        };
        let span = scope.span_of(atom.id.operator_span)?;
        let rendered = rendered_run(&scope.resources, text, run)?;
        let region = if let Some(prepared) = &rendered.retyped {
            let mut expected = text.clone();
            expected.glyphs.clone_from(&prepared.glyphs);
            retyped_region(text, &expected)
        } else {
            region_of(text, run, removed.as_ref())
        };
        rewrites.push(RunSplice {
            retyped: rendered.retyped,
            ordinal,
            start: span.start(),
            end: span.end(),
            replacement: rendered.bytes,
            removed: removed.clone(),
            expected: rendered.expected,
            produced: rendered.produced,
            moved: MovedRun {
                anchor: run.anchor.clone(),
                atom_ordinal: ordinal,
                original_matrix: text.matrices.text.value,
            },
            region,
        });
    }

    rewrites.sort_by_key(|splice| splice.start);
    let decoded = scope.bytes();
    let mut edited = Vec::with_capacity(decoded.len());
    let mut cursor = 0_usize;
    for splice in &rewrites {
        if splice.start < cursor || splice.end < splice.start || splice.end > decoded.len() {
            return Err(SpikeError::SelectionNamesNoRun);
        }
        edited.extend_from_slice(&decoded[cursor..splice.start]);
        edited.extend_from_slice(&splice.replacement);
        cursor = splice.end;
    }
    edited.extend_from_slice(&decoded[cursor..]);

    let inside: Vec<usize> = graph
        .atoms
        .iter()
        .enumerate()
        .filter(|(_, atom)| atom.id.invocation_path.first().copied() == Some(invocation))
        .map(|(ordinal, _)| ordinal)
        .collect();
    let base = *inside.first().ok_or(SpikeError::MoveNotProvable)?;
    if inside
        .iter()
        .enumerate()
        .any(|(step, ordinal)| *ordinal != base + step)
    {
        return Err(SpikeError::MoveNotProvable);
    }

    let before = interpret_form_for_proof(
        &scope.form.bytes,
        scope.form.reference,
        &scope.resources,
        fonts,
    )?;
    if before.atoms.len() != inside.len() {
        return Err(SpikeError::MoveNotProvable);
    }
    let candidate = ByteStore::new(
        SourceId::new(scope.form.bytes.id().get().wrapping_add(1)),
        std::sync::Arc::<[u8]>::from(edited.clone()),
    );
    let after =
        interpret_form_for_proof(&candidate, scope.form.reference, &scope.resources, fonts)?;
    for splice in &mut rewrites {
        splice.ordinal = splice
            .ordinal
            .checked_sub(base)
            .ok_or(SpikeError::MoveNotProvable)?;
    }
    let (mapping, inserted) = prove_rewrite(&before, &after, &rewrites, &[])?;
    let gained = i64::try_from(after.atoms.len()).map_err(|_| SpikeError::MoveNotProvable)?
        - i64::try_from(before.atoms.len()).map_err(|_| SpikeError::MoveNotProvable)?;
    let correspondence = carried_to_page(graph, &mapping, base, inside.len(), gained)?;
    let inserted = inserted
        .into_iter()
        .map(|(one, other)| {
            (
                pdf_semantics::ClusterKey {
                    atom: one.atom + base,
                    glyph: one.glyph,
                },
                pdf_semantics::ClusterKey {
                    atom: other.atom + base,
                    glyph: other.glyph,
                },
            )
        })
        .collect();

    let declared_region = combine_regions(rewrites.iter().map(|splice| &splice.region))?;
    let moved = rewrites.into_iter().map(|splice| splice.moved).collect();
    let written = crate::form_edit::writes_for(source, program, &scope, edited)?;
    Ok(Plan::new(
        Capability::Normalized,
        written.writes,
        Effect {
            page_index,
            moved,
            target_stream: written.target_stream,
            declared_region,
        },
    )
    .with_correspondence(correspondence)
    .with_inserted(inserted))
}

fn carried_to_page(
    graph: &pdf_paint::PaintGraph,
    mapping: &Correspondence,
    base: usize,
    held: usize,
    gained: i64,
) -> Result<Correspondence, SpikeError> {
    let mut out = Correspondence::new();
    for (ordinal, atom) in graph.atoms.iter().enumerate() {
        let pdf_paint::PaintAtomKind::Text(text) = &atom.kind else {
            continue;
        };
        if ordinal < base {
            for glyph in 0..text.glyphs.len().max(1) {
                out.insert(
                    pdf_semantics::ClusterKey {
                        atom: ordinal,
                        glyph,
                    },
                    pdf_semantics::ClusterKey {
                        atom: ordinal,
                        glyph,
                    },
                );
            }
        } else if ordinal >= base + held {
            let moved = i64::try_from(ordinal)
                .ok()
                .and_then(|at| at.checked_add(gained))
                .and_then(|at| usize::try_from(at).ok())
                .ok_or(SpikeError::MoveNotProvable)?;
            for glyph in 0..text.glyphs.len().max(1) {
                out.insert(
                    pdf_semantics::ClusterKey {
                        atom: ordinal,
                        glyph,
                    },
                    pdf_semantics::ClusterKey { atom: moved, glyph },
                );
            }
        }
    }
    for (one, other) in mapping {
        out.insert(
            pdf_semantics::ClusterKey {
                atom: one.atom + base,
                glyph: one.glyph,
            },
            pdf_semantics::ClusterKey {
                atom: other.atom + base,
                glyph: other.glyph,
            },
        );
    }
    Ok(out)
}

fn rendered_run(
    resources: &pdf_content::PageResources,
    text: &TextShowPaint,
    run: &RunRewrite,
) -> Result<Rendered, SpikeError> {
    let (dx, dy) = run.displace;
    let (pieces, removed, close_gap) = match &run.glyphs {
        Some(GlyphChange::Replace {
            glyphs,
            text: typed,
        }) => {
            if run.displace != (0.0, 0.0) {
                return Err(SpikeError::RetypeUnsupported(
                    "replacement and movement must be separate commands",
                ));
            }
            let prepared = crate::retype::prepare(resources, text, glyphs, typed)?;
            let mut bytes = b" ".to_vec();
            for piece in &prepared.pieces {
                let m = prepared.glyphs[piece.start].text_matrix;
                bytes.extend_from_slice(
                    format!("{} {} {} {} {} {} Tm <", m.a, m.b, m.c, m.d, m.e, m.f).as_bytes(),
                );
                for glyph in &prepared.glyphs[piece.clone()] {
                    for byte in &glyph.code.bytes {
                        bytes.extend_from_slice(format!("{byte:02X}").as_bytes());
                    }
                }
                bytes.extend_from_slice(b"> Tj ");
            }
            return Ok(Rendered {
                bytes,
                expected: vec![(0.0, 0.0); prepared.glyphs.len()],
                produced: prepared.pieces.len(),
                retyped: Some(prepared),
            });
        }
        Some(GlyphChange::Remove { glyphs, close_gap }) => (
            split_pieces(text, glyphs)?,
            Some(glyphs.clone()),
            *close_gap,
        ),
        None => (split_pieces(text, &(0..text.glyphs.len()))?, None, false),
    };
    if pieces.is_empty() {
        return Err(SpikeError::SelectionNamesNoRun);
    }
    let hole = removed
        .is_some()
        .then(|| pieces.iter().position(|piece| piece.moved))
        .flatten();

    let mut out = b" ".to_vec();
    let mut expected = Vec::with_capacity(text.glyphs.len());
    let mut produced = 0_usize;
    for (index, piece) in pieces.iter().enumerate() {
        if removed.is_some() && piece.moved {
            continue;
        }
        produced += 1;
        let stands_on = match hole {
            Some(hole) if close_gap && index > hole => pieces[hole].matrix,
            _ => piece.matrix,
        };
        let matrix = offset(stands_on, dx, dy);
        for _ in 0..piece.glyphs {
            expected.push((matrix.e - piece.matrix.e, matrix.f - piece.matrix.f));
        }
        out.extend_from_slice(
            format!(
                "{} {} {} {} {} {} Tm ",
                matrix.a, matrix.b, matrix.c, matrix.d, matrix.e, matrix.f
            )
            .as_bytes(),
        );
        out.extend_from_slice(&piece.body);
        out.push(b' ');
    }
    Ok(Rendered {
        retyped: None,
        bytes: out,
        expected,
        produced,
    })
}

fn combine_regions<'a>(
    regions: impl Iterator<Item = &'a Ink>,
) -> Result<Option<[f64; 4]>, SpikeError> {
    let mut bounded = None;
    let mut unknown = false;
    for region in regions {
        match region {
            Ink::Within(bounds) => {
                bounded = Some(bounded.map_or(*bounds, |old| union(old, *bounds)));
            }
            Ink::Unknown => unknown = true,
            Ink::None => {}
        }
    }
    if unknown && bounded.is_some() {
        return Err(SpikeError::RunExtentUnknown);
    }
    Ok(bounded)
}

fn retyped_region(before: &TextShowPaint, after: &TextShowPaint) -> Ink {
    match (ink_of(before), ink_of(after)) {
        (Ink::Unknown, _) | (_, Ink::Unknown) => Ink::Unknown,
        (Ink::Within(old), Ink::Within(new)) => Ink::Within(union(old, new)),
        (Ink::Within(only), Ink::None) | (Ink::None, Ink::Within(only)) => Ink::Within(only),
        (Ink::None, Ink::None) => Ink::None,
    }
}

fn ink_of(text: &TextShowPaint) -> Ink {
    match text.outline_bounds() {
        Some(bounds) => Ink::Within(bounds),
        None if text.draws_no_ink() => Ink::None,
        None => Ink::Unknown,
    }
}

fn region_of(text: &TextShowPaint, run: &RunRewrite, removed: Option<&Range<usize>>) -> Ink {
    let (dx, dy) = run.displace;
    let stays_put = dx == 0.0 && dy == 0.0;
    if removed.is_none() && stays_put {
        return Ink::None;
    }
    let Some(bounds) = text.outline_bounds() else {
        return if text.draws_no_ink() {
            Ink::None
        } else {
            Ink::Unknown
        };
    };
    if removed.is_none() {
        let to_user = text.state.ctm.value.multiply(text.matrices.text.value);
        let (ux, uy) = (
            to_user.a.mul_add(dx, to_user.c * dy),
            to_user.b.mul_add(dx, to_user.d * dy),
        );
        return Ink::Within(union(
            bounds,
            [
                bounds[0] + ux,
                bounds[1] + uy,
                bounds[2] + ux,
                bounds[3] + uy,
            ],
        ));
    }
    let to_user = text.state.ctm.value.multiply(text.matrices.text.value);
    let (ux, uy) = (
        to_user.a.mul_add(dx, to_user.c * dy),
        to_user.b.mul_add(dx, to_user.d * dy),
    );
    Ink::Within(union(
        bounds,
        [
            bounds[0] + ux,
            bounds[1] + uy,
            bounds[2] + ux,
            bounds[3] + uy,
        ],
    ))
}

type Correspondence =
    std::collections::BTreeMap<pdf_semantics::ClusterKey, pdf_semantics::ClusterKey>;
type Insertions = Vec<(pdf_semantics::ClusterKey, pdf_semantics::ClusterKey)>;

fn prove_rewrite(
    before: &pdf_paint::PaintGraph,
    after: &pdf_paint::PaintGraph,
    rewrites: &[RunSplice],
    gone: &[usize],
) -> Result<(Correspondence, Insertions), SpikeError> {
    use pdf_semantics::ClusterKey;
    let mut mapping = std::collections::BTreeMap::new();
    let mut inserted = Vec::new();
    let mut rewritten = after.atoms.iter().enumerate();
    for (ordinal, one) in before.atoms.iter().enumerate() {
        if gone.contains(&ordinal) {
            continue;
        }
        let Some(splice) = rewrites.iter().find(|splice| splice.ordinal == ordinal) else {
            let (after_atom, other) = rewritten.next().ok_or(SpikeError::DeleteNotIsolated)?;
            match (&one.kind, &other.kind) {
                (pdf_paint::PaintAtomKind::Text(one), pdf_paint::PaintAtomKind::Text(other)) => {
                    crate::block_move::prove_run_displaced(one, other, 0.0, 0.0)
                        .map_err(|_| SpikeError::DeleteNotIsolated)?;
                    for glyph in 0..one.glyphs.len().max(1) {
                        mapping.insert(
                            ClusterKey {
                                atom: ordinal,
                                glyph,
                            },
                            ClusterKey {
                                atom: after_atom,
                                glyph,
                            },
                        );
                    }
                }
                (one, other) => {
                    if pdf_paint::paint_signature(one) != pdf_paint::paint_signature(other) {
                        return Err(SpikeError::DeleteNotIsolated);
                    }
                }
            }
            continue;
        };
        let pdf_paint::PaintAtomKind::Text(one) = &one.kind else {
            return Err(SpikeError::DeleteNotIsolated);
        };
        let mut pieces = Vec::with_capacity(splice.produced);
        let mut position = 0;
        let owner = insertion_owner(splice, one.glyphs.len());
        let mut surviving = (0..one.glyphs.len()).filter(|glyph| {
            !splice
                .removed
                .as_ref()
                .is_some_and(|range| range.contains(glyph))
        });
        for _ in 0..splice.produced {
            let (after_atom, other) = rewritten.next().ok_or(SpikeError::DeleteNotIsolated)?;
            let pdf_paint::PaintAtomKind::Text(other) = &other.kind else {
                return Err(SpikeError::DeleteNotIsolated);
            };
            pieces.push(other);
            for glyph in 0..other.glyphs.len() {
                if let Some(prepared) = &splice.retyped {
                    let key = ClusterKey {
                        atom: after_atom,
                        glyph,
                    };
                    match prepared
                        .retained
                        .get(position)
                        .ok_or(SpikeError::DeleteNotIsolated)?
                    {
                        Some(old) => {
                            mapping.insert(
                                ClusterKey {
                                    atom: ordinal,
                                    glyph: *old,
                                },
                                key,
                            );
                        }
                        None => inserted.push((owner, key)),
                    }
                    position += 1;
                    continue;
                }
                let before_glyph = surviving.next().ok_or(SpikeError::DeleteNotIsolated)?;
                mapping.insert(
                    ClusterKey {
                        atom: ordinal,
                        glyph: before_glyph,
                    },
                    ClusterKey {
                        atom: after_atom,
                        glyph,
                    },
                );
            }
        }
        prove_run_rewritten(one, &pieces, splice)?;
    }
    if rewritten.next().is_some() {
        return Err(SpikeError::DeleteNotIsolated);
    }
    Ok((mapping, inserted))
}

fn insertion_owner(splice: &RunSplice, glyphs: usize) -> pdf_semantics::ClusterKey {
    pdf_semantics::ClusterKey {
        atom: splice.ordinal,
        glyph: splice
            .removed
            .as_ref()
            .map_or(0, |range| range.start)
            .min(glyphs.saturating_sub(1)),
    }
}

fn prove_run_rewritten(
    one: &TextShowPaint,
    pieces: &[&TextShowPaint],
    splice: &RunSplice,
) -> Result<(), SpikeError> {
    let mut kept = one.clone();
    if let Some(prepared) = &splice.retyped {
        kept.glyphs.clone_from(&prepared.glyphs);
    } else if let Some(removed) = &splice.removed {
        if removed.end > kept.glyphs.len() {
            return Err(SpikeError::DeleteNotIsolated);
        }
        kept.glyphs.drain(removed.clone());
    }
    let wrote: Vec<&pdf_paint::PositionedGlyph> = pieces
        .iter()
        .flat_map(|piece| piece.glyphs.iter())
        .collect();
    if kept.glyphs.len() != wrote.len() || kept.glyphs.len() != splice.expected.len() {
        return Err(SpikeError::DeleteNotIsolated);
    }
    for piece in pieces {
        if one.state.text.rendering_mode.value != piece.state.text.rendering_mode.value
            || one.state.ctm.value != piece.state.ctm.value
            || pdf_paint::colour_signature(&one.state.fill_color.value)
                != pdf_paint::colour_signature(&piece.state.fill_color.value)
            || pdf_paint::colour_signature(&one.state.stroke_color.value)
                != pdf_paint::colour_signature(&piece.state.stroke_color.value)
        {
            return Err(SpikeError::DeleteNotIsolated);
        }
    }
    for ((was, now), (dx, dy)) in kept.glyphs.iter().zip(&wrote).zip(&splice.expected) {
        if was.code.value != now.code.value
            || was.code.cid != now.code.cid
            || was.glyph != now.glyph
        {
            return Err(SpikeError::DeleteNotIsolated);
        }
        if crate::block_move::linear(was.matrix) != crate::block_move::linear(now.matrix) {
            return Err(SpikeError::DeleteNotIsolated);
        }
        let ctm = one.state.ctm.value;
        let from = crate::block_move::glyph_origin(was.matrix, ctm);
        let to = crate::block_move::glyph_origin(now.matrix, ctm);
        let (wanted_x, wanted_y) = (
            ctm.a.mul_add(*dx, ctm.c * dy),
            ctm.b.mul_add(*dx, ctm.d * dy),
        );
        if (to.x - from.x - wanted_x).abs() > crate::block_move::PLACEMENT_TOLERANCE
            || (to.y - from.y - wanted_y).abs() > crate::block_move::PLACEMENT_TOLERANCE
        {
            return Err(SpikeError::DeleteNotIsolated);
        }
    }
    Ok(())
}

pub(crate) fn interpret_candidate(
    program: &pdf_content::PageProgram,
    stream_index: usize,
    candidate: &[u8],
    fonts: crate::Fonts<'_>,
) -> Result<pdf_paint::PaintGraph, ()> {
    interpret_candidate_with(program, &program.resources, stream_index, candidate, fonts)
}

pub(crate) fn interpret_candidate_with(
    program: &pdf_content::PageProgram,
    resources: &pdf_content::PageResources,
    stream_index: usize,
    candidate: &[u8],
    fonts: crate::Fonts<'_>,
) -> Result<pdf_paint::PaintGraph, ()> {
    let bytes = ByteStore::new(
        SourceId::new(
            program.streams[stream_index]
                .bytes
                .id()
                .get()
                .wrapping_add(1),
        ),
        std::sync::Arc::<[u8]>::from(candidate.to_vec()),
    );
    let mut sources: Vec<&ByteStore> = program.streams.iter().map(|stream| &stream.bytes).collect();
    sources[stream_index] = &bytes;
    let operations =
        parse_operation_sequence_strict(&sources, ContentLimits::default()).map_err(|_| ())?;
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
    pdf_paint::interpret_stream_sequence_with_fonts(
        &streams,
        program.page,
        &[],
        resources,
        PaintLimits::default(),
        fonts.cloned(),
    )
    .map_err(|_| ())
}

fn union(one: [f64; 4], other: [f64; 4]) -> [f64; 4] {
    [
        one[0].min(other[0]),
        one[1].min(other[1]),
        one[2].max(other[2]),
        one[3].max(other[3]),
    ]
}

struct Replacement {
    neutral: Vec<u8>,
    edited: Vec<u8>,
}

fn replacement_for(
    text: &TextShowPaint,
    glyphs: &Range<usize>,
    edit: ClusterEdit,
    dx: f64,
    dy: f64,
) -> Result<Replacement, SpikeError> {
    if whole_clusters(text, glyphs)? != 1 && edit == ClusterEdit::Move {
        return Err(SpikeError::GlyphRangeIsNotACluster);
    }
    let pieces = split_pieces(text, glyphs)?;
    Ok(Replacement {
        neutral: render(&pieces, ClusterEdit::Move, 0.0, 0.0),
        edited: render(&pieces, edit, dx, dy),
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "one edit's inputs, threaded as its neighbours are"
)]
pub(crate) fn cluster_edit_in_form(
    form: &pdf_content::FormXObject,
    resources: &pdf_content::PageResources,
    form_operations: &[pdf_content::Operation],
    graph: &pdf_paint::PaintGraph,
    normalization: &[(usize, Matrix)],
    atom: &pdf_paint::PaintAtom,
    text: &TextShowPaint,
    glyphs: &Range<usize>,
    edit: ClusterEdit,
    dx: f64,
    dy: f64,
    fonts: crate::Fonts<'_>,
) -> Result<(Vec<u8>, Option<[f64; 4]>), SpikeError> {
    let replacement = replacement_for(text, glyphs, edit, dx, dy)?;
    let instruction = form_operations
        .iter()
        .find(|operation| operation.operator_span() == atom.id.operator_span)
        .ok_or(SpikeError::NoTextRun)?;
    let span = instruction.span();
    let decoded = form.bytes.as_bytes();
    prove_form_split_neutral(
        form,
        resources,
        &replace_span(decoded, span.start(), span.end(), &replacement.neutral),
        fonts,
    )?;
    let edited = if normalization.is_empty() {
        replace_span(decoded, span.start(), span.end(), &replacement.edited)
    } else {
        apply_pins(
            decoded,
            span.start(),
            span.end(),
            &replacement.edited,
            form_operations,
            graph,
            normalization,
            atom,
        )?
    };
    Ok((
        edited,
        match edit {
            ClusterEdit::Move => cluster_region(text, glyphs, dx, dy),
            ClusterEdit::Delete => text.outline_bounds_in(glyphs.clone()),
        },
    ))
}

fn prove_form_split_neutral(
    form: &pdf_content::FormXObject,
    resources: &pdf_content::PageResources,
    neutral: &[u8],
    fonts: crate::Fonts<'_>,
) -> Result<(), SpikeError> {
    let candidate = ByteStore::new(
        SourceId::new(form.bytes.id().get().wrapping_add(1)),
        std::sync::Arc::<[u8]>::from(neutral.to_vec()),
    );
    let before = interpret_form(&form.bytes, form.reference, resources, fonts)?;
    let after = interpret_form(&candidate, form.reference, resources, fonts)?;
    if pdf_paint::glyph_placement_signature(&before) != pdf_paint::glyph_placement_signature(&after)
    {
        return Err(SpikeError::SplitNotNeutral);
    }
    Ok(())
}

pub(crate) fn interpret_form_for_proof(
    bytes: &ByteStore,
    reference: pdf_syntax::Reference,
    resources: &pdf_content::PageResources,
    fonts: crate::Fonts<'_>,
) -> Result<pdf_paint::PaintGraph, SpikeError> {
    interpret_form(bytes, reference, resources, fonts).map_err(|_| SpikeError::MoveNotProvable)
}

fn interpret_form(
    bytes: &ByteStore,
    reference: pdf_syntax::Reference,
    resources: &pdf_content::PageResources,
    fonts: crate::Fonts<'_>,
) -> Result<pdf_paint::PaintGraph, SpikeError> {
    let operations = pdf_content::parse_operations_strict(bytes, ContentLimits::default())
        .map_err(|_| SpikeError::UnsupportedContentLayout)?;
    let stream = PaintStream {
        source: bytes,
        reference,
        operations: &operations,
    };
    pdf_paint::interpret_stream_sequence_with_fonts(
        &[stream],
        reference,
        &[],
        resources,
        PaintLimits::default(),
        fonts.cloned(),
    )
    .map_err(|_| SpikeError::SplitNotNeutral)
}

fn whole_clusters(text: &TextShowPaint, glyphs: &Range<usize>) -> Result<usize, SpikeError> {
    if glyphs.start >= glyphs.end || glyphs.end > text.glyphs.len() {
        return Err(SpikeError::GlyphRangeOutsideRun);
    }
    let clusters = clusters_of(text)?;
    let mut at = glyphs.start;
    let mut covered = 0_usize;
    while at < glyphs.end {
        let cluster = clusters
            .clusters
            .iter()
            .find(|cluster| cluster.glyphs.start == at)
            .ok_or(SpikeError::GlyphRangeIsNotACluster)?;
        if cluster.glyphs.end > glyphs.end {
            return Err(SpikeError::GlyphRangeIsNotACluster);
        }
        at = cluster.glyphs.end;
        covered += 1;
    }
    Ok(covered)
}

fn clusters_of(text: &TextShowPaint) -> Result<pdf_semantics::SemanticIndex, SpikeError> {
    Ok(pdf_semantics::SemanticIndex::of(&pdf_paint::PaintGraph {
        object_scopes: Vec::new(),
        repairs: Vec::new(),
        skipped: Vec::new(),
        atoms: vec![pdf_paint::PaintAtom {
            id: pdf_paint::PaintId {
                page: pdf_syntax::Reference::new(0, 0),
                stream: pdf_syntax::Reference::new(0, 0),
                operator_span: text
                    .elements
                    .first()
                    .map_or_else(
                        || pdf_bytes::SourceSpan::new(SourceId::new(1), 0, 0),
                        |element| match element {
                            TextShowElement::Codes { source_span, .. }
                            | TextShowElement::Adjustment { source_span, .. } => Ok(*source_span),
                        },
                    )
                    .map_err(|_| SpikeError::GlyphRangeOutsideRun)?,
                invocation_path: Vec::new(),
                pattern_path: Vec::new(),
                ordinal: 0,
            },
            kind: pdf_paint::PaintAtomKind::Text(text.clone()),
            marks: Vec::new(),
        }],
    }))
}

fn on_cluster_boundaries(text: &TextShowPaint, glyphs: &Range<usize>) -> Result<(), SpikeError> {
    if !glyphs.is_empty() {
        whole_clusters(text, glyphs)?;
        return Ok(());
    }
    if glyphs.start > text.glyphs.len() {
        return Err(SpikeError::GlyphRangeOutsideRun);
    }
    if glyphs.start == 0 || glyphs.start == text.glyphs.len() {
        return Ok(());
    }
    clusters_of(text)?
        .clusters
        .iter()
        .any(|cluster| cluster.glyphs.start == glyphs.start)
        .then_some(())
        .ok_or(SpikeError::GlyphRangeIsNotACluster)
}

fn split_pieces(text: &TextShowPaint, moved: &Range<usize>) -> Result<Vec<SplitPiece>, SpikeError> {
    let mut pieces = Vec::new();
    for range in [0..moved.start, moved.clone(), moved.end..text.glyphs.len()] {
        if range.is_empty() {
            continue;
        }
        let first = text
            .glyphs
            .get(range.start)
            .ok_or(SpikeError::GlyphRangeOutsideRun)?;
        pieces.push(SplitPiece {
            body: piece_body(text, &range),
            matrix: first.text_matrix,
            glyphs: range.len(),
            moved: range == *moved,
        });
    }
    Ok(pieces)
}

fn piece_body(text: &TextShowPaint, range: &Range<usize>) -> Vec<u8> {
    let mut body = b"[".to_vec();
    let mut glyph = 0_usize;
    let mut pending: Vec<u8> = Vec::new();
    for element in &text.elements {
        match element {
            TextShowElement::Codes { codes, .. } => {
                for code in codes {
                    if range.contains(&glyph) {
                        pending.extend_from_slice(&code.bytes);
                    }
                    glyph += 1;
                }
            }
            TextShowElement::Adjustment { value, .. } => {
                if glyph > range.start && glyph < range.end {
                    if !pending.is_empty() {
                        body.extend_from_slice(&hex_string(&pending));
                        pending.clear();
                    }
                    body.extend_from_slice(format!(" {value} ").as_bytes());
                }
            }
        }
    }
    if !pending.is_empty() {
        body.extend_from_slice(&hex_string(&pending));
    }
    body.extend_from_slice(b"] TJ");
    body
}

fn hex_string(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() * 2 + 2);
    out.push(b'<');
    for byte in bytes {
        out.extend_from_slice(format!("{byte:02X}").as_bytes());
    }
    out.push(b'>');
    out
}

pub(crate) fn kept_in_place(
    text: &TextShowPaint,
    ranges: &[Range<usize>],
) -> Result<Vec<u8>, SpikeError> {
    let mut out = b" ".to_vec();
    for range in ranges {
        let first = text
            .glyphs
            .get(range.start)
            .filter(|_| range.start < range.end && range.end <= text.glyphs.len())
            .ok_or(SpikeError::GlyphRangeOutsideRun)?;
        let m = first.text_matrix;
        out.extend_from_slice(
            format!("{} {} {} {} {} {} Tm ", m.a, m.b, m.c, m.d, m.e, m.f).as_bytes(),
        );
        out.extend_from_slice(&piece_body(text, range));
        out.push(b' ');
    }
    Ok(out)
}

fn render(pieces: &[SplitPiece], edit: ClusterEdit, dx: f64, dy: f64) -> Vec<u8> {
    let mut out = b" ".to_vec();
    for piece in pieces {
        if piece.moved && edit == ClusterEdit::Delete {
            continue;
        }
        let matrix = if piece.moved {
            offset(piece.matrix, dx, dy)
        } else {
            piece.matrix
        };
        out.extend_from_slice(
            format!(
                "{} {} {} {} {} {} Tm ",
                matrix.a, matrix.b, matrix.c, matrix.d, matrix.e, matrix.f
            )
            .as_bytes(),
        );
        out.extend_from_slice(&piece.body);
        out.push(b' ');
    }
    out
}

fn offset(matrix: Matrix, dx: f64, dy: f64) -> Matrix {
    Matrix {
        e: matrix.a.mul_add(dx, matrix.c * dy) + matrix.e,
        f: matrix.b.mul_add(dx, matrix.d * dy) + matrix.f,
        ..matrix
    }
}

fn replace_span(decoded: &[u8], start: usize, end: usize, with: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(decoded.len() + with.len());
    out.extend_from_slice(&decoded[..start]);
    out.extend_from_slice(with);
    out.extend_from_slice(&decoded[end..]);
    out
}

#[expect(
    clippy::too_many_arguments,
    reason = "one edit's inputs, threaded as its neighbours are"
)]
fn apply_pins(
    decoded: &[u8],
    start: usize,
    end: usize,
    replacement: &[u8],
    operations: &[pdf_content::Operation],
    graph: &pdf_paint::PaintGraph,
    normalization: &[(usize, Matrix)],
    atom: &pdf_paint::PaintAtom,
) -> Result<Vec<u8>, SpikeError> {
    let mut pins = Vec::new();
    for (pinned_ordinal, matrix) in normalization {
        let pinned = graph
            .atoms
            .get(*pinned_ordinal)
            .ok_or(SpikeError::NoTextRun)?;
        if pinned.id.stream != atom.id.stream {
            return Err(SpikeError::NormalizationNotNeutral);
        }
        let operation = operations
            .iter()
            .find(|operation| operation.operator_span() == pinned.id.operator_span)
            .ok_or(SpikeError::NoTextRun)?;
        let at = operation.span().start();
        if at >= start && at < end {
            return Err(SpikeError::NormalizationNotNeutral);
        }
        pins.push((
            at,
            format!(
                " {} {} {} {} {} {} Tm ",
                matrix.a, matrix.b, matrix.c, matrix.d, matrix.e, matrix.f
            )
            .into_bytes(),
        ));
    }
    pins.sort_by_key(|(at, _)| *at);

    let mut out = Vec::with_capacity(decoded.len() + replacement.len() + 64 * pins.len());
    let mut cursor = 0_usize;
    let mut replaced = false;
    for (at, bytes) in &pins {
        if !replaced && *at >= end {
            out.extend_from_slice(&decoded[cursor..start]);
            out.extend_from_slice(replacement);
            cursor = end;
            replaced = true;
        }
        out.extend_from_slice(&decoded[cursor..*at]);
        out.extend_from_slice(bytes);
        cursor = *at;
    }
    if !replaced {
        out.extend_from_slice(&decoded[cursor..start]);
        out.extend_from_slice(replacement);
        cursor = end;
    }
    out.extend_from_slice(&decoded[cursor..]);
    Ok(out)
}

fn prove_split_neutral(
    program: &pdf_content::PageProgram,
    stream_index: usize,
    neutral: &[u8],
    original: &pdf_paint::PaintGraph,
    fonts: crate::Fonts<'_>,
) -> Result<(), SpikeError> {
    let candidate = ByteStore::new(
        SourceId::new(
            program.streams[stream_index]
                .bytes
                .id()
                .get()
                .wrapping_add(1),
        ),
        std::sync::Arc::<[u8]>::from(neutral.to_vec()),
    );
    let mut sources: Vec<&ByteStore> = program.streams.iter().map(|stream| &stream.bytes).collect();
    sources[stream_index] = &candidate;
    let operations = parse_operation_sequence_strict(&sources, ContentLimits::default())
        .map_err(|_| SpikeError::SplitNotNeutral)?;
    let paint_streams: Vec<_> = program
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
    let rewritten = pdf_paint::interpret_stream_sequence_with_fonts(
        &paint_streams,
        program.page,
        &[],
        &program.resources,
        PaintLimits::default(),
        fonts.cloned(),
    )
    .map_err(|_| SpikeError::SplitNotNeutral)?;

    if pdf_paint::glyph_placement_signature(original)
        != pdf_paint::glyph_placement_signature(&rewritten)
    {
        return Err(SpikeError::SplitNotNeutral);
    }
    Ok(())
}

fn cluster_region(
    text: &TextShowPaint,
    glyphs: &Range<usize>,
    dx: f64,
    dy: f64,
) -> Option<[f64; 4]> {
    let to_user = text.state.ctm.value.multiply(text.matrices.text.value);
    let shift = [
        to_user.a.mul_add(dx, to_user.c * dy),
        to_user.b.mul_add(dx, to_user.d * dy),
    ];
    text.outline_bounds_in(glyphs.clone()).map(|before| {
        [
            before[0].min(before[0] + shift[0]),
            before[1].min(before[1] + shift[1]),
            before[2].max(before[2] + shift[0]),
            before[3].max(before[3] + shift[1]),
        ]
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::plan::{GlyphChange, RunRewrite};
    use pdf_bytes::{ByteStore, SourceId};
    use pdf_content::{
        ContentLimits, PageContentLimits, load_page_program_strict, parse_operation_sequence_strict,
    };
    use pdf_paint::{
        PaintAtomKind, PaintLimits, PaintStream, interpret_stream_sequence_with_resources,
    };

    use crate::plan::{Capability, Command, SourceAnchor, TextRunSelection};
    use crate::spike_move_text::{SpikeError, plan_command};

    #[test]
    fn unknown_ink_cannot_erase_the_known_part_of_a_selection() {
        use super::{Ink, combine_regions};
        let bounds = [10.0, 20.0, 30.0, 40.0];
        assert!(matches!(
            combine_regions([Ink::Within(bounds), Ink::Unknown].iter()),
            Err(SpikeError::RunExtentUnknown)
        ));
        assert!(matches!(
            combine_regions([Ink::Unknown, Ink::Within(bounds)].iter()),
            Err(SpikeError::RunExtentUnknown)
        ));
        assert_eq!(
            combine_regions([Ink::Within(bounds), Ink::None].iter()).expect("bounded"),
            Some(bounds)
        );
        assert_eq!(
            combine_regions([Ink::Unknown, Ink::None].iter()).expect("unproved legacy path"),
            None
        );
    }

    fn marked_line() -> ByteStore {
        let content = b"BT /F1 12 Tf 1 0 0 1 10 20 Tm <414243> Tj ET";
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /FirstChar 65 /LastChar 67 /Widths [600 0 600] >> >> >> >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        );
        bytes.extend_from_slice(content);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        let xref = bytes.len();
        bytes.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
        );
        for offset in &offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                offsets.len() + 1
            )
            .as_bytes(),
        );
        ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes))
    }

    fn plan(glyphs: std::ops::Range<usize>, dx: f64) -> Result<crate::plan::Plan, SpikeError> {
        let source = marked_line();
        plan_command(
            &source,
            &Command::MoveTextCluster {
                page_index: 0,
                selection: TextRunSelection::Last,
                glyphs,
                dx,
                dy: 0.0,
            },
            b"",
        )
    }

    fn written(plan: &crate::plan::Plan) -> String {
        match &plan.writes()[0].body {
            crate::plan::PlannedBody::ReplacedStream { decoded } => {
                String::from_utf8_lossy(decoded).into_owned()
            }
            other => panic!("expected a replaced stream, got {other:?}"),
        }
    }

    #[test]
    fn moving_one_cluster_leaves_the_rest_of_the_run_where_it_was() {
        let plan = plan(0..2, 5.0).expect("a cluster of this run is movable");
        assert_eq!(plan.capability(), Capability::Normalized);
        let text = written(&plan);
        assert!(
            text.contains("<4142>"),
            "base and mark stay together: {text}"
        );
        assert!(text.contains("<43>"), "the tail is its own show: {text}");
        assert!(text.contains("1 0 0 1 15 20 Tm"), "moved cluster: {text}");
        assert!(text.contains("1 0 0 1 17.2 20 Tm"), "pinned tail: {text}");
    }

    #[test]
    fn moving_the_last_cluster_leaves_the_first_at_its_original_matrix() {
        let plan = plan(2..3, 5.0).expect("the last cluster is movable");
        let text = written(&plan);
        assert!(text.contains("1 0 0 1 10 20 Tm"), "head unmoved: {text}");
        assert!(
            text.contains("1 0 0 1 22.2 20 Tm"),
            "tail moved by 5: {text}"
        );
    }

    fn delete(glyphs: std::ops::Range<usize>) -> Result<crate::plan::Plan, SpikeError> {
        plan_command(
            &marked_line(),
            &Command::DeleteTextClusters {
                page_index: 0,
                selection: TextRunSelection::Last,
                glyphs,
            },
            b"",
        )
    }

    #[test]
    fn deleting_a_cluster_leaves_the_rest_of_the_run_exactly_where_it_was() {
        let plan = delete(0..2).expect("a cluster of this run is deletable");
        let text = written(&plan);
        assert!(!text.contains("<4142>"), "the cluster is gone: {text}");
        assert!(text.contains("<43>"), "the rest is not: {text}");
        assert!(
            text.contains("1 0 0 1 17.2 20 Tm"),
            "the survivor keeps its own position: {text}"
        );
        assert!(
            !text.contains("1 0 0 1 10 20 Tm [<43>]"),
            "the text must not close up; that is reflow: {text}"
        );
    }

    #[test]
    fn deleting_the_last_cluster_leaves_the_first_untouched() {
        let plan = delete(2..3).expect("the last cluster is deletable");
        let text = written(&plan);
        assert!(text.contains("<4142>"));
        assert!(!text.contains("<43>"));
        assert!(text.contains("1 0 0 1 10 20 Tm"));
    }

    #[test]
    fn a_delete_obeys_the_same_cluster_rule_as_a_move() {
        assert!(matches!(
            delete(0..1),
            Err(SpikeError::GlyphRangeIsNotACluster)
        ));
        assert!(matches!(
            delete(1..2),
            Err(SpikeError::GlyphRangeIsNotACluster)
        ));
        assert!(matches!(
            delete(1..3),
            Err(SpikeError::GlyphRangeIsNotACluster)
        ));
    }

    #[test]
    fn deleting_a_selection_of_several_clusters_is_one_command_and_one_split() {
        let source = stateful_row();
        let plan = plan_command(
            &source,
            &Command::DeleteTextClusters {
                page_index: 0,
                selection: TextRunSelection::Last,
                glyphs: 0..3,
            },
            b"",
        )
        .expect("two whole clusters are one selection");
        assert_eq!(plan.capability(), Capability::Normalized);
        let text = written(&plan);
        assert!(!text.contains("<414220>"), "the selection is gone: {text}");
        assert!(
            text.contains("2 0 0 2 82.6 20 Tm [<43>] TJ"),
            "the survivor keeps its own pen position: {text}"
        );
        assert_eq!(text.matches("Tm").count(), 2, "{text}");
        for parameter in ["Tc", "Tw", "Tz", "TL", "Ts", "Tf"] {
            assert_eq!(
                text.matches(parameter).count(),
                1,
                "{parameter} stays where the file set it: {text}"
            );
        }
    }

    #[test]
    fn a_delete_is_still_proved_neutral_on_the_pieces_it_keeps() {
        assert!(delete(0..2).is_ok());
        assert!(delete(2..3).is_ok());
        let text = written(&delete(0..3).expect("a whole run is a selection too"));
        assert!(!text.contains("Tj") && !text.contains("TJ"), "{text}");
    }

    #[test]
    fn deleting_two_runs_is_one_proved_stream_rewrite() {
        let source =
            page_with_content(b"BT /F1 12 Tf 1 0 0 1 10 20 Tm (A) Tj 1 0 0 1 20 20 Tm (C) Tj ET");
        let runs = whole_runs(&source, false);
        assert_eq!(runs.len(), 2, "the instrument must find two runs");
        let plan = plan_command(
            &source,
            &Command::RewriteText {
                page_index: 0,
                runs,
            },
            b"",
        )
        .expect("the two absolute runs are deleted together");
        assert_eq!(plan.writes().len(), 1, "one stream, one atomic write");
        let text = written(&plan);
        assert!(!text.contains("Tj") && !text.contains("TJ"), "{text}");
        assert_eq!(plan.effect().moved.len(), 2);
    }

    #[test]
    fn a_multi_run_delete_that_moves_a_survivor_is_refused() {
        let source = page_with_content(b"BT /F1 12 Tf 1 0 0 1 10 20 Tm (A) Tj (C) Tj ET");
        let first = whole_runs(&source, false).remove(0);
        assert!(matches!(
            plan_command(
                &source,
                &Command::RewriteText {
                    page_index: 0,
                    runs: vec![first],
                },
                b"",
            ),
            Err(SpikeError::DeleteNotIsolated)
        ));
    }

    fn page_with_content(content: &[u8]) -> ByteStore {
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /FirstChar 65 /LastChar 67 /Widths [600 600 600] >> >> >> >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        );
        bytes.extend_from_slice(content);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        let xref = bytes.len();
        bytes.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
        );
        for offset in &offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                offsets.len() + 1
            )
            .as_bytes(),
        );
        ByteStore::new(SourceId::new(91), Arc::<[u8]>::from(bytes))
    }

    const SPACED_CFF: &str = "0100040100010101055465737400010101131d00000030111d000000560f1d0000005d100001010106616c70686100000004010102101e1f0e8b8b15f8888b8bf888fc888b050e8b8b15f8888b8bf888fc888b050e0e000022018700010003414243";

    fn three_kinds_of_run() -> ByteStore {
        let program: Vec<u8> = SPACED_CFF
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect();
        let content: &[u8] = b"BT /F1 24 Tf 1 0 0 1 10 150 Tm (A) Tj ET                                BT /F1 24 Tf 1 0 0 1 10 100 Tm (C) Tj ET                                BT /F1 24 Tf 1 0 0 1 10 50 Tm (D) Tj ET";
        let stream = |body: &[u8], extra: &str| {
            let mut out = format!("<< /Length {} {extra} >>\nstream\n", body.len()).into_bytes();
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendstream");
            out
        };
        let objects = [
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /MediaBox [0 0 200 200] /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
            stream(content, ""),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Test /Encoding << /Differences [65 /A /alpha /space /ghost] >> /FirstChar 65 /LastChar 68 /Widths [600 600 600 600] /FontDescriptor 6 0 R >>".to_vec(),
            b"<< /Type /FontDescriptor /FontName /Test /Flags 4 /FontFile3 7 0 R >>".to_vec(),
            stream(&program, "/Subtype /Type1C"),
        ];
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (index, body) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
            bytes.extend_from_slice(body);
            bytes.extend_from_slice(b"\nendobj\n");
        }
        let xref = bytes.len();
        bytes.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
        );
        for offset in offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        ByteStore::new(SourceId::new(717), Arc::<[u8]>::from(bytes))
    }

    fn text_runs(source: &ByteStore) -> Vec<pdf_paint::TextShowPaint> {
        let program = load_page_program_strict(source, 0, PageContentLimits::default())
            .expect("the fixture is a page");
        let sources: Vec<&ByteStore> = program.streams.iter().map(|stream| &stream.bytes).collect();
        let operations = parse_operation_sequence_strict(&sources, ContentLimits::default())
            .expect("the fixture content parses");
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
        .expect("the fixture paints");
        graph
            .atoms
            .iter()
            .filter_map(|atom| match &atom.kind {
                PaintAtomKind::Text(text) => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    #[allow(
        clippy::float_cmp,
        reason = "the box is carried through unchanged, so the comparison is identity"
    )]
    fn a_retyped_run_tells_no_ink_apart_from_no_answer() {
        use super::{Ink, ink_of, retyped_region};
        let source = three_kinds_of_run();
        let runs = text_runs(&source);
        assert_eq!(runs.len(), 3, "one show operation of each kind");
        let (drawn, blank, missing) = (&runs[0], &runs[1], &runs[2]);

        let Ink::Within(box_of_a) = ink_of(drawn) else {
            panic!("the square glyph draws");
        };
        assert!(matches!(ink_of(blank), Ink::None), "a space resolves");
        assert!(
            matches!(ink_of(missing), Ink::Unknown),
            "a code with no glyph is not an empty box"
        );

        assert!(matches!(retyped_region(drawn, blank), Ink::Within(bounds) if bounds == box_of_a));
        assert!(matches!(retyped_region(blank, drawn), Ink::Within(bounds) if bounds == box_of_a));
        assert!(matches!(retyped_region(blank, blank), Ink::None));
        assert!(matches!(retyped_region(drawn, missing), Ink::Unknown));
        assert!(matches!(retyped_region(missing, drawn), Ink::Unknown));
        assert!(matches!(retyped_region(blank, missing), Ink::Unknown));
        assert!(matches!(retyped_region(missing, missing), Ink::Unknown));
    }

    fn whole_runs(source: &ByteStore, close_gap: bool) -> Vec<RunRewrite> {
        let program = load_page_program_strict(source, 0, PageContentLimits::default())
            .expect("the fixture is a page");
        let sources: Vec<&ByteStore> = program.streams.iter().map(|stream| &stream.bytes).collect();
        let operations = parse_operation_sequence_strict(&sources, ContentLimits::default())
            .expect("the fixture content parses");
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
        .expect("the fixture paints");
        graph
            .atoms
            .iter()
            .filter_map(|atom| match &atom.kind {
                PaintAtomKind::Text(text) => Some(RunRewrite {
                    anchor: SourceAnchor::of(&atom.id),
                    glyphs: Some(GlyphChange::Remove {
                        glyphs: 0..text.glyphs.len(),
                        close_gap,
                    }),
                    displace: (0.0, 0.0),
                }),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_range_that_splits_a_cluster_is_refused_by_name() {
        assert!(matches!(
            plan(0..1, 5.0),
            Err(SpikeError::GlyphRangeIsNotACluster)
        ));
        assert!(matches!(
            plan(1..2, 5.0),
            Err(SpikeError::GlyphRangeIsNotACluster)
        ));
        assert!(matches!(
            plan(0..3, 5.0),
            Err(SpikeError::GlyphRangeIsNotACluster)
        ));
    }

    #[test]
    fn a_range_outside_the_run_is_refused_before_anything_is_written() {
        assert!(matches!(
            plan(0..9, 5.0),
            Err(SpikeError::GlyphRangeOutsideRun)
        ));
        assert!(matches!(
            plan(3..3, 5.0),
            Err(SpikeError::GlyphRangeOutsideRun)
        ));
    }

    #[test]
    fn the_split_is_proved_neutral_before_the_offset_is_applied() {
        let plan = plan(0..2, 0.0).expect("a zero move is still a plan");
        let text = written(&plan);
        assert!(text.contains("1 0 0 1 10 20 Tm"), "cluster unmoved: {text}");
        assert!(text.contains("1 0 0 1 17.2 20 Tm"), "tail unmoved: {text}");
    }

    fn line_then_next_line() -> ByteStore {
        let content = b"BT /F1 12 Tf 14 TL 1 0 0 1 10 20 Tm <414243> Tj T* <4143> Tj ET";
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /FirstChar 65 /LastChar 67 /Widths [600 0 600] >> >> >> >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        );
        bytes.extend_from_slice(content);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        let xref = bytes.len();
        bytes.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
        );
        for offset in &offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                offsets.len() + 1
            )
            .as_bytes(),
        );
        ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes))
    }

    #[test]
    fn a_split_that_would_move_the_next_line_is_refused_rather_than_written() {
        let source = line_then_next_line();
        let refused = plan_command(
            &source,
            &Command::DeleteTextClusters {
                page_index: 0,
                selection: TextRunSelection::Ordinal(0),
                glyphs: 0..2,
            },
            b"",
        );
        assert!(
            matches!(refused, Err(SpikeError::SplitNotNeutral)),
            "{refused:?}"
        );
        let plan = plan_command(
            &source,
            &Command::DeleteTextClusters {
                page_index: 0,
                selection: TextRunSelection::Last,
                glyphs: 0..1,
            },
            b"",
        );
        assert!(plan.is_ok(), "{plan:?}");
    }

    fn stateful_row() -> ByteStore {
        let mut widths = [250; 36];
        widths[65 - 32] = 600;
        widths[66 - 32] = 0;
        widths[67 - 32] = 600;
        let widths: Vec<String> = widths.iter().map(ToString::to_string).collect();
        let content = b"BT /F1 12 Tf 3 Tc 5 Tw 150 Tz 14 TL 2 Ts 2 0 0 2 10 20 Tm <41422043> Tj ET";
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        offsets.push(bytes.len());
        bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 400 200] /Kids [3 0 R] /Count 1 >>\nendobj\n",
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            format!(
                "3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /FirstChar 32 /LastChar 67 /Widths [{}] >> >> >> >>\nendobj\n",
                widths.join(" ")
            )
            .as_bytes(),
        );
        offsets.push(bytes.len());
        bytes.extend_from_slice(
            format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        );
        bytes.extend_from_slice(content);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        let xref = bytes.len();
        bytes.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
        );
        for offset in &offsets {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                offsets.len() + 1
            )
            .as_bytes(),
        );
        ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes))
    }

    #[test]
    fn g05_a_split_under_live_text_state_preserves_every_parameter() {
        let source = stateful_row();
        let plan = plan_command(
            &source,
            &Command::MoveTextCluster {
                page_index: 0,
                selection: TextRunSelection::Last,
                glyphs: 3..4,
                dx: 5.0,
                dy: 0.0,
            },
            b"",
        )
        .expect("the last cluster of a stateful row is movable");
        assert_eq!(plan.capability(), Capability::Normalized);
        let text = written(&plan);
        for parameter in ["Tc", "Tw", "Tz", "TL", "Ts", "Tf"] {
            assert_eq!(
                text.matches(parameter).count(),
                1,
                "{parameter} must appear only where the file set it: {text}"
            );
        }

        assert!(
            text.contains("2 0 0 2 92.6 20 Tm"),
            "the moved cluster starts at 82.6 + 2*5: {text}"
        );
        assert!(
            text.contains("2 0 0 2 10 20 Tm [<414220>] TJ"),
            "the head keeps the matrix the file gave it: {text}"
        );

        let head = plan_command(
            &source,
            &Command::MoveTextCluster {
                page_index: 0,
                selection: TextRunSelection::Last,
                glyphs: 0..2,
                dx: 0.0,
                dy: 0.0,
            },
            b"",
        )
        .expect("the first cluster is movable too");
        assert!(
            written(&head).contains("2 0 0 2 49.599999999999994 20 Tm"),
            "the cursor is restated exactly: {}",
            written(&head)
        );
    }

    #[test]
    fn g05_dropping_a_text_state_parameter_is_detected() {
        let source = stateful_row();
        let (program, graph, start, end) = program_and_graph(&source);
        let decoded = program.streams[0].bytes.as_bytes();

        let with_reset =
            b" 2 0 0 2 10 20 Tm [<4142>] TJ 0 Tc 2 0 0 2 49.599999999999994 20 Tm [<2043>] TJ "
                .to_vec();
        assert!(
            matches!(
                super::prove_split_neutral(
                    &program,
                    0,
                    &super::replace_span(decoded, start, end, &with_reset),
                    &graph,
                    None,
                ),
                Err(SpikeError::SplitNotNeutral)
            ),
            "resetting character spacing must be detected"
        );

        let without_word_spacing =
            b" 2 0 0 2 10 20 Tm [<4142>] TJ 0 Tw 2 0 0 2 49.599999999999994 20 Tm [<2043>] TJ "
                .to_vec();
        assert!(
            matches!(
                super::prove_split_neutral(
                    &program,
                    0,
                    &super::replace_span(decoded, start, end, &without_word_spacing),
                    &graph,
                    None,
                ),
                Err(SpikeError::SplitNotNeutral)
            ),
            "resetting word spacing must be detected"
        );

        let untouched =
            b" 2 0 0 2 10 20 Tm [<4142>] TJ 2 0 0 2 49.599999999999994 20 Tm [<2043>] TJ ".to_vec();
        assert!(
            super::prove_split_neutral(
                &program,
                0,
                &super::replace_span(decoded, start, end, &untouched),
                &graph,
                None,
            )
            .is_ok(),
            "the split with no parameter dropped must be accepted"
        );
    }

    fn program_and_graph(
        source: &ByteStore,
    ) -> (
        pdf_content::PageProgram,
        pdf_paint::PaintGraph,
        usize,
        usize,
    ) {
        use pdf_content::{
            ContentLimits, PageContentLimits, load_page_program_with_password,
            parse_operation_sequence_strict,
        };
        use pdf_paint::{PaintLimits, PaintStream, interpret_stream_sequence_with_resources};

        let program =
            load_page_program_with_password(source, 0, PageContentLimits::default(), b"").unwrap();
        let bytes: Vec<&ByteStore> = program.streams.iter().map(|s| &s.bytes).collect();
        let operations = parse_operation_sequence_strict(&bytes, ContentLimits::default()).unwrap();
        let streams: Vec<_> = program
            .streams
            .iter()
            .zip(&operations)
            .map(|(stream, operations)| PaintStream {
                source: &stream.bytes,
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
        .unwrap();
        let show = operations[0]
            .iter()
            .find(|operation| operation.operator_span() == graph.atoms[0].id.operator_span)
            .unwrap();
        let (start, end) = (show.span().start(), show.span().end());
        (program, graph, start, end)
    }

    #[test]
    fn the_neutrality_proof_refuses_a_split_that_paints_differently() {
        let source = marked_line();
        let (program, graph, start, end) = program_and_graph(&source);
        let decoded = program.streams[0].bytes.as_bytes();

        let wrong = b" 1 0 0 1 10 20 Tm [<4142>] TJ 1 0 0 1 18.2 20 Tm [<43>] TJ ".to_vec();
        assert!(matches!(
            super::prove_split_neutral(
                &program,
                0,
                &super::replace_span(decoded, start, end, &wrong),
                &graph,
                None,
            ),
            Err(SpikeError::SplitNotNeutral)
        ));

        let dropped = b" 1 0 0 1 10 20 Tm [<4142>] TJ ".to_vec();
        assert!(matches!(
            super::prove_split_neutral(
                &program,
                0,
                &super::replace_span(decoded, start, end, &dropped),
                &graph,
                None,
            ),
            Err(SpikeError::SplitNotNeutral)
        ));

        let right = b" 1 0 0 1 10 20 Tm [<4142>] TJ 1 0 0 1 17.2 20 Tm [<43>] TJ ".to_vec();
        assert!(
            super::prove_split_neutral(
                &program,
                0,
                &super::replace_span(decoded, start, end, &right),
                &graph,
                None,
            )
            .is_ok(),
            "the correct split must be accepted, or the proof refuses everything"
        );
    }

    #[test]
    fn every_piece_keeps_the_codes_the_original_carried() {
        let plan = plan(2..3, 1.0).expect("plannable");
        let text = written(&plan);
        let codes: String = text
            .chars()
            .filter(|character| {
                character.is_ascii_hexdigit() || *character == '<' || *character == '>'
            })
            .collect();
        assert!(codes.contains("<4142>") && codes.contains("<43>"), "{text}");
    }
}

#[cfg(test)]
mod group_delete_tests {
    use pdf_bytes::ByteStore;
    use pdf_paint::PaintAtomKind;

    use crate::block_move::tests::{page_with, read};
    use crate::history::History;
    use crate::plan::{Command, GlyphChange, Plan, RunRewrite, SourceAnchor};
    use crate::spike_move_text::{PlannerPage, SpikeError, plan_command_in};

    fn diagram() -> ByteStore {
        page_with(
            b"BT /F1 12 Tf 1 0 0 1 20 150 Tm (A) Tj ET\n\
              BT /F1 12 Tf 1 0 0 1 20 110 Tm (A) Tj ET\n\
              10 10 40 30 re f\n\
              100 100 40 30 re f",
        )
    }

    fn painted(source: &ByteStore) -> (usize, usize) {
        let read = read(source);
        let text = read
            .graph
            .atoms
            .iter()
            .filter(|atom| matches!(atom.kind, PaintAtomKind::Text(_)))
            .count();
        let paths = read
            .graph
            .atoms
            .iter()
            .filter(|atom| matches!(atom.kind, PaintAtomKind::Path(_)))
            .count();
        (text, paths)
    }

    fn members(source: &ByteStore) -> (Vec<RunRewrite>, Vec<SourceAnchor>) {
        let read = read(source);
        let mut runs = Vec::new();
        let mut objects = Vec::new();
        for atom in &read.graph.atoms {
            match &atom.kind {
                PaintAtomKind::Text(text) => runs.push(RunRewrite {
                    anchor: SourceAnchor::of(&atom.id),
                    glyphs: Some(GlyphChange::Remove {
                        glyphs: 0..text.glyphs.len(),
                        close_gap: false,
                    }),
                    displace: (0.0, 0.0),
                }),
                PaintAtomKind::Path(_) => objects.push(SourceAnchor::of(&atom.id)),
                _ => {}
            }
        }
        (runs, objects)
    }

    fn plan(source: &ByteStore, command: &Command) -> Result<Plan, SpikeError> {
        let read = read(source);
        plan_command_in(
            source,
            PlannerPage {
                program: &read.program,
                operations: &read.operations,
                graph: &read.graph,
                fonts: None,
                restrictions: crate::Restrictions::Respect,
                credential: b"",
            },
            command,
        )
    }

    #[test]
    fn the_diagram_paints_two_labels_and_two_boxes() {
        assert_eq!(painted(&diagram()), (2, 2));
    }

    #[test]
    fn a_group_of_four_is_deleted_at_once_and_one_undo_brings_all_four_back() {
        let source = diagram();
        let (runs, objects) = members(&source);
        assert_eq!(runs.len() + objects.len(), 4, "the band caught four things");

        let plan = plan(
            &source,
            &Command::DeleteGroup {
                page_index: 0,
                runs,
                objects,
            },
        )
        .expect("a group of text and drawings is deletable");
        let mut history = History::new(source.clone(), b"");
        history.apply(plan).expect("it commits");

        assert_eq!(
            painted(history.source()),
            (0, 0),
            "four were chosen and fewer than four went"
        );
        assert_eq!(history.undo_depth(), 1, "one gesture, one step");
        assert!(history.undo().expect("it undoes"), "there was a step");
        assert_eq!(
            painted(history.source()),
            (2, 2),
            "one undo did not bring all four back"
        );
    }

    #[test]
    fn a_command_that_deletes_one_thing_does_not_meet_the_count() {
        let source = diagram();
        let (runs, objects) = members(&source);

        let one = plan(
            &source,
            &Command::RemoveObject {
                page_index: 0,
                target: objects[0].clone(),
            },
        )
        .expect("one box is removable");
        let mut history = History::new(source.clone(), b"");
        history.apply(one).expect("it commits");
        assert_eq!(
            painted(history.source()),
            (2, 1),
            "the one-object command is not the group command"
        );

        let text_only = plan(
            &source,
            &Command::RewriteText {
                page_index: 0,
                runs,
            },
        )
        .expect("text");
        let mut history = History::new(source, b"");
        history.apply(text_only).expect("it commits");
        assert_eq!(
            painted(history.source()),
            (0, 2),
            "a text-only delete left the drawings, which is the bug"
        );
    }

    #[test]
    fn a_group_of_drawings_alone_is_one_command() {
        let source = page_with(
            b"10 10 40 30 re f\n\
              60 10 40 30 re f\n\
              110 10 40 30 re f",
        );
        assert_eq!(painted(&source), (0, 3));
        let (_, objects) = members(&source);

        let plan = plan(
            &source,
            &Command::DeleteGroup {
                page_index: 0,
                runs: Vec::new(),
                objects,
            },
        )
        .expect("three drawings are deletable together");
        let mut history = History::new(source, b"");
        history.apply(plan).expect("it commits");
        assert_eq!(painted(history.source()), (0, 0));
        assert_eq!(history.undo_depth(), 1);
        assert!(history.undo().expect("it undoes"));
        assert_eq!(painted(history.source()), (0, 3));
    }

    #[test]
    fn a_member_that_cannot_be_deleted_refuses_the_whole_group() {
        let source = diagram();
        let (runs, mut objects) = members(&source);
        objects.push(runs[0].anchor.clone());

        let refused = plan(
            &source,
            &Command::DeleteGroup {
                page_index: 0,
                runs,
                objects,
            },
        );
        assert!(
            matches!(refused, Err(SpikeError::ObjectIsText)),
            "the group was not refused by name: {refused:?}"
        );
        assert_eq!(painted(&source), (2, 2), "a refused group changed the page");
    }

    #[test]
    fn a_member_named_twice_is_refused() {
        let source = diagram();
        let (runs, mut objects) = members(&source);
        objects.push(objects[0].clone());

        let refused = plan(
            &source,
            &Command::DeleteGroup {
                page_index: 0,
                runs,
                objects,
            },
        );
        assert!(
            matches!(refused, Err(SpikeError::ObjectNamedMoreThanOnce)),
            "{refused:?}"
        );
    }

    #[test]
    fn a_group_naming_nothing_is_refused() {
        let source = diagram();
        let refused = plan(
            &source,
            &Command::DeleteGroup {
                page_index: 0,
                runs: Vec::new(),
                objects: Vec::new(),
            },
        );
        assert!(
            matches!(refused, Err(SpikeError::SelectionNamesNoRun)),
            "{refused:?}"
        );
    }
}
