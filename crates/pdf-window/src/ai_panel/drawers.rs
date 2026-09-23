use eframe::egui;
use egui::containers::menu::SubMenuButton;
use egui::text::{LayoutJob, TextFormat};
use pdf_agent::connect::Effort;
use pdf_app::ai_permission::Mode;
use pdf_app::wording::{Lang, Message};

use super::{AiState, MAX_CONTEXT, Provider, effort_said, mode_means, mode_said, shorten_model};

const DRAWER_WIDE: f32 = 250.0;

const MODES: [Mode; 3] = [Mode::AskBeforeChanges, Mode::DoIt, Mode::Free];

const EFFORTS: [Effort; 5] = [
    Effort::Off,
    Effort::None,
    Effort::Low,
    Effort::Medium,
    Effort::High,
];

const TOWARDS_THE_DRAWER: &str = "\u{23f4}";

fn drawer<'a>(name: &str, value: String) -> SubMenuButton<'a> {
    SubMenuButton::from_button(
        egui::Button::new(format!("{TOWARDS_THE_DRAWER}  {name}"))
            .right_text(egui::RichText::new(value).weak())
            .frame(false)
            .min_size(egui::vec2(DRAWER_WIDE, 0.0)),
    )
}

fn choice(
    ui: &mut egui::Ui,
    (name, what): (&str, &str),
    chosen: bool,
    warn: bool,
) -> egui::Response {
    let style = ui.style();
    let body = egui::TextStyle::Body.resolve(style);
    let small = egui::TextStyle::Small.resolve(style);
    let visuals = &style.visuals;
    let mut job = LayoutJob::default();
    job.append(
        name,
        0.0,
        TextFormat {
            font_id: body,
            color: if warn {
                visuals.warn_fg_color
            } else {
                visuals.text_color()
            },
            ..TextFormat::default()
        },
    );
    if !what.is_empty() {
        job.append(
            &format!("\n{what}"),
            0.0,
            TextFormat {
                font_id: small,
                color: visuals.weak_text_color(),
                ..TextFormat::default()
            },
        );
    }
    job.wrap.max_width = DRAWER_WIDE + 40.0;
    let mut button = egui::Button::new(job)
        .frame(false)
        .min_size(egui::vec2(DRAWER_WIDE + 40.0, 0.0));
    if chosen {
        button = button.right_text("\u{2713}");
    }
    ui.add(button)
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).small().weak());
}

impl AiState {
    pub(super) fn model_label(&self, lang: Lang) -> String {
        let say = |message: Message| message.say(lang);
        let named = if self.model.is_empty() {
            say(Message::AiNoModelChosen)
        } else {
            shorten_model(&self.model)
        };
        if self.effort == Effort::Off {
            return format!("{named}  \u{2304}");
        }
        let level = say(effort_said(self.effort));
        let level_alone = level.rsplit(": ").next().unwrap_or(&level).to_owned();
        format!("{named} \u{00b7} {level_alone}  \u{2304}")
    }

    pub(super) fn mode_label(&self, lang: Lang) -> String {
        format!("{}  \u{2304}", mode_said(self.mode).say(lang))
    }

    pub(super) fn the_model_button(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, lang: Lang) {
        let say = |message: Message| message.say(lang);
        let shown = self.model_label(lang);
        ui.menu_button(egui::RichText::new(shown).small(), |ui| {
            ui.set_min_width(DRAWER_WIDE);
            self.provider_drawer(ui, ctx, lang);
            self.model_drawer(ui, lang);
            self.effort_drawer(ui, lang);
            ui.separator();
            if ui
                .add(
                    egui::Button::new(say(Message::AiResetToDefault))
                        .right_text(egui::RichText::new("\u{21ba}").weak())
                        .frame(false)
                        .min_size(egui::vec2(DRAWER_WIDE, 0.0)),
                )
                .clicked()
            {
                self.effort = Effort::Off;
                self.remember();
                ui.close();
            }
        })
        .response
        .on_hover_text(if self.model.is_empty() {
            say(Message::AiNoModelChosen)
        } else {
            self.model.clone()
        });
    }

    fn provider_drawer(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, lang: Lang) {
        let say = |message: Message| message.say(lang);
        drawer(&say(Message::AiProvider), self.provider.name().to_owned()).ui(ui, |ui| {
            ui.set_min_width(DRAWER_WIDE * 0.8);
            let mut chosen = self.provider;
            for provider in Provider::ALL {
                if choice(ui, (provider.name(), ""), provider == self.provider, false).clicked() {
                    chosen = provider;
                }
            }
            if chosen != self.provider && !self.busy() {
                self.provider = chosen;
                self.base_url = chosen.base_url().into();
                self.model.clear();
                self.invalidate();
                self.settings_open = self.key_missing();
                self.remember();
                self.models_if_possible(ctx);
                ui.close();
            }
        });
    }

    fn model_drawer(&mut self, ui: &mut egui::Ui, lang: Lang) {
        let say = |message: Message| message.say(lang);
        let value = if self.model.is_empty() {
            "\u{2014}".to_owned()
        } else {
            shorten_model(&self.model)
        };
        drawer(&say(Message::AiModel), value).ui(ui, |ui| {
            ui.set_min_width(DRAWER_WIDE);
            if self.models.is_empty() {
                ui.weak(if self.key_missing() {
                    say(Message::AiKeyNeeded)
                } else {
                    say(Message::AiFindModels)
                });
                return;
            }
            let mut picked = None;
            egui::ScrollArea::vertical()
                .id_salt("ai-model-drawer")
                .max_height(320.0)
                .show(ui, |ui| {
                    for model in &self.models {
                        if choice(ui, (&model.id, ""), self.model == model.id, false).clicked() {
                            picked = Some(model.id.clone());
                        }
                    }
                });
            if let Some(model) = picked {
                self.model = model;
                self.notice = None;
                self.connected = true;
                self.settings_open = false;
                self.remember();
                ui.close();
            }
        });
    }

    fn effort_drawer(&mut self, ui: &mut egui::Ui, lang: Lang) {
        let say = |message: Message| message.say(lang);
        let level = say(effort_said(self.effort));
        let value = level.rsplit(": ").next().unwrap_or(&level).to_owned();
        drawer(&say(Message::AiEffort), value).ui(ui, |ui| {
            ui.set_min_width(DRAWER_WIDE);
            let mut effort = self.effort;
            for one in EFFORTS {
                let name = say(effort_said(one));
                let name = capitalised(name.rsplit(": ").next().unwrap_or(&name));
                if choice(
                    ui,
                    (&name, &say(effort_means(one))),
                    one == self.effort,
                    false,
                )
                .clicked()
                {
                    effort = one;
                }
            }
            if effort != self.effort {
                self.effort = effort;
                self.remember();
                ui.close();
            }
        });
    }

    pub(super) fn what_it_may_do(&mut self, ui: &mut egui::Ui, lang: Lang) {
        let say = |message: Message| message.say(lang);
        let free = self.mode == Mode::Free;
        let label = egui::RichText::new(self.mode_label(lang)).small();
        ui.menu_button(
            if free {
                label.color(ui.visuals().warn_fg_color)
            } else {
                label
            },
            |ui| {
                ui.set_min_width(DRAWER_WIDE + 40.0);
                heading(ui, &say(Message::AiMode));
                for one in MODES {
                    let clicked = choice(
                        ui,
                        (&say(mode_said(one)), &say(mode_means(one))),
                        one == self.mode,
                        one == Mode::Free,
                    )
                    .clicked();
                    if clicked {
                        if one == Mode::Free && self.mode != Mode::Free {
                            self.confirming_free = true;
                        } else if one != self.mode {
                            self.mode = one;
                            self.remember();
                        }
                        ui.close();
                    }
                }
            },
        )
        .response
        .on_hover_text(say(mode_means(self.mode)));
    }

    pub(super) fn confirm_full_access(&mut self, ctx: &egui::Context, lang: Lang) {
        if !self.confirming_free {
            return;
        }
        let say = |message: Message| message.say(lang);
        let mut answer = None;
        egui::Modal::new(egui::Id::new("ai-full-access")).show(ctx, |ui| {
            ui.set_max_width(380.0);
            ui.label(
                egui::RichText::new(format!("\u{26a0} {}", say(Message::AiFullAccessAsk)))
                    .strong()
                    .size(egui::TextStyle::Body.resolve(ui.style()).size * 1.15),
            );
            ui.add_space(6.0);
            ui.label(say(Message::AiFullAccessMeans));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new(
                            egui::RichText::new(say(Message::AiFullAccessConfirm))
                                .color(ui.visuals().warn_fg_color),
                        ))
                        .clicked()
                    {
                        answer = Some(true);
                    }
                    if ui
                        .button(say(Message::Home(pdf_app::wording::Home::Cancel)))
                        .clicked()
                    {
                        answer = Some(false);
                    }
                });
            });
        });
        match answer {
            Some(true) => {
                self.mode = Mode::Free;
                self.remember();
                self.confirming_free = false;
            }
            Some(false) => self.confirming_free = false,
            None => {}
        }
    }

    pub(super) fn the_attach_button(&mut self, ui: &mut egui::Ui, lang: Lang) -> bool {
        let say = |message: Message| message.say(lang);
        let mut attach = false;
        ui.menu_button("+", |ui| {
            ui.set_min_width(DRAWER_WIDE + 40.0);
            heading(ui, &say(Message::AiAdd));
            if choice(
                ui,
                (
                    &say(Message::AiAttachFiles),
                    &say(Message::AiAttachFilesMeans),
                ),
                false,
                false,
            )
            .clicked()
                && !self.busy()
            {
                attach = true;
                ui.close();
            }
            if choice(
                ui,
                (
                    &say(Message::AiSendThePage),
                    &say(Message::AiIncludeContext {
                        characters: MAX_CONTEXT,
                    }),
                ),
                self.include_context,
                false,
            )
            .clicked()
            {
                self.include_context = !self.include_context;
            }
        })
        .response
        .on_hover_text(say(Message::AiAttach));
        attach
    }
}

const fn effort_means(effort: Effort) -> Message {
    match effort {
        Effort::Off => Message::AiEffortOffMeans,
        Effort::None => Message::AiEffortNoneMeans,
        Effort::Low => Message::AiEffortLowMeans,
        Effort::Medium => Message::AiEffortMediumMeans,
        Effort::High => Message::AiEffortHighMeans,
    }
}

fn capitalised(word: &str) -> String {
    let mut letters = word.chars();
    letters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(letters).collect()
    })
}
