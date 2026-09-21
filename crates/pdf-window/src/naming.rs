use eframe::egui;

use pdf_app::wording::{Lang, Message};
use pdf_edit::destination::{Naming, Spot};
use pdf_edit::link::Arrival;

use crate::link_tool::{DEFAULT_PERCENT, percent_of, same_choice, zoom_word};
use crate::window_state::{NamingDraft, Window};

const PANEL_WIDTH: f32 = 360.0;

const ARRIVALS: [Arrival; 7] = [
    Arrival::InheritZoom,
    Arrival::FitPage,
    Arrival::FitWidth,
    Arrival::FitHeight,
    Arrival::FitVisible,
    Arrival::ActualSize,
    Arrival::Percent(DEFAULT_PERCENT),
];

impl Window {
    pub(crate) fn toggle_the_named_places(&mut self) {
        self.naming_draft = if self.naming_draft.is_some() {
            None
        } else {
            Some(NamingDraft {
                percent: percent_of(Arrival::InheritZoom),
                ..NamingDraft::default()
            })
        };
    }

    pub(crate) fn named_places_panel(&mut self, ctx: &egui::Context) {
        if self.naming_draft.is_none() || !self.has_document() {
            return;
        }
        let lang = self.lang;
        let places = self.editor.named_places();
        let page = self.focus;
        let busy = self.editor.is_busy();
        let Some(draft) = self.naming_draft.as_mut() else {
            return;
        };
        let mut asked: Option<Naming> = None;
        let mut close = false;
        egui::Window::new(Message::NamedPlaces.say(lang))
            .collapsible(false)
            .resizable(false)
            .default_width(PANEL_WIDTH)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(Message::NamedPlacesWhy.say(lang))
                        .size(11.0)
                        .color(ui.visuals().weak_text_color()),
                );
                ui.separator();
                asked = listed(ui, draft, (&places, busy), lang);
                ui.separator();
                if asked.is_none() {
                    asked = naming_this_page(ui, draft, (page, busy), lang);
                }
                ui.separator();
                close = ui.button(Message::Close.say(lang)).clicked();
            });
        if close {
            self.naming_draft = None;
            return;
        }
        if let Some(change) = asked {
            self.change_the_naming(change);
        }
    }

    fn change_the_naming(&mut self, change: Naming) {
        let job = self.editor.begin_change_naming(self.focus, change);
        if job.is_none() {
            self.editor.say(Message::AnotherEditIsRunning);
            return;
        }
        if let Some(draft) = self.naming_draft.as_mut() {
            draft.renaming = None;
            draft.name.clear();
        }
        self.send(job);
    }
}

fn listed(
    ui: &mut egui::Ui,
    draft: &mut NamingDraft,
    (places, busy): (&[Spot], bool),
    lang: Lang,
) -> Option<Naming> {
    if places.is_empty() {
        ui.label(Message::NoNamedPlacesYet.say(lang));
        return None;
    }
    let mut asked = None;
    egui::ScrollArea::vertical()
        .max_height(220.0)
        .show(ui, |ui| {
            for place in places {
                ui.horizontal(|ui| {
                    if let Some((from, to)) = draft.renaming.as_mut()
                        && *from == place.name
                    {
                        let box_ = ui.add(egui::TextEdit::singleline(to).desired_width(150.0));
                        box_.request_focus();
                        let entered = box_.lost_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter));
                        if ui.button(Message::Apply.say(lang)).clicked() || entered {
                            asked = Some(Naming::Rename {
                                from: from.clone(),
                                to: to.clone(),
                            });
                        }
                        if ui.button(Message::Close.say(lang)).clicked() {
                            asked = None;
                            draft.renaming = None;
                        }
                        return;
                    }
                    ui.label(&place.name);
                    ui.label(
                        egui::RichText::new(format!(
                            "{} {} · {}",
                            Message::LinkPageNumber.say(lang),
                            place.page + 1,
                            zoom_word(place.arrival).say(lang)
                        ))
                        .size(11.0)
                        .color(ui.visuals().weak_text_color()),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                !busy,
                                egui::Button::new(Message::RemoveThePlace.say(lang)),
                            )
                            .on_hover_text(Message::RemoveThePlaceWhy.say(lang))
                            .clicked()
                        {
                            asked = Some(Naming::Remove {
                                name: place.name.clone(),
                            });
                        }
                        if ui
                            .add_enabled(
                                !busy,
                                egui::Button::new(Message::RenameThePlace.say(lang)),
                            )
                            .clicked()
                        {
                            draft.renaming = Some((place.name.clone(), place.name.clone()));
                        }
                    });
                });
            }
        });
    asked
}

fn naming_this_page(
    ui: &mut egui::Ui,
    draft: &mut NamingDraft,
    (page, busy): (usize, bool),
    lang: Lang,
) -> Option<Naming> {
    let mut asked = None;
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut draft.name)
                .hint_text(Message::NameHint.say(lang))
                .desired_width(150.0),
        );
        ui.label(format!(
            "{} {}",
            Message::LinkPageNumber.say(lang),
            page + 1
        ));
    });
    ui.horizontal(|ui| {
        ui.label(Message::LinkZoom.say(lang));
        egui::ComboBox::from_id_salt("named place zoom")
            .selected_text(zoom_word(draft.arrival).say(lang))
            .show_ui(ui, |ui| {
                for choice in ARRIVALS {
                    if ui
                        .selectable_label(
                            same_choice(draft.arrival, choice),
                            zoom_word(choice).say(lang),
                        )
                        .clicked()
                    {
                        draft.arrival = choice;
                        draft.percent = percent_of(choice);
                    }
                }
            });
        if matches!(draft.arrival, Arrival::Percent(_)) {
            ui.add(egui::TextEdit::singleline(&mut draft.percent).desired_width(48.0));
            ui.label("%");
        }
        if ui
            .add_enabled(!busy, egui::Button::new(Message::NameThisPage.say(lang)))
            .clicked()
        {
            asked = asked_for(draft, page);
        }
    });
    asked
}

fn asked_for(draft: &NamingDraft, page: usize) -> Option<Naming> {
    let name = draft.name.trim();
    if name.is_empty() {
        return None;
    }
    let arrival = match draft.arrival {
        Arrival::Percent(_) => {
            let percent: f64 = draft
                .percent
                .trim()
                .trim_end_matches('%')
                .trim()
                .parse()
                .ok()?;
            if !percent.is_finite() || !(1.0..=6400.0).contains(&percent) {
                return None;
            }
            Arrival::Percent(percent)
        }
        other => other,
    };
    Some(Naming::Name {
        name: name.to_owned(),
        page,
        arrival,
    })
}

#[cfg(test)]
mod tests {
    use super::asked_for;
    use crate::window_state::NamingDraft;
    use pdf_edit::destination::Naming;
    use pdf_edit::link::Arrival;

    fn drafted(name: &str, arrival: Arrival, percent: &str) -> NamingDraft {
        NamingDraft {
            name: name.to_owned(),
            arrival,
            percent: percent.to_owned(),
            renaming: None,
        }
    }

    #[test]
    fn a_name_is_asked_for_the_page_on_screen() {
        assert_eq!(
            asked_for(&drafted(" chapter two ", Arrival::FitPage, ""), 4),
            Some(Naming::Name {
                name: "chapter two".to_owned(),
                page: 4,
                arrival: Arrival::FitPage,
            })
        );
    }

    #[test]
    fn a_typed_zoom_is_read_and_a_wrong_one_asks_for_nothing() {
        assert_eq!(
            asked_for(&drafted("start", Arrival::Percent(100.0), "150 %"), 0),
            Some(Naming::Name {
                name: "start".to_owned(),
                page: 0,
                arrival: Arrival::Percent(150.0),
            })
        );
        assert_eq!(
            asked_for(&drafted("start", Arrival::Percent(100.0), "huge"), 0),
            None
        );
        assert_eq!(
            asked_for(&drafted("start", Arrival::Percent(100.0), "9000"), 0),
            None
        );
    }

    #[test]
    fn a_place_with_no_name_is_not_named() {
        assert_eq!(asked_for(&drafted("   ", Arrival::FitPage, ""), 0), None);
    }
}
