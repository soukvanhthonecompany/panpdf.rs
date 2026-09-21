use eframe::egui;

use pdf_app::view::{Quad, SWEEP_ENOUGH};
use pdf_app::wording::Message;
use pdf_edit::PenStep;

use crate::window_state::{Drag, Shape, Tool, Window};

const KAPPA: f64 = 0.552_284_749_830_793_4;

const ARROW_HEAD: f64 = 4.0;
const ARROW_SPREAD: f64 = 0.45;

impl Window {
    pub(crate) fn highlight_box(&self, drag: &Drag) -> Option<(usize, [f64; 4])> {
        let laid = self
            .laid
            .iter()
            .copied()
            .find(|laid| laid.page == drag.page)?;
        let from = laid.placed.point_in_page((drag.from.x, drag.from.y))?;
        let to = laid.placed.point_in_page((drag.to.x, drag.to.y))?;
        let travel = drag.to - drag.from;
        if f64::from(travel.x.abs().max(travel.y.abs())) < SWEEP_ENOUGH {
            return None;
        }
        let (left, right) = (from.0.min(to.0), from.0.max(to.0));
        let tall = (from.1 - to.1).abs();
        let tall = if tall < SWEEP_ENOUGH {
            self.marker.width
        } else {
            tall
        };
        (right - left >= 1.0)
            .then_some((drag.page, [left, right, f64::midpoint(from.1, to.1), tall]))
    }

    pub(crate) fn shape_steps(&self, drag: &Drag) -> Option<(usize, Vec<PenStep>)> {
        let laid = self
            .laid
            .iter()
            .copied()
            .find(|laid| laid.page == drag.page)?;
        let from = laid.placed.point_in_page((drag.from.x, drag.from.y))?;
        let to = laid.placed.point_in_page((drag.to.x, drag.to.y))?;
        let travel = drag.to - drag.from;
        if f64::from(travel.x.abs().max(travel.y.abs())) < SWEEP_ENOUGH {
            return None;
        }
        let width = self
            .editor
            .length_in_user_space(drag.page, self.pen.width)?;
        Some((
            drag.page,
            steps_of(self.shape, from, squared(from, to, drag.straight), width),
        ))
    }

    pub(crate) fn show_the_shape(&self, painter: &egui::Painter, drag: &Drag) {
        if self.tool == Tool::Highlighter {
            self.show_the_highlight(painter, drag);
            return;
        }
        let (Some((_, steps)), Some(laid)) = (
            self.shape_steps(drag),
            self.laid
                .iter()
                .copied()
                .find(|laid| laid.page == drag.page),
        ) else {
            return;
        };
        let corner = egui::pos2(laid.placed.origin.0, laid.placed.origin.1);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a page's pixels are well inside what an f32 counts exactly"
        )]
        let on_screen = |(x, y): (f64, f64)| {
            corner
                + egui::vec2(
                    (x as f32) * laid.placed.stretch,
                    (y as f32) * laid.placed.stretch,
                )
        };
        let colour = crate::draw_pen::on_screen_colour(self.pen.colour);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a pen is a few points wide"
        )]
        let stroke = egui::Stroke::new(
            ((self.pen.width as f32) * laid.placed.stretch).max(1.0),
            colour,
        );
        let flat = preview_points(self.shape, &steps);
        if self.shape_fill {
            painter.add(egui::Shape::convex_polygon(
                flat.iter().copied().map(on_screen).collect(),
                colour.gamma_multiply(0.35),
                egui::Stroke::NONE,
            ));
        }
        painter.add(egui::Shape::line(
            flat.into_iter().map(on_screen).collect(),
            stroke,
        ));
    }

    fn show_the_highlight(&self, painter: &egui::Painter, drag: &Drag) {
        let (Some((_, [left, right, middle, tall])), Some(laid)) = (
            self.highlight_box(drag),
            self.laid
                .iter()
                .copied()
                .find(|laid| laid.page == drag.page),
        ) else {
            return;
        };
        let corner = egui::pos2(laid.placed.origin.0, laid.placed.origin.1);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a page's pixels are well inside what an f32 counts exactly"
        )]
        let on_screen = |x: f64, y: f64| {
            corner
                + egui::vec2(
                    (x as f32) * laid.placed.stretch,
                    (y as f32) * laid.placed.stretch,
                )
        };
        let colour = crate::draw_pen::on_screen_colour(self.marker.colour);
        painter.rect_filled(
            egui::Rect::from_min_max(
                on_screen(left, middle - tall / 2.0),
                on_screen(right, middle + tall / 2.0),
            ),
            0.0,
            colour.gamma_multiply(0.4),
        );
    }

    fn take_the_highlight_drawn(&mut self, drag: &Drag) {
        let Some((page, [left, right, middle, tall])) = self.highlight_box(drag) else {
            return;
        };
        let pixels = [
            PenStep::Move((left, middle)),
            PenStep::Line((right, middle)),
        ];
        let Some(steps) = self.editor.steps_in_user_space(page, &pixels) else {
            return;
        };
        let Some(width) = self.editor.length_in_user_space(page, tall) else {
            return;
        };
        let job = self.editor.begin_draw_path(
            page,
            steps,
            (
                Some(pdf_edit::PenStroke {
                    width,
                    ..self.stroke_now()
                }),
                None,
            ),
            (false, pdf_app::document::Drew::Mark),
        );
        if job.is_none() {
            self.editor.say(Message::AnotherEditIsRunning);
            return;
        }
        self.reselect_object = Some((
            page,
            Quad::of([left, middle - tall / 2.0, right, middle + tall / 2.0]),
        ));
        self.send(job);
    }

    pub(crate) fn take_the_shape_drawn(&mut self, drag: &Drag) {
        if self.tool == Tool::Highlighter {
            self.take_the_highlight_drawn(drag);
            return;
        }
        let Some((page, pixels)) = self.shape_steps(drag) else {
            return;
        };
        let Some(steps) = self.editor.steps_in_user_space(page, &pixels) else {
            return;
        };
        let Some(width) = self.editor.length_in_user_space(page, self.pen.width) else {
            return;
        };
        let closed = self.shape.is_closed();
        let job = self.editor.begin_draw_path(
            page,
            steps,
            (
                Some(pdf_edit::PenStroke::pen(self.pen.colour, width)),
                (self.shape_fill && closed).then_some(self.pen.colour),
            ),
            (closed, pdf_app::document::Drew::Shape),
        );
        if job.is_none() {
            self.editor.say(Message::AnotherEditIsRunning);
            return;
        }
        let bounds = pixels.iter().flat_map(PenStep::points).fold(
            [
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            ],
            |[x0, y0, x1, y1], (x, y)| [x0.min(x), y0.min(y), x1.max(x), y1.max(y)],
        );
        self.reselect_object = Some((page, Quad::of(bounds)));
        self.send(job);
    }

    pub(crate) fn shape_choices(&mut self, ui: &mut egui::Ui) {
        for shape in Shape::ALL {
            let chosen = self.shape == shape;
            let response = crate::draw_pen::swatch_box(ui, chosen, 26.0);
            let colour = if chosen {
                ui.visuals().selection.stroke.color
            } else {
                ui.visuals().text_color()
            };
            crate::icons::Icon::from(shape).draw(ui.painter(), response.rect.shrink(5.0), colour);
            if response.clicked() {
                self.shape = shape;
            }
        }
        ui.add_space(6.0);
        let _ = ui.checkbox(&mut self.shape_fill, Message::FillTheShape.say(self.lang));
    }
}

fn squared(from: (f64, f64), to: (f64, f64), straight: bool) -> (f64, f64) {
    if !straight {
        return to;
    }
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let side = dx.abs().max(dy.abs());
    (from.0 + side.copysign(dx), from.1 + side.copysign(dy))
}

fn steps_of(shape: Shape, from: (f64, f64), to: (f64, f64), width: f64) -> Vec<PenStep> {
    let box_of = [
        from.0.min(to.0),
        from.1.min(to.1),
        from.0.max(to.0),
        from.1.max(to.1),
    ];
    match shape {
        Shape::Line => vec![PenStep::Move(from), PenStep::Line(to)],
        Shape::Arrow => arrow(from, to, width),
        Shape::Rectangle => vec![
            PenStep::Move((box_of[0], box_of[1])),
            PenStep::Line((box_of[2], box_of[1])),
            PenStep::Line((box_of[2], box_of[3])),
            PenStep::Line((box_of[0], box_of[3])),
        ],
        Shape::Ellipse => ellipse(box_of),
    }
}

fn arrow(from: (f64, f64), to: (f64, f64), width: f64) -> Vec<PenStep> {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let length = dx.hypot(dy);
    let mut steps = vec![PenStep::Move(from), PenStep::Line(to)];
    if length <= f64::EPSILON {
        return steps;
    }
    let head = (width * ARROW_HEAD).min(length / 2.0).max(1.0);
    let (back, side) = ((-dx / length, -dy / length), (-dy / length, dx / length));
    for hand in [1.0, -1.0] {
        steps.push(PenStep::Move(to));
        steps.push(PenStep::Line((
            (head * back.0).mul_add(1.0, to.0) + hand * head * ARROW_SPREAD * side.0,
            (head * back.1).mul_add(1.0, to.1) + hand * head * ARROW_SPREAD * side.1,
        )));
    }
    steps
}

fn ellipse([x0, y0, x1, y1]: [f64; 4]) -> Vec<PenStep> {
    let (cx, cy) = (f64::midpoint(x0, x1), f64::midpoint(y0, y1));
    let (rx, ry) = ((x1 - x0) / 2.0, (y1 - y0) / 2.0);
    let (kx, ky) = (rx * KAPPA, ry * KAPPA);
    vec![
        PenStep::Move((cx, y0)),
        PenStep::Curve((cx + kx, y0), (x1, cy - ky), (x1, cy)),
        PenStep::Curve((x1, cy + ky), (cx + kx, y1), (cx, y1)),
        PenStep::Curve((cx - kx, y1), (x0, cy + ky), (x0, cy)),
        PenStep::Curve((x0, cy - ky), (cx - kx, y0), (cx, y0)),
    ]
}

fn preview_points(shape: Shape, steps: &[PenStep]) -> Vec<(f64, f64)> {
    let mut points = flattened(steps);
    if shape.is_closed()
        && let Some(first) = points.first().copied()
        && points.last() != Some(&first)
    {
        points.push(first);
    }
    points
}

fn flattened(steps: &[PenStep]) -> Vec<(f64, f64)> {
    let mut points = Vec::new();
    let mut at = (0.0, 0.0);
    for step in steps {
        match step {
            PenStep::Move(point) | PenStep::Line(point) => {
                points.push(*point);
                at = *point;
            }
            PenStep::Curve(one, other, end) => {
                for piece in 1..=12 {
                    let t = f64::from(piece) / 12.0;
                    points.push(along(at, *one, *other, *end, t));
                }
                at = *end;
            }
        }
    }
    points
}

fn along(
    from: (f64, f64),
    one: (f64, f64),
    other: (f64, f64),
    to: (f64, f64),
    along: f64,
) -> (f64, f64) {
    let left = 1.0 - along;
    let shares = [
        left * left * left,
        3.0 * left * left * along,
        3.0 * left * along * along,
        along * along * along,
    ];
    let at = |from: f64, one: f64, other: f64, to: f64| {
        shares[0].mul_add(
            from,
            shares[1].mul_add(one, shares[2].mul_add(other, shares[3] * to)),
        )
    };
    (
        at(from.0, one.0, other.0, to.0),
        at(from.1, one.1, other.1, to.1),
    )
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    reason = "these shapes are whole pixels, which arrive exactly"
)]
mod tests {
    use pdf_edit::PenStep;

    use super::{along, ellipse, preview_points, squared, steps_of};
    use crate::window_state::Shape;

    #[test]
    fn a_rectangle_is_its_four_corners_once_each() {
        let steps = steps_of(Shape::Rectangle, (10.0, 20.0), (50.0, 60.0), 1.0);
        assert_eq!(
            steps,
            [
                PenStep::Move((10.0, 20.0)),
                PenStep::Line((50.0, 20.0)),
                PenStep::Line((50.0, 60.0)),
                PenStep::Line((10.0, 60.0)),
            ]
        );
    }

    #[test]
    fn a_closed_shape_is_closed_while_it_is_being_dragged() {
        for shape in [Shape::Rectangle, Shape::Ellipse] {
            let steps = steps_of(shape, (10.0, 20.0), (50.0, 60.0), 1.0);
            let points = preview_points(shape, &steps);
            assert_eq!(
                points.first(),
                points.last(),
                "{shape:?} closes: {points:?}"
            );
            for corner in [(10.0, 20.0), (50.0, 20.0), (50.0, 60.0), (10.0, 60.0)] {
                if shape == Shape::Rectangle {
                    assert!(points.contains(&corner), "{corner:?} in {points:?}");
                }
            }
        }
        let line = steps_of(Shape::Line, (10.0, 20.0), (50.0, 60.0), 1.0);
        let points = preview_points(Shape::Line, &line);
        assert_eq!(points, [(10.0, 20.0), (50.0, 60.0)]);
    }

    #[test]
    fn a_rectangle_dragged_backwards_is_the_same_rectangle() {
        assert_eq!(
            steps_of(Shape::Rectangle, (50.0, 60.0), (10.0, 20.0), 1.0),
            steps_of(Shape::Rectangle, (10.0, 20.0), (50.0, 60.0), 1.0)
        );
    }

    #[test]
    fn shift_squares_the_box_in_the_direction_it_was_dragged() {
        assert_eq!(squared((0.0, 0.0), (40.0, 10.0), true), (40.0, 40.0));
        assert_eq!(squared((0.0, 0.0), (-10.0, -40.0), true), (-40.0, -40.0));
        assert_eq!(squared((0.0, 0.0), (40.0, 10.0), false), (40.0, 10.0));
    }

    #[test]
    fn an_ellipse_is_round() {
        let steps = ellipse([0.0, 0.0, 100.0, 60.0]);
        let mut at = match steps.first() {
            Some(PenStep::Move(point)) => *point,
            _ => panic!("an ellipse starts where the pen goes down"),
        };
        assert_eq!(at, (50.0, 0.0), "at the top, going clockwise");
        for step in &steps[1..] {
            let PenStep::Curve(one, other, end) = step else {
                panic!("an ellipse is curves: {step:?}");
            };
            for piece in 0..=20 {
                let t = f64::from(piece) / 20.0;
                let (x, y) = along(at, *one, *other, *end, t);
                let on = ((x - 50.0) / 50.0).hypot((y - 30.0) / 30.0);
                assert!((on - 1.0).abs() < 1e-3, "{x},{y} is {on} of the way out");
            }
            at = *end;
        }
    }

    #[test]
    fn an_arrow_is_a_shaft_and_two_barbs_at_the_end_it_points_to() {
        let steps = steps_of(Shape::Arrow, (0.0, 0.0), (100.0, 0.0), 2.0);
        assert_eq!(steps.len(), 6, "a shaft and two barbs: {steps:?}");
        assert_eq!(steps[0], PenStep::Move((0.0, 0.0)));
        assert_eq!(steps[1], PenStep::Line((100.0, 0.0)));
        for barb in [steps[3], steps[5]] {
            let PenStep::Line((x, y)) = barb else {
                panic!("a barb is a line: {barb:?}");
            };
            assert!(x < 100.0, "it points back down the shaft");
            assert!(y.abs() > 0.0, "and out to one side");
        }
        let (PenStep::Line((_, one)), PenStep::Line((_, other))) = (steps[3], steps[5]) else {
            panic!("barbs");
        };
        assert_eq!(one, -other);
    }
}
