use pdf_paint::{Matrix, Path, PathSegment, Point};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Row {
    pub top: f64,
    pub bottom: f64,
    pub left: f64,
    pub right: f64,
}

pub enum Shape<'a> {
    Placed(Matrix),
    Drawn(&'a Path, Matrix),
    Box([f64; 4]),
}

const CURVE_TOLERANCE: f64 = 0.25;

const MAX_CURVE_DEPTH: u32 = 16;

const MAX_BANDS: usize = 100_000;

#[must_use]
pub fn rows_of(shapes: &[Shape<'_>], frame: [f64; 4], margin: f64, step: f64) -> Vec<Row> {
    let Some((fx0, fy0, fx1, fy1)) = frame_bounds(frame) else {
        return Vec::new();
    };
    if !step.is_finite() || step <= 0.0 || !margin.is_finite() {
        return Vec::new();
    }
    let frame_width = fx1 - fx0;
    let outlines: Vec<Vec<(Point, Point)>> = shapes.iter().map(outline).collect();
    let mut rows = Vec::new();
    for (band_top, band_bottom) in cut_bands(fy1, fy0, step) {
        let mut band_rows: Vec<Row> = outlines
            .iter()
            .filter_map(|edges| {
                row_in_band(edges, band_top, band_bottom, margin, fx0, frame_width, fy1)
            })
            .collect();
        band_rows.sort_by(|left, right| left.left.total_cmp(&right.left));
        rows.extend(band_rows);
    }
    rows
}

#[allow(
    clippy::too_many_arguments,
    reason = "one call site, all in frame or band coordinates -- splitting it into a struct would be a second name for the same six numbers"
)]
fn row_in_band(
    edges: &[(Point, Point)],
    band_top: f64,
    band_bottom: f64,
    margin: f64,
    fx0: f64,
    frame_width: f64,
    fy1: f64,
) -> Option<Row> {
    let (ymin, ymax) = vertical_extent(edges)?;
    let scope_bottom = ymin - margin;
    let scope_top = ymax + margin;
    if band_top < scope_bottom || band_bottom > scope_top {
        return None;
    }
    let clamp_bottom = band_bottom.clamp(ymin, ymax);
    let clamp_top = band_top.clamp(ymin, ymax);
    let mut left = f64::INFINITY;
    let mut right = f64::NEG_INFINITY;
    for &(a, b) in edges {
        if let Some((x_low, x_high)) = edge_x_extent(a, b, clamp_bottom, clamp_top) {
            left = left.min(x_low);
            right = right.max(x_high);
        }
    }
    if !(left.is_finite() && right.is_finite()) {
        return None;
    }
    let row_left = (left - margin - fx0).clamp(0.0, frame_width);
    let row_right = (right + margin - fx0).clamp(0.0, frame_width);
    if row_right <= row_left {
        return None;
    }
    Some(Row {
        top: fy1 - band_top,
        bottom: fy1 - band_bottom,
        left: row_left,
        right: row_right,
    })
}

fn frame_bounds(frame: [f64; 4]) -> Option<(f64, f64, f64, f64)> {
    let [a, b, c, d] = frame;
    if ![a, b, c, d].iter().all(|value| value.is_finite()) {
        return None;
    }
    let (x0, x1) = (a.min(c), a.max(c));
    let (y0, y1) = (b.min(d), b.max(d));
    (x1 > x0 && y1 > y0).then_some((x0, y0, x1, y1))
}

fn cut_bands(top: f64, bottom: f64, step: f64) -> Vec<(f64, f64)> {
    let mut bands = Vec::new();
    let mut band_top = top;
    while band_top > bottom && bands.len() < MAX_BANDS {
        let band_bottom = (band_top - step).max(bottom);
        bands.push((band_top, band_bottom));
        band_top = band_bottom;
    }
    bands
}

fn outline(shape: &Shape<'_>) -> Vec<(Point, Point)> {
    let subpaths = match shape {
        Shape::Box(rect) => Some(vec![box_corners(*rect)]),
        Shape::Placed(matrix) => matrix.inverse().map(|_| vec![placed_corners(*matrix)]),
        Shape::Drawn(path, matrix) => {
            if path.segments.is_empty() || matrix.inverse().is_none() {
                None
            } else {
                Some(flatten_path(path, *matrix))
            }
        }
    };
    let Some(subpaths) = subpaths else {
        return Vec::new();
    };
    if subpaths
        .iter()
        .flatten()
        .any(|point: &Point| !point.x.is_finite() || !point.y.is_finite())
    {
        return Vec::new();
    }
    subpaths
        .iter()
        .flat_map(|points| edges_of(points))
        .collect()
}

fn box_corners(rect: [f64; 4]) -> Vec<Point> {
    let (x0, x1) = (rect[0].min(rect[2]), rect[0].max(rect[2]));
    let (y0, y1) = (rect[1].min(rect[3]), rect[1].max(rect[3]));
    close(&[
        Point { x: x0, y: y0 },
        Point { x: x1, y: y0 },
        Point { x: x1, y: y1 },
        Point { x: x0, y: y1 },
    ])
}

fn placed_corners(matrix: Matrix) -> Vec<Point> {
    let unit = [
        Point { x: 0.0, y: 0.0 },
        Point { x: 1.0, y: 0.0 },
        Point { x: 1.0, y: 1.0 },
        Point { x: 0.0, y: 1.0 },
    ];
    close(&unit.map(|point| matrix.transform(point)))
}

fn close(points: &[Point]) -> Vec<Point> {
    let mut closed = points.to_vec();
    if let Some(&first) = points.first() {
        closed.push(first);
    }
    closed
}

fn edges_of(points: &[Point]) -> Vec<(Point, Point)> {
    points.windows(2).map(|pair| (pair[0], pair[1])).collect()
}

fn vertical_extent(edges: &[(Point, Point)]) -> Option<(f64, f64)> {
    let mut ymin = f64::INFINITY;
    let mut ymax = f64::NEG_INFINITY;
    for &(a, b) in edges {
        ymin = ymin.min(a.y).min(b.y);
        ymax = ymax.max(a.y).max(b.y);
    }
    (ymin.is_finite() && ymax.is_finite()).then_some((ymin, ymax))
}

#[allow(
    clippy::float_cmp,
    reason = "a horizontal edge -- both ends sharing one y -- is a real \
              geometric case, not a closeness question; what routes to it is \
              avoiding a division by zero, not a tolerance"
)]
fn edge_x_extent(a: Point, b: Point, band_bottom: f64, band_top: f64) -> Option<(f64, f64)> {
    let (y_low, y_high) = (a.y.min(b.y), a.y.max(b.y));
    if y_high < band_bottom || y_low > band_top {
        return None;
    }
    if a.y == b.y {
        return Some((a.x.min(b.x), a.x.max(b.x)));
    }
    let x_at = |y: f64| a.x + (y - a.y) / (b.y - a.y) * (b.x - a.x);
    let clip_low = x_at(y_low.max(band_bottom));
    let clip_high = x_at(y_high.min(band_top));
    Some((clip_low.min(clip_high), clip_low.max(clip_high)))
}

fn flatten_path(path: &Path, matrix: Matrix) -> Vec<Vec<Point>> {
    let mut subpaths = Vec::new();
    let mut current: Vec<Point> = Vec::new();
    let mut start: Option<Point> = None;

    for segment in &path.segments {
        match segment {
            PathSegment::MoveTo { point, .. } => {
                finish(&mut current, &mut subpaths);
                let point = matrix.transform(*point);
                current.push(point);
                start = Some(point);
            }
            PathSegment::LineTo { point, .. } => {
                let point = matrix.transform(*point);
                if current.is_empty() {
                    start = Some(point);
                }
                current.push(point);
            }
            PathSegment::CubicTo {
                control_1,
                control_2,
                end,
                ..
            } => {
                let end = matrix.transform(*end);
                let Some(&from) = current.last() else {
                    start.get_or_insert(end);
                    current.push(end);
                    continue;
                };
                let control_1 = matrix.transform(*control_1);
                let control_2 = matrix.transform(*control_2);
                flatten_cubic(from, control_1, control_2, end, 0, &mut current);
            }
            PathSegment::ClosePath { .. } => {
                if let Some(first) = start {
                    if current.last() != Some(&first) {
                        current.push(first);
                    }
                    finish(&mut current, &mut subpaths);
                    current.push(first);
                } else {
                    finish(&mut current, &mut subpaths);
                }
            }
            PathSegment::Rectangle {
                origin,
                width,
                height,
                ..
            } => {
                finish(&mut current, &mut subpaths);
                let origin = *origin;
                let (width, height) = (*width, *height);
                let corners = [
                    origin,
                    Point {
                        x: origin.x + width,
                        y: origin.y,
                    },
                    Point {
                        x: origin.x + width,
                        y: origin.y + height,
                    },
                    Point {
                        x: origin.x,
                        y: origin.y + height,
                    },
                    origin,
                ];
                subpaths.push(corners.map(|point| matrix.transform(point)).to_vec());
                let placed_origin = matrix.transform(origin);
                current.push(placed_origin);
                start = Some(placed_origin);
            }
        }
    }
    finish(&mut current, &mut subpaths);
    subpaths
}

fn finish(current: &mut Vec<Point>, subpaths: &mut Vec<Vec<Point>>) {
    if current.len() >= 2 {
        subpaths.push(std::mem::take(current));
    } else {
        current.clear();
    }
}

#[allow(
    clippy::similar_names,
    reason = "these are De Casteljau's own names: p01 is the midpoint of p0 \
              and p1, so its name says which; distinct names would answer a \
              different, less useful question"
)]
fn flatten_cubic(p0: Point, p1: Point, p2: Point, p3: Point, depth: u32, out: &mut Vec<Point>) {
    if depth >= MAX_CURVE_DEPTH || is_flat(p0, p1, p2, p3) {
        out.push(p3);
        return;
    }
    let p01 = midpoint(p0, p1);
    let p12 = midpoint(p1, p2);
    let p23 = midpoint(p2, p3);
    let p012 = midpoint(p01, p12);
    let p123 = midpoint(p12, p23);
    let p0123 = midpoint(p012, p123);
    flatten_cubic(p0, p01, p012, p0123, depth + 1, out);
    flatten_cubic(p0123, p123, p23, p3, depth + 1, out);
}

fn midpoint(a: Point, b: Point) -> Point {
    Point {
        x: f64::midpoint(a.x, b.x),
        y: f64::midpoint(a.y, b.y),
    }
}

fn is_flat(p0: Point, p1: Point, p2: Point, p3: Point) -> bool {
    let dx = p3.x - p0.x;
    let dy = p3.y - p0.y;
    let chord2 = dx * dx + dy * dy;
    if chord2 < 1e-9 {
        let d1 = (p1.x - p0.x).hypot(p1.y - p0.y);
        let d2 = (p2.x - p0.x).hypot(p2.y - p0.y);
        return d1 <= CURVE_TOLERANCE && d2 <= CURVE_TOLERANCE;
    }
    let chord = chord2.sqrt();
    let distance = |p: Point| ((p.x - p0.x) * dy - (p.y - p0.y) * dx).abs() / chord;
    distance(p1) <= CURVE_TOLERANCE && distance(p2) <= CURVE_TOLERANCE
}

pub const WRAP_ROW: f64 = 10.0;

pub const KEEP_CLEAR: f64 = 3.0;

#[must_use]
pub fn shapes_over(graph: &pdf_paint::PaintGraph, frame: [f64; 4]) -> Vec<Shape<'_>> {
    let mut shapes = Vec::new();
    for atom in &graph.atoms {
        let Some(bounds) = atom.kind.user_bounds() else {
            continue;
        };
        if !overlaps(bounds, frame) {
            continue;
        }
        match &atom.kind {
            pdf_paint::PaintAtomKind::Text(_) => {}
            pdf_paint::PaintAtomKind::Path(paint) => {
                shapes.push(Shape::Drawn(&paint.path, paint.state.ctm.value));
            }
            pdf_paint::PaintAtomKind::Image(_)
            | pdf_paint::PaintAtomKind::TransparencyGroup(_)
            | pdf_paint::PaintAtomKind::Shading(_) => shapes.push(Shape::Box(bounds)),
        }
    }
    shapes
}

#[must_use]
pub fn blocked_in_frame(
    graph: &pdf_paint::PaintGraph,
    frame: [f64; 4],
    pitch: f64,
) -> Vec<super::lines::Blocked> {
    let step = if pitch.is_finite() && pitch > 0.0 {
        pitch
    } else {
        WRAP_ROW
    };
    rows_of(&shapes_over(graph, frame), frame, KEEP_CLEAR, step)
        .into_iter()
        .map(|row| super::lines::Blocked {
            top: row.top,
            bottom: row.bottom,
            left: row.left,
            right: row.right,
        })
        .collect()
}

#[must_use]
pub fn blocked_for_block(
    graph: &pdf_paint::PaintGraph,
    floor: f64,
    (left, right): (f64, f64),
    (top, pitch): (f64, f64),
) -> Vec<super::lines::Blocked> {
    blocked_in_frame(
        graph,
        [left, floor.min(top - pitch), right, top + pitch],
        pitch,
    )
}

fn overlaps(one: [f64; 4], other: [f64; 4]) -> bool {
    one[0] < other[2] && one[2] > other[0] && one[1] < other[3] && one[3] > other[1]
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    reason = "known answers are whole points and halves of them, which floating point represents exactly; where a curve is flattened the test states its own tolerance"
)]
mod tests {
    use super::{Row, Shape, rows_of};
    use pdf_bytes::{SourceId, SourceSpan};
    use pdf_paint::{Matrix, Path, PathSegment, Point};

    fn span() -> SourceSpan {
        SourceSpan::new(SourceId::new(1), 0, 1).expect("a forward span")
    }

    fn at(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    #[test]
    fn an_upright_rectangle_gives_rows_all_of_the_rectangles_own_span() {
        let shapes = [Shape::Box([50.0, 25.0, 150.0, 75.0])];
        let frame = [0.0, 0.0, 200.0, 100.0];
        let rows = rows_of(&shapes, frame, 0.0, 10.0);
        assert_eq!(rows.len(), 6, "{rows:?}");
        for row in &rows {
            assert_eq!(row.left, 50.0, "{rows:?}");
            assert_eq!(row.right, 150.0, "{rows:?}");
        }
        assert_eq!(rows[0].top, 20.0);
        assert_eq!(rows[rows.len() - 1].bottom, 80.0);
    }

    #[test]
    fn a_margin_widens_every_row_and_adds_one_above_and_below() {
        let shapes = [Shape::Box([50.0, 25.0, 150.0, 75.0])];
        let frame = [0.0, 0.0, 200.0, 100.0];
        let rows = rows_of(&shapes, frame, 6.0, 10.0);
        assert_eq!(rows.len(), 8, "{rows:?}");
        for row in &rows {
            assert_eq!(row.left, 44.0, "{rows:?}");
            assert_eq!(row.right, 156.0, "{rows:?}");
        }
    }

    #[test]
    fn a_diamond_widens_to_its_diagonal_at_the_middle_band_and_narrows_elsewhere() {
        let side = 10.0_f64;
        let (sin, cos) = std::f64::consts::FRAC_PI_4.sin_cos();
        let matrix = Matrix {
            a: side * cos,
            b: side * sin,
            c: -side * sin,
            d: side * cos,
            e: side / 2.0 * (sin - cos),
            f: -side / 2.0 * (sin + cos),
        };
        let shapes = [Shape::Placed(matrix)];
        let frame = [-20.0, -20.0, 20.0, 20.0];
        let rows = rows_of(&shapes, frame, 0.0, 2.0);
        assert!(!rows.is_empty());

        let expected_middle = side * std::f64::consts::SQRT_2;
        let middle_line = 20.0_f64;
        let middle_index = rows
            .iter()
            .position(|row| row.top <= middle_line && row.bottom >= middle_line)
            .expect("a band straddles the centre");
        let middle_span = rows[middle_index].right - rows[middle_index].left;
        assert!(
            (middle_span - expected_middle).abs() < 1e-6,
            "middle span {middle_span} vs {expected_middle}"
        );
        for (index, row) in rows.iter().enumerate() {
            if index == middle_index {
                continue;
            }
            let span = row.right - row.left;
            assert!(
                span <= expected_middle + 1e-9,
                "row {index} ({row:?}) is not narrower than the middle band's {expected_middle}"
            );
        }
    }

    #[test]
    fn a_flattened_circle_matches_its_true_width_to_half_a_point() {
        let radius = 50.0_f64;
        let kappa = 0.552_284_749_830_793_6;
        let k = kappa * radius;
        let cubic = |c1: Point, c2: Point, end: Point| PathSegment::CubicTo {
            control_1: c1,
            control_2: c2,
            end,
            provenance: span(),
        };
        let path = Path {
            segments: vec![
                PathSegment::MoveTo {
                    point: at(radius, 0.0),
                    provenance: span(),
                },
                cubic(at(radius, k), at(k, radius), at(0.0, radius)),
                cubic(at(-k, radius), at(-radius, k), at(-radius, 0.0)),
                cubic(at(-radius, -k), at(-k, -radius), at(0.0, -radius)),
                cubic(at(k, -radius), at(radius, -k), at(radius, 0.0)),
            ],
        };
        let shapes = [Shape::Drawn(&path, Matrix::IDENTITY)];
        let frame = [-100.0, -100.0, 100.0, 100.0];
        let rows = rows_of(&shapes, frame, 0.0, 10.0);

        let centre_line = 100.0_f64;
        let centre_row = rows
            .iter()
            .find(|row| row.top <= centre_line && row.bottom >= centre_line)
            .expect("a band straddles the centre");
        let centre_span = centre_row.right - centre_row.left;
        assert!(
            (centre_span - 100.0).abs() < 0.5,
            "centre span {centre_span}"
        );

        let above_row = rows
            .iter()
            .find(|row| row.bottom == 60.0)
            .expect("a band's near edge sits at y = 40");
        let above_span = above_row.right - above_row.left;
        let expected = 2.0 * (radius * radius - 40.0 * 40.0).sqrt();
        assert!(
            (above_span - expected).abs() < 0.5,
            "span 40 pt above centre {above_span} vs {expected}"
        );
    }

    #[test]
    fn a_shape_left_of_the_frame_gives_no_rows() {
        let shapes = [Shape::Box([-30.0, -5.0, -10.0, 5.0])];
        let frame = [0.0, -20.0, 100.0, 20.0];
        assert!(rows_of(&shapes, frame, 0.0, 10.0).is_empty());
        assert!(rows_of(&shapes, frame, 6.0, 10.0).is_empty());
    }

    #[test]
    fn an_empty_path_and_an_uninvertible_matrix_give_no_rows() {
        let empty = Path { segments: vec![] };
        let flat = Matrix {
            a: 0.0,
            b: 0.0,
            c: 0.0,
            d: 0.0,
            e: 0.0,
            f: 0.0,
        };
        let square = Path {
            segments: vec![PathSegment::Rectangle {
                origin: at(0.0, 0.0),
                width: 10.0,
                height: 10.0,
                provenance: span(),
            }],
        };
        let shapes = [
            Shape::Drawn(&empty, Matrix::IDENTITY),
            Shape::Drawn(&square, flat),
            Shape::Box([50.0, 0.0, 60.0, 10.0]),
        ];
        let frame = [0.0, 0.0, 100.0, 20.0];
        let rows = rows_of(&shapes, frame, 0.0, 20.0);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].left, 50.0);
        assert_eq!(rows[0].right, 60.0);
    }

    #[test]
    fn two_shapes_in_one_band_give_two_rows_not_a_merged_one() {
        let shapes = [
            Shape::Box([0.0, 2.0, 10.0, 8.0]),
            Shape::Box([20.0, 2.0, 30.0, 8.0]),
        ];
        let frame = [0.0, 0.0, 50.0, 20.0];
        let rows = rows_of(&shapes, frame, 0.0, 10.0);
        assert_eq!(
            rows,
            vec![
                Row {
                    top: 10.0,
                    bottom: 20.0,
                    left: 0.0,
                    right: 10.0,
                },
                Row {
                    top: 10.0,
                    bottom: 20.0,
                    left: 20.0,
                    right: 30.0,
                },
            ]
        );
    }
}
