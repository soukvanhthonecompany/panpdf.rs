use pdf_paint::{Matrix, Path, PathSegment, Point};

pub(crate) const CLIP_TOLERANCE: f64 = 1e-6;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClipUnreadable {
    Curved,
    Concave,
    Degenerate,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ClipRegion {
    parts: Vec<Vec<Point>>,
    hulls: Vec<Vec<Point>>,
}

const CURVE_SAMPLES: u32 = 16;

impl ClipRegion {
    pub(crate) fn of(path: &Path, ctm: Matrix) -> Result<Self, ClipUnreadable> {
        let (read, sound) = subpaths(path, ctm);
        if !sound {
            return Err(ClipUnreadable::Degenerate);
        }
        let mut parts = Pieces::default();
        for subpath in read {
            finish(&mut parts, subpath)?;
        }
        if parts.parts.is_empty() {
            return Err(ClipUnreadable::Degenerate);
        }
        Ok(Self {
            parts: parts.parts,
            hulls: parts.hulls,
        })
    }

    pub(crate) fn admits(&self, points: &[Point]) -> bool {
        let mut distinct: Vec<Point> = Vec::with_capacity(points.len());
        for point in points {
            if !point.x.is_finite() || !point.y.is_finite() {
                return false;
            }
            if !distinct.iter().any(|held| same_point(*held, *point)) {
                distinct.push(*point);
            }
        }
        if distinct.is_empty() {
            return false;
        }
        let shape = convex_hull(&distinct).unwrap_or(distinct.clone());
        let mut holding = None;
        for (index, part) in self.parts.iter().enumerate() {
            if distinct.iter().all(|point| contains(part, *point)) {
                if holding.is_some() {
                    return false;
                }
                holding = Some(index);
            }
        }
        let Some(holding) = holding else {
            return false;
        };
        self.hulls
            .iter()
            .enumerate()
            .all(|(index, hull)| index == holding || disjoint(hull, &shape))
    }

    pub(crate) fn parts(&self) -> &[Vec<Point>] {
        &self.parts
    }
}

fn subpaths(path: &Path, ctm: Matrix) -> (Vec<Subpath>, bool) {
    let mut read = Vec::new();
    let mut current = Subpath::default();
    let mut sound = true;
    for segment in &path.segments {
        match segment {
            PathSegment::Rectangle {
                origin,
                width,
                height,
                ..
            } => {
                read.push(std::mem::take(&mut current));
                let placed: Vec<Point> = [
                    *origin,
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
                ]
                .iter()
                .map(|point| ctm.transform(*point))
                .collect();
                read.push(Subpath {
                    through: placed.clone(),
                    around: placed,
                    curved: false,
                });
            }
            PathSegment::MoveTo { point, .. } => {
                read.push(std::mem::take(&mut current));
                current.line_to(ctm.transform(*point));
            }
            PathSegment::LineTo { point, .. } => current.line_to(ctm.transform(*point)),
            PathSegment::ClosePath { .. } => {}
            PathSegment::CubicTo {
                control_1,
                control_2,
                end,
                ..
            } => {
                let Some(start) = current.through.last().copied() else {
                    sound = false;
                    continue;
                };
                let [one, two, three] =
                    [control_1, control_2, end].map(|point| ctm.transform(*point));
                for step in 1..=CURVE_SAMPLES {
                    let along = f64::from(step) / f64::from(CURVE_SAMPLES);
                    current
                        .through
                        .push(on_curve([start, one, two, three], along));
                }
                current.around.extend([one, two, three]);
                current.curved = true;
            }
        }
    }
    read.push(current);
    (read, sound)
}

pub(crate) struct ClipCover {
    hulls: Vec<Vec<Point>>,
}

impl ClipCover {
    pub(crate) fn of(path: &Path, ctm: Matrix) -> Self {
        let (read, sound) = subpaths(path, ctm);
        if !sound {
            return Self { hulls: Vec::new() };
        }
        let hulls = read
            .into_iter()
            .filter_map(|subpath| {
                let mut points = subpath.through;
                points.extend(subpath.around);
                convex_hull(&points)
            })
            .collect();
        Self { hulls }
    }

    pub(crate) fn hides(&self, points: &[Point]) -> bool {
        if self.hulls.is_empty() {
            return false;
        }
        let mut distinct: Vec<Point> = Vec::with_capacity(points.len());
        for point in points {
            if !point.x.is_finite() || !point.y.is_finite() {
                return false;
            }
            if !distinct.iter().any(|held| same_point(*held, *point)) {
                distinct.push(*point);
            }
        }
        if distinct.is_empty() {
            return false;
        }
        let shape = convex_hull(&distinct).unwrap_or(distinct);
        self.hulls.iter().all(|hull| disjoint(hull, &shape))
    }
}

fn finish(parts: &mut Pieces, subpath: Subpath) -> Result<(), ClipUnreadable> {
    let Subpath {
        mut through,
        mut around,
        curved,
    } = subpath;
    for corners in [&mut through, &mut around] {
        if corners
            .last()
            .is_some_and(|last| same_point(*last, corners[0]))
        {
            corners.pop();
        }
    }
    match (turning(&around), turning(&through)) {
        (Turning::NoArea, _) => Ok(()),
        (Turning::Convex, Turning::Convex) => {
            parts.parts.push(through);
            parts.hulls.push(around);
            Ok(())
        }
        _ if curved => Err(ClipUnreadable::Curved),
        (Turning::Convex, Turning::NoArea) => Ok(()),
        _ => Err(ClipUnreadable::Concave),
    }
}

fn on_curve(points: [Point; 4], along: f64) -> Point {
    let rest = 1.0 - along;
    let weights = [
        rest * rest * rest,
        3.0 * rest * rest * along,
        3.0 * rest * along * along,
        along * along * along,
    ];
    points
        .iter()
        .zip(weights)
        .fold(Point { x: 0.0, y: 0.0 }, |sum, (point, weight)| Point {
            x: sum.x + weight * point.x,
            y: sum.y + weight * point.y,
        })
}

#[derive(Default)]
struct Pieces {
    parts: Vec<Vec<Point>>,
    hulls: Vec<Vec<Point>>,
}

#[derive(Default)]
struct Subpath {
    through: Vec<Point>,
    around: Vec<Point>,
    curved: bool,
}

impl Subpath {
    fn line_to(&mut self, point: Point) {
        self.through.push(point);
        self.around.push(point);
    }
}

enum Turning {
    NoArea,
    Convex,
    BothWays,
}

fn turning(corners: &[Point]) -> Turning {
    if corners.len() < 3 {
        return Turning::NoArea;
    }
    let mut sign = 0.0_f64;
    let mut turned = 0.0_f64;
    for index in 0..corners.len() {
        let (a, b, c) = (
            corners[index],
            corners[(index + 1) % corners.len()],
            corners[(index + 2) % corners.len()],
        );
        let turn = side(a, b, c);
        if turn.abs() < CLIP_TOLERANCE {
            continue;
        }
        if sign == 0.0 {
            sign = turn;
        } else if (turn > 0.0) != (sign > 0.0) {
            return Turning::BothWays;
        }
        let dot = (b.x - a.x).mul_add(c.x - b.x, (b.y - a.y) * (c.y - b.y));
        turned += turn.abs().atan2(dot);
    }
    if turned > std::f64::consts::TAU + 1e-6 {
        return Turning::BothWays;
    }
    if sign == 0.0 {
        Turning::NoArea
    } else {
        Turning::Convex
    }
}

pub(crate) fn same_point(left: Point, right: Point) -> bool {
    (left.x - right.x).abs() < CLIP_TOLERANCE && (left.y - right.y).abs() < CLIP_TOLERANCE
}

pub(crate) fn side(a: Point, b: Point, p: Point) -> f64 {
    (b.x - a.x).mul_add(p.y - a.y, -((b.y - a.y) * (p.x - a.x)))
}

pub(crate) fn contains(corners: &[Point], point: Point) -> bool {
    let mut sign = 0.0_f64;
    for index in 0..corners.len() {
        let turn = side(corners[index], corners[(index + 1) % corners.len()], point);
        if turn.abs() < CLIP_TOLERANCE {
            continue;
        }
        if sign == 0.0 {
            sign = turn;
        } else if (turn > 0.0) != (sign > 0.0) {
            return false;
        }
    }
    true
}

fn disjoint(one: &[Point], other: &[Point]) -> bool {
    for polygon in [one, other] {
        for index in 0..polygon.len() {
            let a = polygon[index];
            let b = polygon[(index + 1) % polygon.len()];
            let axis = Point {
                x: -(b.y - a.y),
                y: b.x - a.x,
            };
            if axis.x.abs() < CLIP_TOLERANCE && axis.y.abs() < CLIP_TOLERANCE {
                continue;
            }
            let project = |corners: &[Point]| {
                corners.iter().fold((f64::MAX, f64::MIN), |(low, high), p| {
                    let value = axis.x.mul_add(p.x, axis.y * p.y);
                    (low.min(value), high.max(value))
                })
            };
            let (one_low, one_high) = project(one);
            let (other_low, other_high) = project(other);
            if one_high <= other_low + CLIP_TOLERANCE || other_high <= one_low + CLIP_TOLERANCE {
                return true;
            }
        }
    }
    false
}

fn convex_hull(points: &[Point]) -> Option<Vec<Point>> {
    let mut sorted: Vec<Point> = points.to_vec();
    sorted.sort_by(|one, other| {
        one.x
            .partial_cmp(&other.x)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                one.y
                    .partial_cmp(&other.y)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });
    sorted.dedup_by(|one, other| same_point(*one, *other));
    if sorted.len() < 3
        || sorted
            .iter()
            .any(|point| !point.x.is_finite() || !point.y.is_finite())
    {
        return None;
    }
    let mut hull: Vec<Point> = Vec::with_capacity(sorted.len() * 2);
    for pass in 0..2 {
        let lower = hull.len();
        let sweep: Box<dyn Iterator<Item = &Point>> = if pass == 0 {
            Box::new(sorted.iter())
        } else {
            Box::new(sorted.iter().rev())
        };
        for point in sweep {
            while hull.len() > lower + 1
                && side(hull[hull.len() - 2], hull[hull.len() - 1], *point) <= 0.0
            {
                hull.pop();
            }
            hull.push(*point);
        }
        hull.pop();
    }
    (hull.len() >= 3).then_some(hull)
}

#[cfg(test)]
mod tests {
    use super::{ClipRegion, convex_hull, disjoint};
    use pdf_bytes::{SourceId, SourceSpan};
    use pdf_paint::{Matrix, Path, PathSegment, Point};

    fn span() -> SourceSpan {
        SourceSpan::new(SourceId::new(1), 0, 1).expect("a forward span")
    }

    fn at(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    fn square(x: f64, y: f64, side: f64) -> Vec<PathSegment> {
        vec![PathSegment::Rectangle {
            origin: at(x, y),
            width: side,
            height: side,
            provenance: span(),
        }]
    }

    fn quad(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Point> {
        vec![at(x0, y0), at(x1, y0), at(x1, y1), at(x0, y1)]
    }

    fn region(segments: Vec<PathSegment>) -> ClipRegion {
        ClipRegion::of(&Path { segments }, Matrix::IDENTITY).expect("a readable clip")
    }

    #[test]
    fn a_clip_that_encloses_nothing_at_all_is_refused_by_name() {
        for segments in [
            Vec::new(),
            vec![
                PathSegment::MoveTo {
                    point: at(0.0, 0.0),
                    provenance: span(),
                },
                PathSegment::LineTo {
                    point: at(10.0, 0.0),
                    provenance: span(),
                },
            ],
            vec![
                PathSegment::MoveTo {
                    point: at(0.0, 0.0),
                    provenance: span(),
                },
                PathSegment::LineTo {
                    point: at(10.0, 0.0),
                    provenance: span(),
                },
                PathSegment::LineTo {
                    point: at(20.0, 0.0),
                    provenance: span(),
                },
            ],
        ] {
            assert_eq!(
                ClipRegion::of(&Path { segments }, Matrix::IDENTITY),
                Err(super::ClipUnreadable::Degenerate)
            );
        }
    }

    #[test]
    fn an_empty_subpath_beside_a_real_one_is_dropped_rather_than_refusing_both() {
        let mut segments = square(0.0, 0.0, 100.0);
        segments.push(PathSegment::MoveTo {
            point: at(500.0, 500.0),
            provenance: span(),
        });
        let clip = ClipRegion::of(&Path { segments }, Matrix::IDENTITY).expect("one real window");
        assert_eq!(clip.parts().len(), 1);
        assert!(clip.admits(&quad(10.0, 10.0, 90.0, 90.0)));
        assert!(!clip.admits(&quad(400.0, 400.0, 600.0, 600.0)));

        let mut over = square(0.0, 0.0, 100.0);
        over.extend([
            PathSegment::MoveTo {
                point: at(20.0, 20.0),
                provenance: span(),
            },
            PathSegment::LineTo {
                point: at(80.0, 20.0),
                provenance: span(),
            },
        ]);
        let clip = ClipRegion::of(&Path { segments: over }, Matrix::IDENTITY).expect("one window");
        assert_eq!(clip.parts().len(), 1);
        assert!(clip.admits(&quad(10.0, 10.0, 90.0, 90.0)));
    }

    #[test]
    fn one_rectangle_answers_exactly_as_a_single_polygon_did() {
        let clip = region(square(0.0, 0.0, 100.0));
        assert_eq!(clip.parts().len(), 1);
        assert!(clip.admits(&quad(10.0, 10.0, 90.0, 90.0)));
        assert!(
            clip.admits(&quad(0.0, 0.0, 100.0, 100.0)),
            "the boundary is in"
        );
        assert!(!clip.admits(&quad(10.0, 10.0, 110.0, 90.0)));
        assert!(!clip.admits(&quad(200.0, 200.0, 300.0, 300.0)));
    }

    #[test]
    fn two_separate_rectangles_admit_an_object_inside_either_one() {
        let mut segments = square(0.0, 0.0, 100.0);
        segments.extend(square(200.0, 0.0, 100.0));
        let clip = region(segments);
        assert_eq!(clip.parts().len(), 2);
        assert!(clip.admits(&quad(10.0, 10.0, 90.0, 90.0)));
        assert!(clip.admits(&quad(210.0, 10.0, 290.0, 90.0)));
        assert!(!clip.admits(&quad(120.0, 10.0, 180.0, 90.0)));
        assert!(!clip.admits(&quad(50.0, 10.0, 250.0, 90.0)));
    }

    #[test]
    fn a_hole_is_refused_rather_than_read_as_the_shape_around_it() {
        let mut segments = square(0.0, 0.0, 300.0);
        segments.extend(square(100.0, 100.0, 100.0));
        let clip = region(segments);
        assert_eq!(clip.parts().len(), 2);
        assert!(
            !clip.admits(&quad(120.0, 120.0, 180.0, 180.0)),
            "inside both, so the two crossings decide it and this does not read them"
        );
        assert!(
            !clip.admits(&quad(10.0, 10.0, 290.0, 290.0)),
            "inside the outer one, but swallowing the inner one"
        );
        assert!(clip.admits(&quad(10.0, 10.0, 90.0, 90.0)));
        assert!(!clip.admits(&quad(50.0, 50.0, 150.0, 150.0)));
    }

    #[test]
    fn a_convex_curve_is_read_between_two_polygons_and_a_curve_turning_both_ways_is_refused() {
        let arch = |control_1: Point, control_2: Point| Path {
            segments: vec![
                PathSegment::MoveTo {
                    point: at(0.0, 0.0),
                    provenance: span(),
                },
                PathSegment::CubicTo {
                    control_1,
                    control_2,
                    end: at(30.0, 0.0),
                    provenance: span(),
                },
            ],
        };
        let clip = ClipRegion::of(&arch(at(10.0, 10.0), at(20.0, 20.0)), Matrix::IDENTITY)
            .expect("a convex curve is read");
        assert!(
            clip.admits(&quad(12.0, 1.0, 18.0, 5.0)),
            "well under the arch"
        );
        assert!(!clip.admits(&quad(14.0, 11.3, 15.0, 11.4)));
        assert!(!clip.admits(&quad(12.0, -2.0, 18.0, 1.0)));
        let mut segments = square(0.0, -50.0, 40.0).into_iter().collect::<Vec<_>>();
        segments.extend(
            arch(at(10.0, 10.0), at(20.0, 20.0))
                .segments
                .into_iter()
                .map(|segment| match segment {
                    PathSegment::MoveTo { point, provenance } => PathSegment::MoveTo {
                        point: at(point.x, point.y - 30.0),
                        provenance,
                    },
                    PathSegment::CubicTo {
                        control_1,
                        control_2,
                        end,
                        provenance,
                    } => PathSegment::CubicTo {
                        control_1: at(control_1.x, control_1.y - 30.0),
                        control_2: at(control_2.x, control_2.y - 30.0),
                        end: at(end.x, end.y - 30.0),
                        provenance,
                    },
                    other => other,
                }),
        );
        let two = ClipRegion::of(&Path { segments }, Matrix::IDENTITY).expect("two windows");
        assert!(!two.admits(&quad(14.0, -18.7, 15.0, -18.6)));

        assert_eq!(
            ClipRegion::of(&arch(at(10.0, 10.0), at(20.0, -10.0)), Matrix::IDENTITY),
            Err(super::ClipUnreadable::Curved)
        );

        let mut segments = square(0.0, 0.0, 100.0);
        for point in [
            at(200.0, 0.0),
            at(300.0, 0.0),
            at(250.0, 40.0),
            at(300.0, 80.0),
            at(200.0, 80.0),
        ] {
            segments.push(if segments.len() == 1 {
                PathSegment::MoveTo {
                    point,
                    provenance: span(),
                }
            } else {
                PathSegment::LineTo {
                    point,
                    provenance: span(),
                }
            });
        }
        assert_eq!(
            ClipRegion::of(&Path { segments }, Matrix::IDENTITY),
            Err(super::ClipUnreadable::Concave),
            "one bad subpath refuses the whole clip, and says which kind it was"
        );

        let star: Vec<PathSegment> = (0..5)
            .map(|index| {
                let angle = f64::from(index * 2) * std::f64::consts::TAU / 5.0;
                let point = at(100.0 * angle.cos(), 100.0 * angle.sin());
                if index == 0 {
                    PathSegment::MoveTo {
                        point,
                        provenance: span(),
                    }
                } else {
                    PathSegment::LineTo {
                        point,
                        provenance: span(),
                    }
                }
            })
            .collect();
        assert_eq!(
            ClipRegion::of(&Path { segments: star }, Matrix::IDENTITY),
            Err(super::ClipUnreadable::Concave)
        );
    }

    #[test]
    fn the_transform_is_applied_before_anything_is_decided() {
        let doubled = Matrix {
            a: 2.0,
            b: 0.0,
            c: 0.0,
            d: 2.0,
            e: 10.0,
            f: 20.0,
        };
        let clip = ClipRegion::of(
            &Path {
                segments: square(0.0, 0.0, 100.0),
            },
            doubled,
        )
        .expect("a readable clip");
        assert!(clip.admits(&quad(20.0, 30.0, 200.0, 210.0)));
        assert!(!clip.admits(&quad(0.0, 0.0, 100.0, 100.0)));
    }

    #[test]
    fn touching_along_an_edge_is_not_overlapping() {
        let mut segments = square(0.0, 0.0, 100.0);
        segments.extend(square(100.0, 0.0, 100.0));
        let clip = region(segments);
        assert!(clip.admits(&quad(50.0, 10.0, 100.0, 90.0)));
        assert!(disjoint(
            &quad(0.0, 0.0, 10.0, 10.0),
            &quad(10.0, 0.0, 20.0, 10.0)
        ));
        assert!(!disjoint(
            &quad(0.0, 0.0, 10.0, 10.0),
            &quad(5.0, 0.0, 20.0, 10.0)
        ));
    }

    #[test]
    fn a_hull_is_taken_of_the_points_rather_than_their_order() {
        let scrambled = vec![
            at(50.0, 50.0),
            at(0.0, 0.0),
            at(100.0, 100.0),
            at(0.0, 100.0),
            at(100.0, 0.0),
        ];
        let hull = convex_hull(&scrambled).expect("five points, four of them corners");
        assert_eq!(hull.len(), 4, "the interior point is not on the hull");
        let clip = region(square(0.0, 0.0, 100.0));
        assert!(clip.admits(&scrambled));

        assert!(convex_hull(&[at(0.0, 0.0), at(1.0, 1.0)]).is_none());
        assert!(
            convex_hull(&[at(0.0, 0.0), at(1.0, 1.0), at(2.0, 2.0)]).is_none(),
            "collinear points bound no area"
        );
        assert!(convex_hull(&[at(0.0, 0.0), at(1.0, 0.0), at(f64::NAN, 1.0)]).is_none());
    }

    #[test]
    fn an_extent_that_bounds_no_area_is_still_answered_point_by_point() {
        let clip = region(square(0.0, 0.0, 100.0));
        assert!(clip.admits(&[at(50.0, 50.0)]));
        assert!(clip.admits(&[at(10.0, 10.0), at(90.0, 90.0)]));
        assert!(!clip.admits(&[at(10.0, 10.0), at(190.0, 90.0)]));
        assert!(!clip.admits(&[]), "nothing cannot be inside anything");
        assert!(!clip.admits(&[at(50.0, 50.0), at(f64::NAN, 1.0)]));

        let mut segments = square(0.0, 0.0, 300.0);
        segments.extend(square(100.0, 100.0, 100.0));
        let donut = region(segments);
        assert!(!donut.admits(&[at(150.0, 150.0)]));
        assert!(donut.admits(&[at(50.0, 50.0)]));
    }
}
