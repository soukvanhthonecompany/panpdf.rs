use std::path::{Path, PathBuf};
use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_paint::Point;
use pdf_semantics::{ObjectKind, Quad, QuadEvidence, SemanticIndex};

fn name(kind: ObjectKind) -> &'static str {
    match kind {
        ObjectKind::Text(_) => "text",
        ObjectKind::Image => "picture",
        ObjectKind::Form => "form",
        ObjectKind::Shading => "shading",
        ObjectKind::Path => "drawing",
        ObjectKind::TextRun => "text run",
    }
}

fn upright(corners: &[Point; 4]) -> bool {
    let flat =
        |one: Point, other: Point| (one.x - other.x).abs() < 1e-6 || (one.y - other.y).abs() < 1e-6;
    (0..4).all(|index| flat(corners[index], corners[(index + 1) % 4]))
}

fn area(corners: &[Point; 4]) -> f64 {
    let mut twice = 0.0;
    for index in 0..4 {
        let one = corners[index];
        let other = corners[(index + 1) % 4];
        twice += one.x.mul_add(other.y, -(other.x * one.y));
    }
    (twice / 2.0).abs()
}

fn wasted(quad: &Quad) -> f64 {
    let box_ = quad.bounds();
    let boxed = (box_[2] - box_[0]) * (box_[3] - box_[1]);
    if boxed <= 0.0 {
        return 0.0;
    }
    1.0 - area(&quad.corners) / boxed
}

fn report(path: &Path, index: &SemanticIndex) {
    let pictures = index
        .objects
        .iter()
        .filter(|object| object.kind == ObjectKind::Image)
        .count();
    println!(
        "{}: {} objects, {pictures} of them pictures",
        path.display(),
        index.objects.len()
    );
    for (position, object) in index.objects.iter().enumerate() {
        let Some(quad) = object.quad.as_ref() else {
            println!("  {position:>3} {:<8} no extent", name(object.kind));
            continue;
        };
        let corners = quad
            .corners
            .iter()
            .map(|corner| format!("({:.1}, {:.1})", corner.x, corner.y))
            .collect::<Vec<_>>()
            .join(" ");
        println!(
            "  {position:>3} {:<8} {:<9} {:<8} {corners}",
            name(object.kind),
            match quad.evidence {
                QuadEvidence::Placement => "placement",
                QuadEvidence::Ink => "ink",
            },
            if upright(&quad.corners) {
                "upright".to_string()
            } else {
                format!("TURNED {:.0}%", 100.0 * wasted(quad))
            },
        );
    }
    println!(
        "  paths left out: {}, drawings not grouped: {}, objects with no extent: {}",
        index.report.paths_outside_any_object,
        index.report.drawings_not_grouped,
        index.report.objects_without_extent
    );
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let target = PathBuf::from(arguments.next().expect("a pdf file or a directory"));
    let summary = arguments.any(|flag| flag == "--summary");

    let mut paths: Vec<PathBuf> = if target.is_dir() {
        let mut found: Vec<PathBuf> = std::fs::read_dir(&target)
            .expect("readdir")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
            })
            .collect();
        found.sort();
        found
    } else {
        vec![target]
    };
    paths.dedup();

    let (mut pages, mut objects, mut pictures, mut forms, mut shadings) = (0, 0, 0, 0, 0);
    let (mut drawings, mut runs) = (0, 0);
    let (mut turned, mut paths_left_out) = (0, 0);
    let (mut text_without_extent, mut other_without_extent) = (0, 0);
    let (mut wasted_total, mut wasted_worst) = (0.0_f64, 0.0_f64);
    let one_file = paths.len() == 1;
    for path in &paths {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if one_file => panic!("{}: {error}", path.display()),
            Err(_) => continue,
        };
        let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
        let Ok(view) = pdf_session::interpret_page(&source, 0) else {
            continue;
        };
        pages += 1;
        objects += view.index.objects.len();
        for object in &view.index.objects {
            match object.kind {
                ObjectKind::Image => pictures += 1,
                ObjectKind::Form => forms += 1,
                ObjectKind::Shading => shadings += 1,
                ObjectKind::Path => drawings += 1,
                ObjectKind::TextRun => runs += 1,
                ObjectKind::Text(_) => {}
            }
            match object.quad.as_ref() {
                None if matches!(object.kind, ObjectKind::Text(_) | ObjectKind::TextRun) => {
                    text_without_extent += 1;
                }
                None => other_without_extent += 1,
                Some(quad) if !upright(&quad.corners) => {
                    turned += 1;
                    let empty = wasted(quad);
                    wasted_total += empty;
                    wasted_worst = wasted_worst.max(empty);
                }
                Some(_) => {}
            }
        }
        paths_left_out += view.index.report.paths_outside_any_object;
        if !summary {
            report(path, &view.index);
        }
    }
    if summary || paths.len() > 1 {
        println!(
            "\n{pages} pages: {objects} objects -- {pictures} pictures, {forms} forms, \
             {shadings} shadings, {drawings} drawings, {runs} unclustered runs, \
             the rest text"
        );
        println!(
            "  framed by a turned quad: {turned}, whose boxes are {:.0}% empty on average \
             and {:.0}% at worst",
            100.0 * wasted_total / f64::from(u32::try_from(turned.max(1)).unwrap_or(1)),
            100.0 * wasted_worst
        );
        println!(
            "  no extent to frame:      {text_without_extent} text (no font program), \
             {other_without_extent} other"
        );
        println!("  path atoms left out:     {paths_left_out}");
    }
}
