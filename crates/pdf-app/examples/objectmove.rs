use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::{Command, FixedPoint, ObjectSelection, SourceAnchor};
use pdf_paint::Matrix;

const TRAVEL: f64 = 5.0;

const OVERLAY_SCALE: f64 = 1.0;

fn points_of(path: &pdf_paint::Path) -> Vec<pdf_paint::Point> {
    let mut out = Vec::new();
    for segment in &path.segments {
        match segment {
            pdf_paint::PathSegment::MoveTo { point, .. }
            | pdf_paint::PathSegment::LineTo { point, .. } => out.push(*point),
            pdf_paint::PathSegment::CubicTo {
                control_1,
                control_2,
                end,
                ..
            } => out.extend([*control_1, *control_2, *end]),
            pdf_paint::PathSegment::Rectangle {
                origin,
                width,
                height,
                ..
            } => out.extend([
                *origin,
                pdf_paint::Point {
                    x: origin.x + width,
                    y: origin.y + height,
                },
            ]),
            pdf_paint::PathSegment::ClosePath { .. } => {}
        }
    }
    out
}

fn shown(atom: &pdf_paint::PaintAtom) -> Option<f64> {
    let box_of = atom.kind.user_bounds()?;
    let state = match &atom.kind {
        pdf_paint::PaintAtomKind::Image(image) => &image.state,
        pdf_paint::PaintAtomKind::Path(paint) => &paint.state,
        pdf_paint::PaintAtomKind::Shading(shading) => &shading.state,
        pdf_paint::PaintAtomKind::TransparencyGroup(group) => &group.state,
        pdf_paint::PaintAtomKind::Text(_) => return None,
    };
    let mut held = box_of;
    for clip in &state.clip_paths {
        let mut bounds: Option<[f64; 4]> = None;
        for point in points_of(&clip.path) {
            let at = clip.ctm.value.transform(point);
            bounds = Some(match bounds {
                None => [at.x, at.y, at.x, at.y],
                Some(b) => [
                    b[0].min(at.x),
                    b[1].min(at.y),
                    b[2].max(at.x),
                    b[3].max(at.y),
                ],
            });
        }
        let Some(bounds) = bounds else { continue };
        held = [
            held[0].max(bounds[0]),
            held[1].max(bounds[1]),
            held[2].min(bounds[2]),
            held[3].min(bounds[3]),
        ];
    }
    let along = |low: usize, high: usize| {
        let reach = box_of[high] - box_of[low];
        let left = (held[high] - held[low]).max(0.0);
        if reach > 0.0 {
            left / reach
        } else if held[high] >= held[low] {
            1.0
        } else {
            0.0
        }
    };
    Some(along(0, 2) * along(1, 3))
}

fn escape(text: &str) -> String {
    let mut out = String::new();
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' | '\r' | '\t' => out.push(' '),
            other if (other as u32) < 0x20 => out.push(' '),
            other => out.push(other),
        }
    }
    out
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = arguments.next().expect("a PDF path");
    let page_argument = arguments.next().unwrap_or_else(|| "spread".to_owned());
    let Ok(bytes) = std::fs::read(&path) else {
        println!(
            "{{\"file\":\"{}\",\"error\":\"unreadable\"}}",
            escape(&path)
        );
        return;
    };
    let source = ByteStore::new(SourceId::new(7), Arc::<[u8]>::from(bytes));
    let mut session = pdf_session::Session::with_fonts(source, b"", pdf_cli::font_provider());
    session.set_aside_restrictions();
    let count = session.page_count().unwrap_or(0);
    let pages: Vec<usize> = if page_argument == "spread" {
        let mut chosen: Vec<usize> = [1, 2, 3].iter().map(|part| count * part / 4).collect();
        chosen.dedup();
        chosen
    } else {
        vec![
            page_argument
                .parse::<usize>()
                .unwrap_or(1)
                .saturating_sub(1),
        ]
    };
    for page in pages {
        scan_page(&mut session, &path, page);
    }
}

fn scan_page(session: &mut pdf_session::Session, path: &str, page: usize) {
    let Ok(view) = session.page(page) else {
        println!(
            "{{\"file\":\"{}\",\"page\":{page},\"error\":\"the page does not read\"}}",
            escape(path)
        );
        return;
    };
    let Ok(overlay) = pdf_cli::page_overlay_view(&view, OVERLAY_SCALE) else {
        println!(
            "{{\"file\":\"{}\",\"page\":{page},\"error\":\"the page has no size\"}}",
            escape(path)
        );
        return;
    };
    let offset = pdf_cli::page_offset_view(&view, OVERLAY_SCALE, TRAVEL, 0.0);
    let atoms = view.graph.atoms.clone();
    let index = view.index.clone();
    drop(view);
    let Ok((dx, dy)) = offset else {
        println!(
            "{{\"file\":\"{}\",\"page\":{page},\"error\":\"the page cannot be measured\"}}",
            escape(path)
        );
        return;
    };
    if overlay.objects.is_empty() {
        println!(
            "{{\"file\":\"{}\",\"page\":{page},\"error\":\"no objects offered\"}}",
            escape(path)
        );
        return;
    }
    for object in &overlay.objects {
        let Some(decoded) = SourceAnchor::decode(&object.anchor) else {
            continue;
        };
        let seen = index
            .objects
            .get(object.object)
            .and_then(|held| held.members.first())
            .and_then(|member| atoms.get(member.atom))
            .and_then(shown)
            .unwrap_or(1.0);
        let verdict = match session.plan(&Command::PlaceObject {
            page_index: page,
            target: ObjectSelection::Painted(decoded),
            transform: Matrix {
                e: dx,
                f: dy,
                ..Matrix::IDENTITY
            },
            about: FixedPoint::Origin,
        }) {
            Ok(_) => "ok".to_owned(),
            Err(error) => format!("refused: {error}"),
        };
        println!(
            "{{\"file\":\"{}\",\"page\":{page},\"object\":{},\"kind\":\"{}\",\"shown\":{seen:.4},\"move\":\"{}\"}}",
            escape(path),
            object.object,
            escape(&format!("{:?}", object.kind)),
            escape(&verdict)
        );
    }
}
