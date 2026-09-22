use eframe::egui;

use pdf_app::wording::{Control, Lang, Message};

use crate::icons::Icon;
use crate::window_state::Window;

mod font_search;

const SIZES: [f64; 16] = [
    8.0, 9.0, 10.0, 11.0, 12.0, 14.0, 16.0, 18.0, 20.0, 22.0, 24.0, 28.0, 36.0, 48.0, 60.0, 72.0,
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Pressed {
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) underline: bool,
}

impl Pressed {
    pub(crate) fn of(
        faces: &[Vec<pdf_edit::ClusterFace>],
        range: Option<((usize, usize), (usize, usize))>,
    ) -> Self {
        let chosen: Vec<pdf_edit::ClusterFace> = match range {
            None => faces.iter().flatten().copied().collect(),
            Some((from, to)) if from == to => {
                let (line, stop) = from;
                let before = faces
                    .get(line)
                    .and_then(|line| line.get(stop.checked_sub(1)?))
                    .or_else(|| faces.get(line).and_then(|line| line.first()))
                    .or_else(|| {
                        faces[..line.min(faces.len())]
                            .iter()
                            .rev()
                            .find_map(|line| line.last())
                    });
                before.copied().into_iter().collect()
            }
            Some((one, other)) => {
                let (start, end) = (one.min(other), one.max(other));
                let mut chosen = Vec::new();
                for (line, clusters) in faces.iter().enumerate() {
                    if line < start.0 || line > end.0 {
                        continue;
                    }
                    let first = if line == start.0 { start.1 } else { 0 };
                    let last = if line == end.0 { end.1 } else { clusters.len() };
                    chosen.extend(clusters.iter().take(last).skip(first).copied());
                }
                chosen
            }
        };
        let letters: Vec<pdf_edit::ClusterFace> =
            chosen.iter().copied().filter(|face| !face.blank).collect();
        let chosen = if letters.is_empty() { chosen } else { letters };
        if chosen.is_empty() {
            return Self::default();
        }
        Self {
            bold: chosen.iter().all(|face| face.bold),
            italic: chosen.iter().all(|face| face.italic),
            underline: chosen.iter().all(|face| face.underline),
        }
    }

    pub(crate) fn with(self, next: &pdf_edit::TextStyle) -> Self {
        Self {
            bold: next.bold.unwrap_or(self.bold),
            italic: next.italic.unwrap_or(self.italic),
            underline: next.underline.unwrap_or(self.underline),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Set {
    Align(pdf_edit::Alignment),
    FlowRound(bool),
}

pub(crate) enum Wanted {
    Style(pdf_edit::TextStyle),
    Size(f64),
    Align(pdf_edit::Alignment),
    FlowRound(bool),
}

fn step_size(size: f64, larger: bool) -> f64 {
    if larger {
        SIZES
            .iter()
            .copied()
            .find(|step| *step > size + 0.05)
            .unwrap_or(size + 12.0)
    } else {
        SIZES
            .iter()
            .rev()
            .copied()
            .find(|step| *step < size - 0.05)
            .unwrap_or((size - 1.0).max(1.0))
    }
}

impl Window {
    pub(crate) fn block_spacing(&mut self, page: usize, block: usize) -> Option<f64> {
        self.block_paragraph(page, block).map(|(pitch, _)| pitch)
    }

    pub(crate) fn block_alignment(
        &mut self,
        page: usize,
        block: usize,
    ) -> Option<pdf_edit::Alignment> {
        self.block_paragraph(page, block)
            .map(|(_, alignment)| alignment)
    }

    fn block_paragraph(&mut self, page: usize, block: usize) -> Option<(f64, pdf_edit::Alignment)> {
        let reading = std::sync::Arc::as_ptr(&self.editor.leaf(page)?.view).addr();
        let key = (page, block, reading);
        if self.spacing.map(|(held, _)| held) != Some(key) {
            self.spacing = Some((key, self.editor.block_paragraph(page, block)));
        }
        self.spacing.and_then(|(_, read)| read)
    }

    pub(crate) fn block_faces(
        &mut self,
        page: usize,
        block: usize,
    ) -> Option<crate::window_state::BlockFaces> {
        let reading = std::sync::Arc::as_ptr(&self.editor.leaf(page)?.view).addr();
        let key = (page, block, reading);
        if self.faces.as_ref().map(|(held, _)| *held) != Some(key) {
            let faces = self
                .editor
                .block_faces(page, block)
                .map(std::sync::Arc::new);
            self.faces = Some((key, faces));
        }
        self.faces.as_ref().and_then(|(_, faces)| faces.clone())
    }

    pub(crate) fn font_row(&mut self, ui: &mut egui::Ui, showing: &Showing<'_>) -> Option<Wanted> {
        let &Showing {
            font,
            size,
            spacing,
            fill,
            pressed,
            paragraph,
        } = showing;
        let lang = self.lang;
        let mut wanted = None;
        if let Some(style) = font_box(ui, lang, font) {
            wanted = Some(Wanted::Style(style));
        }
        rule(ui);
        if let Some(points) = size_box(ui, lang, size, &mut self.typing.size) {
            wanted = Some(Wanted::Size(points));
        }
        rule(ui);
        if let Some(style) = face_buttons(ui, lang, pressed) {
            wanted = Some(Wanted::Style(style));
        }
        rule(ui);
        if let Some(style) = colour_box(ui, lang, fill, &mut self.colours) {
            wanted = Some(Wanted::Style(style));
        }
        if let Some(style) = spacing_box(ui, lang, spacing, size) {
            wanted = Some(Wanted::Style(style));
        }
        rule(ui);
        if let Some(asked) = paragraph_buttons(ui, lang, paragraph) {
            wanted = Some(asked);
        }
        wanted
    }
}

pub(crate) struct Showing<'a> {
    pub(crate) font: Option<&'a str>,
    pub(crate) size: Option<f64>,
    pub(crate) spacing: Option<f64>,
    pub(crate) fill: Option<[f64; 3]>,
    pub(crate) pressed: Pressed,
    pub(crate) paragraph: ParagraphNow,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ParagraphNow {
    pub(crate) alignment: Option<pdf_edit::Alignment>,
    pub(crate) flows_round: bool,
}

fn paragraph_buttons(ui: &mut egui::Ui, lang: Lang, now: ParagraphNow) -> Option<Wanted> {
    let mut chosen = None;
    ui.spacing_mut().item_spacing.x = 2.0;
    for (icon, hover, alignment) in [
        (
            Icon::AlignStart,
            Message::AlignStartHelp.say(lang),
            pdf_edit::Alignment::Start,
        ),
        (
            Icon::AlignCentre,
            Message::AlignCentreHelp.say(lang),
            pdf_edit::Alignment::Centre,
        ),
        (
            Icon::AlignEnd,
            Message::AlignEndHelp.say(lang),
            pdf_edit::Alignment::End,
        ),
        (
            Icon::AlignJustify,
            Message::AlignJustifyHelp.say(lang),
            pdf_edit::Alignment::Justify,
        ),
    ] {
        let on = now.alignment == Some(alignment);
        if icon_button(ui, icon, &hover, on, true).clicked() {
            chosen = Some(Wanted::Align(alignment));
        }
    }
    if icon_button(
        ui,
        Icon::FlowRound,
        &Message::FlowRoundHelp.say(lang),
        now.flows_round,
        true,
    )
    .clicked()
    {
        chosen = Some(Wanted::FlowRound(!now.flows_round));
    }
    ui.spacing_mut().item_spacing.x = 4.0;
    chosen
}

pub(crate) const CONTROL_HEIGHT: f32 = 28.0;

pub(crate) fn rule(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(9.0, CONTROL_HEIGHT), egui::Sense::hover());
    let x = rect.center().x;
    ui.painter().line_segment(
        [
            egui::pos2(x, rect.top() + 5.0),
            egui::pos2(x, rect.bottom() - 5.0),
        ],
        ui.visuals().widgets.noninteractive.bg_stroke,
    );
}

pub(crate) fn icon_button(
    ui: &mut egui::Ui,
    icon: Icon,
    hover: &str,
    on: bool,
    enabled: bool,
) -> egui::Response {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(CONTROL_HEIGHT, CONTROL_HEIGHT), sense);
    if ui.is_rect_visible(rect) {
        let visuals = ui.visuals();
        if on {
            ui.painter()
                .rect_filled(rect, 5.0, visuals.selection.bg_fill.gamma_multiply(0.35));
        } else if enabled && response.hovered() {
            ui.painter()
                .rect_filled(rect, 5.0, visuals.widgets.hovered.weak_bg_fill);
        }
        let colour = if !enabled {
            visuals.weak_text_color()
        } else if on {
            visuals.selection.stroke.color
        } else {
            visuals.text_color()
        };
        icon.draw_tinted(ui.painter(), rect.shrink(5.0), colour, enabled);
    }
    response.on_hover_text(hover)
}

fn chevron(ui: &egui::Ui, rect: egui::Rect) {
    let middle = egui::pos2(rect.right() - 8.0, rect.center().y);
    ui.painter().add(egui::Shape::line(
        vec![
            middle + egui::vec2(-3.5, -1.8),
            middle + egui::vec2(0.0, 1.8),
            middle + egui::vec2(3.5, -1.8),
        ],
        egui::Stroke::new(1.3, ui.visuals().weak_text_color()),
    ));
}

fn opener(
    ui: &mut egui::Ui,
    width: f32,
    hover: &str,
    face: impl FnOnce(&egui::Ui, egui::Rect),
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, CONTROL_HEIGHT), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response));
        if open {
            ui.painter().rect_filled(
                rect,
                5.0,
                ui.visuals().selection.bg_fill.gamma_multiply(0.35),
            );
        } else if response.hovered() {
            ui.painter()
                .rect_filled(rect, 5.0, ui.visuals().widgets.hovered.weak_bg_fill);
        }
        chevron(ui, rect);
        let inner =
            egui::Rect::from_min_max(rect.min, egui::pos2(rect.right() - 14.0, rect.bottom()));
        face(ui, inner);
    }
    response.on_hover_text(hover)
}

fn font_box(ui: &mut egui::Ui, lang: Lang, font: Option<&str>) -> Option<pdf_edit::TextStyle> {
    let mut chosen = None;
    let shown = font.map_or_else(|| Message::Font.say(lang), str::to_owned);
    let query_id = egui::Id::new("format-font-query");
    let opened_id = egui::Id::new("format-font-opened");
    let list = egui::ComboBox::from_id_salt("format-font")
        .selected_text(egui::RichText::new(shown))
        .width(150.0)
        .height(400.0)
        .truncate()
        .show_ui(ui, |ui| {
            let mut query: String = ui.data(|data| data.get_temp(query_id)).unwrap_or_default();
            let line = ui.add(
                egui::TextEdit::singleline(&mut query)
                    .hint_text(Message::Font.say(lang))
                    .desired_width(f32::INFINITY),
            );
            if ui.data(|data| data.get_temp::<bool>(opened_id)).is_none() {
                line.request_focus();
                ui.data_mut(|data| data.insert_temp(opened_id, true));
            }
            let families = pdf_cli::font_families();
            let shortlist = font_search::matching(families, &query);
            let took_the_first =
                line.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            if took_the_first {
                if let Some(&first) = shortlist.first() {
                    chosen = Some(&families[first]);
                }
                ui.close();
            }
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height(340.0)
                .show(ui, |ui| {
                    for at in shortlist {
                        let family = &families[at];
                        let current = font.is_some_and(|font| font.eq_ignore_ascii_case(family));
                        if ui.selectable_label(current, family).clicked() {
                            chosen = Some(family);
                        }
                    }
                });
            ui.data_mut(|data| data.insert_temp(query_id, query));
        });
    if list.inner.is_none() {
        ui.data_mut(|data| {
            data.remove::<String>(query_id);
            data.remove::<bool>(opened_id);
        });
    }
    list.response.on_hover_text(Message::FontHelp.say(lang));
    chosen.map(|family| pdf_edit::TextStyle {
        family: Some(family.clone()),
        ..pdf_edit::TextStyle::default()
    })
}

fn size_box(
    ui: &mut egui::Ui,
    lang: Lang,
    size: Option<f64>,
    held: &mut Option<String>,
) -> Option<f64> {
    let mut chosen = None;
    let shown = size.map_or_else(String::new, |size| {
        let rounded = (size * 10.0).round() / 10.0;
        format!("{rounded}")
    });
    let visuals = ui.visuals().clone();
    egui::Frame::NONE
        .stroke(visuals.widgets.noninteractive.bg_stroke)
        .corner_radius(6.0)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let smaller = icon_button(
                ui,
                Icon::Minus,
                &Message::ShrinkFont.say(lang),
                false,
                size.is_some(),
            );
            let mut text = held.clone().unwrap_or_else(|| shown.clone());
            let field = ui.add(
                egui::TextEdit::singleline(&mut text)
                    .desired_width(38.0)
                    .frame(egui::Frame::NONE)
                    .horizontal_align(egui::Align::Center)
                    .vertical_align(egui::Align::Center)
                    .min_size(egui::vec2(38.0, CONTROL_HEIGHT)),
            );
            let field = field.on_hover_text(Message::Control(Control::FontSize).say(lang));
            if field.changed() {
                *held = Some(text.clone());
            }
            if field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                *held = None;
                if text.trim() != shown
                    && let Ok(points) = text.trim().parse::<f64>()
                    && points > 0.0
                {
                    chosen = Some(points);
                }
            }
            let larger = icon_button(
                ui,
                Icon::Plus,
                &Message::GrowFont.say(lang),
                false,
                size.is_some(),
            );
            if let Some(size) = size {
                if smaller.clicked() {
                    chosen = Some(step_size(size, false));
                }
                if larger.clicked() {
                    chosen = Some(step_size(size, true));
                }
            }
            egui::Popup::menu(&field)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| {
                    ui.set_min_width(56.0);
                    egui::ScrollArea::vertical()
                        .max_height(300.0)
                        .show(ui, |ui| {
                            for step in SIZES {
                                let current = size.is_some_and(|size| (size - step).abs() < 0.05);
                                if ui.selectable_label(current, format!("{step}")).clicked() {
                                    chosen = Some(step);
                                    *held = None;
                                    ui.close();
                                }
                            }
                        });
                });
        });
    chosen
}

fn face_buttons(ui: &mut egui::Ui, lang: Lang, pressed: Pressed) -> Option<pdf_edit::TextStyle> {
    let mut chosen = None;
    let plain = pdf_edit::TextStyle::default();
    ui.spacing_mut().item_spacing.x = 2.0;
    for (icon, hover, on, style) in [
        (
            Icon::Bold,
            Message::BoldHelp.say(lang),
            pressed.bold,
            pdf_edit::TextStyle {
                bold: Some(!pressed.bold),
                ..plain.clone()
            },
        ),
        (
            Icon::Italic,
            Message::ItalicHelp.say(lang),
            pressed.italic,
            pdf_edit::TextStyle {
                italic: Some(!pressed.italic),
                ..plain.clone()
            },
        ),
        (
            Icon::Underline,
            Message::Underline.say(lang),
            pressed.underline,
            pdf_edit::TextStyle {
                underline: Some(!pressed.underline),
                ..plain.clone()
            },
        ),
        (
            Icon::ClearFormatting,
            Message::ClearFormatting.say(lang),
            false,
            pdf_edit::TextStyle {
                bold: Some(false),
                italic: Some(false),
                underline: Some(false),
                ..plain.clone()
            },
        ),
    ] {
        if icon_button(ui, icon, &hover, on, true).clicked() {
            chosen = Some(style);
        }
    }
    ui.spacing_mut().item_spacing.x = 4.0;
    chosen
}

fn colour_box(
    ui: &mut egui::Ui,
    lang: Lang,
    fill: Option<[f64; 3]>,
    colours: &mut crate::palette::Colours,
) -> Option<pdf_edit::TextStyle> {
    let now = fill.map(crate::palette::bytes_of);
    let bar = now.map_or(ui.visuals().text_color(), |[r, g, b]| {
        egui::Color32::from_rgb(r, g, b)
    });
    let button = opener(ui, 40.0, &Message::TextColour.say(lang), |ui, rect| {
        let letter = egui::pos2(rect.center().x, rect.top() + 12.0);
        ui.painter().text(
            letter,
            egui::Align2::CENTER_CENTER,
            "A",
            egui::FontId::proportional(15.0),
            ui.visuals().text_color(),
        );
        let under = egui::Rect::from_center_size(
            egui::pos2(rect.center().x, rect.bottom() - 6.0),
            egui::vec2(16.0, 4.0),
        );
        ui.painter().rect_filled(under, 1.0, bar);
    });
    crate::palette::popup(&button, now, colours, lang).map(|colour| pdf_edit::TextStyle {
        fill: Some(crate::palette::channels_of(colour)),
        ..pdf_edit::TextStyle::default()
    })
}

fn spacing_box(
    ui: &mut egui::Ui,
    lang: Lang,
    spacing: Option<f64>,
    size: Option<f64>,
) -> Option<pdf_edit::TextStyle> {
    let mut chosen = None;
    let ratio = match (spacing, size) {
        (Some(spacing), Some(size)) if size > 0.0 => Some(spacing / size),
        _ => None,
    };
    let button = opener(ui, 66.0, &Message::LineSpacingHelp.say(lang), |ui, rect| {
        let icon = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 4.0, rect.center().y - 8.0),
            egui::vec2(16.0, 16.0),
        );
        Icon::LineSpacing.draw(ui.painter(), icon, ui.visuals().text_color());
        if let Some(ratio) = ratio {
            ui.painter().text(
                egui::pos2(icon.right() + 4.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                format!("{ratio:.2}"),
                egui::FontId::proportional(12.0),
                ui.visuals().text_color(),
            );
        }
    });
    egui::Popup::menu(&button).show(|ui| {
        let Some(size) = size else {
            return;
        };
        for step in [1.0, 1.15, 1.5, 2.0, 2.5, 3.0] {
            let points = step * size;
            let label = Message::TimesTextSize {
                ratio: step,
                points,
            }
            .say(lang);
            let current = ratio.is_some_and(|ratio| (ratio - step).abs() < 0.01);
            if ui.selectable_label(current, label).clicked() {
                chosen = Some(pdf_edit::TextStyle {
                    line_spacing: Some(points),
                    ..pdf_edit::TextStyle::default()
                });
            }
        }
    });
    chosen
}

#[cfg(test)]
mod tests {
    use super::{Pressed, step_size};

    fn face(bold: bool, italic: bool, underline: bool) -> pdf_edit::ClusterFace {
        pdf_edit::ClusterFace {
            bold,
            italic,
            underline,
            blank: false,
        }
    }

    #[test]
    fn spaces_do_not_decide_whether_text_is_underlined() {
        let space = pdf_edit::ClusterFace {
            blank: true,
            ..pdf_edit::ClusterFace::default()
        };
        let faces = vec![vec![
            face(false, false, true),
            space,
            face(false, false, true),
        ]];
        assert!(Pressed::of(&faces, None).underline);
        assert!(
            !Pressed::of(&[vec![space]], None).underline,
            "a block of spaces is not"
        );
    }

    #[test]
    fn a_style_shows_pressed_only_when_all_it_acts_on_has_it() {
        let faces = vec![
            vec![face(true, false, true), face(true, true, false)],
            Vec::new(),
            vec![face(false, false, false)],
        ];
        let whole = Pressed::of(&faces, None);
        assert_eq!(whole, Pressed::default());
        let first_line = Pressed::of(&faces, Some(((0, 0), (0, 2))));
        assert!(first_line.bold && !first_line.italic && !first_line.underline);
        let backwards = Pressed::of(&faces, Some(((0, 2), (0, 1))));
        assert!(
            backwards.bold && backwards.italic,
            "either way it was drawn"
        );
        let across = Pressed::of(&faces, Some(((0, 1), (2, 1))));
        assert!(!across.bold);
    }

    #[test]
    fn at_a_caret_the_style_is_the_letter_before_it() {
        let faces = vec![vec![face(true, false, false), face(false, true, false)]];
        assert!(Pressed::of(&faces, Some(((0, 1), (0, 1)))).bold);
        assert!(Pressed::of(&faces, Some(((0, 2), (0, 2)))).italic);
        assert!(Pressed::of(&faces, Some(((0, 0), (0, 0)))).bold);
        let next = pdf_edit::TextStyle {
            bold: Some(false),
            ..pdf_edit::TextStyle::default()
        };
        assert!(!Pressed::of(&faces, Some(((0, 1), (0, 1)))).with(&next).bold);
    }

    #[test]
    fn font_size_steps_walk_the_ladder() {
        assert!((step_size(12.0, true) - 14.0).abs() < 1e-9);
        assert!((step_size(12.0, false) - 11.0).abs() < 1e-9);
        assert!((step_size(13.0, true) - 14.0).abs() < 1e-9);
        assert!((step_size(13.0, false) - 12.0).abs() < 1e-9);
        assert!((step_size(72.0, true) - 84.0).abs() < 1e-9);
        assert!((step_size(8.0, false) - 7.0).abs() < 1e-9);
        assert!((step_size(1.0, false) - 1.0).abs() < 1e-9);
    }
}
