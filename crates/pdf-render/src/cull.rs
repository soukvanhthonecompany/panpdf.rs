use pdf_paint::{GlyphPath, GlyphSegment, Matrix, PaintAtom, PaintAtomKind, Path, PathSegment};

use crate::{apply, matrix_scale};

pub(crate) const SLACK: f64 = 1.5;

pub(crate) fn meets(box_of: [f64; 4], bounds: (u32, u32, u32, u32)) -> bool {
    let (x0, y0, x1, y1) = bounds;
    if !box_of.iter().all(|edge| edge.is_finite()) {
        return true;
    }
    box_of[0] - SLACK < f64::from(x1)
        && f64::from(x0) < box_of[2] + SLACK
        && box_of[1] - SLACK < f64::from(y1)
        && f64::from(y0) < box_of[3] + SLACK
}

pub(crate) fn atom_box(atom: &PaintAtom, device: Matrix) -> Option<[f64; 4]> {
    match &atom.kind {
        PaintAtomKind::Path(paint) => {
            let matrix = device.multiply(paint.state.ctm.value);
            let box_of = path_box(&paint.path, matrix)?;
            if !paint.stroke {
                return Some(box_of);
            }
            let pen = matrix_scale(matrix);
            let miter = paint.state.miter_limit.value.max(1.0);
            let grown = (paint.state.line_width.value * pen).abs() * miter + pen;
            Some(grown_by(box_of, grown))
        }
        PaintAtomKind::Image(image) => {
            let matrix = device.multiply(image.state.ctm.value);
            Some(corners_box(
                &[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                matrix,
            ))
        }
        PaintAtomKind::Text(_)
        | PaintAtomKind::Shading(_)
        | PaintAtomKind::TransparencyGroup(_) => None,
    }
}

pub(crate) fn outline_box(outline: &GlyphPath, matrix: Matrix) -> Option<[f64; 4]> {
    let mut box_of: Option<[f64; 4]> = None;
    for segment in &outline.segments {
        match *segment {
            GlyphSegment::MoveTo { x, y } | GlyphSegment::LineTo { x, y } => {
                take(&mut box_of, apply(matrix, [x, y]));
            }
            GlyphSegment::CurveTo {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => {
                take(&mut box_of, apply(matrix, [x1, y1]));
                take(&mut box_of, apply(matrix, [x2, y2]));
                take(&mut box_of, apply(matrix, [x, y]));
            }
            GlyphSegment::Close => {}
        }
    }
    box_of
}

pub(crate) fn placed_box(box_of: [f64; 4], matrix: Matrix) -> [f64; 4] {
    corners_box(
        &[
            [box_of[0], box_of[1]],
            [box_of[2], box_of[1]],
            [box_of[2], box_of[3]],
            [box_of[0], box_of[3]],
        ],
        matrix,
    )
}

fn path_box(path: &Path, matrix: Matrix) -> Option<[f64; 4]> {
    let mut box_of: Option<[f64; 4]> = None;
    for segment in &path.segments {
        match segment {
            PathSegment::MoveTo { point, .. } | PathSegment::LineTo { point, .. } => {
                take(&mut box_of, apply(matrix, [point.x, point.y]));
            }
            PathSegment::CubicTo {
                control_1,
                control_2,
                end,
                ..
            } => {
                take(&mut box_of, apply(matrix, [control_1.x, control_1.y]));
                take(&mut box_of, apply(matrix, [control_2.x, control_2.y]));
                take(&mut box_of, apply(matrix, [end.x, end.y]));
            }
            PathSegment::Rectangle {
                origin,
                width,
                height,
                ..
            } => {
                for corner in [
                    [origin.x, origin.y],
                    [origin.x + width, origin.y],
                    [origin.x + width, origin.y + height],
                    [origin.x, origin.y + height],
                ] {
                    take(&mut box_of, apply(matrix, corner));
                }
            }
            PathSegment::ClosePath { .. } => {}
        }
    }
    box_of
}

fn corners_box(corners: &[[f64; 2]], matrix: Matrix) -> [f64; 4] {
    let mut box_of: Option<[f64; 4]> = None;
    for corner in corners {
        take(&mut box_of, apply(matrix, *corner));
    }
    box_of.unwrap_or([0.0; 4])
}

fn take(box_of: &mut Option<[f64; 4]>, [x, y]: [f64; 2]) {
    match box_of {
        Some(box_of) => {
            box_of[0] = box_of[0].min(x);
            box_of[1] = box_of[1].min(y);
            box_of[2] = box_of[2].max(x);
            box_of[3] = box_of[3].max(y);
        }
        None => *box_of = Some([x, y, x, y]),
    }
}

fn grown_by(box_of: [f64; 4], by: f64) -> [f64; 4] {
    [
        box_of[0] - by,
        box_of[1] - by,
        box_of[2] + by,
        box_of[3] + by,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNIT: Matrix = Matrix {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    #[test]
    fn a_box_beyond_the_canvas_does_not_meet_it() {
        assert!(!meets([600.0, 0.0, 700.0, 100.0], (0, 0, 512, 512)));
        assert!(!meets([0.0, 600.0, 100.0, 700.0], (0, 0, 512, 512)));
        assert!(meets([500.0, 0.0, 700.0, 100.0], (0, 0, 512, 512)));
        assert!(meets([0.0, 0.0, 10.0, 10.0], (0, 0, 512, 512)));
    }

    #[test]
    fn a_box_at_the_edge_meets_the_canvas() {
        assert!(meets([512.0, 0.0, 600.0, 100.0], (0, 0, 512, 512)));
        assert!(meets([-100.0, 0.0, 0.0, 100.0], (0, 0, 512, 512)));
        assert!(!meets([513.6, 0.0, 600.0, 100.0], (0, 0, 512, 512)));
    }

    #[test]
    fn a_box_that_is_not_a_number_meets_everything() {
        assert!(meets([f64::NAN, 0.0, 1.0, 1.0], (0, 0, 8, 8)));
        assert!(meets([f64::INFINITY, 0.0, 1.0, 1.0], (0, 0, 8, 8)));
    }

    fn same(one: [f64; 4], other: [f64; 4]) -> bool {
        one.iter()
            .zip(other)
            .all(|(one, other)| (one - other).abs() < 1e-12)
    }

    #[test]
    fn a_curve_is_bounded_by_its_control_points() {
        let span = pdf_bytes::SourceSpan::new(pdf_bytes::SourceId::new(1), 0, 1).expect("a span");
        let path = Path {
            segments: vec![
                PathSegment::MoveTo {
                    point: pdf_paint::Point { x: 0.0, y: 0.0 },
                    provenance: span,
                },
                PathSegment::CubicTo {
                    control_1: pdf_paint::Point { x: 10.0, y: 40.0 },
                    control_2: pdf_paint::Point { x: 20.0, y: -5.0 },
                    end: pdf_paint::Point { x: 30.0, y: 0.0 },
                    provenance: span,
                },
            ],
        };
        assert!(path_box(&path, UNIT).is_some_and(|box_of| same(box_of, [0.0, -5.0, 30.0, 40.0])));
    }

    #[test]
    fn a_rectangle_names_all_four_of_its_corners() {
        let span = pdf_bytes::SourceSpan::new(pdf_bytes::SourceId::new(1), 0, 1).expect("a span");
        let path = Path {
            segments: vec![PathSegment::Rectangle {
                origin: pdf_paint::Point { x: 5.0, y: 7.0 },
                width: -3.0,
                height: 4.0,
                provenance: span,
            }],
        };
        assert!(path_box(&path, UNIT).is_some_and(|box_of| same(box_of, [2.0, 7.0, 5.0, 11.0])));
    }

    #[test]
    fn a_picture_is_the_square_its_matrix_makes() {
        let placed = Matrix {
            a: 100.0,
            b: 0.0,
            c: 0.0,
            d: 50.0,
            e: 20.0,
            f: 30.0,
        };
        let corners = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        assert!(same(
            corners_box(&corners, placed),
            [20.0, 30.0, 120.0, 80.0]
        ));
    }
}
