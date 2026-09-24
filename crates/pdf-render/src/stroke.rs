use pdf_paint::MulAdd as _;
use pdf_paint::{DashPattern, LineCap, LineJoin};

use crate::geometry::Polygon;

const JOIN_VERTICES: usize = 12;

#[derive(Clone, Copy, Debug)]
pub struct StrokeStyle {
    pub cap: LineCap,
    pub join: LineJoin,
    pub miter_limit: f64,
}

#[must_use]
pub fn outline(
    polygon: &Polygon,
    width: f64,
    style: StrokeStyle,
    dash: &DashPattern,
    dash_scale: f64,
    closed: &[bool],
) -> Polygon {
    let width = if width.is_finite() && width > 1.0 {
        width
    } else {
        1.0
    };
    let half = width / 2.0;
    let mut outline = Polygon::default();
    for (index, contour) in polygon.contours.iter().enumerate() {
        let mut points = contour.clone();
        let closed = closed.get(index).copied().unwrap_or(false);
        if closed
            && points.first() != points.last()
            && let Some(first) = points.first().copied()
        {
            points.push(first);
        }
        let joins_ends = closed && points.len() > 2;
        let runs = dashed_runs(&points, dash, dash_scale);
        let whole = runs.len() == 1 && runs[0].len() == points.len();
        for run in &runs {
            add_run(&mut outline, run, half, style, joins_ends && whole);
        }
    }
    outline.orient_uniformly();
    outline
}

fn add_run(outline: &mut Polygon, run: &[[f64; 2]], half: f64, style: StrokeStyle, closed: bool) {
    let mut segments: Vec<([f64; 2], [f64; 2], [f64; 2])> = Vec::new();
    for window in run.windows(2) {
        let [start, end] = [window[0], window[1]];
        let dx = end[0] - start[0];
        let dy = end[1] - start[1];
        let length = dx.hypot(dy);
        if length <= f64::EPSILON {
            continue;
        }
        segments.push((start, end, [dx / length, dy / length]));
    }
    if segments.is_empty() {
        if matches!(style.cap, LineCap::Round | LineCap::ProjectingSquare)
            && let Some(point) = run.first()
        {
            outline.contours.push(disc(*point, half));
        }
        return;
    }

    let last = segments.len() - 1;
    for (index, (start, end, direction)) in segments.iter().enumerate() {
        let (mut start, mut end) = (*start, *end);
        if matches!(style.cap, LineCap::ProjectingSquare) && !closed {
            if index == 0 {
                start = [
                    direction[0].madd(-half, start[0]),
                    direction[1].madd(-half, start[1]),
                ];
            }
            if index == last {
                end = [
                    direction[0].madd(half, end[0]),
                    direction[1].madd(half, end[1]),
                ];
            }
        }
        let normal = [-direction[1] * half, direction[0] * half];
        outline.contours.push(vec![
            [start[0] + normal[0], start[1] + normal[1]],
            [end[0] + normal[0], end[1] + normal[1]],
            [end[0] - normal[0], end[1] - normal[1]],
            [start[0] - normal[0], start[1] - normal[1]],
        ]);
    }

    for index in 0..segments.len() {
        let next = index + 1;
        let (joint, incoming, outgoing) = if next < segments.len() {
            (segments[index].1, segments[index].2, segments[next].2)
        } else if closed {
            (segments[last].1, segments[last].2, segments[0].2)
        } else {
            continue;
        };
        add_join(outline, joint, incoming, outgoing, half, style);
    }

    if !closed && matches!(style.cap, LineCap::Round) {
        outline.contours.push(disc(segments[0].0, half));
        outline.contours.push(disc(segments[last].1, half));
    }
}

fn add_join(
    outline: &mut Polygon,
    joint: [f64; 2],
    incoming: [f64; 2],
    outgoing: [f64; 2],
    half: f64,
    style: StrokeStyle,
) {
    let cross = incoming[0].madd(outgoing[1], -(incoming[1] * outgoing[0]));
    if cross.abs() <= f64::EPSILON * 8.0 {
        let dot = incoming[0].madd(outgoing[0], incoming[1] * outgoing[1]);
        if dot > 0.0 {
            return;
        }
        if matches!(style.join, LineJoin::Round) {
            outline.contours.push(disc(joint, half));
        }
        return;
    }
    let side = if cross > 0.0 { -1.0 } else { 1.0 };
    let from = [
        joint[0] + -incoming[1] * half * side,
        joint[1] + incoming[0] * half * side,
    ];
    let to = [
        joint[0] + -outgoing[1] * half * side,
        joint[1] + outgoing[0] * half * side,
    ];

    match style.join {
        LineJoin::Round => outline.contours.push(disc(joint, half)),
        LineJoin::Bevel => outline.contours.push(vec![joint, from, to]),
        LineJoin::Miter => {
            let cos_phi = (-incoming[0]).madd(outgoing[0], -incoming[1] * outgoing[1]);
            let half_sin = ((1.0 - cos_phi.clamp(-1.0, 1.0)) / 2.0).sqrt();
            let limit = if style.miter_limit.is_finite() && style.miter_limit >= 1.0 {
                style.miter_limit
            } else {
                10.0
            };
            if half_sin <= f64::EPSILON || 1.0 / half_sin > limit {
                outline.contours.push(vec![joint, from, to]);
                return;
            }
            let bisector = [
                from[0] + to[0] - 2.0 * joint[0],
                from[1] + to[1] - 2.0 * joint[1],
            ];
            let length = bisector[0].hypot(bisector[1]);
            if length <= f64::EPSILON {
                outline.contours.push(vec![joint, from, to]);
                return;
            }
            let reach = half / half_sin;
            let tip = [
                bisector[0] / length * reach + joint[0],
                bisector[1] / length * reach + joint[1],
            ];
            outline.contours.push(vec![joint, from, tip, to]);
        }
    }
}

fn disc(centre: [f64; 2], radius: f64) -> Vec<[f64; 2]> {
    (0..JOIN_VERTICES)
        .map(|step| {
            #[allow(clippy::cast_precision_loss)]
            let angle = std::f64::consts::TAU * (step as f64) / (JOIN_VERTICES as f64);
            [
                radius.madd(angle.cos(), centre[0]),
                radius.madd(angle.sin(), centre[1]),
            ]
        })
        .collect()
}

fn dashed_runs(points: &[[f64; 2]], dash: &DashPattern, scale: f64) -> Vec<Vec<[f64; 2]>> {
    let usable = !dash.array.is_empty()
        && dash
            .array
            .iter()
            .all(|entry| entry.is_finite() && *entry >= 0.0)
        && dash.array.iter().any(|entry| *entry > 0.0)
        && scale.is_finite()
        && scale > 0.0;
    if !usable || points.len() < 2 {
        return vec![points.to_vec()];
    }
    let pattern: Vec<f64> = dash.array.iter().map(|entry| entry * scale).collect();
    let total: f64 = pattern.iter().sum();
    let mut index = 0_usize;
    let mut remaining = pattern[0];
    let mut painting = true;
    let mut phase = if dash.phase.is_finite() && dash.phase > 0.0 {
        (dash.phase * scale) % (total * if pattern.len() % 2 == 1 { 2.0 } else { 1.0 })
    } else {
        0.0
    };
    while phase > 0.0 {
        if phase < remaining {
            remaining -= phase;
            break;
        }
        phase -= remaining;
        index = (index + 1) % pattern.len();
        remaining = pattern[index];
        painting = !painting;
    }
    let mut runs = Vec::new();
    let mut current: Vec<[f64; 2]> = Vec::new();
    if painting {
        current.push(points[0]);
    }
    for window in points.windows(2) {
        let [start, end] = [window[0], window[1]];
        let mut travelled = 0.0;
        let segment = (end[0] - start[0]).hypot(end[1] - start[1]);
        if segment <= f64::EPSILON {
            continue;
        }
        while segment - travelled > remaining {
            travelled += remaining;
            let at = [
                (travelled / segment).madd(end[0] - start[0], start[0]),
                (travelled / segment).madd(end[1] - start[1], start[1]),
            ];
            if painting {
                current.push(at);
                runs.push(std::mem::take(&mut current));
            } else {
                current.clear();
                current.push(at);
            }
            painting = !painting;
            index = (index + 1) % pattern.len();
            remaining = pattern[index];
            if remaining <= 0.0 {
                remaining = f64::EPSILON;
            }
        }
        remaining -= segment - travelled;
        if painting {
            current.push(end);
        }
    }
    if current.len() >= 2 || (painting && current.len() == 1) {
        runs.push(current);
    }
    runs.retain(|run| !run.is_empty());
    runs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Polygon;

    fn corner() -> Polygon {
        Polygon {
            contours: vec![vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]]],
        }
    }

    fn solid() -> DashPattern {
        DashPattern {
            array: Vec::new(),
            phase: 0.0,
        }
    }

    fn style(join: LineJoin, miter_limit: f64) -> StrokeStyle {
        StrokeStyle {
            cap: LineCap::Butt,
            join,
            miter_limit,
        }
    }

    fn bisector_reach(outline: &Polygon, joint: [f64; 2], bisector: [f64; 2]) -> f64 {
        let length = bisector[0].hypot(bisector[1]);
        let unit = [bisector[0] / length, bisector[1] / length];
        outline
            .contours
            .iter()
            .flatten()
            .map(|point| (point[0] - joint[0]) * unit[0] + (point[1] - joint[1]) * unit[1])
            .fold(f64::NEG_INFINITY, f64::max)
    }

    fn reach(outline: &Polygon, joint: [f64; 2]) -> f64 {
        outline
            .contours
            .iter()
            .flatten()
            .map(|point| (point[0] - joint[0]).hypot(point[1] - joint[1]))
            .fold(0.0_f64, f64::max)
    }

    const OUTSIDE: [f64; 2] = [1.0, -1.0];

    #[test]
    fn a_right_angle_miter_reaches_the_point_the_specification_gives() {
        let outline = outline(
            &corner(),
            4.0,
            style(LineJoin::Miter, 10.0),
            &solid(),
            1.0,
            &[],
        );
        let expected = 2.0 * std::f64::consts::SQRT_2;
        let reached = bisector_reach(&outline, [10.0, 0.0], OUTSIDE);
        assert!(
            (reached - expected).abs() < 1e-9,
            "miter reached {reached}, expected {expected}"
        );
    }

    #[test]
    fn a_miter_past_its_limit_becomes_a_bevel() {
        let outline = outline(
            &corner(),
            4.0,
            style(LineJoin::Miter, 1.4),
            &solid(),
            1.0,
            &[],
        );
        let reached = bisector_reach(&outline, [10.0, 0.0], OUTSIDE);
        assert!(
            (reached - std::f64::consts::SQRT_2).abs() < 1e-9,
            "bevel reached {reached}, expected sqrt(2)"
        );
    }

    #[test]
    fn a_miter_exactly_at_its_limit_is_still_a_miter() {
        let outline = outline(
            &corner(),
            4.0,
            style(LineJoin::Miter, std::f64::consts::SQRT_2),
            &solid(),
            1.0,
            &[],
        );
        let reached = bisector_reach(&outline, [10.0, 0.0], OUTSIDE);
        assert!((reached - 2.0 * std::f64::consts::SQRT_2).abs() < 1e-9);
    }

    #[test]
    fn a_bevel_join_ignores_the_miter_limit() {
        let outline = outline(
            &corner(),
            4.0,
            style(LineJoin::Bevel, 100.0),
            &solid(),
            1.0,
            &[],
        );
        let reached = bisector_reach(&outline, [10.0, 0.0], OUTSIDE);
        assert!((reached - std::f64::consts::SQRT_2).abs() < 1e-9);
    }

    #[test]
    fn a_round_join_is_a_disc_about_the_corner() {
        let outline = outline(
            &corner(),
            4.0,
            style(LineJoin::Round, 10.0),
            &solid(),
            1.0,
            &[],
        );
        let disc = outline
            .contours
            .iter()
            .find(|contour| contour.len() == JOIN_VERTICES)
            .expect("a round join contour");
        for point in disc {
            let radius = (point[0] - 10.0).hypot(point[1] - 0.0);
            assert!((radius - 2.0).abs() < 1e-9, "radius {radius}");
        }
    }

    #[test]
    fn a_projecting_cap_extends_the_ends_and_not_the_corner() {
        let outline = outline(
            &corner(),
            4.0,
            StrokeStyle {
                cap: LineCap::ProjectingSquare,
                join: LineJoin::Bevel,
                miter_limit: 10.0,
            },
            &solid(),
            1.0,
            &[],
        );
        let left = outline
            .contours
            .iter()
            .flatten()
            .map(|point| point[0])
            .fold(f64::INFINITY, f64::min);
        let top = outline
            .contours
            .iter()
            .flatten()
            .map(|point| point[1])
            .fold(f64::NEG_INFINITY, f64::max);
        assert!((left - -2.0).abs() < 1e-9, "the start extends to x = -2");
        assert!((top - 12.0).abs() < 1e-9, "the end extends to y = 12");
        let reached = bisector_reach(&outline, [10.0, 0.0], OUTSIDE);
        assert!(
            reached <= std::f64::consts::SQRT_2 + 1e-9,
            "the corner grew a cap: {reached}"
        );
    }

    #[test]
    fn a_contour_the_path_closed_is_stroked_on_every_side() {
        let square = Polygon {
            contours: vec![vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]],
        };
        let reaches_left = |closed: &[bool]| {
            outline(
                &square,
                2.0,
                style(LineJoin::Miter, 10.0),
                &solid(),
                1.0,
                closed,
            )
            .contours
            .iter()
            .flatten()
            .any(|point| {
                (point[0] + 1.0).abs() < 1e-9 && point[1] > -1e-9 && point[1] < 10.0 + 1e-9
            })
        };
        assert!(reaches_left(&[true]), "the closing side is stroked");
        assert!(
            !reaches_left(&[false]),
            "an open contour has no closing side"
        );
    }

    #[test]
    fn ends_that_meet_are_capped_unless_the_path_closed_them() {
        let returns = Polygon {
            contours: vec![vec![
                [0.0, 0.0],
                [10.0, 0.0],
                [10.0, 10.0],
                [0.0, 10.0],
                [0.0, 0.0],
            ]],
        };
        let mitred_at_start = |closed: &[bool]| {
            outline(
                &returns,
                4.0,
                style(LineJoin::Miter, 10.0),
                &solid(),
                1.0,
                closed,
            )
            .contours
            .iter()
            .flatten()
            .any(|point| point[0] < -1e-9 && point[1] < -1e-9)
        };
        assert!(!mitred_at_start(&[false]), "butt caps meet at the start");
        assert!(mitred_at_start(&[true]), "a closed path joins there");
    }

    #[test]
    fn a_closed_contour_joins_its_own_ends() {
        let square = Polygon {
            contours: vec![vec![
                [0.0, 0.0],
                [10.0, 0.0],
                [10.0, 10.0],
                [0.0, 10.0],
                [0.0, 0.0],
            ]],
        };
        let outline = outline(
            &square,
            4.0,
            style(LineJoin::Miter, 10.0),
            &solid(),
            1.0,
            &[true],
        );
        for corner in [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]] {
            let reached = reach(
                &Polygon {
                    contours: outline
                        .contours
                        .iter()
                        .filter(|contour| {
                            contour.iter().any(|point| {
                                (point[0] - corner[0]).hypot(point[1] - corner[1]) < 1e-9
                            })
                        })
                        .cloned()
                        .collect(),
                },
                corner,
            );
            assert!(
                (reached - 2.0 * std::f64::consts::SQRT_2).abs() < 1e-9,
                "corner {corner:?} reached {reached}"
            );
        }
    }

    #[test]
    fn a_straight_line_written_in_pieces_grows_no_join() {
        let straight = Polygon {
            contours: vec![vec![[0.0, 0.0], [5.0, 0.0], [5.0, 0.0], [10.0, 0.0]]],
        };
        let outline = outline(
            &straight,
            4.0,
            style(LineJoin::Round, 10.0),
            &solid(),
            1.0,
            &[],
        );
        let top = outline
            .contours
            .iter()
            .flatten()
            .map(|point| point[1])
            .fold(f64::NEG_INFINITY, f64::max);
        assert!(
            (top - 2.0).abs() < 1e-9,
            "nothing bulges past the half width"
        );
        assert!(
            outline.contours.iter().all(|contour| contour.len() == 4),
            "only segment quads, no join patches"
        );
    }
}
