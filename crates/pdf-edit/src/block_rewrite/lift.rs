use super::{BTreeSet, PaintAtomKind, PaintGraph};
use crate::spike_move_text::PlannerPage;

pub(super) type ClipEdit = (usize, usize, Vec<u8>);

pub(super) fn inert_clips(
    page: PlannerPage<'_>,
    stream_index: usize,
    graph: &PaintGraph,
    named: &BTreeSet<usize>,
) -> Vec<ClipEdit> {
    let mut candidates: Vec<(pdf_bytes::SourceSpan, &pdf_paint::ClipPath)> = Vec::new();
    for ordinal in named {
        if let Some(PaintAtomKind::Text(text)) = graph.atoms.get(*ordinal).map(|atom| &atom.kind) {
            for clip in &text.state.clip_paths {
                if !candidates.iter().any(|(had, _)| *had == clip.provenance) {
                    candidates.push((clip.provenance, clip));
                }
            }
        }
    }
    let decoded = page.program.streams[stream_index].bytes.as_bytes();
    candidates
        .into_iter()
        .filter_map(|(provenance, clip)| {
            let operation = page.operations[stream_index]
                .iter()
                .find(|operation| operation.span() == provenance)?;
            let span = (operation.span().start(), operation.span().end());
            if !matches!(decoded.get(span.0..span.1), Some(b"W" | b"W*")) {
                return None;
            }
            let region = crate::clip_region::ClipRegion::of(&clip.path, clip.ctm.value).ok()?;
            let crop = page.program.geometry.crop_box;
            let hides_nothing = graph.atoms.iter().all(|atom| {
                let state = state_of(&atom.kind);
                !state
                    .clip_paths
                    .iter()
                    .any(|held| held.provenance == provenance)
                    || matches!(&atom.kind, PaintAtomKind::Text(text) if text.draws_no_ink())
                    || extent_of(&atom.kind).is_some_and(|bounds| match shown(bounds, crop) {
                        None => true,
                        Some(visible) => region.admits(&corners(visible)),
                    })
            });
            hides_nothing.then_some((span.0, span.1, b" ".to_vec()))
        })
        .collect()
}

pub(super) fn grown_clips(
    page: PlannerPage<'_>,
    stream_index: usize,
    graph: &PaintGraph,
    named: &BTreeSet<usize>,
    needed: [f64; 4],
) -> Option<Vec<ClipEdit>> {
    let mut candidates: Vec<&pdf_paint::ClipPath> = Vec::new();
    for ordinal in named {
        if let Some(PaintAtomKind::Text(text)) = graph.atoms.get(*ordinal).map(|atom| &atom.kind) {
            for clip in &text.state.clip_paths {
                if !candidates
                    .iter()
                    .any(|had| had.provenance == clip.provenance)
                {
                    candidates.push(clip);
                }
            }
        }
    }
    let mut edits = Vec::new();
    for clip in candidates {
        let region = crate::clip_region::ClipRegion::of(&clip.path, clip.ctm.value).ok()?;
        if region.admits(&corners(needed)) {
            continue;
        }
        edits.push(grow(page, stream_index, graph, (clip, &region), needed)?);
    }
    (!edits.is_empty()).then_some(edits)
}

fn grow(
    page: PlannerPage<'_>,
    stream_index: usize,
    graph: &PaintGraph,
    (clip, region): (&pdf_paint::ClipPath, &crate::clip_region::ClipRegion),
    needed: [f64; 4],
) -> Option<ClipEdit> {
    let operations = &page.operations[stream_index];
    let decoded = page.program.streams[stream_index].bytes.as_bytes();
    let ctm = clip.ctm.value;
    let [
        pdf_paint::PathSegment::Rectangle {
            origin,
            width,
            height,
            ..
        },
    ] = clip.path.segments.as_slice()
    else {
        return None;
    };
    let at = operations
        .iter()
        .position(|operation| operation.span() == clip.provenance)?;
    let rectangle = operations.get(at.checked_sub(1)?)?;
    let operator = rectangle.operator_span();
    if ctm.b.abs() > 1e-9
        || ctm.c.abs() > 1e-9
        || !matches!(
            decoded.get(clip.provenance.start()..clip.provenance.end()),
            Some(b"W" | b"W*")
        )
        || decoded.get(operator.start()..operator.end()) != Some(b"re")
    {
        return None;
    }
    let one = ctm.transform(*origin);
    let other = ctm.transform(pdf_paint::Point {
        x: origin.x + width,
        y: origin.y + height,
    });
    let grown = [
        one.x.min(other.x).min(needed[0]),
        one.y.min(other.y).min(needed[1]),
        one.x.max(other.x).max(needed[2]),
        one.y.max(other.y).max(needed[3]),
    ];
    if shows_more(page, graph, (clip, region), grown) {
        return None;
    }
    let inverse = ctm.inverse()?;
    let low = inverse.transform(pdf_paint::Point {
        x: grown[0],
        y: grown[1],
    });
    let high = inverse.transform(pdf_paint::Point {
        x: grown[2],
        y: grown[3],
    });
    Some((
        rectangle.span().start(),
        rectangle.span().end(),
        format!(
            "{} {} {} {} re",
            low.x.min(high.x),
            low.y.min(high.y),
            (high.x - low.x).abs(),
            (high.y - low.y).abs()
        )
        .into_bytes(),
    ))
}

fn shows_more(
    page: PlannerPage<'_>,
    graph: &PaintGraph,
    (clip, region): (&pdf_paint::ClipPath, &crate::clip_region::ClipRegion),
    grown: [f64; 4],
) -> bool {
    let crop = page.program.geometry.crop_box;
    graph.atoms.iter().any(|atom| {
        if !state_of(&atom.kind)
            .clip_paths
            .iter()
            .any(|held| held.provenance == clip.provenance)
            || matches!(&atom.kind, PaintAtomKind::Text(text) if text.draws_no_ink())
        {
            return false;
        }
        let Some(bounds) = extent_of(&atom.kind) else {
            return true;
        };
        let inside = [
            bounds[0].max(grown[0]),
            bounds[1].max(grown[1]),
            bounds[2].min(grown[2]),
            bounds[3].min(grown[3]),
        ];
        if inside[0] > inside[2] || inside[1] > inside[3] {
            return false;
        }
        shown(inside, crop).is_some_and(|visible| !region.admits(&corners(visible)))
    })
}

fn state_of(kind: &PaintAtomKind) -> &pdf_paint::GraphicsState {
    match kind {
        PaintAtomKind::Path(paint) => &paint.state,
        PaintAtomKind::Text(text) => &text.state,
        PaintAtomKind::TransparencyGroup(group) => &group.state,
        PaintAtomKind::Shading(shading) => &shading.state,
        PaintAtomKind::Image(image) => &image.state,
    }
}

fn shown(bounds: [f64; 4], crop: [f64; 4]) -> Option<[f64; 4]> {
    let visible = [
        bounds[0].max(crop[0]) + UNSEEN_OVERHANG,
        bounds[1].max(crop[1]) + UNSEEN_OVERHANG,
        bounds[2].min(crop[2]) - UNSEEN_OVERHANG,
        bounds[3].min(crop[3]) - UNSEEN_OVERHANG,
    ];
    (visible[0] <= visible[2] && visible[1] <= visible[3]).then_some(visible)
}

const UNSEEN_OVERHANG: f64 = 0.05;

fn corners(bounds: [f64; 4]) -> [pdf_paint::Point; 4] {
    let point = |x, y| pdf_paint::Point { x, y };
    [
        point(bounds[0], bounds[1]),
        point(bounds[2], bounds[1]),
        point(bounds[2], bounds[3]),
        point(bounds[0], bounds[3]),
    ]
}

fn extent_of(kind: &PaintAtomKind) -> Option<[f64; 4]> {
    let mut bounds = kind.user_bounds()?;
    if let PaintAtomKind::Path(paint) = kind
        && paint.stroke
    {
        let ctm = paint.state.ctm.value;
        let scale = (ctm.a.abs() + ctm.b.abs()).max(ctm.c.abs() + ctm.d.abs());
        let reach = paint.state.line_width.value.abs().max(1.0) * scale / 2.0;
        bounds = [
            bounds[0] - reach,
            bounds[1] - reach,
            bounds[2] + reach,
            bounds[3] + reach,
        ];
    }
    bounds
        .iter()
        .all(|value| value.is_finite())
        .then_some(bounds)
}
