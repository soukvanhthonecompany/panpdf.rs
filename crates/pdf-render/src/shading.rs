#[must_use]
pub fn axial_parameter(coords: [f64; 4], point: [f64; 2], extend: [bool; 2]) -> Option<f64> {
    let [x0, y0, x1, y1] = coords;
    let dx = x1 - x0;
    let dy = y1 - y0;
    let squared = dx.mul_add(dx, dy * dy);
    if squared <= 0.0 {
        return None;
    }
    let s = dx.mul_add(point[0] - x0, dy * (point[1] - y0)) / squared;
    clamp_to_extend(s, extend)
}

#[must_use]
pub fn radial_parameter(coords: [f64; 6], point: [f64; 2], extend: [bool; 2]) -> Option<f64> {
    let [x0, y0, r0, x1, y1, r1] = coords;
    let dx = x1 - x0;
    let dy = y1 - y0;
    let dr = r1 - r0;
    let px = point[0] - x0;
    let py = point[1] - y0;
    let a = dx.mul_add(dx, dy.mul_add(dy, -(dr * dr)));
    let b = -2.0 * dx.mul_add(px, dy.mul_add(py, r0 * dr));
    let c = px.mul_add(px, py.mul_add(py, -(r0 * r0)));
    let candidates = if a.abs() <= f64::EPSILON {
        if b.abs() <= f64::EPSILON {
            return None;
        }
        vec![-c / b]
    } else {
        let discriminant = b.mul_add(b, -(4.0 * a * c));
        if discriminant < 0.0 {
            return None;
        }
        let root = discriminant.sqrt();
        vec![(-b + root) / (2.0 * a), (-b - root) / (2.0 * a)]
    };
    candidates
        .into_iter()
        .filter(|s| s.is_finite() && dr.mul_add(*s, r0) >= 0.0)
        .filter_map(|s| clamp_to_extend(s, extend).map(|clamped| (s, clamped)))
        .max_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, clamped)| clamped)
}

fn clamp_to_extend(s: f64, extend: [bool; 2]) -> Option<f64> {
    if !s.is_finite() {
        return None;
    }
    if s < 0.0 {
        return extend[0].then_some(0.0);
    }
    if s > 1.0 {
        return extend[1].then_some(1.0);
    }
    Some(s)
}
