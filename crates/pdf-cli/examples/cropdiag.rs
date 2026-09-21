use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};

fn overlap(one: [f64; 4], other: [f64; 4]) -> f64 {
    let wide = (one[2].min(other[2]) - one[0].max(other[0])).max(0.0);
    let high = (one[3].min(other[3]) - one[1].max(other[1])).max(0.0);
    let area = (one[2] - one[0]) * (one[3] - one[1]);
    if area <= 0.0 { 0.0 } else { wide * high / area }
}

fn bounds_of(path: &pdf_paint::Path, ctm: pdf_paint::Matrix) -> Option<[f64; 4]> {
    let mut out: Option<[f64; 4]> = None;
    for segment in &path.segments {
        let points: Vec<pdf_paint::Point> = match segment {
            pdf_paint::PathSegment::MoveTo { point, .. }
            | pdf_paint::PathSegment::LineTo { point, .. } => vec![*point],
            pdf_paint::PathSegment::CubicTo {
                control_1,
                control_2,
                end,
                ..
            } => {
                vec![*control_1, *control_2, *end]
            }
            pdf_paint::PathSegment::Rectangle {
                origin,
                width,
                height,
                ..
            } => vec![
                *origin,
                pdf_paint::Point {
                    x: origin.x + width,
                    y: origin.y + height,
                },
            ],
            pdf_paint::PathSegment::ClosePath { .. } => Vec::new(),
        };
        for point in points {
            let at = ctm.transform(point);
            out = Some(match out {
                None => [at.x, at.y, at.x, at.y],
                Some(b) => [
                    b[0].min(at.x),
                    b[1].min(at.y),
                    b[2].max(at.x),
                    b[3].max(at.y),
                ],
            });
        }
    }
    out
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = arguments.next().expect("a PDF path");
    let page = arguments
        .next()
        .and_then(|value| value.parse::<usize>().ok())
        .map_or(0, |value| value.saturating_sub(1));
    let bytes = std::fs::read(&path).expect("the file reads");
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
    let view =
        pdf_session::interpret_page_fully(&source, page, b"", None, pdf_cli::font_provider())
            .expect("the page reads");
    let overlay = pdf_cli::page_overlay_view(&view, 1.0).expect("the page has a size");
    println!(
        "{} objects offered, {} scopes",
        overlay.objects.len(),
        view.graph.object_scopes.len()
    );
    for object in overlay.objects.iter().take(12) {
        let atom = &view.graph.atoms[view.index.objects[object.object].members[0].atom];
        let Some(box_of) = atom.kind.user_bounds() else {
            continue;
        };
        let state = match &atom.kind {
            pdf_paint::PaintAtomKind::Image(image) => &image.state,
            pdf_paint::PaintAtomKind::Path(paint) => &paint.state,
            pdf_paint::PaintAtomKind::Shading(shading) => &shading.state,
            pdf_paint::PaintAtomKind::TransparencyGroup(group) => &group.state,
            pdf_paint::PaintAtomKind::Text(_) => continue,
        };
        let ordinal = view.index.objects[object.object].members[0].atom;
        let scope = view
            .graph
            .object_scopes
            .iter()
            .find(|scope| scope.atoms.start <= ordinal && ordinal < scope.atoms.end);
        println!(
            "object {:3} {:?} box [{:.0} {:.0} {:.0} {:.0}] clips {} scope {:?}",
            object.object,
            object.kind,
            box_of[0],
            box_of[1],
            box_of[2],
            box_of[3],
            state.clip_paths.len(),
            scope.map(|s| (s.atoms.clone(), s.inherited_clips)),
        );
        for (index, clip) in state.clip_paths.iter().enumerate() {
            let shape = bounds_of(&clip.path, clip.ctm.value);
            let subpaths = clip
                .path
                .segments
                .iter()
                .filter(|segment| {
                    matches!(
                        segment,
                        pdf_paint::PathSegment::MoveTo { .. }
                            | pdf_paint::PathSegment::Rectangle { .. }
                    )
                })
                .count();
            let curved = clip
                .path
                .segments
                .iter()
                .any(|segment| matches!(segment, pdf_paint::PathSegment::CubicTo { .. }));
            match shape {
                Some(shape) => println!(
                    "    clip {index}: box [{:.0} {:.0} {:.0} {:.0}] subpaths {subpaths} curved {curved} covers {:.0}% of the object",
                    shape[0],
                    shape[1],
                    shape[2],
                    shape[3],
                    overlap(box_of, shape) * 100.0
                ),
                None => println!("    clip {index}: no bounds"),
            }
        }
    }
}
