use pdf_edit::PenStep;

#[must_use]
pub fn line(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<PenStep> {
    vec![PenStep::Move((x0, y0)), PenStep::Line((x1, y1))]
}

#[must_use]
pub fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<PenStep> {
    vec![
        PenStep::Move((x0, y0)),
        PenStep::Line((x1, y0)),
        PenStep::Line((x1, y1)),
        PenStep::Line((x0, y1)),
        PenStep::Line((x0, y0)),
    ]
}

#[must_use]
pub fn rounded(x0: f64, y0: f64, x1: f64, y1: f64, radius: f64) -> Vec<PenStep> {
    let (left, right) = (x0.min(x1), x0.max(x1));
    let (top, bottom) = (y0.min(y1), y0.max(y1));
    let r = radius
        .min((right - left) / 2.0)
        .min((bottom - top) / 2.0)
        .max(0.0);
    if r <= 0.0 {
        return rect(left, top, right, bottom);
    }
    let k = r * HANDLE;
    vec![
        PenStep::Move((left + r, top)),
        PenStep::Line((right - r, top)),
        PenStep::Curve((right - r + k, top), (right, top + r - k), (right, top + r)),
        PenStep::Line((right, bottom - r)),
        PenStep::Curve(
            (right, bottom - r + k),
            (right - r + k, bottom),
            (right - r, bottom),
        ),
        PenStep::Line((left + r, bottom)),
        PenStep::Curve(
            (left + r - k, bottom),
            (left, bottom - r + k),
            (left, bottom - r),
        ),
        PenStep::Line((left, top + r)),
        PenStep::Curve((left, top + r - k), (left + r - k, top), (left + r, top)),
    ]
}

const HANDLE: f64 = 0.552_284_749_830_793_6;

#[must_use]
pub fn circle(cx: f64, cy: f64, r: f64) -> Vec<PenStep> {
    let mut steps = vec![PenStep::Move((cx + r, cy))];
    steps.extend(arc_steps(cx, cy, (r, r), 0.0, std::f64::consts::TAU));
    steps
}

#[must_use]
pub fn ellipse(cx: f64, cy: f64, rx: f64, ry: f64) -> Vec<PenStep> {
    let mut steps = vec![PenStep::Move((cx + rx, cy))];
    steps.extend(arc_steps(cx, cy, (rx, ry), 0.0, std::f64::consts::TAU));
    steps
}

#[must_use]
pub fn wedge(cx: f64, cy: f64, r: f64, from: f64, to: f64) -> Vec<PenStep> {
    let mut steps = vec![
        PenStep::Move((cx, cy)),
        PenStep::Line((cx + r * from.cos(), cy + r * from.sin())),
    ];
    steps.extend(arc_steps(cx, cy, (r, r), from, to));
    steps.push(PenStep::Line((cx, cy)));
    steps
}

#[must_use]
pub fn ring_piece(
    cx: f64,
    cy: f64,
    (inner, outer): (f64, f64),
    from: f64,
    to: f64,
) -> Vec<PenStep> {
    let mut steps = vec![PenStep::Move((
        cx + outer * from.cos(),
        cy + outer * from.sin(),
    ))];
    steps.extend(arc_steps(cx, cy, (outer, outer), from, to));
    steps.push(PenStep::Line((
        cx + inner * to.cos(),
        cy + inner * to.sin(),
    )));
    steps.extend(arc_steps(cx, cy, (inner, inner), to, from));
    steps.push(PenStep::Line((
        cx + outer * from.cos(),
        cy + outer * from.sin(),
    )));
    steps
}

#[must_use]
pub fn arc_steps(cx: f64, cy: f64, (rx, ry): (f64, f64), from: f64, to: f64) -> Vec<PenStep> {
    let sweep = to - from;
    if sweep.abs() < 1e-12 {
        return Vec::new();
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a count of quarter turns, small and positive"
    )]
    let pieces = ((sweep.abs() / std::f64::consts::FRAC_PI_2).ceil() as usize).max(1);
    #[expect(clippy::cast_precision_loss, reason = "a small count")]
    let step = sweep / pieces as f64;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let mut out = Vec::with_capacity(pieces);
    let mut at = from;
    for _ in 0..pieces {
        let next = at + step;
        let (sin0, cos0) = at.sin_cos();
        let (sin1, cos1) = next.sin_cos();
        out.push(PenStep::Curve(
            (cx + rx * (cos0 - k * sin0), cy + ry * (sin0 + k * cos0)),
            (cx + rx * (cos1 + k * sin1), cy + ry * (sin1 - k * cos1)),
            (cx + rx * cos1, cy + ry * sin1),
        ));
        at = next;
    }
    out
}

#[must_use]
pub fn polygon(points: &[(f64, f64)]) -> Vec<PenStep> {
    let Some((&first, rest)) = points.split_first() else {
        return Vec::new();
    };
    let mut steps = vec![PenStep::Move(first)];
    steps.extend(rest.iter().map(|&point| PenStep::Line(point)));
    steps.push(PenStep::Line(first));
    steps
}

#[must_use]
pub fn polyline(points: &[(f64, f64)]) -> Vec<PenStep> {
    let Some((&first, rest)) = points.split_first() else {
        return Vec::new();
    };
    let mut steps = vec![PenStep::Move(first)];
    steps.extend(rest.iter().map(|&point| PenStep::Line(point)));
    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    fn end(step: &PenStep) -> (f64, f64) {
        match step {
            PenStep::Move(point) | PenStep::Line(point) | PenStep::Curve(_, _, point) => *point,
        }
    }

    #[test]
    fn circles_and_corners_land_where_they_should() {
        let steps = circle(50.0, 50.0, 10.0);
        assert_eq!(steps.len(), 5);
        let ends: Vec<(f64, f64)> = steps.iter().map(end).collect();
        let near =
            |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9;
        assert!(near(ends[1], (50.0, 60.0)), "{ends:?}");
        assert!(near(ends[2], (40.0, 50.0)), "{ends:?}");
        assert!(near(ends[3], (50.0, 40.0)), "{ends:?}");
        assert!(near(ends[4], (60.0, 50.0)), "{ends:?}");
        let PenStep::Curve(handle, _, _) = steps[1] else {
            panic!("a curve");
        };
        assert!(near(handle, (60.0, 50.0 + 10.0 * HANDLE)), "{handle:?}");

        for step in rounded(0.0, 0.0, 100.0, 20.0, 50.0) {
            for (x, y) in match step {
                PenStep::Move(p) | PenStep::Line(p) => vec![p],
                PenStep::Curve(a, b, c) => vec![a, b, c],
            } {
                assert!((0.0..=100.0).contains(&x) && (0.0..=20.0).contains(&y));
            }
        }
    }
}
