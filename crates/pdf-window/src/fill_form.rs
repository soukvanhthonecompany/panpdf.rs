use eframe::egui;

use pdf_app::document::FieldBox;
use pdf_app::wording::Message;
use pdf_edit::form::{FieldKind, FieldValue};

use crate::canvas::box_on_screen;
use crate::window_state::{Filling, Laid, Tool, Window};

impl Window {
    pub(crate) fn field_under(&mut self, page: usize, (x, y): (f64, f64)) -> Option<FieldBox> {
        if self.tool != Tool::Select {
            return None;
        }
        self.editor
            .fields_on(page)
            .iter()
            .rev()
            .find(|found| {
                let [x0, y0, x1, y1] = found.pixels;
                (x0..=x1).contains(&x) && (y0..=y1).contains(&y)
            })
            .cloned()
    }

    pub(crate) fn clicked_a_field(
        &mut self,
        ctx: &egui::Context,
        response: &egui::Response,
    ) -> bool {
        if !response.clicked() {
            return false;
        }
        if self.tool == Tool::Link {
            if !self.editor.is_busy()
                && let Some(at) = response.interact_pointer_pos()
                && let Some((page, point)) = self.page_point(at)
            {
                let adding = ctx.input(|input| input.modifiers.shift || input.modifiers.command);
                self.link_tool_clicked(page, point, adding);
            }
            return true;
        }
        if self.tool == Tool::Form {
            if !self.editor.is_busy()
                && let Some(at) = response.interact_pointer_pos()
                && let Some((page, point)) = self.page_point(at)
            {
                let adding = ctx.input(|input| input.modifiers.shift || input.modifiers.command);
                self.form_tool_clicked(page, point, adding);
            }
            return true;
        }
        if !self.editor.is_busy()
            && !ctx.input(|input| input.modifiers.shift)
            && let Some(at) = response.interact_pointer_pos()
            && let Some((page, point)) = self.page_point(at)
            && let Some(found) = self.field_under(page, point)
            && self.answer_the_field(page, found)
        {
            self.context = None;
            return true;
        }
        self.finish_the_field();
        false
    }

    pub(crate) fn answer_the_field(&mut self, page: usize, found: FieldBox) -> bool {
        self.finish_the_field();
        let field = &found.field;
        if field.kind == FieldKind::Push {
            match field.link.clone() {
                Some(address) => self.open_address(address),
                None => self.editor.say(Message::FieldNoLink),
            }
            return true;
        }
        if field.kind == FieldKind::Signature {
            self.editor.say(Message::SignatureFieldsAreNotSignedYet);
            return true;
        }
        if field.read_only {
            self.editor.say(Message::FieldIsReadOnly);
            return true;
        }
        match field.kind {
            FieldKind::Checkbox | FieldKind::Radio => {
                let Some(state) = field.states.first().cloned() else {
                    return false;
                };
                let value = if !field.is_on() {
                    FieldValue::State(state)
                } else if field.kind == FieldKind::Checkbox {
                    FieldValue::Empty
                } else {
                    return true;
                };
                self.write_the_field(page, &found, value);
            }
            FieldKind::Text | FieldKind::Combo | FieldKind::List => {
                self.filling = Some(Filling {
                    page,
                    text: field.value.shown().to_owned(),
                    field: found,
                    take_the_keyboard: true,
                });
                self.editor.say(Message::HowToFinishAField);
            }
            FieldKind::Push | FieldKind::Signature => {}
        }
        true
    }

    pub(crate) fn finish_the_field(&mut self) {
        let Some(mut filling) = self.filling.take() else {
            return;
        };
        if filling.text == filling.field.field.value.shown() {
            return;
        }
        if let Some(format) = filling.field.field.date_format.clone()
            && !filling.text.is_empty()
            && !pdf_app::dates::reads_as_a_date(&format, &filling.text)
        {
            self.editor.say(Message::FieldNotADate);
            filling.take_the_keyboard = true;
            self.filling = Some(filling);
            return;
        }
        let value = if filling.text.is_empty() {
            FieldValue::Empty
        } else {
            FieldValue::Text(filling.text.clone())
        };
        if self.editor.is_busy() {
            self.editor.say(Message::AnotherEditIsRunning);
            self.filling = Some(filling);
            return;
        }
        self.write_the_field(filling.page, &filling.field, value);
    }

    fn write_the_field(&mut self, page: usize, found: &FieldBox, value: FieldValue) {
        let job = self
            .editor
            .begin_fill_field(page, found.field.widget, value);
        if job.is_none() {
            self.editor.say(Message::AnotherEditIsRunning);
            return;
        }
        self.send(job);
    }

    fn go_to_the_next_field(&mut self, backwards: bool) {
        let Some(filling) = self.filling.as_ref() else {
            return;
        };
        let page = filling.page;
        let widget = filling.field.field.widget;
        let fields = self.editor.fields_on(page);
        let typed: Vec<&FieldBox> = fields
            .iter()
            .filter(|found| {
                matches!(
                    found.field.kind,
                    FieldKind::Text | FieldKind::Combo | FieldKind::List
                ) && !found.field.read_only
            })
            .collect();
        let next = typed
            .iter()
            .position(|found| found.field.widget == widget)
            .and_then(|at| {
                let count = typed.len();
                let next = if backwards { at + count - 1 } else { at + 1 } % count;
                (next != at).then(|| (*typed[next]).clone())
            });
        self.finish_the_field();
        if let Some(next) = next {
            self.filling = Some(Filling {
                page,
                text: next.field.value.shown().to_owned(),
                field: next,
                take_the_keyboard: true,
            });
        }
    }

    pub(crate) fn fields_on_the_page(&mut self, glass: &egui::Painter, laid: Laid) {
        let tint = egui::Color32::from_rgba_unmultiplied(80, 130, 255, 38);
        let edge = egui::Color32::from_rgba_unmultiplied(40, 90, 220, 120);
        let open = self
            .filling
            .as_ref()
            .filter(|filling| filling.page == laid.page)
            .map(|filling| filling.field.field.widget);
        if self.tool == Tool::Form {
            self.fields_as_the_form_tool_sees_them(glass, laid);
            return;
        }
        for found in self.editor.fields_on(laid.page).iter() {
            if !found.field.is_fillable() || Some(found.field.widget) == open {
                continue;
            }
            let area = box_on_screen(laid.placed, found.pixels);
            glass.rect_filled(area, 1.0, tint);
            glass.rect_stroke(
                area,
                1.0,
                egui::Stroke::new(1.0, edge),
                egui::StrokeKind::Inside,
            );
        }
    }

    pub(crate) fn field_being_filled(&mut self, ctx: &egui::Context) {
        let Some(filling) = self.filling.as_mut() else {
            return;
        };
        let Some(laid) = self
            .laid
            .iter()
            .copied()
            .find(|laid| laid.page == filling.page)
        else {
            return;
        };
        let area = box_on_screen(laid.placed, filling.field.pixels);
        let field = &filling.field.field;
        let lines = if field.multiline { 3.0 } else { 1.0 };
        let size = (area.height() / lines * 0.62).clamp(8.0, 48.0);
        let font = egui::FontId::proportional(size);
        let mut finished = false;
        let mut cancelled = false;
        let mut tab = None;
        let mut chosen = None;
        egui::Area::new(egui::Id::new("filling a form field"))
            .fixed_pos(area.min)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let editor = if field.multiline {
                    egui::TextEdit::multiline(&mut filling.text)
                } else {
                    egui::TextEdit::singleline(&mut filling.text)
                };
                let response = ui.add_sized(
                    area.size(),
                    editor
                        .font(font.clone())
                        .password(field.password)
                        .char_limit(field.max_len.unwrap_or(usize::MAX))
                        .margin(egui::vec2(2.0, 0.0))
                        .background_color(egui::Color32::WHITE)
                        .text_color(egui::Color32::BLACK),
                );
                if filling.take_the_keyboard {
                    response.request_focus();
                    filling.take_the_keyboard = false;
                }
                let (enter, escape, tabbed, shift) = ui.input(|input| {
                    (
                        input.key_pressed(egui::Key::Enter),
                        input.key_pressed(egui::Key::Escape),
                        input.key_pressed(egui::Key::Tab),
                        input.modifiers.shift,
                    )
                });
                if escape {
                    cancelled = true;
                } else if tabbed {
                    tab = Some(shift);
                } else if response.lost_focus() && (enter || !field.multiline) {
                    finished = true;
                }
                if !field.options.is_empty() {
                    ui.set_max_width(area.width().max(160.0));
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .min_scrolled_height(220.0)
                            .max_height(220.0)
                            .show(ui, |ui| {
                                for option in &field.options {
                                    let on = *option == filling.text;
                                    if ui.selectable_label(on, option).clicked() {
                                        chosen = Some(option.clone());
                                    }
                                }
                            });
                    });
                }
            });
        if let Some(option) = chosen {
            filling.text = option;
            self.finish_the_field();
        } else if cancelled {
            self.filling = None;
        } else if let Some(backwards) = tab {
            self.go_to_the_next_field(backwards);
        } else if finished {
            self.finish_the_field();
        }
    }
}
