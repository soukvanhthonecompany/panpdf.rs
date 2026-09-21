use eframe::egui;

use pdf_app::view::Quad;
use pdf_app::wording::Message;

use crate::window_state::{Drag, Ink, Pen, Tool, Window};

const FAR_ENOUGH: f64 = 1.5;

const STRAIGHT_ENOUGH: f64 = 0.75;

pub(crate) const MARKER_COLOURS: [[f64; 3]; 5] = [
    [1.0, 0.92, 0.23],
    [0.55, 0.90, 0.30],
    [0.40, 0.80, 0.95],
    [1.0, 0.55, 0.75],
    [1.0, 0.65, 0.25],
];

pub(crate) const MARKER_HEIGHT: f64 = 14.0;

pub(crate) const MARKER_OPACITY: f64 = 0.4;

pub(crate) const PEN_COLOURS: [[f64; 3]; 6] = [
    [0.85, 0.11, 0.11],
    [0.95, 0.60, 0.07],
    [0.13, 0.60, 0.20],
    [0.11, 0.35, 0.85],
    [0.55, 0.20, 0.75],
    [0.0, 0.0, 0.0],
];

pub(crate) const PEN_WIDTHS: [f64; 3] = [1.0, 2.5, 6.0];

impl Window {
    pub(crate) fn take_up(&mut self, tool: Tool) {
        self.pictures.clear();
        self.ink = None;
        self.tool = tool;
        self.editor.say(match tool {
            Tool::Highlighter => Message::DragToMarkThePage,
            _ => Message::DragToDrawALine,
        });
    }

    pub(crate) fn stroke_now(&self) -> pdf_edit::PenStroke {
        if self.tool == Tool::Highlighter {
            pdf_edit::PenStroke {
                colour: self.marker.colour,
                width: self.marker.width,
                opacity: MARKER_OPACITY,
                blend: pdf_edit::PenBlend::Multiply,
                round_ends: false,
            }
        } else {
            pdf_edit::PenStroke::pen(self.pen.colour, self.pen.width)
        }
    }

    fn choices_now(&self) -> (&'static [[f64; 3]], &'static [f64], Pen) {
        if self.tool == Tool::Highlighter {
            (&MARKER_COLOURS, &[], self.marker)
        } else {
            (&PEN_COLOURS, &PEN_WIDTHS, self.pen)
        }
    }

    fn hold(&mut self, chosen: Pen) {
        if self.tool == Tool::Highlighter {
            self.marker = chosen;
        } else {
            self.pen = chosen;
        }
    }

    pub(crate) fn start_the_ink(&mut self, drag: &Drag) {
        self.ink = self.page_ink(drag.page, drag.to).map(|point| Ink {
            page: drag.page,
            points: vec![point],
            drawn_to: 1,
        });
    }

    pub(crate) fn carry_the_ink(&mut self, page: usize, to: egui::Pos2, straight: bool) {
        let Some(point) = self.page_ink(page, to) else {
            return;
        };
        let Some(ink) = self.ink.as_mut() else {
            return;
        };
        if ink.page != page {
            return;
        }
        if straight {
            ink.points.truncate(ink.drawn_to.max(1));
            ink.points.push(point);
            return;
        }
        ink.drawn_to = ink.points.len();
        let far = ink
            .points
            .last()
            .is_none_or(|last| (last.0 - point.0).hypot(last.1 - point.1) >= FAR_ENOUGH);
        if far {
            ink.points.push(point);
            ink.drawn_to = ink.points.len();
        }
    }

    fn page_ink(&self, page: usize, at: egui::Pos2) -> Option<(f64, f64)> {
        let laid = self.laid.iter().copied().find(|laid| laid.page == page)?;
        laid.placed.point_in_page((at.x, at.y))
    }

    pub(crate) fn pen_choices(&mut self, ui: &mut egui::Ui) {
        let (colours, widths, held) = self.choices_now();
        for colour in colours.iter().copied() {
            let chosen = held
                .colour
                .iter()
                .zip(colour)
                .all(|(one, other)| (one - other).abs() < f64::EPSILON);
            if swatch(ui, on_screen_colour(colour), chosen, 18.0).clicked() {
                self.hold(Pen { colour, ..held });
            }
        }
        let own = !colours.iter().any(|colour| {
            held.colour
                .iter()
                .zip(colour)
                .all(|(one, other)| (one - other).abs() < f64::EPSILON)
        });
        let well = swatch(ui, egui::Color32::TRANSPARENT, own, 18.0);
        if own {
            ui.painter()
                .rect_filled(well.rect.shrink(2.0), 3.0, on_screen_colour(held.colour));
        } else {
            crate::icons::Icon::CustomColour.draw_tinted(
                ui.painter(),
                well.rect,
                ui.visuals().text_color(),
                true,
            );
        }
        let well = well.on_hover_text(
            Message::Control(pdf_app::wording::Control::CustomColour).say(self.lang),
        );
        let now = crate::palette::bytes_of(held.colour);
        if let Some(colour) = crate::palette::popup(&well, Some(now), &mut self.colours, self.lang)
        {
            self.hold(Pen {
                colour: crate::palette::channels_of(colour),
                ..held
            });
        }
        ui.add_space(6.0);
        for width in widths.iter().copied() {
            let chosen = (held.width - width).abs() < f64::EPSILON;
            #[expect(
                clippy::cast_possible_truncation,
                reason = "a pen is a few points wide"
            )]
            let thickness = (width as f32).min(10.0);
            let colour = on_screen_colour(held.colour);
            let response = swatch(ui, egui::Color32::TRANSPARENT, chosen, 26.0);
            let middle = response.rect.center();
            ui.painter().line_segment(
                [
                    egui::pos2(middle.x - 7.0, middle.y),
                    egui::pos2(middle.x + 7.0, middle.y),
                ],
                egui::Stroke::new(thickness.max(1.0), colour),
            );
            if response.clicked() {
                self.hold(Pen { width, ..held });
            }
        }
    }

    pub(crate) fn show_the_ink(&self, painter: &egui::Painter, drag: &Drag) {
        let (Some(ink), Some(laid)) = (
            self.ink.as_ref(),
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
        let points: Vec<egui::Pos2> = ink
            .points
            .iter()
            .map(|(x, y)| {
                corner
                    + egui::vec2(
                        (*x as f32) * laid.placed.stretch,
                        (*y as f32) * laid.placed.stretch,
                    )
            })
            .collect();
        let stroke = self.stroke_now();
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "how see-through a stroke is runs from zero to one"
        )]
        let colour =
            on_screen_colour(stroke.colour).gamma_multiply_u8((stroke.opacity * 255.0) as u8);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a pen is a few points wide"
        )]
        let width = (stroke.width as f32) * laid.placed.stretch;
        painter.add(egui::Shape::line(
            points,
            egui::Stroke::new(width.max(1.0), colour),
        ));
    }

    pub(crate) fn take_the_ink_drawn(&mut self) {
        let Some(ink) = self.ink.take() else {
            return;
        };
        let kept = simplified(&ink.points, STRAIGHT_ENOUGH);
        if kept.len() < 2 {
            return;
        }
        let drawn: Vec<pdf_edit::PenStep> = kept
            .iter()
            .enumerate()
            .map(|(at, point)| {
                if at == 0 {
                    pdf_edit::PenStep::Move(*point)
                } else {
                    pdf_edit::PenStep::Line(*point)
                }
            })
            .collect();
        let Some(steps) = self.editor.steps_in_user_space(ink.page, &drawn) else {
            return;
        };
        let drawing = self.stroke_now();
        let Some(width) = self.editor.length_in_user_space(ink.page, drawing.width) else {
            return;
        };
        let drew = if self.tool == Tool::Highlighter {
            pdf_app::document::Drew::Mark
        } else {
            pdf_app::document::Drew::Line
        };
        let job = self.editor.begin_draw_path(
            ink.page,
            steps,
            (Some(pdf_edit::PenStroke { width, ..drawing }), None),
            (false, drew),
        );
        if job.is_none() {
            self.editor.say(Message::AnotherEditIsRunning);
            return;
        }
        let bounds = kept.iter().fold(
            [
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            ],
            |[x0, y0, x1, y1], (x, y)| [x0.min(*x), y0.min(*y), x1.max(*x), y1.max(*y)],
        );
        self.reselect_object = Some((ink.page, Quad::of(bounds)));
        self.send(job);
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a colour is between zero and one"
)]
pub(crate) fn on_screen_colour([red, green, blue]: [f64; 3]) -> egui::Color32 {
    egui::Color32::from_rgb(
        (red * 255.0) as u8,
        (green * 255.0) as u8,
        (blue * 255.0) as u8,
    )
}

pub(crate) fn swatch_box(ui: &mut egui::Ui, chosen: bool, side: f32) -> egui::Response {
    swatch(ui, egui::Color32::TRANSPARENT, chosen, side)
}

fn swatch(ui: &mut egui::Ui, fill: egui::Color32, chosen: bool, side: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let box_of = rect.shrink(2.0);
        ui.painter().rect_filled(box_of, 3.0, fill);
        if chosen || response.hovered() {
            let colour = if chosen {
                ui.visuals().selection.stroke.color
            } else {
                ui.visuals().widgets.hovered.fg_stroke.color
            };
            ui.painter().rect_stroke(
                rect,
                4.0,
                egui::Stroke::new(if chosen { 2.0 } else { 1.0 }, colour),
                egui::StrokeKind::Inside,
            );
        }
    }
    response
}

fn simplified(points: &[(f64, f64)], slack: f64) -> Vec<(f64, f64)> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let (first, last) = (points[0], points[points.len() - 1]);
    let (mut furthest, mut at) = (0.0, 0);
    for (index, point) in points.iter().enumerate().take(points.len() - 1).skip(1) {
        let away = off_the_line(*point, first, last);
        if away > furthest {
            furthest = away;
            at = index;
        }
    }
    if furthest <= slack {
        return vec![first, last];
    }
    let mut kept = simplified(&points[..=at], slack);
    kept.pop();
    kept.extend(simplified(&points[at..], slack));
    kept
}

fn off_the_line((x, y): (f64, f64), (x0, y0): (f64, f64), (x1, y1): (f64, f64)) -> f64 {
    let (dx, dy) = (x1 - x0, y1 - y0);
    let length = dx.hypot(dy);
    if length <= f64::EPSILON {
        return (x - x0).hypot(y - y0);
    }
    (dy.mul_add(x - x0, -(dx * (y - y0)))).abs() / length
}

impl Pen {
    pub(crate) const fn marker() -> Self {
        Self {
            colour: MARKER_COLOURS[0],
            width: MARKER_HEIGHT,
        }
    }
}

impl Default for Pen {
    fn default() -> Self {
        Self {
            colour: PEN_COLOURS[0],
            width: PEN_WIDTHS[1],
        }
    }
}

pub(crate) fn pen_pointer(painter: &egui::Painter, tip: egui::Pos2, ink: egui::Color32) {
    let along = egui::vec2(1.0, -1.0) / std::f32::consts::SQRT_2;
    let across = egui::vec2(1.0, 1.0) / std::f32::consts::SQRT_2;
    let (cone, body, half) = (7.0, 22.0, 3.2);
    let point = |forward: f32, side: f32| tip + along * forward + across * side;
    let edge = egui::Stroke::new(1.2, egui::Color32::from_gray(30));
    painter.add(egui::Shape::convex_polygon(
        vec![
            point(0.0, 0.0),
            point(cone, half),
            point(body, half),
            point(body, -half),
            point(cone, -half),
        ],
        egui::Color32::WHITE,
        edge,
    ));
    painter.add(egui::Shape::convex_polygon(
        vec![
            point(0.0, 0.0),
            point(cone * 0.55, half * 0.55),
            point(cone * 0.55, -half * 0.55),
        ],
        ink,
        egui::Stroke::NONE,
    ));
    painter.line_segment([point(cone, half), point(cone, -half)], edge);
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    reason = "these strokes are whole pixels, which arrive exactly"
)]
mod tests {
    use super::{off_the_line, simplified};

    #[test]
    fn a_straight_stroke_is_kept_as_its_two_ends() {
        let straight: Vec<(f64, f64)> = (0..20).map(|step| (f64::from(step), 0.0)).collect();
        assert_eq!(simplified(&straight, 0.75), [(0.0, 0.0), (19.0, 0.0)]);
    }

    #[test]
    fn a_corner_is_kept_and_the_wobble_along_the_way_is_not() {
        let stroke = [
            (0.0, 0.0),
            (5.0, 0.4),
            (10.0, 0.0),
            (15.0, 0.3),
            (20.0, 0.0),
            (20.0, 10.0),
            (20.0, 20.0),
        ];
        assert_eq!(
            simplified(&stroke, 0.75),
            [(0.0, 0.0), (20.0, 0.0), (20.0, 20.0)]
        );
    }

    #[test]
    fn a_curve_keeps_enough_points_to_stay_a_curve() {
        let curve: Vec<(f64, f64)> = (0..=30)
            .map(|step| {
                let angle = f64::from(step) * std::f64::consts::FRAC_PI_2 / 30.0;
                (50.0 * angle.cos(), 50.0 * angle.sin())
            })
            .collect();
        let kept = simplified(&curve, 0.75);
        assert!(
            (5..=12).contains(&kept.len()),
            "a quarter circle is a handful of points, not {}",
            kept.len()
        );
        for point in &curve {
            let near = kept
                .windows(2)
                .map(|pair| off_the_line(*point, pair[0], pair[1]))
                .fold(f64::INFINITY, f64::min);
            assert!(near <= 0.75 + 1e-9, "{point:?} is {near} away");
        }
    }

    #[test]
    fn two_points_are_left_alone() {
        let stroke = [(1.0, 2.0), (3.0, 4.0)];
        assert_eq!(simplified(&stroke, 0.75), stroke);
    }
}
