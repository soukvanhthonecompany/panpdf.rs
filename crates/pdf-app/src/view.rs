use pdf_cli::{CaretStop, ObjectBox, TextBlockBox, TextClusterBox};
use pdf_paint::MulAdd as _;

use crate::document::RunBox;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quad {
    pub corners: [(f64, f64); 4],
}

impl Quad {
    #[must_use]
    pub const fn of(bounds: [f64; 4]) -> Self {
        Self {
            corners: [
                (bounds[0], bounds[1]),
                (bounds[2], bounds[1]),
                (bounds[2], bounds[3]),
                (bounds[0], bounds[3]),
            ],
        }
    }

    #[must_use]
    pub const fn from_pixels(quad: pdf_cli::QuadPixels) -> Self {
        Self {
            corners: [
                (quad[0][0], quad[0][1]),
                (quad[1][0], quad[1][1]),
                (quad[2][0], quad[2][1]),
                (quad[3][0], quad[3][1]),
            ],
        }
    }

    #[must_use]
    pub fn bounds(&self) -> [f64; 4] {
        let mut box_of = [
            self.corners[0].0,
            self.corners[0].1,
            self.corners[0].0,
            self.corners[0].1,
        ];
        for (x, y) in &self.corners[1..] {
            box_of[0] = box_of[0].min(*x);
            box_of[1] = box_of[1].min(*y);
            box_of[2] = box_of[2].max(*x);
            box_of[3] = box_of[3].max(*y);
        }
        box_of
    }

    #[must_use]
    pub fn area(&self) -> f64 {
        let mut twice = 0.0;
        for index in 0..4 {
            let (x0, y0) = self.corners[index];
            let (x1, y1) = self.corners[(index + 1) % 4];
            twice += x0.madd(y1, -(x1 * y0));
        }
        (twice / 2.0).abs()
    }

    #[must_use]
    pub fn center(&self) -> (f64, f64) {
        (
            self.corners.iter().map(|corner| corner.0).sum::<f64>() / 4.0,
            self.corners.iter().map(|corner| corner.1).sum::<f64>() / 4.0,
        )
    }

    #[must_use]
    pub fn contains(&self, point: (f64, f64)) -> bool {
        let (mut positive, mut negative) = (false, false);
        for index in 0..4 {
            let (x0, y0) = self.corners[index];
            let (x1, y1) = self.corners[(index + 1) % 4];
            let side = (x1 - x0).madd(point.1 - y0, -((y1 - y0) * (point.0 - x0)));
            positive |= side > 0.0;
            negative |= side < 0.0;
        }
        if positive && negative {
            return false;
        }
        if positive || negative {
            return true;
        }
        let box_of = self.bounds();
        (box_of[0]..=box_of[2]).contains(&point.0) && (box_of[1]..=box_of[3]).contains(&point.1)
    }

    #[must_use]
    pub fn inside(&self, box_of: [f64; 4]) -> bool {
        self.corners.iter().all(|(x, y)| {
            (box_of[0]..=box_of[2]).contains(x) && (box_of[1]..=box_of[3]).contains(y)
        })
    }

    #[must_use]
    pub fn grown(&self, by: f64) -> Self {
        let mut twice = 0.0;
        for index in 0..4 {
            let (x0, y0) = self.corners[index];
            let (x1, y1) = self.corners[(index + 1) % 4];
            twice += x0.madd(y1, -(x1 * y0));
        }
        if twice.abs() <= f64::EPSILON {
            return *self;
        }
        let turn = if twice > 0.0 { 1.0 } else { -1.0 };
        let mut normals = [(0.0, 0.0); 4];
        for (index, normal) in normals.iter_mut().enumerate() {
            let (x0, y0) = self.corners[index];
            let (x1, y1) = self.corners[(index + 1) % 4];
            let (dx, dy) = (x1 - x0, y1 - y0);
            let length = dx.hypot(dy);
            if length <= f64::EPSILON {
                return *self;
            }
            *normal = (turn * dy / length, -turn * dx / length);
        }
        let mut corners = self.corners;
        for index in 0..4 {
            let (n1x, n1y) = normals[(index + 3) % 4];
            let (n2x, n2y) = normals[index];
            let along = n1x.madd(n2x, n1y * n2y);
            if (1.0 + along).abs() <= 1e-12 {
                return *self;
            }
            let scale = by / (1.0 + along);
            corners[index] = (
                scale.madd(n1x + n2x, self.corners[index].0),
                scale.madd(n1y + n2y, self.corners[index].1),
            );
        }
        Self { corners }
    }

    #[must_use]
    pub fn transformed(&self, matrix: pdf_paint::Matrix, about: (f64, f64)) -> Self {
        Self {
            corners: self.corners.map(|(x, y)| {
                let placed = matrix.transform(pdf_paint::Point {
                    x: x - about.0,
                    y: y - about.1,
                });
                (placed.x + about.0, placed.y + about.1)
            }),
        }
    }

    #[must_use]
    pub fn shifted(&self, dx: f64, dy: f64) -> Self {
        Self {
            corners: self.corners.map(|(x, y)| (x + dx, y + dy)),
        }
    }

    #[must_use]
    pub fn upright(&self) -> bool {
        (0..4).all(|index| {
            let (x0, y0) = self.corners[index];
            let (x1, y1) = self.corners[(index + 1) % 4];
            (x0 - x1).abs() < 1e-6 || (y0 - y1).abs() < 1e-6
        })
    }

    #[must_use]
    pub fn rectangular(&self) -> bool {
        let (along, across) = (
            (
                self.corners[1].0 - self.corners[0].0,
                self.corners[1].1 - self.corners[0].1,
            ),
            (
                self.corners[3].0 - self.corners[0].0,
                self.corners[3].1 - self.corners[0].1,
            ),
        );
        let (long, tall) = (along.0.hypot(along.1), across.0.hypot(across.1));
        if long <= 1e-6 || tall <= 1e-6 {
            return false;
        }
        let far = (
            self.corners[0].0 + along.0 + across.0,
            self.corners[0].1 + along.1 + across.1,
        );
        let slack = 1e-6 * long.max(tall);
        (far.0 - self.corners[2].0).abs() <= slack
            && (far.1 - self.corners[2].1).abs() <= slack
            && along.0.madd(across.0, along.1 * across.1).abs() <= slack * long.max(tall)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Step {
    Left,
    Right,
    Up,
    Down,
    RowStart,
    RowEnd,
}

#[must_use]
pub fn caret_at_in(stops: &[CaretStop], rows: &[usize], point: (f64, f64)) -> Option<usize> {
    let row = nearest_row(stops, rows, point)?;
    let mut best: Option<(usize, f64)> = None;
    for (index, stop) in stops.iter().enumerate() {
        if stop.line != row {
            continue;
        }
        let distance = along_row_distance(stop, point);
        if best.is_none_or(|(_, shortest)| distance < shortest) {
            best = Some((index, distance));
        }
    }
    best.map(|(index, _)| index)
}

fn nearest_row(stops: &[CaretStop], rows: &[usize], point: (f64, f64)) -> Option<usize> {
    let mut best: Option<(usize, f64)> = None;
    for &line in rows {
        let Some(anchor) = stops.iter().find(|stop| stop.line == line) else {
            continue;
        };
        let height = anchor.height();
        let distance = if height <= 1e-9 {
            (anchor.at[0] - point.0).hypot(anchor.at[1] - point.1)
        } else {
            let across = (point.0 - anchor.at[0]).madd(
                anchor.up[0] / height,
                (point.1 - anchor.at[1]) * (anchor.up[1] / height),
            );
            if across < 0.0 {
                -across
            } else if across > height {
                across - height
            } else {
                0.0
            }
        };
        if best.is_none_or(|(_, shortest)| distance < shortest) {
            best = Some((line, distance));
        }
    }
    best.map(|(line, _)| line)
}

fn along_row_distance(stop: &CaretStop, point: (f64, f64)) -> f64 {
    let height = stop.height();
    if height <= 1e-9 {
        return (stop.at[0] - point.0).hypot(stop.at[1] - point.1);
    }
    let along = (-stop.up[1] / height, stop.up[0] / height);
    (point.0 - stop.at[0])
        .madd(along.0, (point.1 - stop.at[1]) * along.1)
        .abs()
}

fn caret_distance(stop: &CaretStop, point: (f64, f64)) -> f64 {
    let middle = (
        stop.up[0].madd(0.5, stop.at[0]),
        stop.up[1].madd(0.5, stop.at[1]),
    );
    (middle.0 - point.0).hypot(middle.1 - point.1)
}

#[must_use]
pub fn caret_at(stops: &[CaretStop], point: (f64, f64)) -> Option<usize> {
    let mut best: Option<(usize, f64)> = None;
    for (index, stop) in stops.iter().enumerate() {
        let distance = caret_distance(stop, point);
        if best.is_none_or(|(_, shortest)| distance < shortest) {
            best = Some((index, distance));
        }
    }
    best.map(|(index, _)| index)
}

#[must_use]
pub fn caret_step_in(stops: &[CaretStop], rows: &[usize], from: usize, step: Step) -> usize {
    let Some(current) = stops.get(from) else {
        return from;
    };
    if !rows.contains(&current.line) {
        return from;
    }
    match step {
        Step::RowStart | Step::RowEnd => caret_step(stops, from, step),
        Step::Left | Step::Right => {
            let stepped = caret_step(stops, from, step);
            if stepped != from {
                return stepped;
            }
            let Some(row) = rows.iter().position(|line| *line == current.line) else {
                return from;
            };
            let next = if step == Step::Left {
                row.checked_sub(1)
            } else {
                Some(row + 1)
            };
            let Some(line) = next.and_then(|next| rows.get(next)) else {
                return from;
            };
            let on_row = stops
                .iter()
                .enumerate()
                .filter(|(_, stop)| stop.line == *line);
            let landed = if step == Step::Left {
                on_row.max_by_key(|(_, stop)| stop.offset)
            } else {
                on_row.min_by_key(|(_, stop)| stop.offset)
            };
            landed.map_or(from, |(index, _)| index)
        }
        Step::Up | Step::Down => neighbouring_stop(stops, rows, current, step).unwrap_or(from),
    }
}

fn neighbouring_stop(
    stops: &[CaretStop],
    rows: &[usize],
    current: &CaretStop,
    step: Step,
) -> Option<usize> {
    let mut best: Option<(usize, f64, f64)> = None;
    for (index, stop) in stops.iter().enumerate() {
        if !rows.contains(&stop.line) || stop.line == current.line {
            continue;
        }
        let gap = stop.at[1] - current.at[1];
        if gap.abs() <= ROW_APART || (step == Step::Up) != (gap < 0.0) {
            continue;
        }
        let along = (stop.at[0] - current.at[0]).abs();
        let nearer = best.is_none_or(|(_, height, sideways)| {
            gap.abs() < height - ROW_APART
                || ((gap.abs() - height).abs() <= ROW_APART && along < sideways)
        });
        if nearer {
            best = Some((index, gap.abs(), along));
        }
    }
    best.map(|(index, _, _)| index)
}

const ROW_APART: f64 = 0.5;

#[must_use]
pub fn caret_step(stops: &[CaretStop], from: usize, step: Step) -> usize {
    let Some(current) = stops.get(from) else {
        return from;
    };
    match step {
        Step::RowStart | Step::RowEnd => {
            let on_row = stops
                .iter()
                .enumerate()
                .filter(|(_, stop)| stop.line == current.line);
            let end = if step == Step::RowStart {
                on_row.min_by_key(|(_, stop)| stop.offset)
            } else {
                on_row.max_by_key(|(_, stop)| stop.offset)
            };
            end.map_or(from, |(index, _)| index)
        }
        Step::Left | Step::Right => {
            let wanted = if step == Step::Left {
                current.offset.checked_sub(1)
            } else {
                Some(current.offset + 1)
            };
            let Some(wanted) = wanted else { return from };
            stops
                .iter()
                .position(|stop| stop.line == current.line && stop.offset == wanted)
                .unwrap_or(from)
        }
        Step::Up | Step::Down => {
            let wanted = if step == Step::Up {
                current.line.checked_sub(1)
            } else {
                Some(current.line + 1)
            };
            let Some(wanted) = wanted else { return from };
            let row: Vec<(usize, &CaretStop)> = stops
                .iter()
                .enumerate()
                .filter(|(_, stop)| stop.line == wanted)
                .collect();
            row.iter()
                .min_by(|(_, left), (_, right)| {
                    (left.at[0] - current.at[0])
                        .abs()
                        .total_cmp(&(right.at[0] - current.at[0]).abs())
                })
                .map_or(from, |(index, _)| *index)
        }
    }
}

#[must_use]
pub fn selection_between(
    stops: &[CaretStop],
    anchor: usize,
    caret: usize,
) -> Option<(usize, usize, usize)> {
    let (anchor, caret) = (stops.get(anchor)?, stops.get(caret)?);
    if anchor.line != caret.line {
        return None;
    }
    let (from, to) = if anchor.offset <= caret.offset {
        (anchor.offset, caret.offset)
    } else {
        (caret.offset, anchor.offset)
    };
    (from != to).then_some((anchor.line, from, to))
}

#[must_use]
pub fn block_position(stops: &[CaretStop], rows: &[usize], stop: usize) -> Option<(usize, usize)> {
    let stop = stops.get(stop)?;
    let row = rows.iter().position(|line| *line == stop.line)?;
    Some((row, stop.offset))
}

#[must_use]
pub fn selection_rows(
    stops: &[CaretStop],
    rows: &[usize],
    anchor: usize,
    caret: usize,
) -> Vec<(usize, usize, usize)> {
    let (Some(one), Some(other)) = (
        block_position(stops, rows, anchor),
        block_position(stops, rows, caret),
    ) else {
        return Vec::new();
    };
    let (first, last) = if one <= other {
        (one, other)
    } else {
        (other, one)
    };
    if first == last {
        return Vec::new();
    }
    let mut spans = Vec::new();
    for (row, line) in rows.iter().enumerate().take(last.0 + 1).skip(first.0) {
        let end = stops
            .iter()
            .filter(|stop| stop.line == *line)
            .map(|stop| stop.offset)
            .max()
            .unwrap_or(0);
        let from = if row == first.0 { first.1 } else { 0 };
        let to = if row == last.0 { last.1 } else { end };
        if from < to {
            spans.push((*line, from, to));
        }
    }
    spans
}

#[must_use]
pub fn deletion_between(
    stops: &[CaretStop],
    anchor: usize,
    caret: usize,
    backwards: bool,
) -> Option<(usize, usize, usize)> {
    if anchor != caret {
        return selection_between(stops, anchor, caret);
    }
    let stop = stops.get(caret)?;
    let end = stops
        .iter()
        .filter(|other| other.line == stop.line)
        .map(|other| other.offset)
        .max()?;
    if backwards {
        Some((stop.line, stop.offset.checked_sub(1)?, stop.offset))
    } else {
        (stop.offset < end).then_some((stop.line, stop.offset, stop.offset + 1))
    }
}

fn is_a_gap(cluster: &TextClusterBox) -> bool {
    cluster
        .text
        .as_ref()
        .is_some_and(|text| !text.is_empty() && text.chars().all(char::is_whitespace))
}

#[must_use]
pub fn word_at(clusters: &[TextClusterBox], line: usize, offset: usize) -> Option<(usize, usize)> {
    let on_row = |index: usize| {
        clusters
            .iter()
            .find(|cluster| cluster.line == line && cluster.index_in_line == index)
    };
    let end = clusters
        .iter()
        .filter(|cluster| cluster.line == line)
        .map(|cluster| cluster.index_in_line + 1)
        .max()?;
    let start = [offset, offset.checked_sub(1).unwrap_or(offset)]
        .into_iter()
        .find(|index| on_row(*index).is_some_and(|cluster| !is_a_gap(cluster)))?;
    let mut from = start;
    while from > 0 && on_row(from - 1).is_some_and(|cluster| !is_a_gap(cluster)) {
        from -= 1;
    }
    let mut to = start + 1;
    while to < end && on_row(to).is_some_and(|cluster| !is_a_gap(cluster)) {
        to += 1;
    }
    Some((from, to))
}

#[must_use]
pub fn row_at(stops: &[CaretStop], line: usize) -> Option<(usize, usize)> {
    let mut offsets = stops
        .iter()
        .filter(|stop| stop.line == line)
        .map(|stop| stop.offset);
    let first = offsets.next()?;
    let (from, to) = offsets.fold((first, first), |(low, high), offset| {
        (low.min(offset), high.max(offset))
    });
    (from != to).then_some((from, to))
}

#[must_use]
pub fn stop_at(stops: &[CaretStop], line: usize, offset: usize) -> Option<usize> {
    stops
        .iter()
        .position(|stop| stop.line == line && stop.offset == offset)
}

#[must_use]
pub fn selection_quad(
    stops: &[CaretStop],
    clusters: &[TextClusterBox],
    line: usize,
    from: usize,
    to: usize,
) -> Option<Quad> {
    let (from, to) = (from.min(to), from.max(to));
    if from == to {
        return None;
    }
    let start = stops
        .iter()
        .find(|stop| stop.line == line && stop.offset == from)?;
    let end = stops
        .iter()
        .find(|stop| stop.line == line && stop.offset == to)?;

    let rise = if start.height() > 0.0 {
        start.up
    } else {
        end.up
    };
    let span = (end.at[0] - start.at[0], end.at[1] - start.at[1]);
    let length = span.0.hypot(span.1);
    let along = if length > 1e-9 {
        (span.0 / length, span.1 / length)
    } else {
        (1.0, 0.0)
    };
    let height = rise[0].hypot(rise[1]);
    let up = if height > 1e-9 {
        (rise[0] / height, rise[1] / height)
    } else {
        (-along.1, along.0)
    };

    let origin = (start.at[0], start.at[1]);
    let mut extent: Option<[f64; 4]> = None;
    let hold = |x: f64, y: f64, extent: &mut Option<[f64; 4]>| {
        let (dx, dy) = (x - origin.0, y - origin.1);
        let a = dx.madd(along.0, dy * along.1);
        let b = dx.madd(up.0, dy * up.1);
        *extent = Some(match *extent {
            None => [a, b, a, b],
            Some(had) => [had[0].min(a), had[1].min(b), had[2].max(a), had[3].max(b)],
        });
    };
    let mut measured = start.height() > 0.0 || end.height() > 0.0;
    for stop in [start, end] {
        hold(stop.at[0], stop.at[1], &mut extent);
        hold(
            stop.at[0] + stop.up[0],
            stop.at[1] + stop.up[1],
            &mut extent,
        );
    }
    for cluster in clusters.iter().filter(|cluster| {
        cluster.line == line && cluster.index_in_line >= from && cluster.index_in_line < to
    }) {
        let Some(bounds) = cluster.box_pixels else {
            continue;
        };
        measured = true;
        for (x, y) in [
            (bounds[0], bounds[1]),
            (bounds[2], bounds[1]),
            (bounds[2], bounds[3]),
            (bounds[0], bounds[3]),
        ] {
            hold(x, y, &mut extent);
        }
    }
    let [a0, b0, a1, b1] = extent?;
    if !measured || a1 <= a0 || b1 <= b0 {
        return None;
    }
    let corner = |a: f64, b: f64| {
        (
            a.madd(along.0, b.madd(up.0, origin.0)),
            a.madd(along.1, b.madd(up.1, origin.1)),
        )
    };
    Some(Quad {
        corners: [
            corner(a0, b0),
            corner(a1, b0),
            corner(a1, b1),
            corner(a0, b1),
        ],
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub origin: (f32, f32),
    pub size: (u32, u32),
    pub stretch: f32,
}

impl Placement {
    #[must_use]
    pub fn screen_size(&self) -> (f32, f32) {
        let (width, height) = screen_size(self.size.0, self.size.1);
        (width * self.stretch, height * self.stretch)
    }

    #[must_use]
    pub fn page_point(&self, pointer: (f32, f32)) -> Option<(f64, f64)> {
        if self.stretch <= 0.0 {
            return None;
        }
        let x = f64::from((pointer.0 - self.origin.0) / self.stretch);
        let y = f64::from((pointer.1 - self.origin.1) / self.stretch);
        if x < 0.0 || y < 0.0 || x >= f64::from(self.size.0) || y >= f64::from(self.size.1) {
            return None;
        }
        Some((x, y))
    }

    #[must_use]
    pub fn point_in_page(&self, pointer: (f32, f32)) -> Option<(f64, f64)> {
        (self.stretch > 0.0).then(|| {
            (
                f64::from((pointer.0 - self.origin.0) / self.stretch),
                f64::from((pointer.1 - self.origin.1) / self.stretch),
            )
        })
    }

    #[must_use]
    pub fn screen_box(&self, bounds: [f64; 4]) -> [f32; 4] {
        let [x0, y0, x1, y1] = screen_box(bounds);
        [
            x0 * self.stretch,
            y0 * self.stretch,
            x1 * self.stretch,
            y1 * self.stretch,
        ]
    }

    #[must_use]
    pub fn visible_page_box(&self, screen: [f32; 4]) -> Option<[u32; 4]> {
        if self.stretch <= 0.0 {
            return None;
        }
        let map = |value: f32, corner: f32, limit: u32, outward: fn(f32) -> f32| -> u32 {
            let inside = outward((value - corner) / self.stretch).max(0.0);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            if inside >= screen_size(limit, limit).0 {
                limit
            } else {
                inside as u32
            }
        };
        let x0 = map(screen[0], self.origin.0, self.size.0, f32::floor);
        let y0 = map(screen[1], self.origin.1, self.size.1, f32::floor);
        let x1 = map(screen[2], self.origin.0, self.size.0, f32::ceil);
        let y1 = map(screen[3], self.origin.1, self.size.1, f32::ceil);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        Some([x0, y0, x1, y1])
    }

    #[must_use]
    pub fn caret_line(&self, stop: &CaretStop) -> ([f32; 2], [f32; 2]) {
        let (top, bottom) = caret_line(stop);
        (
            [top[0] * self.stretch, top[1] * self.stretch],
            [bottom[0] * self.stretch, bottom[1] * self.stretch],
        )
    }
}

pub const MARGIN: u32 = 128;

#[must_use]
pub fn draw_window(page: (u32, u32), visible: [u32; 4]) -> Option<[u32; 4]> {
    let [x0, y0, x1, y1] = visible;
    if page.0 == 0 || page.1 == 0 || x1 <= x0 || y1 <= y0 {
        return None;
    }
    let window = [
        x0.saturating_sub(MARGIN),
        y0.saturating_sub(MARGIN),
        x1.saturating_add(MARGIN).min(page.0),
        y1.saturating_add(MARGIN).min(page.1),
    ];
    if window[0] >= window[2] || window[1] >= window[3] {
        return None;
    }
    Some(window)
}

#[must_use]
pub fn covers(drawn: [u32; 4], visible: [u32; 4]) -> bool {
    visible[0] >= drawn[0]
        && visible[1] >= drawn[1]
        && visible[2] <= drawn[2]
        && visible[3] <= drawn[3]
}

#[must_use]
pub fn zoom_anchor(
    offset: (f32, f32),
    pointer: (f32, f32),
    ratio: f32,
    content: (f32, f32),
    view: (f32, f32),
    margin: f32,
) -> (f32, f32) {
    let across = |strip: f32| {
        let area = strip.max(view.0);
        ((area - strip) / 2.0, area)
    };
    let down = |strip: f32| (margin, (2.0f32.madd(margin, strip)).max(view.1));
    let axis = |offset: f32, pointer: f32, before: (f32, f32), after: (f32, f32), view: f32| {
        let on_strip = offset + pointer - before.0;
        let wanted = on_strip.madd(ratio, after.0 - pointer);
        wanted.clamp(0.0, (after.1 - view).max(0.0))
    };
    (
        axis(
            offset.0,
            pointer.0,
            across(content.0),
            across(content.0 * ratio),
            view.0,
        ),
        axis(
            offset.1,
            pointer.1,
            down(content.1),
            down(content.1 * ratio),
            view.1,
        ),
    )
}

#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn visible_after(offset: (f32, f32), view: (f32, f32), page: (u32, u32)) -> Option<[u32; 4]> {
    let axis = |start: f32, extent: f32, limit: u32| -> (u32, u32) {
        let low = start.floor().max(0.0);
        let high = (start + extent).ceil().max(0.0);
        let cut = |value: f32| -> u32 {
            if value >= screen_size(limit, limit).0 {
                limit
            } else {
                value as u32
            }
        };
        (cut(low), cut(high))
    };
    let (x0, x1) = axis(offset.0, view.0, page.0);
    let (y0, y1) = axis(offset.1, view.1, page.1);
    (x0 < x1 && y0 < y1).then_some([x0, y0, x1, y1])
}

#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn size_at(size: (u32, u32), from: f64, to: f64) -> (u32, u32) {
    if !(from.is_finite() && to.is_finite()) || from <= 0.0 || to <= 0.0 {
        return size;
    }
    let ratio = to / from;
    let axis = |value: u32| -> u32 {
        let scaled = (f64::from(value) * ratio).ceil().max(1.0);
        if scaled >= f64::from(u32::MAX) {
            u32::MAX
        } else {
            scaled as u32
        }
    };
    (axis(size.0), axis(size.1))
}

#[must_use]
pub fn block_at(blocks: &[Quad], point: (f64, f64)) -> Option<usize> {
    standing_block_at(blocks, |_| true, point)
}

#[must_use]
pub fn stands(holds_text: bool, block: usize, typing_in: Option<usize>) -> bool {
    holds_text || typing_in == Some(block)
}

#[must_use]
pub fn on_the_ink(clusters: &[TextClusterBox], lines: &[usize], point: (f64, f64)) -> bool {
    clusters
        .iter()
        .filter(|cluster| lines.contains(&cluster.line))
        .filter_map(|cluster| cluster.box_pixels)
        .any(|ink| point.0 >= ink[0] && point.0 <= ink[2] && point.1 >= ink[1] && point.1 <= ink[3])
}

#[must_use]
pub fn standing_block_at(
    blocks: &[Quad],
    standing: impl Fn(usize) -> bool,
    point: (f64, f64),
) -> Option<usize> {
    blocks
        .iter()
        .enumerate()
        .filter(|(index, _)| standing(*index))
        .filter(|(_, quad)| frame_quad(quad).contains(point))
        .min_by(|(_, one), (_, other)| one.area().total_cmp(&other.area()))
        .map(|(index, _)| index)
}

type WrittenAt = (u32, u16, Vec<(u32, u16, usize)>, usize);

fn written_at(anchor: &str) -> Option<WrittenAt> {
    let anchor = pdf_edit::SourceAnchor::decode(anchor)?;
    Some((
        anchor.stream.object_number(),
        anchor.stream.generation(),
        anchor
            .invocation_path
            .iter()
            .map(|(form, offset)| (form.object_number(), form.generation(), *offset))
            .collect(),
        anchor.operator_offset,
    ))
}

#[must_use]
pub fn in_file_order(anchors: &[String]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..anchors.len()).collect();
    order.sort_by(|one, other| {
        let (one_at, other_at) = (written_at(&anchors[*one]), written_at(&anchors[*other]));
        match (one_at, other_at) {
            (Some(one_at), Some(other_at)) => one_at.cmp(&other_at),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
        .then(one.cmp(other))
    });
    order
}

#[must_use]
pub fn next_in_order(order: &[usize], from: Option<usize>) -> Option<usize> {
    let first = order.first().copied();
    let Some(from) = from else { return first };
    let Some(place) = order.iter().position(|at| *at == from) else {
        return first;
    };
    order.get(place + 1).copied().or(first)
}

#[must_use]
pub fn sweep_box(from: (f64, f64), to: (f64, f64)) -> [f64; 4] {
    [
        from.0.min(to.0),
        from.1.min(to.1),
        from.0.max(to.0),
        from.1.max(to.1),
    ]
}

#[must_use]
pub fn swept(quads: &[Quad], sweep: [f64; 4]) -> Vec<usize> {
    quads
        .iter()
        .enumerate()
        .filter(|(_, quad)| quad.inside(sweep))
        .map(|(index, _)| index)
        .collect()
}

pub const SWEEP_ENOUGH: f64 = 3.0;

#[must_use]
pub fn along_one_axis(travel: (f64, f64)) -> (f64, f64) {
    if travel.0.abs() >= travel.1.abs() {
        (travel.0, 0.0)
    } else {
        (0.0, travel.1)
    }
}

#[must_use]
pub fn block_moved_to(blocks: &[[f64; 4]], target: [f64; 4]) -> Option<usize> {
    blocks
        .iter()
        .enumerate()
        .map(|(index, bounds)| (index, corner_distance(*bounds, target)))
        .filter(|(_, distance)| *distance <= SAME_BLOCK)
        .min_by(|(_, one), (_, other)| one.total_cmp(other))
        .map(|(index, _)| index)
}

const SAME_BLOCK: f64 = 0.1;

fn corner_distance(one: [f64; 4], other: [f64; 4]) -> f64 {
    (one[0] - other[0])
        .abs()
        .max((one[1] - other[1]).abs())
        .max((one[2] - other[2]).abs())
        .max((one[3] - other[3]).abs())
}

#[must_use]
pub fn box_shifted(bounds: [f64; 4], dx: f64, dy: f64) -> [f64; 4] {
    [
        bounds[0] + dx,
        bounds[1] + dy,
        bounds[2] + dx,
        bounds[3] + dy,
    ]
}

#[must_use]
pub fn dashes(length: f64) -> Vec<(f64, f64)> {
    const DASH: f64 = 5.0;
    const GAP: f64 = 3.5;
    if !length.is_finite() || length <= DASH {
        return vec![(0.0, 1.0)];
    }
    let count = ((length + GAP) / (DASH + GAP)).round().max(1.0);
    let unscaled = count.madd(DASH, (count - 1.0) * GAP);
    let scale = length / unscaled;
    let (dash, gap) = (DASH * scale / length, GAP * scale / length);
    let mut found = Vec::new();
    let mut index = 0.0_f64;
    while index < count {
        let start = index * (dash + gap);
        found.push((start, (start + dash).min(1.0)));
        index += 1.0;
    }
    found
}

pub const FRAME_INSET: f64 = 2.0;

#[must_use]
pub fn handles(quad: &Quad) -> [(f64, f64); 8] {
    let corners = frame_quad(quad).corners;
    let middle = |one: usize, other: usize| {
        (
            f64::midpoint(corners[one].0, corners[other].0),
            f64::midpoint(corners[one].1, corners[other].1),
        )
    };
    [
        corners[0],
        corners[1],
        corners[2],
        corners[3],
        middle(0, 1),
        middle(1, 2),
        middle(2, 3),
        middle(3, 0),
    ]
}

pub fn text_handles(quad: &Quad) -> impl Iterator<Item = (usize, (f64, f64))> {
    let places = handles(quad);
    let rectangular = quad.rectangular();
    [7, 5]
        .into_iter()
        .filter(move |_| rectangular)
        .map(move |index| (index, places[index]))
        .chain(std::iter::once((ROTATE_HANDLE, rotate_handle(quad))))
}

#[must_use]
pub fn travel_along(quad: &Quad, travel: (f64, f64)) -> (f64, f64) {
    let Some((along, across)) = axes(quad) else {
        return travel;
    };
    (
        travel.0.madd(along.0, travel.1 * along.1),
        travel.0.madd(across.0, travel.1 * across.1),
    )
}

#[must_use]
pub fn text_handle_at(quad: &Quad, point: (f64, f64)) -> Option<usize> {
    text_handles(quad)
        .map(|(index, (x, y))| (index, (x - point.0).hypot(y - point.1)))
        .filter(|(_, distance)| *distance <= HANDLE_REACH)
        .min_by(|(_, one), (_, other)| one.total_cmp(other))
        .map(|(index, _)| index)
}

#[must_use]
pub fn frame_of(bounds: [f64; 4]) -> [f64; 4] {
    grow(bounds, FRAME_INSET)
}

#[must_use]
pub fn frame_quad(quad: &Quad) -> Quad {
    quad.grown(FRAME_INSET)
}

pub const ROTATE_REACH_OUT: f64 = 18.0;

#[must_use]
pub fn rotate_handle(quad: &Quad) -> (f64, f64) {
    rotate_stem(quad).1
}

#[must_use]
pub fn rotate_stem(quad: &Quad) -> ((f64, f64), (f64, f64)) {
    let frame = frame_quad(quad).corners;
    let side = |one: usize, other: usize| {
        (
            f64::midpoint(frame[one].0, frame[other].0),
            f64::midpoint(frame[one].1, frame[other].1),
        )
    };
    let mut twice = 0.0;
    for index in 0..4 {
        let (x0, y0) = frame[index];
        let (x1, y1) = frame[(index + 1) % 4];
        twice += x0.madd(y1, -(x1 * y0));
    }
    let (foot, head) = if twice < 0.0 {
        (side(2, 3), side(1, 0))
    } else {
        (side(0, 1), side(3, 2))
    };
    let (dx, dy) = (foot.0 - head.0, foot.1 - head.1);
    let length = dx.hypot(dy);
    if length <= f64::EPSILON {
        return (foot, foot);
    }
    (
        foot,
        (
            ROTATE_REACH_OUT.madd(dx / length, foot.0),
            ROTATE_REACH_OUT.madd(dy / length, foot.1),
        ),
    )
}

pub const ROTATE_HANDLE: usize = 8;

#[must_use]
pub const fn handle_places_ink(handle: usize) -> bool {
    matches!(handle, 0..=3 | ROTATE_HANDLE)
}

pub const SMALLEST_SCALE: f64 = 0.02;

#[must_use]
pub fn grip(quad: &Quad, handle: usize) -> (f64, f64) {
    let corners = quad.corners;
    let middle = |one: usize, other: usize| {
        (
            f64::midpoint(corners[one].0, corners[other].0),
            f64::midpoint(corners[one].1, corners[other].1),
        )
    };
    match handle {
        0..=3 => corners[handle],
        4 => middle(0, 1),
        5 => middle(1, 2),
        6 => middle(2, 3),
        7 => middle(3, 0),
        _ => rotate_handle(quad),
    }
}

#[must_use]
pub fn fixed_point(quad: &Quad, handle: usize) -> (f64, f64) {
    match handle {
        0..=3 => grip(quad, (handle + 2) % 4),
        4..=7 => grip(quad, 4 + (handle + 2) % 4),
        _ => quad.center(),
    }
}

#[must_use]
pub fn shaped(
    quad: &Quad,
    handle: usize,
    travel: (f64, f64),
    proportions: Proportions,
) -> Option<(pdf_paint::Matrix, (f64, f64))> {
    if handle > ROTATE_HANDLE
        || !travel.0.is_finite()
        || !travel.1.is_finite()
        || quad
            .corners
            .iter()
            .any(|(x, y)| !x.is_finite() || !y.is_finite())
    {
        return None;
    }
    let (along, across) = axes(quad)?;
    let basis = pdf_paint::Matrix {
        a: along.0,
        b: along.1,
        c: across.0,
        d: across.1,
        e: 0.0,
        f: 0.0,
    };
    let inverse = basis.inverse()?;
    let held = fixed_point(quad, handle);
    let from = grip(quad, handle);
    let to = (from.0 + travel.0, from.1 + travel.1);
    if handle == ROTATE_HANDLE {
        let turn = angle_between(held, from, to)?;
        return Some((
            pdf_paint::Matrix {
                a: turn.cos(),
                b: turn.sin(),
                c: -turn.sin(),
                d: turn.cos(),
                e: 0.0,
                f: 0.0,
            },
            held,
        ));
    }
    let local = |point: (f64, f64)| {
        inverse.transform(pdf_paint::Point {
            x: point.0 - held.0,
            y: point.1 - held.1,
        })
    };
    let (was, now) = (local(from), local(to));
    let (was_u, was_v) = (was.x, was.y);
    let (now_u, now_v) = (now.x, now.y);
    let (mut sx, mut sy) = match handle {
        0..=3 if proportions == Proportions::Kept => {
            let square = was_u.madd(was_u, was_v * was_v);
            if square <= f64::EPSILON {
                return None;
            }
            let factor = now_u.madd(was_u, now_v * was_v) / square;
            (factor, factor)
        }
        0..=3 => (ratio(was_u, now_u)?, ratio(was_v, now_v)?),
        4 | 6 => (1.0, ratio(was_v, now_v)?),
        _ => (ratio(was_u, now_u)?, 1.0),
    };
    sx = sx.max(SMALLEST_SCALE);
    sy = sy.max(SMALLEST_SCALE);
    if !sx.is_finite() || !sy.is_finite() {
        return None;
    }
    Some((
        basis
            .multiply(pdf_paint::Matrix {
                a: sx,
                d: sy,
                ..pdf_paint::Matrix::IDENTITY
            })
            .multiply(inverse),
        held,
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Proportions {
    Kept,
    Free,
}

fn axes(quad: &Quad) -> Option<((f64, f64), (f64, f64))> {
    let unit = |one: (f64, f64), other: (f64, f64)| {
        let (dx, dy) = (other.0 - one.0, other.1 - one.1);
        let length = dx.hypot(dy);
        (length > 1e-9).then_some((dx / length, dy / length))
    };
    Some((
        unit(quad.corners[0], quad.corners[1])?,
        unit(quad.corners[0], quad.corners[3])?,
    ))
}

fn ratio(was: f64, now: f64) -> Option<f64> {
    (was.abs() > 1e-9).then(|| now / was)
}

fn angle_between(centre: (f64, f64), from: (f64, f64), to: (f64, f64)) -> Option<f64> {
    let one = (from.0 - centre.0, from.1 - centre.1);
    let other = (to.0 - centre.0, to.1 - centre.1);
    if one.0.hypot(one.1) < 1e-6 || other.0.hypot(other.1) < 1e-6 {
        return None;
    }
    Some(other.1.atan2(other.0) - one.1.atan2(one.0))
}

pub const HANDLE_REACH: f64 = 6.0;

pub const SMALLEST_FRAME: f64 = HANDLE_REACH * 4.0;

#[must_use]
pub fn handle_at(quad: &Quad, point: (f64, f64)) -> Option<usize> {
    let mut places = Vec::with_capacity(9);
    places.extend_from_slice(&handles(quad));
    places.push(rotate_handle(quad));
    nearest_handle(&places, point)
}

fn nearest_handle(places: &[(f64, f64)], point: (f64, f64)) -> Option<usize> {
    places
        .iter()
        .enumerate()
        .map(|(index, (x, y))| (index, (x - point.0).hypot(y - point.1)))
        .filter(|(_, distance)| *distance <= HANDLE_REACH)
        .min_by(|(_, one), (_, other)| one.total_cmp(other))
        .map(|(index, _)| index)
}

#[must_use]
pub fn resized(bounds: [f64; 4], handle: usize, dx: f64, dy: f64) -> [f64; 4] {
    let [x0, y0, x1, y1] = bounds;
    let (left, top, right, bottom) = match handle {
        0 => (true, true, false, false),
        1 => (false, true, true, false),
        2 => (false, false, true, true),
        3 => (true, false, false, true),
        4 => (false, true, false, false),
        5 => (false, false, true, false),
        6 => (false, false, false, true),
        7 => (true, false, false, false),
        _ => (false, false, false, false),
    };
    [
        if left {
            (x0 + dx).min(x1 - SMALLEST_FRAME)
        } else {
            x0
        },
        if top {
            (y0 + dy).min(y1 - SMALLEST_FRAME)
        } else {
            y0
        },
        if right {
            (x1 + dx).max(x0 + SMALLEST_FRAME)
        } else {
            x1
        },
        if bottom {
            (y1 + dy).max(y0 + SMALLEST_FRAME)
        } else {
            y1
        },
    ]
}

#[must_use]
pub fn grow(bounds: [f64; 4], by: f64) -> [f64; 4] {
    [
        bounds[0] - by,
        bounds[1] - by,
        bounds[2] + by,
        bounds[3] + by,
    ]
}

const WIDTH_SLACK: f64 = 0.5;

#[must_use]
pub fn frame_width_changed(started: [f64; 4], now: [f64; 4]) -> bool {
    ((now[2] - now[0]) - (started[2] - started[0])).abs() > WIDTH_SLACK
}

#[must_use]
pub fn toolbar_at(
    block: [f64; 4],
    size: (f64, f64),
    view: [f64; 4],
    clear_above: f64,
) -> (f64, f64) {
    let gap = FRAME_INSET * 3.0;
    let below = block[3] + gap;
    let above = block[1] - gap - clear_above.max(0.0) - size.1;
    let y = if below + size.1 <= view[3] {
        below
    } else if above >= view[1] {
        above
    } else {
        below.min(view[3] - size.1).max(view[1])
    };
    let x = (f64::midpoint(block[0], block[2]) - size.0 / 2.0)
        .clamp(view[0], (view[2] - size.0).max(view[0]));
    (x, y)
}

#[must_use]
pub fn cluster_styling(clusters: &[TextClusterBox], stop: &CaretStop) -> Option<usize> {
    let on_row = |index_in_line: usize| {
        clusters
            .iter()
            .position(|cluster| cluster.line == stop.line && cluster.index_in_line == index_in_line)
    };
    stop.offset
        .checked_sub(1)
        .and_then(on_row)
        .or_else(|| on_row(stop.offset))
}

#[must_use]
pub fn laid_over(held: &pdf_edit::TextStyle, asked: &pdf_edit::TextStyle) -> pdf_edit::TextStyle {
    pdf_edit::TextStyle {
        size: asked.size.or(held.size),
        fill: asked.fill.or(held.fill),
        bold: asked.bold.or(held.bold),
        italic: asked.italic.or(held.italic),
        family: asked.family.clone().or_else(|| held.family.clone()),
        underline: asked.underline.or(held.underline),
        line_spacing: asked.line_spacing.or(held.line_spacing),
    }
}

#[must_use]
pub fn run_at(runs: &[RunBox], point: (f64, f64)) -> Option<usize> {
    runs.iter()
        .enumerate()
        .rfind(|(_, run)| {
            let [x0, y0, x1, y1] = run.bounds;
            point.0 >= x0 && point.0 < x1 && point.1 >= y0 && point.1 < y1
        })
        .map(|(index, _)| index)
}

#[must_use]
pub fn object_at(objects: &[ObjectBox], point: (f64, f64)) -> Option<usize> {
    objects
        .iter()
        .enumerate()
        .rfind(|(_, object)| frame_quad(&Quad::from_pixels(object.quad)).contains(point))
        .map(|(index, _)| index)
}

#[must_use]
pub fn object_put_at(objects: &[ObjectBox], wanted: &Quad) -> Option<usize> {
    let centres = objects
        .iter()
        .map(|object| Quad::from_pixels(object.quad).center());
    put_at(centres, wanted, &[])
}

fn put_at(
    centres: impl Iterator<Item = (f64, f64)>,
    wanted: &Quad,
    taken: &[usize],
) -> Option<usize> {
    let [x0, y0, x1, y1] = wanted.bounds();
    let reach = ((x1 - x0).hypot(y1 - y0) / 4.0).max(4.0);
    let (cx, cy) = wanted.center();
    centres
        .enumerate()
        .filter(|(index, _)| !taken.contains(index))
        .map(|(index, (x, y))| (index, (x - cx).hypot(y - cy)))
        .filter(|(_, distance)| *distance <= reach)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(index, _)| index)
}

#[must_use]
pub fn group_put_at(
    blocks: &[TextBlockBox],
    objects: &[ObjectBox],
    wanted_blocks: &[Quad],
    wanted_objects: &[Quad],
) -> Option<(Vec<usize>, Vec<usize>)> {
    let mut found_blocks: Vec<usize> = Vec::with_capacity(wanted_blocks.len());
    for wanted in wanted_blocks {
        let centres = blocks
            .iter()
            .map(|block| Quad::from_pixels(block.quad).center());
        found_blocks.push(put_at(centres, wanted, &found_blocks)?);
    }
    let mut found_objects: Vec<usize> = Vec::with_capacity(wanted_objects.len());
    for wanted in wanted_objects {
        let centres = objects
            .iter()
            .map(|object| Quad::from_pixels(object.quad).center());
        found_objects.push(put_at(centres, wanted, &found_objects)?);
    }
    found_blocks.sort_unstable();
    found_objects.sort_unstable();
    Some((found_blocks, found_objects))
}

#[must_use]
#[allow(clippy::cast_precision_loss)]
fn screen_size(width: u32, height: u32) -> (f32, f32) {
    (width as f32, height as f32)
}

#[must_use]
#[allow(clippy::cast_possible_truncation)]
fn screen_box(bounds: [f64; 4]) -> [f32; 4] {
    [
        bounds[0] as f32,
        bounds[1] as f32,
        bounds[2] as f32,
        bounds[3] as f32,
    ]
}

#[must_use]
#[allow(clippy::cast_possible_truncation)]
fn caret_line(stop: &CaretStop) -> ([f32; 2], [f32; 2]) {
    let foot = [stop.at[0] as f32, stop.at[1] as f32];
    (
        [
            (stop.at[0] + stop.up[0]) as f32,
            (stop.at[1] + stop.up[1]) as f32,
        ],
        foot,
    )
}

#[must_use]
pub fn shown_caret(
    stops: &[CaretStop],
    clusters: &[TextClusterBox],
    at: usize,
    frame: Option<[f64; 4]>,
) -> Option<CaretStop> {
    let mut stop = stops.get(at)?.clone();
    let Some(frame) = frame else {
        return Some(stop);
    };
    let blank = |index: usize| {
        clusters
            .iter()
            .find(|cluster| cluster.line == stop.line && cluster.index_in_line == index)
            .and_then(|cluster| cluster.text.as_deref())
            .is_some_and(|text| !text.is_empty() && text.chars().all(char::is_whitespace))
    };
    if stop.offset == 0 || !blank(stop.offset - 1) {
        return Some(stop);
    }
    let height = stop.up[0].hypot(stop.up[1]);
    if height <= 1e-9 {
        return Some(stop);
    }
    let along = (-stop.up[1] / height, stop.up[0] / height);
    let mut reach = f64::INFINITY;
    for (point, low, high, step) in [
        (stop.at[0], frame[0], frame[2], along.0),
        (stop.at[1], frame[1], frame[3], along.1),
    ] {
        if step.abs() > 1e-9 {
            reach = reach.min(((if step > 0.0 { high } else { low }) - point) / step);
        }
    }
    if reach >= 0.0 {
        return Some(stop);
    }
    let mut ink = stop.offset - 1;
    while ink > 0 && blank(ink - 1) {
        ink -= 1;
    }
    let word_end = stops
        .iter()
        .find(|other| other.line == stop.line && other.offset == ink)
        .map_or(f64::NEG_INFINITY, |end| {
            (end.at[0] - stop.at[0]).madd(along.0, (end.at[1] - stop.at[1]) * along.1)
        });
    let back = reach.max(word_end).min(0.0);
    stop.at = [
        back.madd(along.0, stop.at[0]),
        back.madd(along.1, stop.at[1]),
    ];
    Some(stop)
}

#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn texture_box(bounds: [f64; 4], size: (u32, u32)) -> Option<[f32; 4]> {
    if size.0 == 0 || size.1 == 0 {
        return None;
    }
    let (width, height) = (f64::from(size.0), f64::from(size.1));
    Some([
        (bounds[0] / width) as f32,
        (bounds[1] / height) as f32,
        (bounds[2] / width) as f32,
        (bounds[3] / height) as f32,
    ])
}

#[cfg(test)]
mod tests {
    use pdf_cli::{CaretStop, TextClusterBox};

    use super::{
        FRAME_INSET, HANDLE_REACH, MARGIN, Placement, Proportions, Quad, RunBox, SMALLEST_FRAME,
        Step, block_at, caret_at, caret_step, covers, draw_window, frame_of, frame_width_changed,
        grow, handle_at, handles, on_the_ink, resized, run_at, selection_between, selection_quad,
        size_at, standing_block_at, stands, texture_box, toolbar_at, visible_after, zoom_anchor,
    };

    const fn flat() -> Placement {
        Placement {
            origin: (10.0, 20.0),
            size: (100, 50),
            stretch: 1.0,
        }
    }

    fn stops() -> Vec<CaretStop> {
        let mut stops = Vec::new();
        for offset in 0..5_u32 {
            stops.push(CaretStop {
                line: 0,
                offset: offset as usize,
                at: [20.0f64.mul_add(f64::from(offset), 10.0), 30.0],
                up: [0.0, -12.0],
            });
        }
        for offset in 0..3_u32 {
            stops.push(CaretStop {
                line: 1,
                offset: offset as usize,
                at: [20.0f64.mul_add(f64::from(offset), 12.0), 60.0],
                up: [0.0, -12.0],
            });
        }
        stops
    }

    #[test]
    fn typing_takes_the_style_of_the_cluster_before_the_caret() {
        let cluster = |line: usize, index_in_line: usize| TextClusterBox {
            anchor: format!("{line}:{index_in_line}"),
            glyphs: 0..1,
            box_pixels: None,
            stacked: false,
            line,
            index_in_line,
            text: None,
        };
        let stop = |line: usize, offset: usize| CaretStop {
            line,
            offset,
            at: [0.0, 0.0],
            up: [0.0, -10.0],
        };
        let clusters = vec![cluster(0, 0), cluster(0, 1), cluster(1, 0), cluster(1, 1)];
        assert_eq!(
            super::cluster_styling(&clusters, &stop(0, 2)),
            Some(1),
            "after the last"
        );
        assert_eq!(
            super::cluster_styling(&clusters, &stop(1, 1)),
            Some(2),
            "between two"
        );
        assert_eq!(
            super::cluster_styling(&clusters, &stop(1, 0)),
            Some(2),
            "at a row's start"
        );
        assert_eq!(
            super::cluster_styling(&clusters, &stop(3, 0)),
            None,
            "an empty row"
        );
    }

    #[test]
    fn a_choice_at_a_caret_is_laid_over_the_ones_before_it() {
        let held = pdf_edit::TextStyle {
            size: Some(20.0),
            bold: Some(true),
            ..pdf_edit::TextStyle::default()
        };
        let asked = pdf_edit::TextStyle {
            size: Some(14.0),
            fill: Some([1.0, 0.0, 0.0]),
            family: Some("Noto Sans".to_owned()),
            ..pdf_edit::TextStyle::default()
        };
        assert_eq!(
            super::laid_over(&held, &asked),
            pdf_edit::TextStyle {
                size: Some(14.0),
                fill: Some([1.0, 0.0, 0.0]),
                bold: Some(true),
                family: Some("Noto Sans".to_owned()),
                ..pdf_edit::TextStyle::default()
            }
        );
        assert_eq!(super::laid_over(&asked, &held).size, Some(20.0));
        assert_eq!(
            super::laid_over(&asked, &held).family.as_deref(),
            Some("Noto Sans")
        );
    }

    #[test]
    fn an_edited_object_is_found_again_by_where_it_went() {
        let object = |quad: pdf_cli::QuadPixels| pdf_cli::ObjectBox {
            object: 0,
            anchor: String::new(),
            kind: pdf_semantics::ObjectKind::Path,
            box_pixels: [0.0; 4],
            quad,
        };
        let upright =
            |x0: f64, y0: f64, x1: f64, y1: f64| object([[x0, y1], [x1, y1], [x1, y0], [x0, y0]]);
        let objects = vec![
            upright(116.0, 486.0, 316.0, 486.0),
            object([
                [100.0, 400.0],
                [400.0, 100.0],
                [480.0, 180.0],
                [180.0, 480.0],
            ]),
        ];
        let turned = Quad::from_pixels([
            [101.0, 401.0],
            [401.0, 101.0],
            [481.0, 181.0],
            [181.0, 481.0],
        ]);
        assert_eq!(super::object_put_at(&objects, &turned), Some(1));
        assert_eq!(super::object_put_at(&objects[..1], &turned), None);
        let rule = Quad::from_pixels([
            [116.0, 489.0],
            [316.0, 489.0],
            [316.0, 489.0],
            [116.0, 489.0],
        ]);
        assert_eq!(super::object_put_at(&objects, &rule), Some(0));
        let far = Quad::from_pixels([
            [116.0, 600.0],
            [316.0, 600.0],
            [316.0, 600.0],
            [116.0, 600.0],
        ]);
        assert_eq!(super::object_put_at(&objects, &far), None);
    }

    fn a_page() -> (Vec<pdf_cli::TextBlockBox>, Vec<pdf_cli::ObjectBox>) {
        let block = |x0: f64, y0: f64, x1: f64, y1: f64| pdf_cli::TextBlockBox {
            box_pixels: [x0, y0, x1, y1],
            layout_pixels: [x0, y0, x1, y1],
            turn: 0.0,
            quad: [[x0, y1], [x1, y1], [x1, y0], [x0, y0]],
            lines: Vec::new(),
            anchors: Vec::new(),
            runs_on: false,
            shape: None,
            font: None,
        };
        let object = |x0: f64, y0: f64, x1: f64, y1: f64| pdf_cli::ObjectBox {
            object: 0,
            anchor: String::new(),
            kind: pdf_semantics::ObjectKind::Path,
            box_pixels: [x0, y0, x1, y1],
            quad: [[x0, y1], [x1, y1], [x1, y0], [x0, y0]],
        };
        (
            vec![
                block(100.0, 100.0, 300.0, 140.0),
                block(100.0, 200.0, 300.0, 240.0),
                block(100.0, 300.0, 300.0, 340.0),
            ],
            vec![object(400.0, 100.0, 500.0, 200.0)],
        )
    }

    fn quad_of(x0: f64, y0: f64, x1: f64, y1: f64) -> Quad {
        Quad::from_pixels([[x0, y1], [x1, y1], [x1, y0], [x0, y0]])
    }

    #[test]
    fn a_whole_group_is_found_again_where_the_move_put_it() {
        let (blocks, objects) = a_page();
        let wanted_blocks = [
            quad_of(100.0, 200.0, 300.0, 240.0),
            quad_of(100.0, 300.0, 300.0, 340.0),
        ];
        let wanted_objects = [quad_of(400.0, 100.0, 500.0, 200.0)];
        assert_eq!(
            super::group_put_at(&blocks, &objects, &wanted_blocks, &wanted_objects),
            Some((vec![1, 2], vec![0]))
        );
    }

    #[test]
    fn a_group_missing_a_member_is_no_group_at_all() {
        let (blocks, objects) = a_page();
        let merged = blocks[..2].to_vec();
        let wanted = [
            quad_of(100.0, 100.0, 300.0, 140.0),
            quad_of(100.0, 200.0, 300.0, 240.0),
            quad_of(100.0, 300.0, 300.0, 340.0),
        ];
        assert_eq!(
            super::group_put_at(&merged, &objects, &wanted, &[]),
            None,
            "a group kept a member that is not there"
        );
        assert_eq!(
            super::group_put_at(&blocks, &objects, &wanted, &[]),
            Some((vec![0, 1, 2], Vec::new()))
        );
        let one = blocks[1..2].to_vec();
        let both = [
            quad_of(100.0, 200.0, 300.0, 240.0),
            quad_of(104.0, 204.0, 304.0, 244.0),
        ];
        assert_eq!(
            super::group_put_at(&one, &objects, &both, &[]),
            None,
            "one block was handed back as two members"
        );
    }

    #[test]
    fn deletion_direction_and_row_boundaries_are_explicit() {
        let stops = stops();
        assert_eq!(super::deletion_between(&stops, 0, 0, true), None);
        assert_eq!(super::deletion_between(&stops, 4, 4, false), None);
        assert_eq!(super::deletion_between(&stops, 2, 2, true), Some((0, 1, 2)));
        assert_eq!(
            super::deletion_between(&stops, 2, 2, false),
            Some((0, 2, 3))
        );
        assert_eq!(super::deletion_between(&stops, 3, 1, true), Some((0, 1, 3)));
        assert_eq!(
            super::deletion_between(&stops, 3, 1, false),
            Some((0, 1, 3))
        );
        assert_eq!(super::deletion_between(&stops, 4, 5, false), None);
    }

    fn spelled() -> Vec<TextClusterBox> {
        ["a", "b", " ", "c", "d"]
            .into_iter()
            .enumerate()
            .map(|(index, text)| TextClusterBox {
                anchor: "run".to_owned(),
                glyphs: index..index + 1,
                box_pixels: Some([
                    20.0f64.mul_add(f64::from(u32::try_from(index).unwrap_or(0)), 10.0),
                    18.0,
                    20.0f64.mul_add(f64::from(u32::try_from(index).unwrap_or(0)), 28.0),
                    30.0,
                ]),
                stacked: false,
                line: 0,
                index_in_line: index,
                text: Some(text.to_owned()),
            })
            .collect()
    }

    #[test]
    fn a_double_click_takes_the_word_and_not_the_space_beside_it() {
        let clusters = spelled();
        for offset in 0..=2 {
            assert_eq!(
                super::word_at(&clusters, 0, offset),
                Some((0, 2)),
                "{offset}"
            );
        }
        for offset in 3..=5 {
            assert_eq!(
                super::word_at(&clusters, 0, offset),
                Some((3, 5)),
                "{offset}"
            );
        }
    }

    #[test]
    fn a_double_click_on_a_lone_space_selects_nothing() {
        let mut clusters = spelled();
        clusters.retain(|cluster| cluster.index_in_line == 2);
        assert_eq!(super::word_at(&clusters, 0, 2), None);
        assert_eq!(super::word_at(&clusters, 0, 3), None);
    }

    #[test]
    fn a_page_that_declares_no_text_has_one_word_per_row() {
        let clusters: Vec<TextClusterBox> = spelled()
            .into_iter()
            .map(|cluster| TextClusterBox {
                text: None,
                ..cluster
            })
            .collect();
        assert_eq!(super::word_at(&clusters, 0, 2), Some((0, 5)));
        assert_eq!(super::row_at(&stops(), 0), Some((0, 4)));
    }

    #[test]
    fn a_triple_click_takes_the_row_it_is_on() {
        let stops = stops();
        assert_eq!(super::row_at(&stops, 0), Some((0, 4)));
        assert_eq!(super::row_at(&stops, 1), Some((0, 2)));
        assert_eq!(super::row_at(&stops, 2), None);
        assert_eq!(super::stop_at(&stops, 1, 0), Some(5));
        assert_eq!(super::stop_at(&stops, 1, 9), None);
    }

    #[test]
    fn a_marquee_takes_what_it_wholly_contains() {
        let quads = [
            Quad::of([10.0, 10.0, 40.0, 40.0]),
            Quad::of([50.0, 10.0, 90.0, 40.0]),
            Quad::of([80.0, 50.0, 120.0, 80.0]),
        ];
        assert_eq!(super::swept(&quads, [0.0, 0.0, 100.0, 45.0]), vec![0, 1]);
        assert_eq!(super::swept(&quads, [0.0, 0.0, 45.0, 45.0]), vec![0]);
        assert!(super::swept(&quads, [0.0, 0.0, 5.0, 5.0]).is_empty());
        let swept = super::sweep_box((100.0, 45.0), (0.0, 0.0));
        assert!(
            swept
                .iter()
                .zip([0.0, 0.0, 100.0, 45.0])
                .all(|(one, other)| (one - other).abs() < f64::EPSILON),
            "{swept:?}"
        );
    }

    #[test]
    fn a_marquee_measures_a_turned_object_by_its_corners() {
        let line = Quad {
            corners: [(20.0, 20.0), (80.0, 80.0), (78.0, 82.0), (18.0, 22.0)],
        };
        assert!(line.inside([10.0, 10.0, 90.0, 90.0]));
        assert!(!line.inside([10.0, 10.0, 90.0, 50.0]));
    }

    #[test]
    fn shift_on_a_corner_lets_the_two_axes_differ() {
        let quad = Quad::of([100.0, 100.0, 300.0, 200.0]);
        let (free, about) =
            super::shaped(&quad, 2, (200.0, 0.0), Proportions::Free).expect("a corner drag");
        let stretched = quad.transformed(free, about);
        assert_eq!(about, (100.0, 100.0));
        assert!((stretched.corners[2].0 - 500.0).abs() < 1e-9);
        assert!((stretched.corners[2].1 - 200.0).abs() < 1e-9);
        let (kept, about) =
            super::shaped(&quad, 2, (200.0, 0.0), Proportions::Kept).expect("a corner drag");
        let grown = quad.transformed(kept, about);
        assert!((grown.corners[2].1 - 200.0).abs() > 1.0);
        let side = |quad: &Quad, one: usize, other: usize| {
            (quad.corners[other].0 - quad.corners[one].0)
                .hypot(quad.corners[other].1 - quad.corners[one].1)
        };
        let was = side(&quad, 0, 1) / side(&quad, 0, 3);
        let now = side(&grown, 0, 1) / side(&grown, 0, 3);
        assert!((was - now).abs() < 1e-9, "{was} vs {now}");
    }

    #[test]
    fn shift_changes_nothing_on_a_side_handle_or_the_rotate_handle() {
        let quad = Quad::of([100.0, 100.0, 300.0, 200.0]);
        for handle in [4, 5, 6, 7, super::ROTATE_HANDLE] {
            let kept = super::shaped(&quad, handle, (30.0, 20.0), Proportions::Kept);
            let free = super::shaped(&quad, handle, (30.0, 20.0), Proportions::Free);
            assert_eq!(kept, free, "handle {handle}");
        }
    }

    #[test]
    fn shift_on_a_drag_keeps_the_larger_of_the_two_travels() {
        let same = |one: (f64, f64), other: (f64, f64)| {
            (one.0 - other.0).abs() < f64::EPSILON && (one.1 - other.1).abs() < f64::EPSILON
        };
        assert!(same(super::along_one_axis((30.0, -7.0)), (30.0, 0.0)));
        assert!(same(super::along_one_axis((-3.0, 40.0)), (0.0, 40.0)));
        assert!(same(super::along_one_axis((5.0, -5.0)), (5.0, 0.0)));
    }

    #[test]
    fn tab_walks_the_page_in_the_order_the_file_writes_it() {
        let anchors = [
            "7:0:900".to_owned(),
            "7:0:100".to_owned(),
            "7:0:400,9:0:20".to_owned(),
            "not an anchor".to_owned(),
        ];
        let order = super::in_file_order(&anchors);
        assert_eq!(order, vec![1, 0, 2, 3]);
        assert_eq!(super::next_in_order(&order, None), Some(1));
        assert_eq!(super::next_in_order(&order, Some(1)), Some(0));
        assert_eq!(super::next_in_order(&order, Some(0)), Some(2));
        assert_eq!(super::next_in_order(&order, Some(3)), Some(1));
        assert_eq!(super::next_in_order(&order, Some(99)), Some(1));
        assert_eq!(super::next_in_order(&[], Some(0)), None);
    }

    fn run(bounds: [f64; 4]) -> RunBox {
        RunBox {
            anchor: format!("{bounds:?}"),
            bounds,
            text: std::sync::Arc::default(),
            em: 10.0,
            fill: None,
            family: None,
        }
    }

    #[test]
    fn a_pointer_outside_the_image_is_not_on_the_page() {
        let placed = flat();
        assert_eq!(placed.page_point((10.0, 20.0)), Some((0.0, 0.0)));
        assert_eq!(placed.page_point((60.0, 45.0)), Some((50.0, 25.0)));
        assert_eq!(placed.page_point((110.0, 45.0)), None);
        assert_eq!(placed.page_point((60.0, 70.0)), None);
        assert_eq!(placed.page_point((9.0, 45.0)), None);
        assert_eq!(placed.page_point((60.0, 19.0)), None);
    }

    #[test]
    fn a_stretched_page_maps_a_click_back_through_its_stretch() {
        let placed = Placement {
            origin: (10.0, 20.0),
            size: (100, 50),
            stretch: 2.0,
        };
        assert_eq!(placed.screen_size(), (200.0, 100.0));
        assert_eq!(placed.page_point((10.0, 20.0)), Some((0.0, 0.0)));
        assert_eq!(placed.page_point((110.0, 70.0)), Some((50.0, 25.0)));
        assert_eq!(placed.page_point((209.0, 70.0)), Some((99.5, 25.0)));
        assert_eq!(placed.page_point((210.0, 70.0)), None);
        assert!(same(
            placed.screen_box([1.0, 2.0, 3.0, 4.0]),
            [2.0, 4.0, 6.0, 8.0]
        ));
    }

    #[test]
    fn a_collapsed_placement_has_no_points_on_it() {
        let placed = Placement {
            origin: (0.0, 0.0),
            size: (100, 50),
            stretch: 0.0,
        };
        assert_eq!(placed.page_point((0.0, 0.0)), None);
    }

    #[test]
    fn overlapping_runs_resolve_to_the_one_drawn_last() {
        let runs = [
            run([0.0, 0.0, 40.0, 20.0]),
            run([10.0, 5.0, 50.0, 25.0]),
            run([80.0, 80.0, 90.0, 90.0]),
        ];
        assert_eq!(run_at(&runs, (5.0, 2.0)), Some(0));
        assert_eq!(run_at(&runs, (15.0, 10.0)), Some(1));
        assert_eq!(run_at(&runs, (85.0, 85.0)), Some(2));
        assert_eq!(run_at(&runs, (60.0, 60.0)), None);
    }

    #[test]
    fn a_runs_far_edge_belongs_to_what_is_beyond_it() {
        let runs = [run([0.0, 0.0, 10.0, 10.0])];
        assert_eq!(run_at(&runs, (9.999, 9.999)), Some(0));
        assert_eq!(run_at(&runs, (10.0, 5.0)), None);
        assert_eq!(run_at(&runs, (5.0, 10.0)), None);
    }

    fn same<const N: usize>(actual: [f32; N], expected: [f32; N]) -> bool {
        actual
            .iter()
            .zip(expected)
            .all(|(actual, expected)| actual.to_bits() == expected.to_bits())
    }

    fn alike(actual: [f64; 4], expected: [f64; 4]) -> bool {
        actual
            .iter()
            .zip(expected)
            .all(|(actual, expected)| (actual - expected).abs() < 1e-9)
    }

    #[test]
    fn a_page_box_becomes_screen_offsets_and_texture_coordinates() {
        let placed = Placement {
            origin: (0.0, 0.0),
            size: (511, 709),
            stretch: 1.0,
        };
        let (width, height) = placed.screen_size();
        assert_eq!(width.to_bits(), 511.0_f32.to_bits());
        assert_eq!(height.to_bits(), 709.0_f32.to_bits());
        assert!(same(
            placed.screen_box([1.5, 2.5, 3.5, 4.5]),
            [1.5, 2.5, 3.5, 4.5]
        ));
        let coordinates =
            texture_box([0.0, 0.0, 50.0, 25.0], (100, 100)).expect("a page with area");
        assert!(same(coordinates, [0.0, 0.0, 0.5, 0.25]));
        assert!(texture_box([0.0, 0.0, 1.0, 1.0], (0, 10)).is_none());
        assert!(texture_box([0.0, 0.0, 1.0, 1.0], (10, 0)).is_none());
    }

    #[test]
    fn a_click_finds_the_nearest_caret_stop() {
        let stops = stops();
        assert_eq!(caret_at(&stops, (52.0, 24.0)), Some(2));
        assert_eq!(caret_at(&stops, (11.0, 25.0)), Some(0));
        assert_eq!(caret_at(&stops, (14.0, 55.0)), Some(5));
        assert_eq!(caret_at(&[], (0.0, 0.0)), None);
    }

    #[test]
    fn arrows_walk_the_row_and_stay_on_the_page() {
        let stops = stops();
        assert_eq!(caret_step(&stops, 2, Step::Right), 3);
        assert_eq!(caret_step(&stops, 2, Step::Left), 1);
        assert_eq!(caret_step(&stops, 0, Step::Left), 0);
        assert_eq!(caret_step(&stops, 4, Step::Right), 4);
        assert_eq!(caret_step(&stops, 0, Step::Up), 0);
        assert_eq!(caret_step(&stops, 5, Step::Down), 5);
    }

    #[test]
    fn up_and_down_keep_the_column_rather_than_the_offset() {
        let stops = stops();
        assert_eq!(caret_step(&stops, 4, Step::Down), 7);
        assert_eq!(caret_step(&stops, 5, Step::Up), 0);
        assert_eq!(caret_step(&stops, 7, Step::Up), 2);
    }

    #[test]
    fn a_caret_is_drawn_upward_from_its_stop() {
        let stop = &stops()[0];
        let (top, bottom) = flat().caret_line(stop);
        assert!(same(bottom, [10.0, 30.0]), "{bottom:?}");
        assert!(same(top, [10.0, 18.0]), "{top:?}");
        assert!(top[1] < bottom[1], "a caret that hangs below its own row");
    }

    #[test]
    fn what_a_scroll_area_shows_is_named_in_the_pages_own_pixels() {
        let placed = Placement {
            origin: (10.0, 20.0),
            size: (100, 50),
            stretch: 1.0,
        };
        assert_eq!(
            placed.visible_page_box([10.5, 20.5, 40.5, 30.5]),
            Some([0, 0, 31, 11]),
            "a half-covered pixel is covered"
        );
        assert_eq!(
            placed.visible_page_box([0.0, 0.0, 15.0, 25.0]),
            Some([0, 0, 5, 5])
        );
        assert_eq!(placed.visible_page_box([200.0, 0.0, 300.0, 25.0]), None);
        assert_eq!(placed.visible_page_box([0.0, 0.0, 5.0, 5.0]), None);

        let stretched = Placement {
            stretch: 2.0,
            ..placed
        };
        assert_eq!(
            stretched.visible_page_box([10.0, 20.0, 110.0, 120.0]),
            Some([0, 0, 50, 50]),
            "twice as large on screen is half as many page pixels"
        );
    }

    #[test]
    fn a_drawn_window_is_the_viewport_grown_and_clipped_to_the_page() {
        let page = (2041, 2834);
        let window = draw_window(page, [400, 500, 1600, 1400]).expect("a viewport with area");
        assert_eq!(
            window,
            [400 - MARGIN, 500 - MARGIN, 1600 + MARGIN, 1400 + MARGIN]
        );

        assert_eq!(
            draw_window(page, [0, 0, 100, 100]),
            Some([0, 0, 100 + MARGIN, 100 + MARGIN])
        );
        assert_eq!(
            draw_window(page, [1900, 2700, 2041, 2834]),
            Some([1900 - MARGIN, 2700 - MARGIN, 2041, 2834])
        );

        assert_eq!(draw_window(page, [10, 10, 10, 20]), None, "no width");
        assert_eq!(draw_window((0, 0), [0, 0, 10, 10]), None, "no page");
    }

    #[test]
    fn a_scroll_inside_what_is_drawn_needs_nothing_drawn() {
        let drawn = [100, 100, 900, 900];
        assert!(covers(drawn, [100, 100, 900, 900]), "exactly it");
        assert!(covers(drawn, [200, 200, 800, 800]), "well inside it");
        assert!(!covers(drawn, [99, 200, 800, 800]), "one pixel left of it");
        assert!(
            !covers(drawn, [200, 200, 901, 800]),
            "one pixel right of it"
        );
        assert!(!covers(drawn, [200, 200, 800, 901]), "one pixel below it");
    }

    #[test]
    fn a_zoom_keeps_the_point_under_the_pointer_under_it() {
        let offset = (100.0, 100.0);
        let pointer = (50.0, 50.0);
        let after = zoom_anchor(offset, pointer, 2.0, (1000.0, 1000.0), (400.0, 400.0), 0.0);
        assert_eq!(after, (250.0, 250.0));
        assert!((150.0f32.mul_add(2.0, -after.0) - pointer.0).abs() < 1e-3);

        assert_eq!(
            zoom_anchor(
                (0.0, 0.0),
                (10.0, 10.0),
                0.5,
                (1000.0, 1000.0),
                (400.0, 400.0),
                0.0
            ),
            (0.0, 0.0)
        );
        assert_eq!(
            zoom_anchor(
                (600.0, 600.0),
                (399.0, 399.0),
                0.5,
                (1000.0, 1000.0),
                (400.0, 400.0),
                0.0
            ),
            (100.0, 100.0),
            "the far edge of a 500-wide page in a 400-wide view"
        );
    }

    #[test]
    fn a_zoom_on_a_centred_page_keeps_the_point_under_the_pointer() {
        let across = zoom_anchor(
            (0.0, 0.0),
            (500.0, 116.0),
            4.0,
            (600.0, 1000.0),
            (1000.0, 800.0),
            16.0,
        );
        assert!((across.0 - 700.0).abs() < 1e-3, "{across:?}");
        let down = zoom_anchor(
            (0.0, 0.0),
            (500.0, 116.0),
            2.0,
            (600.0, 1000.0),
            (1000.0, 800.0),
            16.0,
        );
        assert!((down.1 - 100.0).abs() < 1e-3, "{down:?}");
        let small = zoom_anchor(
            (0.0, 0.0),
            (500.0, 116.0),
            1.25,
            (600.0, 1000.0),
            (1000.0, 800.0),
            16.0,
        );
        assert!(small.0.abs() < 1e-3, "{small:?}");
    }

    #[test]
    fn a_scroll_offset_says_which_page_pixels_will_be_on_screen() {
        assert_eq!(
            visible_after((100.0, 200.0), (400.0, 300.0), (2041, 2834)),
            Some([100, 200, 500, 500])
        );
        assert_eq!(
            visible_after((99.5, 0.0), (400.0, 300.0), (2041, 2834)),
            Some([99, 0, 500, 300])
        );
        assert_eq!(
            visible_after((0.0, 0.0), (4000.0, 4000.0), (500, 700)),
            Some([0, 0, 500, 700])
        );
        assert_eq!(visible_after((0.0, 0.0), (0.0, 300.0), (500, 700)), None);
    }

    #[test]
    fn a_page_at_a_new_scale_is_the_old_one_scaled() {
        assert_eq!(size_at((1021, 1417), 2.0, 4.0), (2042, 2834));
        assert_eq!(size_at((1021, 1417), 2.0, 1.0), (511, 709));
        assert_eq!(size_at((1021, 1417), 0.0, 4.0), (1021, 1417));
    }

    #[test]
    fn an_empty_block_is_gone_unless_it_is_being_typed_in() {
        assert!(stands(true, 0, None));
        assert!(!stands(false, 0, None));
        assert!(!stands(false, 0, Some(1)));
        assert!(stands(false, 1, Some(1)));
        let blocks = [
            Quad::of([0.0, 0.0, 100.0, 100.0]),
            Quad::of([10.0, 10.0, 30.0, 30.0]),
        ];
        let empty = [false, true];
        let standing = |typing_in| move |index: usize| stands(!empty[index], index, typing_in);
        assert_eq!(
            standing_block_at(&blocks, standing(None), (20.0, 20.0)),
            Some(0),
            "the emptied inner block is clicked through"
        );
        assert_eq!(
            standing_block_at(&blocks, standing(Some(1)), (20.0, 20.0)),
            Some(1),
            "unless the caret is in it"
        );
    }

    #[test]
    fn a_click_inside_two_blocks_takes_the_smaller() {
        let blocks = [
            Quad::of([0.0, 0.0, 100.0, 100.0]),
            Quad::of([10.0, 10.0, 30.0, 30.0]),
            Quad::of([200.0, 200.0, 210.0, 210.0]),
        ];
        assert_eq!(block_at(&blocks, (20.0, 20.0)), Some(1), "inside both");
        assert_eq!(block_at(&blocks, (50.0, 50.0)), Some(0), "only the outer");
        assert_eq!(block_at(&blocks, (205.0, 205.0)), Some(2));
        assert_eq!(block_at(&blocks, (150.0, 150.0)), None, "on neither");
        assert_eq!(block_at(&blocks, (100.0, 50.0)), Some(0));
        assert_eq!(block_at(&blocks, (-1.0, 50.0)), Some(0));
        assert_eq!(block_at(&blocks, (100.0 + FRAME_INSET, 50.0)), Some(0));
        assert_eq!(
            block_at(&blocks, (100.0 + FRAME_INSET + 1e-9, 50.0)),
            None,
            "and beyond the frame there is nothing"
        );
    }

    #[test]
    fn a_turned_block_is_framed_by_its_ink_and_not_by_its_bounding_box() {
        let angle: f64 = 15.0_f64.to_radians();
        let (along, up) = (
            (angle.cos() * 200.0, angle.sin() * 200.0),
            (-angle.sin() * 12.0, angle.cos() * 12.0),
        );
        let turned = Quad {
            corners: [
                (100.0, 100.0),
                (100.0 + along.0, 100.0 + along.1),
                (100.0 + along.0 + up.0, 100.0 + along.1 + up.1),
                (100.0 + up.0, 100.0 + up.1),
            ],
        };
        assert!(!turned.upright());
        let box_of = turned.bounds();
        let boxed = (box_of[2] - box_of[0]) * (box_of[3] - box_of[1]);
        let empty = 1.0 - turned.area() / boxed;
        assert!(empty > 0.35, "{empty}");

        for corner in [
            (box_of[0] + 1.0, box_of[3] - 1.0),
            (box_of[2] - 1.0, box_of[1] + 1.0),
        ] {
            assert_eq!(
                block_at(&[turned], corner),
                None,
                "{corner:?} is a corner of the box and not of the block"
            );
        }
        let middle = turned.center();
        assert_eq!(block_at(&[turned], middle), Some(0), "its own middle");
        let out = turned.grown(FRAME_INSET);
        assert!(out.contains(turned.corners[0]));
        assert!(out.area() > turned.area());
    }

    #[test]
    fn text_handles_change_only_width_and_exclude_object_controls() {
        let bounds = [100.0, 100.0, 300.0, 200.0];
        let quad = Quad::of(bounds);
        let turn = super::rotate_handle(&quad);
        let expected = [
            (7, (98.0, 150.0)),
            (5, (302.0, 150.0)),
            (super::ROTATE_HANDLE, turn),
        ];
        assert_eq!(super::text_handles(&quad).collect::<Vec<_>>(), expected);
        for (index, point) in expected {
            assert_eq!(super::text_handle_at(&quad, point), Some(index));
        }
        assert!(alike(
            resized(bounds, 7, -20.0, 40.0),
            [80.0, 100.0, 300.0, 200.0]
        ));
        assert!(alike(
            resized(bounds, 5, 20.0, -40.0),
            [100.0, 100.0, 320.0, 200.0]
        ));
        for (index, point) in handles(&quad)
            .into_iter()
            .chain(std::iter::once(super::rotate_handle(&quad)))
            .enumerate()
        {
            if ![5, 7, super::ROTATE_HANDLE].contains(&index) {
                assert_eq!(handle_at(&quad, point), Some(index));
                assert_eq!(super::text_handle_at(&quad, point), None);
            }
        }
        let short = Quad::of([100.0, 100.0, 300.0, 102.0]);
        assert_eq!(super::text_handle_at(&short, (98.0, 98.0)), Some(7));
        let turned = quad.transformed(
            pdf_paint::Matrix {
                a: 0.0,
                b: 1.0,
                c: -1.0,
                d: 0.0,
                e: 0.0,
                f: 0.0,
            },
            (0.0, 0.0),
        );
        assert_eq!(
            super::text_handles(&turned)
                .map(|(handle, _)| handle)
                .collect::<Vec<_>>(),
            [7, 5, super::ROTATE_HANDLE]
        );
        assert_eq!(super::text_handle_at(&turned, (98.0, 150.0)), None);
        assert_eq!(super::text_handle_at(&turned, (-150.0, 98.0)), Some(7));
        let sheared = Quad {
            corners: [
                (100.0, 100.0),
                (300.0, 100.0),
                (340.0, 200.0),
                (100.0, 200.0),
            ],
        };
        assert_eq!(
            super::text_handles(&sheared)
                .map(|(handle, _)| handle)
                .collect::<Vec<_>>(),
            [super::ROTATE_HANDLE]
        );
    }

    #[test]
    fn the_width_handles_of_a_turned_block_stand_on_its_own_turned_axis() {
        let bounds = [100.0, 100.0, 300.0, 200.0];
        let level = Quad::of(bounds);
        let widths = |quad: &Quad| {
            super::text_handles(quad)
                .filter(|(handle, _)| *handle != super::ROTATE_HANDLE)
                .collect::<Vec<_>>()
        };
        assert_eq!(widths(&level), [(7, (98.0, 150.0)), (5, (302.0, 150.0))]);

        let angle: f64 = 30.0_f64.to_radians();
        let turned = level.transformed(
            pdf_paint::Matrix {
                a: angle.cos(),
                b: angle.sin(),
                c: -angle.sin(),
                d: angle.cos(),
                e: 0.0,
                f: 0.0,
            },
            (200.0, 150.0),
        );
        let turned_about = |(x, y): (f64, f64)| {
            (
                angle.cos().mul_add(x - 200.0, -(angle.sin() * (y - 150.0))) + 200.0,
                angle.sin().mul_add(x - 200.0, angle.cos() * (y - 150.0)) + 150.0,
            )
        };
        for ((handle, place), (was, level_place)) in widths(&turned).into_iter().zip(widths(&level))
        {
            assert_eq!(handle, was);
            let want = turned_about(level_place);
            assert!(
                close(place.0, want.0) && close(place.1, want.1),
                "handle {handle}: {place:?} wanted {want:?}"
            );
            assert_eq!(super::text_handle_at(&turned, place), Some(handle));
        }

        let along = (angle.cos() * 20.0, angle.sin() * 20.0);
        let (dx, dy) = super::travel_along(&turned, along);
        assert!(close(dx, 20.0) && close(dy, 0.0), "{dx} {dy}");
        assert!(alike(
            resized(bounds, 5, dx, dy),
            [100.0, 100.0, 320.0, 200.0]
        ));
        let (dx, dy) = super::travel_along(&level, (20.0, -40.0));
        assert!(close(dx, 20.0) && close(dy, -40.0), "{dx} {dy}");
        let (dx, dy) = super::travel_along(&turned, (0.0, 10.0));
        assert!(close(dx, 10.0 * angle.sin()) && close(dy, 10.0 * angle.cos()));
    }

    #[test]
    fn a_handle_drag_moves_its_own_edges_and_stops_before_turning_the_frame_inside_out() {
        let ink = [100.0, 100.0, 300.0, 200.0];
        let [x0, y0, x1, y1] = frame_of(ink);

        let quad = Quad::of(ink);
        assert_eq!(handle_at(&quad, (x0 + 2.0, y0 + 2.0)), Some(0));
        assert_eq!(handle_at(&quad, (x1 - 2.0, y1 - 2.0)), Some(2));
        assert_eq!(handle_at(&quad, (f64::midpoint(x0, x1), y0)), Some(4));
        assert_eq!(
            handle_at(&quad, (f64::midpoint(x0, x1), f64::midpoint(y0, y1))),
            None
        );
        assert_eq!(handle_at(&quad, (x0 - HANDLE_REACH * 2.0, y0)), None);

        assert!(alike(
            resized(ink, 5, -40.0, 15.0),
            [100.0, 100.0, 260.0, 200.0]
        ));
        assert!(alike(
            resized(ink, 0, 10.0, 20.0),
            [110.0, 120.0, 300.0, 200.0]
        ));
        let squashed = resized(ink, 5, -1000.0, 0.0);
        assert!(
            (squashed[2] - squashed[0] - SMALLEST_FRAME).abs() < 1e-9,
            "{squashed:?}"
        );
        let squashed = resized(ink, 7, 1000.0, 0.0);
        assert!(
            (squashed[2] - squashed[0] - SMALLEST_FRAME).abs() < 1e-9,
            "{squashed:?}"
        );
    }

    #[test]
    fn a_frame_may_be_dragged_narrower_than_its_own_text() {
        let ink = [100.0, 100.0, 300.0, 200.0];
        let frame = [90.0, 90.0, 400.0, 210.0];
        let pulled = resized(frame, 5, -200.0, 0.0);
        assert!(alike(pulled, [90.0, 90.0, 200.0, 210.0]), "{pulled:?}");
        assert!(
            pulled[2] < ink[2],
            "the frame did not reach inside the text: {pulled:?}"
        );
    }

    #[test]
    fn a_frames_width_change_is_told_from_a_slide_and_from_rounding() {
        let started = [10.0, 10.0, 110.0, 60.0];
        assert!(frame_width_changed(started, [10.0, 10.0, 140.0, 60.0]));
        assert!(!frame_width_changed(started, [40.0, 10.0, 140.0, 60.0]));
        assert!(!frame_width_changed(started, [10.0, 10.0, 110.2, 60.0]));
        assert!(!frame_width_changed(started, [10.0, 10.0, 110.0, 260.0]));
    }

    #[test]
    fn the_frame_the_pointer_and_the_handles_are_one_rectangle() {
        let ink = [40.0, 60.0, 140.0, 90.0];
        let [x0, y0, x1, y1] = frame_of(ink);
        for (edge, expected) in [x0, y0, x1, y1].into_iter().zip(grow(ink, FRAME_INSET)) {
            assert!((edge - expected).abs() < 1e-9, "{edge} is not {expected}");
        }

        let just_inside = 1e-9;
        for point in [
            (x0, y0),
            (x1 - just_inside, y0),
            (x0, y1 - just_inside),
            (f64::midpoint(x0, x1), y0),
            (x0, f64::midpoint(y0, y1)),
        ] {
            assert_eq!(
                block_at(&[Quad::of(ink)], point),
                Some(0),
                "{point:?} is on the frame"
            );
        }
        assert_eq!(
            block_at(&[Quad::of(ink)], (x0 - just_inside - 1.0, y0)),
            None
        );

        for (x, y) in handles(&Quad::of(ink)) {
            assert!(
                (x - x0).abs() < 1e-9
                    || (x - x1).abs() < 1e-9
                    || (y - y0).abs() < 1e-9
                    || (y - y1).abs() < 1e-9,
                "handle ({x}, {y}) is not on the frame {:?}",
                [x0, y0, x1, y1]
            );
        }
    }

    #[test]
    fn a_corner_drag_scales_about_the_corner_opposite_it() {
        let quad = Quad::of([100.0, 100.0, 300.0, 200.0]);
        assert_eq!(super::fixed_point(&quad, 2), (100.0, 100.0));
        let (matrix, about) =
            super::shaped(&quad, 2, (200.0, 100.0), Proportions::Kept).expect("a corner drag");
        assert_eq!(about, (100.0, 100.0));
        let grown = quad.transformed(matrix, about);
        assert!(close(grown.corners[0].0, 100.0) && close(grown.corners[0].1, 100.0));
        assert!(close(grown.corners[2].0, 500.0), "{grown:?}");
        assert!(close(grown.corners[2].1, 300.0), "{grown:?}");

        let (matrix, about) =
            super::shaped(&quad, 2, (100.0, 0.0), Proportions::Kept).expect("a corner drag");
        let sideways = quad.transformed(matrix, about);
        let width = sideways.corners[1].0 - sideways.corners[0].0;
        let height = sideways.corners[3].1 - sideways.corners[0].1;
        assert!(
            (width / height - 200.0 / 100.0).abs() < 1e-9,
            "{width} by {height}"
        );
    }

    #[test]
    fn a_side_drag_changes_one_dimension_and_holds_the_other_side_still() {
        let quad = Quad::of([100.0, 100.0, 300.0, 200.0]);
        assert_eq!(super::fixed_point(&quad, 5), (100.0, 150.0));
        let (matrix, about) =
            super::shaped(&quad, 5, (200.0, 0.0), Proportions::Kept).expect("a side drag");
        let wider = quad.transformed(matrix, about);
        assert!(close(wider.corners[1].0, 500.0), "{wider:?}");
        assert!(close(wider.corners[0].0, 100.0), "{wider:?}");
        assert!(
            close(wider.corners[3].1 - wider.corners[0].1, 100.0),
            "{wider:?}"
        );
    }

    #[test]
    fn the_ninth_handle_turns_the_object_about_its_middle() {
        let quad = Quad::of([100.0, 100.0, 300.0, 200.0]);
        let centre = quad.center();
        assert_eq!(super::fixed_point(&quad, super::ROTATE_HANDLE), centre);
        let grip = super::grip(&quad, super::ROTATE_HANDLE);
        let radius = centre.1 - grip.1;
        let (matrix, about) = super::shaped(
            &quad,
            super::ROTATE_HANDLE,
            (centre.0 - radius - grip.0, centre.1 - grip.1),
            Proportions::Kept,
        )
        .expect("a turn");
        let turned = quad.transformed(matrix, about);
        let landed = super::grip(&turned, super::ROTATE_HANDLE);
        assert!(close(landed.0, centre.0 - radius), "{landed:?}");
        assert!(close(landed.1, centre.1), "{landed:?}");
        let middle = turned.center();
        assert!(
            close(middle.0, centre.0) && close(middle.1, centre.1),
            "{middle:?}"
        );
        assert!((turned.area() - quad.area()).abs() < 1e-6);
    }

    #[test]
    fn the_rotate_handle_hangs_below_the_object_however_it_is_turned() {
        let quad = Quad::of([100.0, 100.0, 300.0, 200.0]);
        let (x, y) = super::rotate_handle(&quad);
        assert!(close(x, 200.0), "{x}");
        assert!(y < 100.0, "{y}");
        assert!(
            (100.0 - FRAME_INSET - y - super::ROTATE_REACH_OUT).abs() < 1e-9,
            "{y}"
        );
        assert_eq!(super::handle_at(&quad, (x, y)), Some(super::ROTATE_HANDLE));
        assert_eq!(super::nearest_handle(&handles(&quad), (x, y)), None);
    }

    #[test]
    fn a_pictures_turning_handle_stands_off_the_same_side_as_a_blocks() {
        let block = Quad::of([100.0, 100.0, 300.0, 200.0]);
        let picture = Quad {
            corners: [
                (100.0, 200.0),
                (300.0, 200.0),
                (300.0, 100.0),
                (100.0, 100.0),
            ],
        };
        let (foot, handle) = super::rotate_stem(&picture);
        assert!(close(handle.0, 200.0), "{handle:?}");
        assert!(close(foot.1, 100.0 - FRAME_INSET), "{foot:?}");
        assert!(
            close(handle.1, 100.0 - FRAME_INSET - super::ROTATE_REACH_OUT),
            "{handle:?}"
        );
        assert_eq!(super::rotate_handle(&block), handle);
        assert_eq!(
            super::handle_at(&picture, handle),
            Some(super::ROTATE_HANDLE)
        );
        assert_eq!(super::nearest_handle(&handles(&picture), handle), None);
        let centre = picture.center();
        let turned = picture.transformed(
            pdf_paint::Matrix {
                a: 0.0,
                b: 1.0,
                c: -1.0,
                d: 0.0,
                e: 0.0,
                f: 0.0,
            },
            centre,
        );
        let moved = super::rotate_handle(&turned);
        let want = (
            centre.0 - (handle.1 - centre.1),
            centre.1 + (handle.0 - centre.0),
        );
        assert!(
            close(moved.0, want.0) && close(moved.1, want.1),
            "{moved:?}"
        );
    }

    #[test]
    fn a_side_drag_on_a_turned_object_stretches_it_without_shearing_it() {
        let angle: f64 = 30.0_f64.to_radians();
        let (along, up) = (
            (angle.cos() * 200.0, angle.sin() * 200.0),
            (-angle.sin() * 100.0, angle.cos() * 100.0),
        );
        let quad = Quad {
            corners: [
                (100.0, 100.0),
                (100.0 + along.0, 100.0 + along.1),
                (100.0 + along.0 + up.0, 100.0 + along.1 + up.1),
                (100.0 + up.0, 100.0 + up.1),
            ],
        };
        let (matrix, about) = super::shaped(
            &quad,
            5,
            (50.0 * angle.cos(), 50.0 * angle.sin()),
            Proportions::Kept,
        )
        .expect("a side drag");
        let wider = quad.transformed(matrix, about);
        for index in 0..4 {
            let a = (
                wider.corners[(index + 1) % 4].0 - wider.corners[index].0,
                wider.corners[(index + 1) % 4].1 - wider.corners[index].1,
            );
            let b = (
                wider.corners[(index + 3) % 4].0 - wider.corners[index].0,
                wider.corners[(index + 3) % 4].1 - wider.corners[index].1,
            );
            let dot = a.0.mul_add(b.0, a.1 * b.1);
            assert!(
                dot.abs() < 1e-6,
                "corner {index} is no longer square: {dot}"
            );
        }
        let length = |one: (f64, f64), other: (f64, f64)| (other.0 - one.0).hypot(other.1 - one.1);
        assert!(
            (length(wider.corners[0], wider.corners[1]) - 250.0).abs() < 1e-6,
            "{wider:?}"
        );
        assert!(
            (length(wider.corners[0], wider.corners[3]) - 100.0).abs() < 1e-6,
            "{wider:?}"
        );
    }

    fn close(had: f64, want: f64) -> bool {
        (had - want).abs() < 1e-9
    }

    #[test]
    fn sheared_handle_drag_preserves_the_other_axis_and_zero_travel_is_neutral() {
        let quad = Quad {
            corners: [(0.0, 0.0), (100.0, 0.0), (150.0, 100.0), (50.0, 100.0)],
        };
        for handle in 0..=super::ROTATE_HANDLE {
            let (matrix, about) =
                super::shaped(&quad, handle, (0.0, 0.0), Proportions::Kept).unwrap();
            for (got, expected) in quad
                .transformed(matrix, about)
                .corners
                .iter()
                .zip(quad.corners)
            {
                assert!(close(got.0, expected.0) && close(got.1, expected.1));
            }
        }
        let (matrix, about) = super::shaped(&quad, 5, (10.0, 0.0), Proportions::Kept).unwrap();
        let expected = [(0.0, 0.0), (110.0, 0.0), (160.0, 100.0), (50.0, 100.0)];
        for (got, expected) in quad.transformed(matrix, about).corners.iter().zip(expected) {
            assert!(
                close(got.0, expected.0) && close(got.1, expected.1),
                "{got:?} != {expected:?}"
            );
        }
        let (matrix, about) = super::shaped(&quad, 2, (150.0, 100.0), Proportions::Kept).unwrap();
        for (got, original) in quad
            .transformed(matrix, about)
            .corners
            .iter()
            .zip(quad.corners)
        {
            assert!(close(got.0, original.0 * 2.0) && close(got.1, original.1 * 2.0));
        }
        assert!(super::shaped(&quad, 5, (f64::NAN, 0.0), Proportions::Kept).is_none());
        assert!(super::shaped(&quad, 9, (10.0, 0.0), Proportions::Kept).is_none());
    }

    #[test]
    fn the_handles_sit_on_the_frame_a_block_is_drawn_with() {
        let bounds = [10.0, 20.0, 110.0, 60.0];
        let places = handles(&Quad::of(bounds));
        let (x0, y0) = (10.0 - FRAME_INSET, 20.0 - FRAME_INSET);
        let (x1, y1) = (110.0 + FRAME_INSET, 60.0 + FRAME_INSET);
        assert_eq!(places[0], (x0, y0), "top left");
        assert_eq!(places[2], (x1, y1), "bottom right");
        assert_eq!(places[4], (f64::midpoint(x0, x1), y0), "top middle");
        assert_eq!(places[7], (x0, f64::midpoint(y0, y1)), "left middle");
        let one_of = |value: f64, low: f64, high: f64| {
            [low, high, f64::midpoint(low, high)]
                .iter()
                .any(|candidate| (value - candidate).abs() < 1e-9)
        };
        for (x, y) in places {
            assert!(one_of(x, x0, x1), "{x}");
            assert!(one_of(y, y0, y1), "{y}");
        }
    }

    #[test]
    fn the_toolbar_goes_below_a_block_unless_there_is_no_room() {
        let view = [0.0, 0.0, 600.0, 800.0];
        let size = (60.0, 24.0);
        let (x, y) = toolbar_at([100.0, 100.0, 300.0, 140.0], size, view, 0.0);
        assert!((x - 170.0).abs() < 1e-9, "centred: {x}");
        assert!(y > 140.0, "below the block: {y}");

        let (_, y) = toolbar_at([100.0, 740.0, 300.0, 790.0], size, view, 0.0);
        assert!(y + size.1 <= 740.0, "above the block: {y}");

        let (x, _) = toolbar_at([-50.0, 100.0, 20.0, 140.0], size, view, 0.0);
        assert!(x >= 0.0, "{x}");
    }

    #[test]
    fn a_point_is_on_a_blocks_ink_only_inside_one_of_its_clusters() {
        let cluster = |line: usize, x: f64| TextClusterBox {
            anchor: String::new(),
            glyphs: 0..1,
            box_pixels: Some([x, 10.0, x + 10.0, 20.0]),
            stacked: false,
            line,
            index_in_line: 0,
            text: None,
        };
        let clusters = [cluster(0, 10.0), cluster(0, 30.0), cluster(1, 10.0)];
        assert!(on_the_ink(&clusters, &[0], (15.0, 15.0)));
        assert!(on_the_ink(&clusters, &[0], (35.0, 15.0)));
        assert!(
            !on_the_ink(&clusters, &[0], (25.0, 15.0)),
            "the gap between"
        );
        assert!(
            !on_the_ink(&clusters, &[0], (45.0, 15.0)),
            "paper past the row"
        );
        assert!(
            !on_the_ink(&clusters, &[1], (35.0, 15.0)),
            "another row's cluster"
        );
        let none: [TextClusterBox; 0] = [];
        assert!(!on_the_ink(&none, &[0], (15.0, 15.0)));
    }

    #[test]
    fn a_toolbar_above_a_block_keeps_off_the_turning_handles_stem() {
        let view = [0.0, 0.0, 600.0, 800.0];
        let size = (60.0, 24.0);
        let block = [100.0, 740.0, 300.0, 790.0];
        let (_, plain) = toolbar_at(block, size, view, 0.0);
        assert!(
            (plain + size.1 - (740.0 - FRAME_INSET * 3.0)).abs() < 1e-9,
            "{plain}"
        );
        let (_, cleared) = toolbar_at(block, size, view, 24.0);
        assert!(
            (plain - cleared - 24.0).abs() < 1e-9,
            "{plain} against {cleared}"
        );
        let low = [100.0, 100.0, 300.0, 140.0];
        assert_eq!(
            toolbar_at(low, size, view, 24.0),
            toolbar_at(low, size, view, 0.0)
        );
    }

    #[test]
    fn a_zoomed_block_keeps_its_toolbar_centred_under_it() {
        let view = [100.0, 60.0, 1100.0, 580.0];
        let block = [310.0, 210.0, 552.0, 260.0];
        let (x, y) = toolbar_at(block, (500.0, 64.0), view, 0.0);
        assert!((x - 181.0).abs() < 1e-9, "{x}");
        assert!(y > 260.0 && y + 64.0 <= 580.0, "{y}");
        let (x, _) = toolbar_at([-300.0, 300.0, 200.0, 340.0], (500.0, 64.0), view, 0.0);
        assert!((x - 100.0).abs() < 1e-9, "{x}");
        let (_, y) = toolbar_at([300.0, 0.0, 500.0, 900.0], (500.0, 64.0), view, 0.0);
        assert!(y >= 60.0 && y + 64.0 <= 580.0, "{y}");
    }

    #[test]
    fn a_selection_spans_one_row_or_none() {
        let stops = stops();
        assert_eq!(selection_between(&stops, 1, 3), Some((0, 1, 3)));
        assert_eq!(selection_between(&stops, 3, 1), Some((0, 1, 3)));
        assert_eq!(selection_between(&stops, 2, 2), None, "an empty selection");
        assert_eq!(
            selection_between(&stops, 3, 6),
            None,
            "there is no inferred reading order between two separate rows"
        );
    }

    #[test]
    fn a_selection_is_one_continuous_row_box() {
        let clusters = vec![
            TextClusterBox {
                anchor: "run".to_owned(),
                glyphs: 1..2,
                box_pixels: Some([32.0, 16.0, 47.0, 34.0]),
                stacked: false,
                line: 0,
                index_in_line: 1,
                text: Some("A".to_owned()),
            },
            TextClusterBox {
                anchor: "run".to_owned(),
                glyphs: 2..3,
                box_pixels: Some([53.0, 14.0, 66.0, 32.0]),
                stacked: true,
                line: 0,
                index_in_line: 2,
                text: None,
            },
        ];
        let band = selection_quad(&stops(), &clusters, 0, 1, 3).expect("a selection");
        assert!(alike(band.bounds(), [30.0, 14.0, 70.0, 34.0]), "{band:?}");
        assert!((band.corners[0].0 - 30.0).abs() < 1e-9, "{band:?}");
        assert!((band.corners[0].1 - 34.0).abs() < 1e-9, "{band:?}");
        assert!(band.upright());
        assert_eq!(selection_quad(&stops(), &clusters, 0, 2, 2), None);
        assert_eq!(selection_quad(&stops(), &clusters, 0, 1, 9), None);
    }

    #[test]
    fn a_selection_on_a_turned_row_is_not_a_level_rectangle() {
        let angle: f64 = 20.0_f64.to_radians();
        let stops: Vec<CaretStop> = (0..3_u32)
            .map(|offset| CaretStop {
                line: 0,
                offset: offset as usize,
                at: [
                    40.0f64.mul_add(f64::from(offset) * angle.cos(), 10.0),
                    40.0f64.mul_add(f64::from(offset) * angle.sin(), 100.0),
                ],
                up: [12.0 * angle.sin(), -12.0 * angle.cos()],
            })
            .collect();
        let band = selection_quad(&stops, &[], 0, 0, 2).expect("a selection");
        assert!(!band.upright(), "{band:?}");
        let box_of = band.bounds();
        assert!(box_of[0] <= 10.0 && box_of[2] >= 10.0 + 80.0 * angle.cos());
        let boxed = (box_of[2] - box_of[0]) * (box_of[3] - box_of[1]);
        assert!(band.area() < boxed * 0.85, "{} of {boxed}", band.area());
    }
}

#[cfg(test)]
mod block_move_tests {
    use super::{SAME_BLOCK, block_moved_to, box_shifted};

    #[test]
    fn a_block_is_found_again_where_the_move_put_it() {
        let before = [10.0, 10.0, 60.0, 30.0];
        let after = [[10.0, 40.0, 60.0, 60.0], [17.0, 5.0, 67.0, 25.0]];
        let target = box_shifted(before, 7.0, -5.0);
        assert_eq!(
            block_moved_to(&after, target),
            Some(1),
            "the block is found by where it went, not by where it was in the list"
        );
    }

    #[test]
    fn the_move_s_own_error_is_still_the_same_block() {
        let target = [10.0, 10.0, 60.0, 30.0];
        let after = [[10.001, 9.999, 60.001, 29.999]];
        assert_eq!(block_moved_to(&after, target), Some(0));
    }

    #[test]
    fn the_tolerance_stays_below_the_closest_two_blocks_the_corpus_has() {
        const CLOSEST_MEASURED: f64 = 0.2051;
        let target = [10.0, 10.0, 60.0, 30.0];
        let neighbour = [[10.0 + CLOSEST_MEASURED, 10.0, 60.0 + CLOSEST_MEASURED, 30.0]];
        assert_eq!(
            block_moved_to(&neighbour, target),
            None,
            "a block that is not there any more is not replaced by the one beside it"
        );
    }

    #[test]
    fn nothing_near_enough_leaves_nothing_selected() {
        let target = [10.0, 10.0, 60.0, 30.0];
        let far = SAME_BLOCK + 1.0;
        let after = [[10.0 + far, 10.0, 60.0 + far, 30.0]];
        assert_eq!(
            block_moved_to(&after, target),
            None,
            "a neighbour is not the block a person had selected"
        );
    }
}

#[cfg(test)]
mod scoped_caret_tests {
    use pdf_cli::CaretStop;

    use super::{Step, caret_at, caret_at_in, caret_step, caret_step_in, dashes};

    fn stops() -> Vec<CaretStop> {
        let mut stops = Vec::new();
        let mut row = |line: usize, y: f64, count: usize, x0: f64| {
            for offset in 0..count {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "a handful of stops in a fixture"
                )]
                let x = 20.0f64.mul_add(offset as f64, x0);
                stops.push(CaretStop {
                    line,
                    offset,
                    at: [x, y],
                    up: [0.0, -12.0],
                });
            }
        };
        row(0, 30.0, 3, 10.0);
        row(1, 90.0, 3, 10.0);
        row(2, 45.0, 3, 10.0);
        stops
    }

    #[test]
    fn a_click_inside_a_block_cannot_put_the_caret_in_another() {
        let stops = stops();
        let between = (10.0, 50.0);
        let unscoped = caret_at(&stops, between).expect("some stop is nearest");
        assert_eq!(
            stops[unscoped].line, 2,
            "the instrument: unscoped, this point belongs to the other block"
        );
        let scoped = caret_at_in(&stops, &[0, 1], between).expect("a stop of this block");
        assert!(
            [0, 1].contains(&stops[scoped].line),
            "scoped to the block, the caret lands on one of its own rows"
        );
    }

    #[test]
    fn a_block_with_no_rows_offers_no_caret() {
        assert_eq!(caret_at_in(&stops(), &[], (10.0, 50.0)), None);
    }

    #[test]
    fn a_step_down_goes_down_the_page_and_not_up_the_numbering() {
        let mut stops = stops();
        for stop in &mut stops {
            stop.at[1] = match stop.line {
                2 => 10.0,
                0 => 30.0,
                _ => 90.0,
            };
        }
        let middle = stops
            .iter()
            .position(|stop| stop.line == 0)
            .expect("row 0 exists");

        let up = caret_step_in(&stops, &[0, 1, 2], middle, Step::Up);
        assert_eq!(
            stops[up].line, 2,
            "up the page is row 2, though its number is larger"
        );
        assert!(
            stops[up].at[1] < stops[middle].at[1],
            "and it is drawn higher"
        );

        let down = caret_step_in(&stops, &[0, 1, 2], middle, Step::Down);
        assert_eq!(stops[down].line, 1);
        assert!(
            stops[down].at[1] > stops[middle].at[1],
            "down the page is drawn lower"
        );
    }

    #[test]
    fn a_step_crosses_a_row_the_block_does_not_own() {
        let stops = stops();
        let top = stops
            .iter()
            .position(|stop| stop.line == 0)
            .expect("row 0 exists");

        assert_eq!(
            stops[caret_step(&stops, top, Step::Down)].line,
            1,
            "the instrument: unscoped, down from row 0 lands on the row between"
        );
        let landed = caret_step_in(&stops, &[0, 2], top, Step::Down);
        assert_eq!(
            stops[landed].line, 2,
            "scoped, it skips the row the block does not own and reaches its own next row"
        );
    }

    #[test]
    fn a_step_out_of_the_block_stays_where_it_was() {
        let stops = stops();
        let last_row_of_a = stops
            .iter()
            .position(|stop| stop.line == 1)
            .expect("the fixture has a second row");

        let unscoped = caret_step(&stops, last_row_of_a, Step::Down);
        assert_eq!(
            stops[unscoped].line, 2,
            "the instrument: unscoped, down from this row leaves the block"
        );
        assert_eq!(
            caret_step_in(&stops, &[0, 1], last_row_of_a, Step::Down),
            last_row_of_a,
            "scoped, a step that would leave the block does not happen"
        );
        let inside = caret_step_in(&stops, &[0, 1], last_row_of_a, Step::Up);
        assert_ne!(inside, last_row_of_a);
        assert_eq!(stops[inside].line, 0);
    }

    fn three_rows() -> Vec<CaretStop> {
        let mut stops = Vec::new();
        let mut row = |line: usize, y: f64, xs: &[f64]| {
            for (offset, &x) in xs.iter().enumerate() {
                stops.push(CaretStop {
                    line,
                    offset,
                    at: [x, y],
                    up: [0.0, -12.0],
                });
            }
        };
        row(0, 30.0, &[10.0, 30.0, 50.0, 70.0, 90.0]);
        row(1, 60.0, &[10.0, 30.0]);
        row(2, 90.0, &[10.0, 30.0, 50.0, 70.0, 90.0]);
        stops
    }

    #[test]
    fn a_click_far_out_in_blank_space_lands_on_the_row_it_is_level_with() {
        let stops = three_rows();
        let far_right = (200.0, 54.0);
        let landed = caret_at_in(&stops, &[0, 1, 2], far_right).expect("a stop of this block");
        assert_eq!(
            stops[landed].line, 1,
            "lands on the row it is level with, not a longer neighbour"
        );
        assert_eq!(
            stops[landed].offset, 1,
            "and on that row's own last stop, the nearest one along it"
        );
    }

    #[test]
    fn control_the_old_nearest_middle_rule_answered_a_different_row() {
        let stops = three_rows();
        let far_right = (200.0, 54.0);
        let middle_of = |stop: &CaretStop| {
            (
                stop.up[0].mul_add(0.5, stop.at[0]),
                stop.up[1].mul_add(0.5, stop.at[1]),
            )
        };
        let old = stops
            .iter()
            .filter(|stop| [0, 1, 2].contains(&stop.line))
            .min_by(|a, b| {
                let da = middle_of(a);
                let db = middle_of(b);
                let dist_a = (da.0 - far_right.0).hypot(da.1 - far_right.1);
                let dist_b = (db.0 - far_right.0).hypot(db.1 - far_right.1);
                dist_a.total_cmp(&dist_b)
            })
            .expect("some stop of the block");
        assert_ne!(
            old.line, 1,
            "the instrument: the old rule does not pick the middle row for this click"
        );

        let landed = caret_at_in(&stops, &[0, 1, 2], far_right).expect("a stop of this block");
        assert_eq!(
            stops[landed].line, 1,
            "the fixed rule picks the row the click is level with, where the old one did not"
        );
    }

    #[test]
    fn a_click_outside_every_band_lands_on_the_nearest_end_row() {
        let stops = three_rows();
        let above = caret_at_in(&stops, &[0, 1, 2], (10.0, -50.0)).expect("a stop");
        assert_eq!(stops[above].line, 0, "above every row: the first");
        let below = caret_at_in(&stops, &[0, 1, 2], (10.0, 500.0)).expect("a stop");
        assert_eq!(stops[below].line, 2, "below every row: the last");
    }

    #[test]
    fn a_click_in_the_gap_between_two_rows_goes_to_the_nearer() {
        let stops = three_rows();
        let nearer_top = caret_at_in(&stops, &[0, 1, 2], (10.0, 35.0)).expect("a stop");
        assert_eq!(stops[nearer_top].line, 0, "closer to row 0's band");
        let nearer_bottom = caret_at_in(&stops, &[0, 1, 2], (10.0, 45.0)).expect("a stop");
        assert_eq!(stops[nearer_bottom].line, 1, "closer to row 1's band");
    }

    #[test]
    fn a_click_in_the_middle_of_a_rows_own_text_still_lands_where_it_always_did() {
        let stops = three_rows();
        let landed = caret_at_in(&stops, &[0, 1, 2], (52.0, 24.0)).expect("a stop");
        assert_eq!(stops[landed].line, 0);
        assert_eq!(
            stops[landed].offset, 2,
            "nearest the click along the row, same as before"
        );
    }

    #[test]
    fn a_row_set_at_an_angle_behaves_the_same_in_its_own_frame() {
        let theta = 20.0_f64.to_radians();
        let height = 12.0;
        let up = [height * theta.sin(), -height * theta.cos()];
        let along = [-up[1] / height, up[0] / height];
        let base = [100.0, 200.0];

        let mut stops = Vec::new();
        let mut push_row = |line: usize, row_base: [f64; 2], count: u32| {
            for offset in 0..count {
                let d = 20.0 * f64::from(offset);
                stops.push(CaretStop {
                    line,
                    offset: offset as usize,
                    at: [
                        d.mul_add(along[0], row_base[0]),
                        d.mul_add(along[1], row_base[1]),
                    ],
                    up,
                });
            }
        };
        push_row(0, base, 5);
        let row1_base = [
            (-30.0f64 / height).mul_add(up[0], base[0]),
            (-30.0f64 / height).mul_add(up[1], base[1]),
        ];
        push_row(1, row1_base, 2);

        let far = [
            300.0f64.mul_add(along[0], row1_base[0]),
            300.0f64.mul_add(along[1], row1_base[1]),
        ];
        let landed = caret_at_in(&stops, &[0, 1], (far[0], far[1])).expect("a stop");
        assert_eq!(
            stops[landed].line, 1,
            "lands on the angled row it is level with"
        );
        assert_eq!(stops[landed].offset, 1, "and that row's own last stop");
    }

    #[test]
    fn a_dashed_edge_starts_and_ends_on_a_dash() {
        let found = dashes(100.0);
        assert!(found.len() > 1, "a long edge is more than one dash");
        assert!(
            (found[0].0 - 0.0).abs() < 1e-9,
            "the first dash starts at the corner"
        );
        let last = *found.last().expect("at least one");
        assert!(
            (last.1 - 1.0).abs() < 1e-9,
            "the last dash ends at the next corner, not part way through a gap: {last:?}"
        );
        for pair in found.windows(2) {
            assert!(pair[0].1 < pair[1].0, "a gap between each: {pair:?}");
        }
    }

    #[test]
    fn a_short_edge_is_one_solid_segment() {
        assert_eq!(dashes(3.0), vec![(0.0, 1.0)]);
        assert_eq!(dashes(0.0), vec![(0.0, 1.0)]);
        assert_eq!(dashes(f64::NAN), vec![(0.0, 1.0)]);
    }
}

#[cfg(test)]
mod hanging_space_tests {
    use pdf_cli::{CaretStop, TextClusterBox};

    use super::shown_caret;

    fn row(text: &str, up: [f64; 2]) -> (Vec<CaretStop>, Vec<TextClusterBox>) {
        let height = up[0].hypot(up[1]);
        let along = (-up[1] / height, up[0] / height);
        let stops = (0..=text.chars().count())
            .map(|offset| {
                #[expect(clippy::cast_precision_loss, reason = "a short fixture")]
                let run = 10.0 * offset as f64;
                CaretStop {
                    line: 0,
                    offset,
                    at: [run * along.0, run * along.1],
                    up,
                }
            })
            .collect();
        let clusters = text
            .chars()
            .enumerate()
            .map(|(index, character)| TextClusterBox {
                anchor: "run".to_owned(),
                glyphs: index..index + 1,
                box_pixels: None,
                stacked: false,
                line: 0,
                index_in_line: index,
                text: Some(character.to_string()),
            })
            .collect();
        (stops, clusters)
    }

    fn x(stops: &[CaretStop], clusters: &[TextClusterBox], at: usize, frame: [f64; 4]) -> [f64; 2] {
        shown_caret(stops, clusters, at, Some(frame))
            .expect("a stop")
            .at
    }

    fn near(one: [f64; 2], other: [f64; 2]) -> bool {
        (one[0] - other[0]).abs() < 1e-9 && (one[1] - other[1]).abs() < 1e-9
    }

    #[test]
    fn a_caret_after_hanging_spaces_is_held_at_the_frame_edge() {
        let (stops, clusters) = row("ab    ", [0.0, -12.0]);
        let frame = [0.0, -20.0, 25.0, 5.0];
        assert!(
            near(x(&stops, &clusters, 6, frame), [25.0, 0.0]),
            "{:?}",
            x(&stops, &clusters, 6, frame)
        );
        assert!(
            near(x(&stops, &clusters, 4, frame), [25.0, 0.0]),
            "{:?}",
            x(&stops, &clusters, 4, frame)
        );
        assert!(
            near(x(&stops, &clusters, 2, frame), [20.0, 0.0]),
            "{:?}",
            x(&stops, &clusters, 2, frame)
        );
        assert!(
            near(x(&stops, &clusters, 1, frame), [10.0, 0.0]),
            "{:?}",
            x(&stops, &clusters, 1, frame)
        );
        let free = shown_caret(&stops, &clusters, 6, None).unwrap().at;
        assert!(near(free, [60.0, 0.0]), "{free:?}");
    }

    #[test]
    fn a_word_past_the_edge_keeps_its_caret_and_its_spaces_hang_from_it() {
        let (stops, clusters) = row("abcd  ", [0.0, -12.0]);
        let frame = [0.0, -20.0, 25.0, 5.0];
        assert!(
            near(x(&stops, &clusters, 4, frame), [40.0, 0.0]),
            "{:?}",
            x(&stops, &clusters, 4, frame)
        );
        assert!(
            near(x(&stops, &clusters, 6, frame), [40.0, 0.0]),
            "{:?}",
            x(&stops, &clusters, 6, frame)
        );
    }

    #[test]
    fn a_turned_row_is_held_along_its_own_baseline() {
        let (stops, clusters) = row("a   ", [12.0, 0.0]);
        let frame = [-5.0, 0.0, 20.0, 25.0];
        let at = x(&stops, &clusters, 4, frame);
        assert!(
            (at[0]).abs() < 1e-9 && (at[1] - 25.0).abs() < 1e-9,
            "{at:?}"
        );
    }

    #[test]
    fn an_upside_down_row_is_held_at_the_left_edge() {
        let (stops, clusters) = row("a   ", [0.0, 12.0]);
        let frame = [-25.0, -5.0, 0.0, 20.0];
        let at = x(&stops, &clusters, 4, frame);
        assert!(near(at, [-25.0, 0.0]), "{at:?}");
    }
}
