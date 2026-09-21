use std::path::PathBuf;
use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_paint::{PaintAtomKind, Path, PathSegment};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Shape {
    ConvexPolygon,
    ConcavePolygon,
    Curved,
    Several,
    Degenerate,
}

fn shape_of(path: &Path) -> Shape {
    let mut points = 0_usize;
    let mut subpaths = 0_usize;
    let mut curved = false;
    let mut corners: Vec<pdf_paint::Point> = Vec::new();
    for segment in &path.segments {
        match segment {
            PathSegment::Rectangle {
                origin,
                width,
                height,
                ..
            } => {
                subpaths += 1;
                points += 4;
                corners.extend([
                    *origin,
                    pdf_paint::Point {
                        x: origin.x + width,
                        y: origin.y,
                    },
                    pdf_paint::Point {
                        x: origin.x + width,
                        y: origin.y + height,
                    },
                    pdf_paint::Point {
                        x: origin.x,
                        y: origin.y + height,
                    },
                ]);
            }
            PathSegment::MoveTo { point, .. } => {
                subpaths += 1;
                points += 1;
                corners.push(*point);
            }
            PathSegment::LineTo { point, .. } => {
                points += 1;
                corners.push(*point);
            }
            PathSegment::ClosePath { .. } => {}
            PathSegment::CubicTo { .. } => curved = true,
        }
    }
    if subpaths > 1 {
        return Shape::Several;
    }
    if curved {
        return Shape::Curved;
    }
    if subpaths == 0 || points < 3 {
        return Shape::Degenerate;
    }
    if corners
        .last()
        .is_some_and(|last| same_point(*last, corners[0]))
    {
        corners.pop();
    }
    if is_convex(&corners) {
        Shape::ConvexPolygon
    } else {
        Shape::ConcavePolygon
    }
}

const TOLERANCE: f64 = 1e-6;

fn same_point(left: pdf_paint::Point, right: pdf_paint::Point) -> bool {
    (left.x - right.x).abs() < TOLERANCE && (left.y - right.y).abs() < TOLERANCE
}

fn side(a: pdf_paint::Point, b: pdf_paint::Point, p: pdf_paint::Point) -> f64 {
    (b.x - a.x).mul_add(p.y - a.y, -((b.y - a.y) * (p.x - a.x)))
}

fn is_convex(corners: &[pdf_paint::Point]) -> bool {
    if corners.len() < 3 {
        return false;
    }
    let mut sign = 0.0_f64;
    for index in 0..corners.len() {
        let turn = side(
            corners[index],
            corners[(index + 1) % corners.len()],
            corners[(index + 2) % corners.len()],
        );
        if turn.abs() < TOLERANCE {
            continue;
        }
        if sign == 0.0 {
            sign = turn;
        } else if (turn > 0.0) != (sign > 0.0) {
            return false;
        }
    }
    sign != 0.0
}

#[derive(Default)]
struct Tally {
    pages: usize,
    objects: usize,
    unclipped: usize,
    all_convex: usize,
    concave: usize,
    curved: usize,
    several: usize,
    degenerate: usize,
    clips: usize,
    clip_shapes: [usize; 5],
}

fn slot(shape: Shape) -> usize {
    match shape {
        Shape::ConvexPolygon => 0,
        Shape::ConcavePolygon => 1,
        Shape::Curved => 2,
        Shape::Several => 3,
        Shape::Degenerate => 4,
    }
}

fn measure(path: &PathBuf, tally: &mut Tally, verbose: bool) {
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
    let Ok(view) = pdf_session::interpret_page(&source, 0) else {
        return;
    };
    tally.pages += 1;
    for atom in &view.graph.atoms {
        let clips = match &atom.kind {
            PaintAtomKind::Image(paint) => &paint.state.clip_paths,
            PaintAtomKind::Shading(paint) => &paint.state.clip_paths,
            PaintAtomKind::TransparencyGroup(paint) => &paint.state.clip_paths,
            PaintAtomKind::Text(_) | PaintAtomKind::Path(_) => continue,
        };
        tally.objects += 1;
        let shapes: Vec<Shape> = clips.iter().map(|clip| shape_of(&clip.path)).collect();
        tally.clips += shapes.len();
        for shape in &shapes {
            tally.clip_shapes[slot(*shape)] += 1;
        }
        if shapes.is_empty() {
            tally.unclipped += 1;
        } else if let Some(worst) = shapes.iter().find(|shape| **shape != Shape::ConvexPolygon) {
            match worst {
                Shape::ConcavePolygon => tally.concave += 1,
                Shape::Curved => tally.curved += 1,
                Shape::Several => tally.several += 1,
                Shape::Degenerate => tally.degenerate += 1,
                Shape::ConvexPolygon => unreachable!("found by not being one"),
            }
        } else {
            tally.all_convex += 1;
        }
    }
    if verbose {
        println!(
            "{}: {} clipped objects",
            path.display(),
            tally.objects - tally.unclipped
        );
    }
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

    let mut tally = Tally::default();
    for path in &paths {
        measure(path, &mut tally, !summary);
    }
    println!(
        "\n{} pages, {} placeable objects, {} clips on them",
        tally.pages, tally.objects, tally.clips
    );
    println!(
        "  under no clip at all:                  {}",
        tally.unclipped
    );
    println!(
        "  every clip a convex polygon:           {}",
        tally.all_convex
    );
    println!("  refused by a clip that is:");
    println!("    several subpaths                     {}", tally.several);
    println!("    curved                               {}", tally.curved);
    println!("    a concave polygon                    {}", tally.concave);
    println!(
        "    degenerate                           {}",
        tally.degenerate
    );
    println!(
        "\n  the clips themselves: {} convex, {} concave, {} curved, {} several, {} degenerate",
        tally.clip_shapes[0],
        tally.clip_shapes[1],
        tally.clip_shapes[2],
        tally.clip_shapes[3],
        tally.clip_shapes[4]
    );
}

#[cfg(test)]
mod tests {
    use super::{Shape, shape_of};
    use pdf_bytes::{SourceId, SourceSpan};
    use pdf_paint::{Path, PathSegment, Point};

    fn span() -> SourceSpan {
        SourceSpan::new(SourceId::new(1), 0, 1).expect("a forward span")
    }

    fn at(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    fn path(segments: Vec<PathSegment>) -> Path {
        Path { segments }
    }

    fn line(x: f64, y: f64) -> PathSegment {
        PathSegment::LineTo {
            point: at(x, y),
            provenance: span(),
        }
    }

    fn start(x: f64, y: f64) -> PathSegment {
        PathSegment::MoveTo {
            point: at(x, y),
            provenance: span(),
        }
    }

    #[test]
    fn each_shape_is_told_apart_from_the_others() {
        assert_eq!(
            shape_of(&path(vec![PathSegment::Rectangle {
                origin: at(0.0, 0.0),
                width: 10.0,
                height: 10.0,
                provenance: span(),
            }])),
            Shape::ConvexPolygon
        );
        assert_eq!(
            shape_of(&path(vec![
                start(0.0, 0.0),
                line(10.0, 0.0),
                line(5.0, 8.0),
                PathSegment::ClosePath { provenance: span() },
            ])),
            Shape::ConvexPolygon
        );
        assert_eq!(
            shape_of(&path(vec![
                start(0.0, 0.0),
                line(10.0, 0.0),
                line(5.0, 4.0),
                line(10.0, 8.0),
                line(0.0, 8.0),
            ])),
            Shape::ConcavePolygon
        );
        assert_eq!(
            shape_of(&path(vec![
                start(0.0, 0.0),
                PathSegment::CubicTo {
                    control_1: at(1.0, 1.0),
                    control_2: at(2.0, 2.0),
                    end: at(3.0, 0.0),
                    provenance: span(),
                },
            ])),
            Shape::Curved
        );
        assert_eq!(
            shape_of(&path(vec![
                start(0.0, 0.0),
                line(10.0, 0.0),
                line(5.0, 8.0),
                start(20.0, 20.0),
                line(30.0, 20.0),
                line(25.0, 28.0),
            ])),
            Shape::Several
        );
        assert_eq!(
            shape_of(&path(vec![start(0.0, 0.0), line(10.0, 0.0)])),
            Shape::Degenerate
        );
        assert_eq!(shape_of(&path(Vec::new())), Shape::Degenerate);
    }

    #[test]
    fn a_repeated_closing_point_does_not_make_a_square_concave() {
        assert_eq!(
            shape_of(&path(vec![
                start(0.0, 0.0),
                line(10.0, 0.0),
                line(10.0, 10.0),
                line(0.0, 10.0),
                line(0.0, 0.0),
            ])),
            Shape::ConvexPolygon
        );
    }

    #[test]
    fn several_subpaths_outrank_a_curve_because_either_alone_refuses() {
        assert_eq!(
            shape_of(&path(vec![
                start(0.0, 0.0),
                PathSegment::CubicTo {
                    control_1: at(1.0, 1.0),
                    control_2: at(2.0, 2.0),
                    end: at(3.0, 0.0),
                    provenance: span(),
                },
                start(20.0, 20.0),
                line(30.0, 20.0),
                line(25.0, 28.0),
            ])),
            Shape::Several
        );
    }
}
