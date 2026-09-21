use eframe::egui;

use pdf_app::wording::{Control, Lang, Message};

use crate::icons::Icon;

const RECENT: usize = 10;

const SWATCH: f32 = 20.0;

const GRID_WIDTH: f32 = 10.0 * SWATCH + 9.0 * 2.0;

const GREYS: [[u8; 3]; 10] = [
    [0, 0, 0],
    [67, 67, 67],
    [102, 102, 102],
    [153, 153, 153],
    [183, 183, 183],
    [204, 204, 204],
    [217, 217, 217],
    [239, 239, 239],
    [243, 243, 243],
    [255, 255, 255],
];

const HUES: [[u8; 3]; 10] = [
    [152, 0, 0],
    [255, 0, 0],
    [255, 153, 0],
    [255, 255, 0],
    [0, 255, 0],
    [0, 255, 255],
    [74, 134, 232],
    [0, 0, 255],
    [153, 0, 255],
    [255, 0, 255],
];

const SHADES: [f32; 5] = [0.8, 0.6, 0.4, -0.25, -0.5];

#[derive(Clone, Debug)]
pub(crate) struct Colours {
    pub(crate) recent: Vec<[u8; 3]>,
    pub(crate) mixing: egui::Color32,
    pub(crate) typed: Option<String>,
}

impl Default for Colours {
    fn default() -> Self {
        Self {
            recent: Vec::new(),
            mixing: egui::Color32::from_rgb(37, 99, 235),
            typed: None,
        }
    }
}

impl Colours {
    pub(crate) fn used(&mut self, colour: [u8; 3]) {
        self.recent.retain(|had| *had != colour);
        self.recent.insert(0, colour);
        self.recent.truncate(RECENT);
    }
}

fn shade([r, g, b]: [u8; 3], share: f32) -> [u8; 3] {
    let mix = |channel: u8| {
        let value = f32::from(channel);
        let mixed = if share >= 0.0 {
            value + (255.0 - value) * share
        } else {
            value * (1.0 + share)
        };
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a channel between nought and 255"
        )]
        let byte = mixed.round().clamp(0.0, 255.0) as u8;
        byte
    };
    [mix(r), mix(g), mix(b)]
}

pub(crate) fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02X}{g:02X}{b:02X}")
}

pub(crate) fn parse_hex(text: &str) -> Option<[u8; 3]> {
    let digits = text.trim().trim_start_matches('#');
    let full: String = match digits.len() {
        3 => digits.chars().flat_map(|digit| [digit, digit]).collect(),
        6 => digits.to_owned(),
        _ => return None,
    };
    let channel = |at: usize| u8::from_str_radix(full.get(at..at + 2)?, 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}

pub(crate) fn bytes_of(rgb: [f64; 3]) -> [u8; 3] {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a colour component between 0 and 1, scaled to a byte"
    )]
    rgb.map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8)
}

pub(crate) fn channels_of(bytes: [u8; 3]) -> [f64; 3] {
    bytes.map(|value| f64::from(value) / 255.0)
}

pub(crate) fn popup(
    button: &egui::Response,
    current: Option<[u8; 3]>,
    colours: &mut Colours,
    lang: Lang,
) -> Option<[u8; 3]> {
    let mut chosen = None;
    egui::Popup::menu(button)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_width(GRID_WIDTH);
            chosen = grid(ui, current, colours, lang);
            if chosen.is_some() {
                ui.close();
            }
        });
    if let Some(colour) = chosen {
        colours.used(colour);
    }
    chosen
}

fn grid(
    ui: &mut egui::Ui,
    current: Option<[u8; 3]>,
    colours: &mut Colours,
    lang: Lang,
) -> Option<[u8; 3]> {
    let say = |control: Control| Message::Control(control).say(lang);
    let mut chosen = None;
    ui.spacing_mut().item_spacing = egui::vec2(2.0, 2.0);
    let mut row = |ui: &mut egui::Ui, row: &[[u8; 3]]| {
        ui.horizontal(|ui| {
            for colour in row {
                if swatch(ui, *colour, current == Some(*colour)).clicked() {
                    chosen = Some(*colour);
                }
            }
        });
    };
    row(ui, &GREYS);
    ui.add_space(4.0);
    row(ui, &HUES);
    ui.add_space(4.0);
    for share in SHADES {
        let shades: Vec<[u8; 3]> = HUES.iter().map(|hue| shade(*hue, share)).collect();
        row(ui, &shades);
    }
    if !colours.recent.is_empty() {
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(say(Control::RecentColours))
                .small()
                .weak(),
        );
        let recent = colours.recent.clone();
        row(ui, &recent);
    }
    ui.add_space(6.0);
    ui.separator();
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
        Icon::CustomColour.draw_tinted(ui.painter(), rect, ui.visuals().text_color(), true);
        ui.label(
            egui::RichText::new(say(Control::CustomColour))
                .small()
                .weak(),
        );
    });
    ui.spacing_mut().slider_width = GRID_WIDTH;
    egui::widgets::color_picker::color_picker_color32(
        ui,
        &mut colours.mixing,
        egui::widgets::color_picker::Alpha::Opaque,
    );
    let mixed = [colours.mixing.r(), colours.mixing.g(), colours.mixing.b()];
    ui.horizontal(|ui| {
        let mut typed = colours.typed.clone().unwrap_or_else(|| hex(mixed));
        let field = ui.add(egui::TextEdit::singleline(&mut typed).desired_width(72.0));
        if field.changed() {
            if let Some([r, g, b]) = parse_hex(&typed) {
                colours.mixing = egui::Color32::from_rgb(r, g, b);
            }
            colours.typed = Some(typed);
        } else if !field.has_focus() {
            colours.typed = None;
        }
        let (preview, _) = ui.allocate_exact_size(egui::vec2(28.0, 20.0), egui::Sense::hover());
        ui.painter().rect_filled(preview, 4.0, colours.mixing);
        ui.painter().rect_stroke(
            preview,
            4.0,
            ui.visuals().widgets.noninteractive.bg_stroke,
            egui::StrokeKind::Inside,
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let use_it = egui::Button::new(
                egui::RichText::new(say(Control::UseThisColour))
                    .color(ui.visuals().selection.stroke.color),
            )
            .fill(ui.visuals().selection.bg_fill.gamma_multiply(0.4))
            .corner_radius(5.0);
            if ui.add(use_it).clicked() {
                chosen = Some(mixed);
            }
        });
    });
    chosen
}

fn swatch(ui: &mut egui::Ui, [r, g, b]: [u8; 3], now: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(SWATCH, SWATCH), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = egui::Color32::from_rgb(r, g, b);
        let inner = rect.shrink(1.0);
        ui.painter().rect_filled(inner, 3.0, fill);
        ui.painter().rect_stroke(
            inner,
            3.0,
            egui::Stroke::new(1.0, egui::Color32::from_black_alpha(40)),
            egui::StrokeKind::Inside,
        );
        if now || response.hovered() {
            let ring = if now {
                ui.visuals().selection.stroke.color
            } else {
                ui.visuals().widgets.hovered.fg_stroke.color
            };
            ui.painter().rect_stroke(
                rect,
                4.0,
                egui::Stroke::new(2.0, ring),
                egui::StrokeKind::Inside,
            );
        }
    }
    response.on_hover_text(hex([r, g, b]))
}

#[cfg(test)]
mod tests {
    use super::{Colours, hex, parse_hex, shade};

    #[test]
    fn a_hex_code_reads_back_as_the_colour_it_was_written_from() {
        for colour in [[0, 0, 0], [255, 255, 255], [18, 52, 86], [171, 205, 239]] {
            assert_eq!(parse_hex(&hex(colour)), Some(colour));
        }
        assert_eq!(parse_hex("f80"), Some([255, 136, 0]), "the short form");
        assert_eq!(
            parse_hex(" 00ff7F "),
            Some([0, 255, 127]),
            "no `#`, any case"
        );
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("#GG0000"), None);
    }

    #[test]
    fn shades_go_towards_white_and_towards_black() {
        assert_eq!(shade([255, 0, 0], 0.5), [255, 128, 128]);
        assert_eq!(shade([200, 100, 0], -0.5), [100, 50, 0]);
        assert_eq!(shade([10, 20, 30], 0.0), [10, 20, 30]);
    }

    #[test]
    fn colours_used_lately_keep_one_row_newest_first() {
        let mut colours = Colours::default();
        for step in 0..14_u8 {
            colours.used([step, 0, 0]);
        }
        colours.used([12, 0, 0]);
        assert_eq!(colours.recent.len(), 10);
        assert_eq!(colours.recent[0], [12, 0, 0]);
        assert_eq!(colours.recent[1], [13, 0, 0]);
        assert_eq!(
            colours
                .recent
                .iter()
                .filter(|had| **had == [12, 0, 0])
                .count(),
            1
        );
    }
}
