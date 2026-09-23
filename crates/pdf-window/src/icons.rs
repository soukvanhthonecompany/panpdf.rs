use eframe::egui;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Icon {
    Open,
    Save,
    Undo,
    Redo,
    Select,
    Text,
    Pen,
    Highlighter,
    Shape,
    Line,
    Arrow,
    Rectangle,
    Ellipse,
    Form,
    Link,
    Picture,
    Delete,
    Previous,
    Next,
    ZoomIn,
    ZoomOut,
    NewDocument,
    Theme,
    Home,
    Document,
    Folder,
    Bold,
    Italic,
    Underline,
    ClearFormatting,
    LineSpacing,
    AlignStart,
    AlignCentre,
    AlignEnd,
    AlignJustify,
    FlowRound,
    Minus,
    Plus,
    CustomColour,
    TextField,
    ParagraphField,
    Checkbox,
    RadioButton,
    Dropdown,
    ListBox,
    DateField,
    SignatureField,
    PushButton,
    Arrange,
    FitToPaper,
    BringToFront,
    BringForward,
    SendBackward,
    SendToBack,
    LinkAddresses,
    NamedPlaces,
    Unlink,
    Settings,
    RotateLeft,
    RotateRight,
    More,
    #[cfg_attr(
        target_arch = "wasm32",
        expect(dead_code, reason = "the assistant panel is not built for the browser")
    )]
    Copy,
    #[cfg_attr(
        target_arch = "wasm32",
        expect(dead_code, reason = "the assistant panel is not built for the browser")
    )]
    Edit,
    #[cfg_attr(
        target_arch = "wasm32",
        expect(dead_code, reason = "the assistant panel is not built for the browser")
    )]
    AskAgain,
    #[cfg_attr(
        target_arch = "wasm32",
        expect(dead_code, reason = "the assistant panel is not built for the browser")
    )]
    Send,
    #[cfg_attr(
        target_arch = "wasm32",
        expect(dead_code, reason = "the assistant panel is not built for the browser")
    )]
    Stop,
    #[cfg_attr(
        target_arch = "wasm32",
        expect(
            dead_code,
            reason = "the assistant panel that closes is not built for the browser"
        )
    )]
    Close,
}

const WEIGHT: f32 = 0.085;

const INSET: f32 = 0.08;

const INK: [u8; 3] = [37, 99, 235];
const BAND: [u8; 3] = [250, 204, 21];
const TICK: [u8; 3] = [22, 163, 74];
const SUN: [u8; 3] = [245, 158, 11];
const HILL: [u8; 3] = [34, 150, 90];
const CHAIN: [u8; 3] = [14, 116, 196];
const TAKE_AWAY: [u8; 3] = [220, 38, 38];
const SHAPE_FILL: [u8; 3] = [249, 115, 22];
const STACK: [u8; 3] = [124, 58, 237];

const HEAVIER: f32 = 1.3;
const BOLD_WEIGHT: f32 = 1.7;

impl Icon {
    pub(crate) fn draw(self, painter: &egui::Painter, rect: egui::Rect, colour: egui::Color32) {
        self.draw_tinted(painter, rect, colour, false);
    }

    #[expect(
        clippy::too_many_lines,
        reason = "a table of drawings, one arm per icon"
    )]
    pub(crate) fn draw_tinted(
        self,
        painter: &egui::Painter,
        rect: egui::Rect,
        colour: egui::Color32,
        tinted: bool,
    ) {
        let side = rect.width().min(rect.height());
        let box_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(side, side));
        let pen = Pen {
            painter,
            rect: box_rect.shrink(side * INSET),
            stroke: egui::Stroke::new((side * WEIGHT).max(1.0), colour),
            colour,
            tinted,
        };
        match self {
            Self::Open => open(&pen),
            Self::Save => save(&pen),
            Self::Undo => turn(&pen, false),
            Self::Redo => turn(&pen, true),
            Self::Select => select(&pen),
            Self::Text => text(&pen),
            Self::Pen => nib(&pen),
            Self::Highlighter => marker(&pen),
            Self::Shape => shape(&pen),
            Self::Line => pen.line(&[(0.14, 0.82), (0.86, 0.18)]),
            Self::Arrow => arrow(&pen),
            Self::Rectangle => pen.frame((0.12, 0.22), (0.88, 0.78), 0.02),
            Self::Ellipse => pen.ring((0.5, 0.5), 0.36),
            Self::Form => form(&pen),
            Self::Link => chain(&pen),
            Self::Picture => picture(&pen),
            Self::Delete => delete(&pen),
            Self::Previous => chevron(&pen, false),
            Self::Next => chevron(&pen, true),
            Self::ZoomIn => magnifier(&pen, Some(true)),
            Self::ZoomOut => magnifier(&pen, Some(false)),
            Self::NewDocument => new_document(&pen),
            Self::Theme => theme(&pen),
            Self::Home => home(&pen),
            Self::Document => document(&pen),
            Self::Folder => folder(&pen),
            Self::Bold => bold(&pen),
            Self::Italic => italic(&pen),
            Self::Underline => underline(&pen),
            Self::ClearFormatting => clear_formatting(&pen),
            Self::LineSpacing => line_spacing(&pen),
            Self::AlignStart => aligned(&pen, Alignment::Start),
            Self::AlignCentre => aligned(&pen, Alignment::Centre),
            Self::AlignEnd => aligned(&pen, Alignment::End),
            Self::AlignJustify => aligned(&pen, Alignment::Justify),
            Self::FlowRound => flow_round(&pen),
            Self::Minus => pen.line(&[(0.22, 0.5), (0.78, 0.5)]),
            Self::Plus => {
                pen.line(&[(0.22, 0.5), (0.78, 0.5)]);
                pen.line(&[(0.5, 0.22), (0.5, 0.78)]);
            }
            Self::Close => {
                pen.line(&[(0.26, 0.26), (0.74, 0.74)]);
                pen.line(&[(0.74, 0.26), (0.26, 0.74)]);
            }
            Self::CustomColour => custom_colour(&pen),
            Self::TextField => text_field(&pen),
            Self::ParagraphField => paragraph_field(&pen),
            Self::Checkbox => checkbox(&pen),
            Self::RadioButton => radio_button(&pen),
            Self::Dropdown => dropdown(&pen),
            Self::ListBox => list_box(&pen),
            Self::DateField => date_field(&pen),
            Self::SignatureField => signature_field(&pen),
            Self::PushButton => push_button(&pen),
            Self::Arrange => arrange(&pen),
            Self::FitToPaper => fit_to_paper(&pen),
            Self::BringToFront => ordering(&pen, true, true),
            Self::BringForward => ordering(&pen, true, false),
            Self::SendBackward => ordering(&pen, false, false),
            Self::SendToBack => ordering(&pen, false, true),
            Self::LinkAddresses => link_addresses(&pen),
            Self::NamedPlaces => named_places(&pen),
            Self::Unlink => unlink(&pen),
            Self::Settings => settings(&pen),
            Self::RotateLeft => rotate(&pen, false),
            Self::RotateRight => rotate(&pen, true),
            Self::More => {
                for x in [0.2, 0.5, 0.8] {
                    pen.dot((x, 0.5), 0.09);
                }
            }
            Self::Copy => {
                pen.line(&[
                    (0.34, 0.30),
                    (0.34, 0.10),
                    (0.90, 0.10),
                    (0.90, 0.66),
                    (0.70, 0.66),
                ]);
                pen.frame((0.10, 0.34), (0.66, 0.90), 0.06);
            }
            Self::Edit => {
                pen.line(&[
                    (0.14, 0.86),
                    (0.18, 0.66),
                    (0.68, 0.16),
                    (0.84, 0.32),
                    (0.34, 0.82),
                    (0.14, 0.86),
                ]);
                pen.line(&[(0.58, 0.26), (0.74, 0.42)]);
            }
            Self::AskAgain => again(&pen),
            Self::Send => {
                pen.line(&[(0.5, 0.86), (0.5, 0.16)]);
                pen.line(&[(0.22, 0.42), (0.5, 0.14), (0.78, 0.42)]);
            }
            Self::Stop => pen.block((0.24, 0.24), (0.76, 0.76), 0.08),
        }
    }
}

struct Pen<'a> {
    painter: &'a egui::Painter,
    rect: egui::Rect,
    stroke: egui::Stroke,
    colour: egui::Color32,
    tinted: bool,
}

impl Pen<'_> {
    fn at(&self, x: f32, y: f32) -> egui::Pos2 {
        egui::pos2(
            self.rect.left() + self.rect.width() * x,
            self.rect.top() + self.rect.height() * y,
        )
    }

    fn accent(&self, rgb: [u8; 3]) -> egui::Color32 {
        if self.tinted {
            let [r, g, b] = rgb;
            egui::Color32::from_rgb(r, g, b)
        } else {
            self.colour
        }
    }

    fn in_accent(&self, rgb: [u8; 3]) -> Self {
        let colour = self.accent(rgb);
        Pen {
            painter: self.painter,
            rect: self.rect,
            stroke: egui::Stroke::new(self.stroke.width, colour),
            colour,
            tinted: self.tinted,
        }
    }

    fn weak(&self) -> Self {
        let colour = self.colour.gamma_multiply(0.38);
        Pen {
            painter: self.painter,
            rect: self.rect,
            stroke: egui::Stroke::new(self.stroke.width, colour),
            colour,
            tinted: self.tinted,
        }
    }

    fn heavier(&self, times: f32) -> Self {
        Pen {
            painter: self.painter,
            rect: self.rect,
            stroke: egui::Stroke::new(self.stroke.width * times, self.colour),
            colour: self.colour,
            tinted: self.tinted,
        }
    }

    fn line(&self, points: &[(f32, f32)]) {
        let points: Vec<egui::Pos2> = points.iter().map(|(x, y)| self.at(*x, *y)).collect();
        self.painter.add(egui::Shape::line(points, self.stroke));
    }

    fn frame(&self, from: (f32, f32), to: (f32, f32), rounding: f32) {
        let rect = egui::Rect::from_two_pos(self.at(from.0, from.1), self.at(to.0, to.1));
        self.painter.rect_stroke(
            rect,
            self.rect.width() * rounding,
            self.stroke,
            egui::StrokeKind::Middle,
        );
    }

    fn block(&self, from: (f32, f32), to: (f32, f32), rounding: f32) {
        let rect = egui::Rect::from_two_pos(self.at(from.0, from.1), self.at(to.0, to.1));
        self.painter
            .rect_filled(rect, self.rect.width() * rounding, self.colour);
    }

    fn fill(&self, points: &[(f32, f32)]) {
        let points: Vec<egui::Pos2> = points.iter().map(|(x, y)| self.at(*x, *y)).collect();
        self.painter.add(egui::Shape::convex_polygon(
            points,
            self.colour,
            self.stroke,
        ));
    }

    fn ring(&self, centre: (f32, f32), radius: f32) {
        self.painter.circle_stroke(
            self.at(centre.0, centre.1),
            self.rect.width() * radius,
            self.stroke,
        );
    }

    fn dot(&self, centre: (f32, f32), radius: f32) {
        self.painter.circle_filled(
            self.at(centre.0, centre.1),
            self.rect.width() * radius,
            self.colour,
        );
    }
}

fn open(pen: &Pen<'_>) {
    pen.line(&[
        (0.06, 0.82),
        (0.06, 0.22),
        (0.40, 0.22),
        (0.48, 0.36),
        (0.80, 0.36),
        (0.80, 0.48),
    ]);
    pen.line(&[
        (0.06, 0.82),
        (0.24, 0.48),
        (0.98, 0.48),
        (0.80, 0.82),
        (0.06, 0.82),
    ]);
}

fn save(pen: &Pen<'_>) {
    pen.line(&[
        (0.10, 0.10),
        (0.74, 0.10),
        (0.90, 0.26),
        (0.90, 0.90),
        (0.10, 0.90),
        (0.10, 0.10),
    ]);
    pen.line(&[(0.28, 0.10), (0.28, 0.34), (0.64, 0.34), (0.64, 0.10)]);
    pen.frame((0.26, 0.56), (0.74, 0.90), 0.0);
}

fn again(pen: &Pen<'_>) {
    let steps = 20_u8;
    let (centre, radius) = ((0.5_f32, 0.52_f32), 0.33_f32);
    let start = -std::f32::consts::FRAC_PI_2 + 0.45;
    let sweep = std::f32::consts::TAU * 0.82;
    let arc: Vec<(f32, f32)> = (0..=steps)
        .map(|step| {
            let angle = start + sweep * f32::from(step) / f32::from(steps);
            (
                centre.0 + radius * angle.cos(),
                centre.1 + radius * angle.sin(),
            )
        })
        .collect();
    pen.line(&arc);
    let end = start + sweep;
    let (tip_x, tip_y) = (centre.0 + radius * end.cos(), centre.1 + radius * end.sin());
    let (along_x, along_y) = (-end.sin(), end.cos());
    let barb = |turn: f32| {
        let (sin, cos) = turn.sin_cos();
        let (back_x, back_y) = (-along_x, -along_y);
        (
            tip_x + 0.2 * (back_x * cos - back_y * sin),
            tip_y + 0.2 * (back_x * sin + back_y * cos),
        )
    };
    let spread = 0.6;
    pen.line(&[barb(spread), (tip_x, tip_y), barb(-spread)]);
}

fn turn(pen: &Pen<'_>, forwards: bool) {
    let x = |v: f32| if forwards { 1.0 - v } else { v };
    pen.line(&[
        (x(0.10), 0.52),
        (x(0.26), 0.30),
        (x(0.58), 0.26),
        (x(0.86), 0.44),
        (x(0.88), 0.76),
    ]);
    pen.line(&[(x(0.04), 0.30), (x(0.10), 0.54), (x(0.34), 0.50)]);
}

fn rotate(pen: &Pen<'_>, clockwise: bool) {
    let x = |v: f32| if clockwise { v } else { 1.0 - v };
    let steps = 16_u8;
    let arc: Vec<(f32, f32)> = (0..=steps)
        .map(|step| {
            let share = f32::from(step) / f32::from(steps);
            let angle = std::f32::consts::PI * (1.25 - 1.4 * share);
            (x(0.5 + 0.34 * angle.cos()), 0.54 - 0.34 * angle.sin())
        })
        .collect();
    pen.line(&arc);
    let Some(&(end_x, end_y)) = arc.last() else {
        return;
    };
    let side = if clockwise { 1.0 } else { -1.0 };
    pen.line(&[
        (end_x - side * 0.2, end_y - 0.06),
        (end_x, end_y),
        (end_x + side * 0.05, end_y - 0.2),
    ]);
}

fn select(pen: &Pen<'_>) {
    pen.fill(&[
        (0.22, 0.08),
        (0.22, 0.80),
        (0.40, 0.63),
        (0.52, 0.92),
        (0.66, 0.85),
        (0.54, 0.57),
        (0.78, 0.55),
    ]);
}

fn text(pen: &Pen<'_>) {
    let heavy = pen.heavier(HEAVIER);
    heavy.line(&[(0.26, 0.26), (0.74, 0.26)]);
    heavy.line(&[(0.5, 0.26), (0.5, 0.76)]);
    for (x, y, dx, dy) in [
        (0.04, 0.04, 1.0, 1.0),
        (0.96, 0.04, -1.0, 1.0),
        (0.04, 0.96, 1.0, -1.0),
        (0.96, 0.96, -1.0, -1.0),
    ] {
        pen.line(&[(x, y + dy * 0.2), (x, y), (x + dx * 0.2, y)]);
    }
}

fn form(pen: &Pen<'_>) {
    pen.frame((0.06, 0.08), (0.40, 0.42), 0.06);
    pen.in_accent(TICK)
        .heavier(HEAVIER)
        .line(&[(0.12, 0.25), (0.21, 0.35), (0.36, 0.14)]);
    pen.line(&[(0.54, 0.25), (0.94, 0.25)]);
    pen.frame((0.06, 0.58), (0.40, 0.92), 0.06);
    pen.line(&[(0.54, 0.75), (0.94, 0.75)]);
}

fn chain(pen: &Pen<'_>) {
    let pen = pen.in_accent(CHAIN);
    link_loop(&pen, (0.10, 0.46), (0.56, 0.90));
    link_loop(&pen, (0.44, 0.10), (0.90, 0.54));
}

fn link_loop(pen: &Pen<'_>, from: (f32, f32), to: (f32, f32)) {
    const RADIUS: f32 = 0.13;
    let start = (from.0 + RADIUS, to.1 - RADIUS);
    let end = (to.0 - RADIUS, from.1 + RADIUS);
    let heading = (end.1 - start.1).atan2(end.0 - start.0);
    let half = std::f32::consts::FRAC_PI_2;
    let mut points = Vec::new();
    for (centre, from_angle) in [(end, heading - half), (start, heading + half)] {
        for step in 0..=8 {
            #[expect(clippy::cast_precision_loss, reason = "eight steps")]
            let angle = from_angle + std::f32::consts::PI * (step as f32) / 8.0;
            points.push((
                centre.0 + RADIUS * angle.cos(),
                centre.1 + RADIUS * angle.sin(),
            ));
        }
    }
    points.push(points[0]);
    pen.line(&points);
}

fn shape(pen: &Pen<'_>) {
    pen.frame((0.08, 0.34), (0.62, 0.90), 0.04);
    let fill = pen.in_accent(SHAPE_FILL);
    fill.dot((0.64, 0.36), 0.26);
    pen.ring((0.64, 0.36), 0.26);
}

fn nib(pen: &Pen<'_>) {
    pen.line(&[
        (0.78, 0.06),
        (0.92, 0.20),
        (0.36, 0.70),
        (0.18, 0.76),
        (0.24, 0.58),
        (0.78, 0.06),
    ]);
    pen.line(&[(0.66, 0.18), (0.80, 0.32)]);
    pen.in_accent(INK).line(&[
        (0.06, 0.92),
        (0.16, 0.84),
        (0.28, 0.92),
        (0.40, 0.84),
        (0.52, 0.92),
        (0.64, 0.84),
        (0.76, 0.92),
    ]);
}

fn marker(pen: &Pen<'_>) {
    pen.in_accent(BAND).block((0.04, 0.74), (0.96, 0.96), 0.02);
    pen.line(&[
        (0.50, 0.04),
        (0.88, 0.30),
        (0.62, 0.62),
        (0.30, 0.62),
        (0.30, 0.40),
        (0.50, 0.04),
    ]);
    pen.line(&[(0.30, 0.62), (0.24, 0.72), (0.40, 0.72), (0.46, 0.62)]);
    pen.line(&[(0.40, 0.26), (0.72, 0.46)]);
}

impl From<crate::window_state::Shape> for Icon {
    fn from(shape: crate::window_state::Shape) -> Self {
        match shape {
            crate::window_state::Shape::Line => Self::Line,
            crate::window_state::Shape::Arrow => Self::Arrow,
            crate::window_state::Shape::Rectangle => Self::Rectangle,
            crate::window_state::Shape::Ellipse => Self::Ellipse,
        }
    }
}

fn arrow(pen: &Pen<'_>) {
    pen.line(&[(0.14, 0.82), (0.86, 0.18)]);
    pen.line(&[(0.52, 0.18), (0.86, 0.18), (0.86, 0.52)]);
}

fn picture(pen: &Pen<'_>) {
    pen.in_accent(HILL)
        .fill(&[(0.12, 0.80), (0.40, 0.50), (0.62, 0.80)]);
    pen.in_accent(HILL)
        .fill(&[(0.46, 0.80), (0.68, 0.58), (0.88, 0.80)]);
    pen.in_accent(SUN).dot((0.68, 0.34), 0.09);
    pen.frame((0.06, 0.14), (0.94, 0.86), 0.06);
}

fn delete(pen: &Pen<'_>) {
    let pen = pen.in_accent(TAKE_AWAY);
    pen.line(&[(0.12, 0.26), (0.88, 0.26)]);
    pen.line(&[(0.38, 0.26), (0.38, 0.14), (0.62, 0.14), (0.62, 0.26)]);
    pen.line(&[(0.22, 0.26), (0.28, 0.92), (0.72, 0.92), (0.78, 0.26)]);
    pen.line(&[(0.42, 0.42), (0.44, 0.78)]);
    pen.line(&[(0.58, 0.42), (0.56, 0.78)]);
}

fn chevron(pen: &Pen<'_>, forwards: bool) {
    if forwards {
        pen.line(&[(0.34, 0.12), (0.70, 0.5), (0.34, 0.88)]);
    } else {
        pen.line(&[(0.66, 0.12), (0.30, 0.5), (0.66, 0.88)]);
    }
}

fn magnifier(pen: &Pen<'_>, closer: Option<bool>) {
    pen.ring((0.44, 0.42), 0.30);
    pen.line(&[(0.66, 0.66), (0.92, 0.92)]);
    match closer {
        Some(true) => {
            pen.line(&[(0.28, 0.42), (0.60, 0.42)]);
            pen.line(&[(0.44, 0.26), (0.44, 0.58)]);
        }
        Some(false) => pen.line(&[(0.28, 0.42), (0.60, 0.42)]),
        None => {}
    }
}

fn new_document(pen: &Pen<'_>) {
    pen.line(&[
        (0.14, 0.94),
        (0.14, 0.06),
        (0.56, 0.06),
        (0.76, 0.26),
        (0.76, 0.56),
    ]);
    pen.line(&[(0.56, 0.06), (0.56, 0.26), (0.76, 0.26)]);
    pen.line(&[(0.14, 0.94), (0.52, 0.94)]);
    pen.line(&[(0.62, 0.78), (0.94, 0.78)]);
    pen.line(&[(0.78, 0.62), (0.78, 0.94)]);
}

fn home(pen: &Pen<'_>) {
    pen.line(&[(0.06, 0.50), (0.5, 0.10), (0.94, 0.50)]);
    pen.line(&[(0.20, 0.38), (0.20, 0.92), (0.80, 0.92), (0.80, 0.38)]);
    pen.line(&[(0.40, 0.92), (0.40, 0.64), (0.60, 0.64), (0.60, 0.92)]);
}

fn document(pen: &Pen<'_>) {
    pen.line(&[
        (0.18, 0.06),
        (0.60, 0.06),
        (0.82, 0.28),
        (0.82, 0.94),
        (0.18, 0.94),
        (0.18, 0.06),
    ]);
    pen.line(&[(0.60, 0.06), (0.60, 0.28), (0.82, 0.28)]);
    pen.line(&[(0.32, 0.50), (0.68, 0.50)]);
    pen.line(&[(0.32, 0.66), (0.68, 0.66)]);
    pen.line(&[(0.32, 0.82), (0.56, 0.82)]);
}

fn folder(pen: &Pen<'_>) {
    pen.line(&[
        (0.06, 0.84),
        (0.06, 0.18),
        (0.38, 0.18),
        (0.48, 0.30),
        (0.94, 0.30),
        (0.94, 0.84),
        (0.06, 0.84),
    ]);
    pen.line(&[(0.06, 0.42), (0.94, 0.42)]);
}

fn theme(pen: &Pen<'_>) {
    pen.ring((0.5, 0.5), 0.38);
    pen.fill(&[
        (0.5, 0.12),
        (0.77, 0.23),
        (0.88, 0.5),
        (0.77, 0.77),
        (0.5, 0.88),
    ]);
}

fn bold(pen: &Pen<'_>) {
    let pen = pen.heavier(BOLD_WEIGHT);
    pen.line(&[
        (0.28, 0.12),
        (0.56, 0.12),
        (0.68, 0.18),
        (0.70, 0.30),
        (0.64, 0.42),
        (0.54, 0.47),
        (0.28, 0.47),
    ]);
    pen.line(&[
        (0.28, 0.47),
        (0.60, 0.47),
        (0.72, 0.54),
        (0.75, 0.68),
        (0.70, 0.82),
        (0.58, 0.88),
        (0.28, 0.88),
        (0.28, 0.12),
    ]);
}

fn italic(pen: &Pen<'_>) {
    pen.line(&[(0.62, 0.12), (0.38, 0.88)]);
    pen.line(&[(0.46, 0.12), (0.78, 0.12)]);
    pen.line(&[(0.22, 0.88), (0.54, 0.88)]);
}

fn underline(pen: &Pen<'_>) {
    pen.line(&[
        (0.28, 0.08),
        (0.28, 0.52),
        (0.33, 0.66),
        (0.5, 0.72),
        (0.67, 0.66),
        (0.72, 0.52),
        (0.72, 0.08),
    ]);
    pen.line(&[(0.18, 0.92), (0.82, 0.92)]);
}

fn clear_formatting(pen: &Pen<'_>) {
    pen.line(&[(0.10, 0.84), (0.36, 0.12), (0.62, 0.84)]);
    pen.line(&[(0.20, 0.58), (0.52, 0.58)]);
    pen.in_accent(TAKE_AWAY)
        .heavier(HEAVIER)
        .line(&[(0.58, 0.54), (0.94, 0.90)]);
    pen.in_accent(TAKE_AWAY)
        .heavier(HEAVIER)
        .line(&[(0.94, 0.54), (0.58, 0.90)]);
}

fn line_spacing(pen: &Pen<'_>) {
    pen.line(&[(0.20, 0.08), (0.20, 0.92)]);
    pen.line(&[(0.06, 0.22), (0.20, 0.08), (0.34, 0.22)]);
    pen.line(&[(0.06, 0.78), (0.20, 0.92), (0.34, 0.78)]);
    pen.line(&[(0.46, 0.20), (0.94, 0.20)]);
    pen.line(&[(0.46, 0.50), (0.94, 0.50)]);
    pen.line(&[(0.46, 0.80), (0.94, 0.80)]);
}

#[derive(Clone, Copy)]
enum Alignment {
    Start,
    Centre,
    End,
    Justify,
}

fn aligned(pen: &Pen<'_>, alignment: Alignment) {
    const LONG: f32 = 0.84;
    const SHORT: f32 = 0.52;
    for (step, y) in [0.16, 0.38, 0.60, 0.82].into_iter().enumerate() {
        let short = step % 2 == 1;
        let full = match alignment {
            Alignment::Justify => step + 1 < 4,
            _ => !short,
        };
        let width = if full { LONG } else { SHORT };
        let left = match alignment {
            Alignment::Start | Alignment::Justify => 0.08,
            Alignment::Centre => 0.5 - width / 2.0,
            Alignment::End => 0.92 - width,
        };
        pen.line(&[(left, y), (left + width, y)]);
    }
}

fn flow_round(pen: &Pen<'_>) {
    pen.frame((0.56, 0.34), (0.94, 0.72), 0.04);
    for (y, right) in [(0.16, 0.92), (0.38, 0.50), (0.60, 0.50), (0.82, 0.92)] {
        pen.line(&[(0.08, y), (right, y)]);
    }
}

fn custom_colour(pen: &Pen<'_>) {
    const HUES: [[u8; 3]; 6] = [
        [239, 68, 68],
        [245, 158, 11],
        [234, 179, 8],
        [34, 197, 94],
        [59, 130, 246],
        [168, 85, 247],
    ];
    for (at, hue) in HUES.iter().enumerate() {
        #[expect(clippy::cast_precision_loss, reason = "six hues")]
        let angle = std::f32::consts::TAU * (at as f32) / 6.0;
        let centre = (0.5 + 0.30 * angle.cos(), 0.5 + 0.30 * angle.sin());
        pen.in_accent(*hue).dot(centre, 0.14);
    }
    pen.line(&[(0.40, 0.5), (0.60, 0.5)]);
    pen.line(&[(0.5, 0.40), (0.5, 0.60)]);
}

fn text_field(pen: &Pen<'_>) {
    pen.frame((0.04, 0.24), (0.96, 0.76), 0.08);
    pen.line(&[(0.20, 0.36), (0.20, 0.64)]);
    pen.line(&[(0.14, 0.36), (0.26, 0.36)]);
    pen.line(&[(0.14, 0.64), (0.26, 0.64)]);
}

fn paragraph_field(pen: &Pen<'_>) {
    pen.frame((0.04, 0.12), (0.96, 0.88), 0.06);
    pen.line(&[(0.18, 0.32), (0.82, 0.32)]);
    pen.line(&[(0.18, 0.50), (0.82, 0.50)]);
    pen.line(&[(0.18, 0.68), (0.60, 0.68)]);
}

fn checkbox(pen: &Pen<'_>) {
    pen.frame((0.14, 0.14), (0.86, 0.86), 0.08);
    pen.in_accent(TICK)
        .heavier(HEAVIER)
        .line(&[(0.28, 0.50), (0.44, 0.66), (0.72, 0.34)]);
}

fn radio_button(pen: &Pen<'_>) {
    pen.ring((0.5, 0.5), 0.38);
    pen.in_accent(TICK).dot((0.5, 0.5), 0.18);
}

fn dropdown(pen: &Pen<'_>) {
    pen.frame((0.04, 0.24), (0.96, 0.76), 0.08);
    pen.line(&[(0.64, 0.24), (0.64, 0.76)]);
    pen.line(&[(0.72, 0.42), (0.80, 0.56), (0.88, 0.42)]);
    pen.line(&[(0.16, 0.5), (0.50, 0.5)]);
}

fn list_box(pen: &Pen<'_>) {
    pen.frame((0.06, 0.08), (0.94, 0.92), 0.06);
    pen.line(&[(0.20, 0.28), (0.80, 0.28)]);
    pen.in_accent(CHAIN).block((0.14, 0.42), (0.86, 0.58), 0.02);
    pen.line(&[(0.20, 0.74), (0.80, 0.74)]);
}

fn date_field(pen: &Pen<'_>) {
    pen.frame((0.08, 0.16), (0.92, 0.92), 0.06);
    pen.line(&[(0.08, 0.36), (0.92, 0.36)]);
    pen.line(&[(0.30, 0.06), (0.30, 0.24)]);
    pen.line(&[(0.70, 0.06), (0.70, 0.24)]);
    for y in [0.54, 0.74] {
        for x in [0.28, 0.5, 0.72] {
            pen.dot((x, y), 0.05);
        }
    }
}

fn signature_field(pen: &Pen<'_>) {
    pen.line(&[(0.06, 0.84), (0.94, 0.84)]);
    pen.line(&[(0.10, 0.62), (0.18, 0.70), (0.26, 0.62)]);
    pen.line(&[(0.10, 0.70), (0.26, 0.62)]);
    pen.in_accent(INK).line(&[
        (0.30, 0.66),
        (0.40, 0.30),
        (0.46, 0.66),
        (0.56, 0.38),
        (0.62, 0.62),
        (0.72, 0.44),
        (0.80, 0.56),
        (0.92, 0.46),
    ]);
}

fn push_button(pen: &Pen<'_>) {
    pen.frame((0.04, 0.18), (0.80, 0.62), 0.14);
    pen.line(&[(0.20, 0.40), (0.56, 0.40)]);
    pen.fill(&[
        (0.62, 0.48),
        (0.62, 0.94),
        (0.73, 0.83),
        (0.80, 0.98),
        (0.87, 0.94),
        (0.80, 0.80),
        (0.94, 0.79),
    ]);
}

fn arrange(pen: &Pen<'_>) {
    pen.line(&[(0.10, 0.06), (0.10, 0.94)]);
    pen.block((0.20, 0.14), (0.90, 0.32), 0.04);
    pen.block((0.20, 0.41), (0.62, 0.59), 0.04);
    pen.block((0.20, 0.68), (0.78, 0.86), 0.04);
}

fn fit_to_paper(pen: &Pen<'_>) {
    pen.frame((0.06, 0.02), (0.94, 0.98), 0.04);
    let inner = pen.in_accent(STACK);
    inner.frame((0.30, 0.30), (0.70, 0.70), 0.03);
    let arrows = pen.heavier(HEAVIER);
    arrows.line(&[(0.12, 0.22), (0.24, 0.22), (0.24, 0.10)]);
    arrows.line(&[(0.88, 0.22), (0.76, 0.22), (0.76, 0.10)]);
    arrows.line(&[(0.12, 0.78), (0.24, 0.78), (0.24, 0.90)]);
    arrows.line(&[(0.88, 0.78), (0.76, 0.78), (0.76, 0.90)]);
}

fn ordering(pen: &Pen<'_>, forward: bool, all_the_way: bool) {
    let other = ((0.02, 0.34), (0.52, 0.84));
    let moving = ((0.18, 0.14), (0.68, 0.64));
    let pale = pen.weak();
    let accent = pen.in_accent(STACK);
    if forward {
        pale.block(other.0, other.1, 0.08);
        accent.block(moving.0, moving.1, 0.08);
    } else {
        accent.block(moving.0, moving.1, 0.08);
        pale.block(other.0, other.1, 0.08);
    }
    let ys: &[f32] = match (forward, all_the_way) {
        (true, false) => &[0.36],
        (true, true) => &[0.20, 0.48],
        (false, false) => &[0.60],
        (false, true) => &[0.48, 0.76],
    };
    let badge = pen.heavier(HEAVIER);
    for &y in ys {
        chevron_badge(&badge, y, forward);
    }
}

fn chevron_badge(pen: &Pen<'_>, y: f32, up: bool) {
    let (near, far) = if up {
        (y + 0.10, y - 0.10)
    } else {
        (y - 0.10, y + 0.10)
    };
    pen.line(&[(0.74, near), (0.87, far), (1.00, near)]);
}

fn link_addresses(pen: &Pen<'_>) {
    for start in [0.04, 0.34, 0.64] {
        pen.line(&[
            (start, 0.10),
            (start + 0.07, 0.38),
            (start + 0.14, 0.18),
            (start + 0.21, 0.38),
            (start + 0.28, 0.10),
        ]);
    }
    let chain = pen.in_accent(CHAIN);
    chain.frame((0.10, 0.58), (0.54, 0.90), 0.16);
    chain.frame((0.46, 0.58), (0.90, 0.90), 0.16);
}

fn named_places(pen: &Pen<'_>) {
    pen.in_accent(CHAIN).line(&[
        (0.24, 0.06),
        (0.76, 0.06),
        (0.76, 0.94),
        (0.5, 0.72),
        (0.24, 0.94),
        (0.24, 0.06),
    ]);
}

fn unlink(pen: &Pen<'_>) {
    let chain = pen.in_accent(CHAIN);
    link_loop(&chain, (0.04, 0.52), (0.48, 0.96));
    link_loop(&chain, (0.52, 0.04), (0.96, 0.48));
    let red = pen.in_accent(TAKE_AWAY);
    red.line(&[(0.40, 0.14), (0.44, 0.30)]);
    red.line(&[(0.60, 0.70), (0.56, 0.86)]);
    red.line(&[(0.14, 0.40), (0.30, 0.44)]);
    red.line(&[(0.70, 0.60), (0.86, 0.56)]);
}

fn settings(pen: &Pen<'_>) {
    for (y, knob) in [(0.20, 0.30), (0.50, 0.68), (0.80, 0.44)] {
        pen.line(&[(0.06, y), (0.94, y)]);
        pen.in_accent(CHAIN).dot((knob, y), 0.10);
    }
}

#[cfg(test)]
mod tests {
    use super::{INSET, Icon, WEIGHT};

    #[test]
    fn every_icon_is_drawn_inside_its_own_box() {
        let source = include_str!("icons.rs");
        let pairs = source
            .match_indices("(0.")
            .chain(source.match_indices("(1.0"))
            .count();
        assert!(pairs > 40, "the drawings are still here: {pairs}");
        for found in source.split('(').skip(1) {
            let Some(number) = found.split([',', ')']).next() else {
                continue;
            };
            let Ok(value) = number.trim().parse::<f32>() else {
                continue;
            };
            assert!(
                (0.0..=1.0).contains(&value),
                "{value} is outside the icon's own box"
            );
        }
    }

    #[test]
    fn an_icons_weight_and_inset_are_shares_of_its_size() {
        let (weight, inset) = (std::hint::black_box(WEIGHT), std::hint::black_box(INSET));
        assert!((0.0..0.2).contains(&weight), "{weight}");
        assert!((0.0..0.25).contains(&inset), "{inset}");
    }

    #[test]
    fn every_icon_stands_for_something_a_person_does() {
        let every = [
            Icon::Open,
            Icon::Save,
            Icon::Undo,
            Icon::Redo,
            Icon::Select,
            Icon::Text,
            Icon::Shape,
            Icon::Form,
            Icon::Link,
            Icon::Picture,
            Icon::Delete,
            Icon::Previous,
            Icon::Next,
            Icon::ZoomIn,
            Icon::ZoomOut,
            Icon::NewDocument,
            Icon::Theme,
            Icon::Home,
            Icon::Document,
            Icon::Folder,
            Icon::Pen,
            Icon::Highlighter,
            Icon::Bold,
            Icon::Italic,
            Icon::Underline,
            Icon::ClearFormatting,
            Icon::LineSpacing,
            Icon::AlignStart,
            Icon::AlignCentre,
            Icon::AlignEnd,
            Icon::AlignJustify,
            Icon::FlowRound,
            Icon::Minus,
            Icon::Plus,
            Icon::CustomColour,
            Icon::TextField,
            Icon::ParagraphField,
            Icon::Checkbox,
            Icon::RadioButton,
            Icon::Dropdown,
            Icon::ListBox,
            Icon::DateField,
            Icon::SignatureField,
            Icon::PushButton,
            Icon::Arrange,
            Icon::BringToFront,
            Icon::BringForward,
            Icon::SendBackward,
            Icon::SendToBack,
            Icon::LinkAddresses,
            Icon::NamedPlaces,
            Icon::Unlink,
            Icon::Settings,
            Icon::RotateLeft,
            Icon::RotateRight,
            Icon::More,
            Icon::Close,
            Icon::Copy,
            Icon::Edit,
            Icon::AskAgain,
            Icon::Send,
            Icon::Stop,
        ];
        for (step, icon) in every.iter().enumerate() {
            for other in &every[step + 1..] {
                assert_ne!(icon, other, "two of the same icon");
            }
        }
        assert_eq!(every.len(), 62);
    }
}
