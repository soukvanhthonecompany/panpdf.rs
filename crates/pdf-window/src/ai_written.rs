use eframe::egui;
use pdf_agent::markup::tree::{Align, Block, Inline, List};

use pdf_app::wording::{Lang, Message};

const HEADINGS: [f32; 6] = [1.45, 1.25, 1.12, 1.05, 1.0, 1.0];

const ABOVE_A_HEADING: f32 = 0.85;
const BETWEEN_BLOCKS: f32 = 0.45;

const INDENT: f32 = 1.15;
const QUOTE_INDENT: f32 = 0.9;

pub(crate) fn written(ui: &mut egui::Ui, text: &str, salt: &str, lang: Lang) {
    let text = pdf_agent::markup::laying_out::mathematics_as_text(text);
    let blocks = pdf_agent::markup::blocks(&text);
    draw_blocks(ui, &blocks, &mut Place::new(salt, lang));
}

struct Place<'a> {
    salt: &'a str,
    lang: Lang,
    fences: usize,
    started: bool,
}

impl<'a> Place<'a> {
    fn new(salt: &'a str, lang: Lang) -> Self {
        Self {
            salt,
            lang,
            fences: 0,
            started: false,
        }
    }

    fn space_above(&mut self, ui: &mut egui::Ui, share: f32) {
        if self.started {
            ui.add_space(body(ui) * share);
        }
        self.started = true;
    }
}

fn body(ui: &egui::Ui) -> f32 {
    egui::TextStyle::Body.resolve(ui.style()).size
}

fn draw_blocks(ui: &mut egui::Ui, blocks: &[Block], place: &mut Place) {
    for block in blocks {
        draw_block(ui, block, place);
    }
}

fn draw_block(ui: &mut egui::Ui, block: &Block, place: &mut Place) {
    match block {
        Block::Heading { level, inlines } => {
            place.space_above(ui, ABOVE_A_HEADING);
            let size = HEADINGS
                .get(usize::from(level.saturating_sub(1)))
                .copied()
                .unwrap_or(1.0)
                * body(ui);
            let style = Style {
                size,
                strong: true,
                ..Style::plain(ui)
            };
            lines_of(ui, inlines, style, place.lang);
        }
        Block::Paragraph { inlines } => {
            place.space_above(ui, BETWEEN_BLOCKS);
            lines_of(ui, inlines, Style::plain(ui), place.lang);
        }
        Block::Code { info, text } => {
            place.space_above(ui, BETWEEN_BLOCKS);
            place.fences += 1;
            fenced(ui, (info, text), (place.salt, place.fences), place.lang);
        }
        Block::Break => {
            place.space_above(ui, BETWEEN_BLOCKS);
            ui.separator();
        }
        Block::Quote { blocks } => {
            place.space_above(ui, BETWEEN_BLOCKS);
            quoted(ui, blocks, place);
        }
        Block::List(list) => {
            place.space_above(ui, BETWEEN_BLOCKS);
            listed(ui, list, place);
        }
        Block::Table { head, rows, align } => {
            place.space_above(ui, BETWEEN_BLOCKS);
            table(ui, (head, rows, align), place);
        }
    }
}

fn quoted(ui: &mut egui::Ui, blocks: &[Block], place: &mut Place) {
    let colour = ui.visuals().weak_text_color().gamma_multiply(0.6);
    let step = body(ui);
    ui.horizontal(|ui| {
        let (bar, _) = ui.allocate_exact_size(
            egui::vec2(2.0, ui.available_height().max(step)),
            egui::Sense::hover(),
        );
        ui.painter().rect_filled(bar, 1.0, colour);
        ui.add_space(step * QUOTE_INDENT);
        ui.vertical(|ui| {
            let mut inside = Place {
                salt: place.salt,
                lang: place.lang,
                fences: place.fences,
                started: false,
            };
            draw_blocks(ui, blocks, &mut inside);
            place.fences = inside.fences;
        });
    });
}

fn listed(ui: &mut egui::Ui, list: &List, place: &mut Place) {
    let step = body(ui);
    for (at, item) in list.items.iter().enumerate() {
        if list.loose && at > 0 {
            ui.add_space(step * BETWEEN_BLOCKS);
        }
        let mark = match list.first {
            None => "\u{2022}".to_owned(),
            Some(first) => format!("{}.", first.saturating_add(at as u64)),
        };
        ui.horizontal_top(|ui| {
            ui.add_space(step * INDENT * 0.5);
            ui.allocate_ui_with_layout(
                egui::vec2(step * 1.4, step),
                egui::Layout::right_to_left(egui::Align::TOP),
                |ui| {
                    ui.add_space(step * 0.35);
                    ui.weak(egui::RichText::new(mark).size(step));
                },
            );
            ui.vertical(|ui| {
                let mut inside = Place {
                    salt: place.salt,
                    lang: place.lang,
                    fences: place.fences,
                    started: false,
                };
                draw_blocks(ui, item, &mut inside);
                place.fences = inside.fences;
            });
        });
    }
}

type Table<'a> = (&'a [Vec<Inline>], &'a [Vec<Vec<Inline>>], &'a [Align]);

fn table(ui: &mut egui::Ui, (head, rows, align): Table<'_>, place: &mut Place) {
    let salt = format!("{}-table-{}", place.salt, place.fences);
    place.fences += 1;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        egui::Grid::new(salt)
            .striped(true)
            .num_columns(head.len().max(1))
            .show(ui, |ui| {
                for (at, cell) in head.iter().enumerate() {
                    set_cell(ui, cell, align.get(at).copied(), true, place.lang);
                }
                ui.end_row();
                for row in rows {
                    for (at, cell) in row.iter().enumerate() {
                        set_cell(ui, cell, align.get(at).copied(), false, place.lang);
                    }
                    ui.end_row();
                }
            });
    });
}

fn set_cell(ui: &mut egui::Ui, cell: &[Inline], align: Option<Align>, heading: bool, lang: Lang) {
    let layout = match align.unwrap_or_default() {
        Align::Start => egui::Layout::left_to_right(egui::Align::Center),
        Align::Middle => egui::Layout::top_down(egui::Align::Center),
        Align::End => egui::Layout::right_to_left(egui::Align::Center),
    };
    ui.with_layout(layout, |ui| {
        let style = Style {
            strong: heading,
            ..Style::plain(ui)
        };
        run(ui, cell, style, lang);
    });
}

fn fenced(ui: &mut egui::Ui, (info, text): (&str, &str), (salt, nth): (&str, usize), lang: Lang) {
    let visuals = ui.visuals();
    let frame = egui::Frame::group(ui.style())
        .fill(visuals.extreme_bg_color)
        .stroke(egui::Stroke::new(
            1.0,
            visuals.widgets.noninteractive.bg_stroke.color,
        ))
        .inner_margin(egui::Margin::symmetric(8, 6));
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let named = info.split_whitespace().next().unwrap_or_default();
            ui.weak(egui::RichText::new(named).small());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if crate::format::quiet_icon_button(
                    ui,
                    crate::icons::Icon::Copy,
                    &Message::AiCopyCode.say(lang),
                )
                .clicked()
                {
                    ui.ctx().copy_text(text.to_owned());
                }
            });
        });
        egui::ScrollArea::horizontal()
            .id_salt(format!("{salt}-code-{nth}"))
            .show(ui, |ui| {
                ui.add(
                    egui::Label::new(egui::RichText::new(text.trim_end_matches('\n')).monospace())
                        .selectable(true)
                        .wrap_mode(egui::TextWrapMode::Extend),
                );
            });
    });
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "one per inline mark, and the marks combine"
)]
#[derive(Clone, Copy)]
struct Style {
    size: f32,
    strong: bool,
    italic: bool,
    strike: bool,
    code: bool,
    link: Option<egui::Color32>,
}

impl Style {
    fn plain(ui: &egui::Ui) -> Self {
        Self {
            size: body(ui),
            strong: false,
            italic: false,
            strike: false,
            code: false,
            link: None,
        }
    }

    fn apply(self, text: &str) -> egui::RichText {
        let mut rich = egui::RichText::new(text).size(self.size);
        if self.strong {
            rich = rich.strong();
        }
        if self.italic {
            rich = rich.italics();
        }
        if self.strike {
            rich = rich.strikethrough();
        }
        if self.code {
            rich = rich.code();
        }
        if let Some(colour) = self.link {
            rich = rich.color(colour).underline();
        }
        rich
    }
}

fn lines_of(ui: &mut egui::Ui, inlines: &[Inline], style: Style, lang: Lang) {
    let mut line: Vec<Inline> = Vec::new();
    for piece in inlines {
        if matches!(piece, Inline::Hard) {
            wrapped(ui, &line, style, lang);
            line.clear();
        } else {
            line.push(piece.clone());
        }
    }
    wrapped(ui, &line, style, lang);
}

fn wrapped(ui: &mut egui::Ui, inlines: &[Inline], style: Style, lang: Lang) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        run(ui, inlines, style, lang);
    });
}

fn run(ui: &mut egui::Ui, inlines: &[Inline], style: Style, lang: Lang) {
    for piece in inlines {
        match piece {
            Inline::Text(text) => {
                ui.add(egui::Label::new(style.apply(text)).selectable(true).wrap());
            }
            Inline::Soft | Inline::Hard => {
                ui.add(egui::Label::new(style.apply(" ")).selectable(false));
            }
            Inline::Code(text) => {
                ui.add(
                    egui::Label::new(
                        Style {
                            code: true,
                            ..style
                        }
                        .apply(text),
                    )
                    .selectable(true)
                    .wrap(),
                );
            }
            Inline::Emphasis(inside) => run(
                ui,
                inside,
                Style {
                    italic: true,
                    ..style
                },
                lang,
            ),
            Inline::Strong(inside) => run(
                ui,
                inside,
                Style {
                    strong: true,
                    ..style
                },
                lang,
            ),
            Inline::Strike(inside) => run(
                ui,
                inside,
                Style {
                    strike: true,
                    ..style
                },
                lang,
            ),
            Inline::Link { to, text } => {
                let colour = ui.visuals().hyperlink_color;
                let style = Style {
                    link: Some(colour),
                    ..style
                };
                let from = ui.cursor().min;
                run(ui, text, style, lang);
                let to = to.clone();
                let rect = egui::Rect::from_min_max(from, ui.cursor().min);
                let clicked = ui
                    .interact(
                        rect,
                        egui::Id::new(&to).with(from.x.to_bits()),
                        egui::Sense::click(),
                    )
                    .on_hover_text(&to)
                    .clicked();
                if clicked && web_address(&to) {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(to));
                }
            }
            Inline::Image { at, text } => {
                let shown = if text.is_empty() {
                    Message::AiAPicture.say(lang)
                } else {
                    pdf_agent::markup::tree::plain(text)
                };
                ui.add(
                    egui::Label::new(
                        Style {
                            italic: true,
                            ..style
                        }
                        .apply(&format!("\u{1f5bc} {shown}")),
                    )
                    .selectable(true)
                    .wrap(),
                )
                .on_hover_text(at);
            }
        }
    }
}

fn web_address(to: &str) -> bool {
    let lowered = to.trim().to_ascii_lowercase();
    lowered.starts_with("https://") || lowered.starts_with("http://")
}

#[cfg(test)]
mod tests {
    use super::web_address;

    #[test]
    fn only_a_page_on_the_web_is_opened() {
        assert!(web_address("https://example.org/a"));
        assert!(web_address("HTTP://example.org"));
        assert!(web_address("  https://example.org  "));
        assert!(!web_address("file:///etc/shadow"));
        assert!(!web_address("javascript:alert(1)"));
        assert!(!web_address("/home/someone/.ssh/id_rsa"));
        assert!(!web_address("mailto:somebody@example.org"));
        assert!(!web_address(""));
    }
}
