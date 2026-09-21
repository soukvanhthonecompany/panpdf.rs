use eframe::egui;

use pdf_app::arrange::{Arrangement, arranged, shifted};
use pdf_app::document::{FieldBox, OVERLAY_SCALE};
use pdf_app::view::SWEEP_ENOUGH;
use pdf_app::wording::{Control, Lang, Message};
use pdf_edit::new_field::NewFieldKind;
use pdf_syntax::Reference;

use crate::canvas::box_on_screen;
use crate::format::{icon_button, rule};
use crate::icons::Icon;
use crate::window_state::{ArrowPress, Carrying, ChosenFields, Drag, Laid, Tool, Window};

const fn usual_size(kind: NewFieldKind) -> (f64, f64) {
    match kind {
        NewFieldKind::Text => (160.0, 22.0),
        NewFieldKind::Paragraph => (240.0, 80.0),
        NewFieldKind::Checkbox | NewFieldKind::Radio => (14.0, 14.0),
        NewFieldKind::Dropdown => (140.0, 22.0),
        NewFieldKind::ListBox => (140.0, 70.0),
        NewFieldKind::Date => (100.0, 22.0),
        NewFieldKind::Signature => (180.0, 44.0),
        NewFieldKind::Button => (90.0, 24.0),
    }
}

const KINDS: [(NewFieldKind, Message, Icon); 9] = [
    (NewFieldKind::Text, Message::FieldText, Icon::TextField),
    (
        NewFieldKind::Paragraph,
        Message::FieldParagraph,
        Icon::ParagraphField,
    ),
    (
        NewFieldKind::Checkbox,
        Message::FieldCheckbox,
        Icon::Checkbox,
    ),
    (NewFieldKind::Radio, Message::FieldRadio, Icon::RadioButton),
    (
        NewFieldKind::Dropdown,
        Message::FieldDropdown,
        Icon::Dropdown,
    ),
    (NewFieldKind::ListBox, Message::FieldListBox, Icon::ListBox),
    (NewFieldKind::Date, Message::FieldDate, Icon::DateField),
    (
        NewFieldKind::Signature,
        Message::FieldSignature,
        Icon::SignatureField,
    ),
    (NewFieldKind::Button, Message::FieldButton, Icon::PushButton),
];

const ARRANGEMENTS: [(Arrangement, Message); 13] = [
    (Arrangement::AlignLeft, Message::AlignLeftEdges),
    (Arrangement::AlignRight, Message::AlignRightEdges),
    (Arrangement::AlignTop, Message::AlignTops),
    (Arrangement::AlignBottom, Message::AlignBottoms),
    (
        Arrangement::AlignHorizontalCentres,
        Message::AlignCentresAcross,
    ),
    (Arrangement::AlignVerticalCentres, Message::AlignCentresDown),
    (
        Arrangement::DistributeHorizontally,
        Message::DistributeAcross,
    ),
    (Arrangement::DistributeVertically, Message::DistributeDown),
    (Arrangement::MatchWidth, Message::MatchWidth),
    (Arrangement::MatchHeight, Message::MatchHeight),
    (Arrangement::MatchBoth, Message::MatchSize),
    (
        Arrangement::CentreHorizontallyOnPage,
        Message::CentreAcrossPage,
    ),
    (Arrangement::CentreVerticallyOnPage, Message::CentreDownPage),
];

const HANDLE_REACH: f32 = 9.0;

const PASTE_STEP: f64 = 12.0;

fn contains([x0, y0, x1, y1]: [f64; 4], (x, y): (f64, f64)) -> bool {
    (x0..=x1).contains(&x) && (y0..=y1).contains(&y)
}

impl Window {
    pub(crate) fn take_up_the_form_tool(&mut self) {
        self.finish_the_field();
        self.pictures.clear();
        self.ink = None;
        self.tool = Tool::Form;
        self.form_tool.kind = None;
        self.editor.say(Message::DragOutAField);
    }

    fn chosen_boxes(&mut self) -> Option<(usize, Vec<FieldBox>)> {
        let chosen = self.chosen_fields.clone()?;
        let fields = self.editor.fields_on(chosen.page);
        let boxes: Vec<FieldBox> = chosen
            .widgets
            .iter()
            .filter_map(|widget| {
                fields
                    .iter()
                    .find(|found| found.field.widget == *widget)
                    .cloned()
            })
            .collect();
        Some((chosen.page, boxes))
    }

    pub(crate) fn form_tool_choices(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        if icon_button(
            ui,
            Icon::Select,
            &Message::FieldPointer.say(lang),
            self.form_tool.kind.is_none(),
            true,
        )
        .clicked()
        {
            self.form_tool.kind = None;
        }
        rule(ui);
        for (kind, name, icon) in KINDS {
            let hover = format!(
                "{}\n{}",
                name.say(lang),
                Message::Control(Control::FieldPurpose(kind)).say(lang)
            );
            if icon_button(ui, icon, &hover, self.form_tool.kind == Some(kind), true).clicked() {
                self.form_tool.kind = Some(kind);
                if kind == NewFieldKind::Radio && self.form_tool.group.is_empty() {
                    self.form_tool.group = self.next_group_name();
                }
            }
        }
        if let Some(kind) = self.form_tool.kind
            && matches!(
                kind,
                NewFieldKind::Button
                    | NewFieldKind::Dropdown
                    | NewFieldKind::ListBox
                    | NewFieldKind::Radio
            )
        {
            rule(ui);
            let opener = icon_button(
                ui,
                Icon::Settings,
                &Message::Control(Control::FieldSettings).say(lang),
                false,
                true,
            );
            egui::Popup::menu(&opener)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| self.field_settings(ui, kind));
        }
        let count = self
            .chosen_fields
            .as_ref()
            .map_or(0, |chosen| chosen.widgets.len());
        if count > 0
            && self.form_tool.kind.is_none()
            && let Some(arrangement) = arrange_menu(ui, lang, count)
        {
            self.arrange_the_chosen_fields(arrangement);
        }
    }

    fn field_settings(&mut self, ui: &mut egui::Ui, kind: NewFieldKind) {
        let lang = self.lang;
        match kind {
            NewFieldKind::Button => {
                ui.label(Message::FieldCaption.say(lang));
                ui.add(
                    egui::TextEdit::singleline(&mut self.form_tool.caption).desired_width(200.0),
                );
            }
            NewFieldKind::Dropdown | NewFieldKind::ListBox => {
                ui.label(Message::FieldChoices.say(lang));
                ui.add(
                    egui::TextEdit::singleline(&mut self.form_tool.choices).desired_width(260.0),
                );
            }
            NewFieldKind::Radio => {
                ui.label(Message::FieldGroup.say(lang));
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.form_tool.group).desired_width(140.0),
                    );
                    if ui.button(Message::FieldNewGroup.say(lang)).clicked() {
                        self.form_tool.group = self.next_group_name();
                    }
                });
            }
            _ => {}
        }
    }

    pub(crate) fn form_tool_hint(&self) -> Option<String> {
        let lang = self.lang;
        let kind = self.form_tool.kind?;
        let name = KINDS
            .iter()
            .find(|(each, _, _)| *each == kind)
            .map(|(_, name, _)| name.say(lang))
            .unwrap_or_default();
        Some(Message::Control(Control::PlaceField(name)).say(lang))
    }

    fn next_group_name(&self) -> String {
        let taken = self.editor.field_names();
        (1..=taken.len() + 1)
            .map(|number| format!("Group{number}"))
            .find(|name| !taken.contains(name))
            .unwrap_or_default()
    }

    fn arrange_the_chosen_fields(&mut self, arrangement: Arrangement) {
        let Some((page, boxes)) = self.chosen_boxes() else {
            return;
        };
        let Some((width, height)) = self.editor.page_pixels(page, OVERLAY_SCALE) else {
            return;
        };
        let pixels: Vec<[f64; 4]> = boxes.iter().map(|found| found.pixels).collect();
        let moved = arranged(
            &pixels,
            pixels.len().saturating_sub(1),
            arrangement,
            (f64::from(width), f64::from(height)),
        );
        let changed: Vec<(Reference, [f64; 4])> = boxes
            .iter()
            .zip(moved)
            .filter(|(found, to)| {
                found
                    .pixels
                    .iter()
                    .zip(to)
                    .any(|(before, after)| (before - after).abs() > f64::EPSILON)
            })
            .map(|(found, to)| (found.field.widget, to))
            .collect();
        self.set_the_boxes(page, &changed);
    }

    fn set_the_boxes(&mut self, page: usize, boxes: &[(Reference, [f64; 4])]) {
        if boxes.is_empty() {
            return;
        }
        let job = self.editor.begin_set_field_boxes(page, boxes);
        if job.is_none() {
            self.editor.say(Message::AnotherEditIsRunning);
            return;
        }
        self.send(job);
    }

    pub(crate) fn form_tool_clicked(&mut self, page: usize, point: (f64, f64), adding: bool) {
        if let Some(kind) = self.form_tool.kind {
            let (width, height) = usual_size(kind);
            self.put_the_field(page, [point.0, point.1, point.0 + width, point.1 + height]);
            return;
        }
        let under = self
            .editor
            .fields_on(page)
            .iter()
            .rev()
            .find(|found| contains(found.pixels, point))
            .map(|found| found.field.widget);
        self.choose_field(page, under, adding);
    }

    fn choose_field(&mut self, page: usize, widget: Option<Reference>, adding: bool) {
        let Some(widget) = widget else {
            if !adding {
                self.chosen_fields = None;
            }
            return;
        };
        match self.chosen_fields.as_mut() {
            Some(chosen) if adding && chosen.page == page => {
                if let Some(at) = chosen.widgets.iter().position(|held| *held == widget) {
                    chosen.widgets.remove(at);
                } else {
                    chosen.widgets.push(widget);
                }
                if chosen.widgets.is_empty() {
                    self.chosen_fields = None;
                }
            }
            _ => {
                self.chosen_fields = Some(ChosenFields {
                    page,
                    widgets: vec![widget],
                });
            }
        }
    }

    pub(crate) fn box_dragged_out(&self, drag: &Drag) -> Option<[f64; 4]> {
        let laid = self
            .laid
            .iter()
            .copied()
            .find(|laid| laid.page == drag.page)?;
        let travel = drag.to - drag.from;
        if f64::from(travel.x.abs().max(travel.y.abs())) < SWEEP_ENOUGH {
            return None;
        }
        let from = laid.placed.point_in_page((drag.from.x, drag.from.y))?;
        let to = laid.placed.point_in_page((drag.to.x, drag.to.y))?;
        Some([
            from.0.min(to.0),
            from.1.min(to.1),
            from.0.max(to.0),
            from.1.max(to.1),
        ])
    }

    pub(crate) fn show_the_field_box(&self, painter: &egui::Painter, drag: &Drag) {
        let (Some(pixels), Some(laid)) = (
            self.box_dragged_out(drag),
            self.laid
                .iter()
                .copied()
                .find(|laid| laid.page == drag.page),
        ) else {
            return;
        };
        let area = box_on_screen(laid.placed, pixels);
        let blue = egui::Color32::from_rgb(0, 90, 200);
        painter.rect_filled(area, 1.0, blue.gamma_multiply(0.12));
        painter.rect_stroke(
            area,
            1.0,
            egui::Stroke::new(1.5, blue),
            egui::StrokeKind::Inside,
        );
    }

    pub(crate) fn take_the_field_drawn(&mut self, drag: &Drag) {
        if let Some(pixels) = self.box_dragged_out(drag) {
            self.put_the_field(drag.page, pixels);
        }
    }

    pub(crate) fn take_the_fields_swept(&mut self, drag: &Drag) {
        let Carrying::FieldSweep { adding } = drag.what else {
            return;
        };
        let Some(swept) = self.box_dragged_out(drag) else {
            if !adding {
                self.chosen_fields = None;
            }
            return;
        };
        let touched: Vec<Reference> = self
            .editor
            .fields_on(drag.page)
            .iter()
            .filter(|found| {
                let [x0, y0, x1, y1] = found.pixels;
                x0 <= swept[2] && swept[0] <= x1 && y0 <= swept[3] && swept[1] <= y1
            })
            .map(|found| found.field.widget)
            .collect();
        let mut widgets = match self.chosen_fields.take() {
            Some(chosen) if adding && chosen.page == drag.page => chosen.widgets,
            _ => Vec::new(),
        };
        for widget in touched {
            if !widgets.contains(&widget) {
                widgets.push(widget);
            }
        }
        self.chosen_fields = (!widgets.is_empty()).then_some(ChosenFields {
            page: drag.page,
            widgets,
        });
    }

    fn put_the_field(&mut self, page: usize, pixels: [f64; 4]) {
        let Some(kind) = self.form_tool.kind else {
            return;
        };
        let (name, options) = match kind {
            NewFieldKind::Radio => {
                let group = self.form_tool.group.trim().to_owned();
                (Some(group).filter(|group| !group.is_empty()), Vec::new())
            }
            NewFieldKind::Button => (
                None,
                vec![self.form_tool.caption.trim().to_owned()]
                    .into_iter()
                    .filter(|caption| !caption.is_empty())
                    .collect(),
            ),
            NewFieldKind::Dropdown | NewFieldKind::ListBox => (
                None,
                self.form_tool
                    .choices
                    .split(',')
                    .map(str::trim)
                    .filter(|choice| !choice.is_empty())
                    .map(str::to_owned)
                    .collect(),
            ),
            _ => (None, Vec::new()),
        };
        let job = self
            .editor
            .begin_add_field(page, pixels, kind, (name, options));
        if job.is_none() {
            self.editor.say(Message::AnotherEditIsRunning);
            return;
        }
        self.send(job);
        self.form_tool.kind = None;
        self.landing_fields = Some((page, vec![pixels]));
    }

    pub(crate) fn remove_the_chosen_fields(&mut self) -> bool {
        if self.tool != Tool::Form {
            return false;
        }
        let Some(chosen) = self.chosen_fields.take() else {
            return false;
        };
        let job = self
            .editor
            .begin_remove_fields(chosen.page, chosen.widgets.clone());
        if job.is_none() {
            self.editor.say(Message::AnotherEditIsRunning);
            self.chosen_fields = Some(chosen);
            return true;
        }
        self.send(job);
        true
    }

    pub(crate) fn settle_the_landing(&mut self) {
        let Some((page, boxes)) = self.landing_fields.clone() else {
            return;
        };
        if self.editor.is_busy() || self.editor.leaf(page).is_none() {
            return;
        }
        let fields = self.editor.fields_on(page);
        let near = |one: [f64; 4], other: [f64; 4]| {
            one.iter().zip(other).all(|(a, b)| (a - b).abs() < 1.0)
        };
        let widgets: Vec<Reference> = boxes
            .iter()
            .filter_map(|wanted| {
                fields
                    .iter()
                    .rev()
                    .find(|found| near(found.pixels, *wanted))
                    .map(|found| found.field.widget)
            })
            .collect();
        self.landing_fields = None;
        if !widgets.is_empty() {
            self.chosen_fields = Some(ChosenFields { page, widgets });
        }
    }

    pub(crate) fn form_tool_keys(&mut self, ctx: &egui::Context, presses: &[ArrowPress]) {
        self.settle_the_landing();
        let command = |key| {
            ctx.input(|input| {
                input.events.iter().any(|event| {
                    matches!(event, egui::Event::Key { key: down, pressed: true, modifiers, .. }
                        if *down == key && modifiers.command)
                })
            })
        };
        let copied = command(egui::Key::C)
            || ctx.input(|input| {
                input
                    .events
                    .iter()
                    .any(|event| matches!(event, egui::Event::Copy))
            });
        let cut = command(egui::Key::X)
            || ctx.input(|input| {
                input
                    .events
                    .iter()
                    .any(|event| matches!(event, egui::Event::Cut))
            });
        let marker = self.field_clipboard_text.clone();
        let pasted = ctx.input(|input| {
            input.events.iter().any(|event| {
                matches!(event, egui::Event::Paste(text) if marker.as_deref() == Some(text.as_str()))
            })
        });
        let (all, delete, escape) = ctx.input(|input| {
            (
                input.modifiers.command && input.key_pressed(egui::Key::A),
                input.key_pressed(egui::Key::Delete) || input.key_pressed(egui::Key::Backspace),
                input.key_pressed(egui::Key::Escape),
            )
        });
        if escape {
            if self.form_tool.kind.is_some() {
                self.form_tool.kind = None;
            } else if self.chosen_fields.is_some() {
                self.chosen_fields = None;
            } else {
                self.tool = Tool::Select;
            }
            return;
        }
        if all {
            let page = self.focus;
            let widgets: Vec<Reference> = self
                .editor
                .fields_on(page)
                .iter()
                .map(|found| found.field.widget)
                .collect();
            self.chosen_fields = (!widgets.is_empty()).then_some(ChosenFields { page, widgets });
        }
        if (copied || cut) && self.chosen_fields.is_some() {
            self.field_clipboard.clone_from(&self.chosen_fields);
            self.pastes = 0;
            let names = self
                .chosen_boxes()
                .map(|(_, boxes)| {
                    boxes
                        .iter()
                        .map(|found| found.field.name.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            ctx.copy_text(names.clone());
            self.field_clipboard_text = Some(names);
        }
        if cut || (delete && !self.editor.is_busy()) {
            self.remove_the_chosen_fields();
            return;
        }
        if pasted {
            self.paste_the_fields();
        }
        self.nudge_the_chosen_fields(presses);
    }

    fn nudge_the_chosen_fields(&mut self, presses: &[ArrowPress]) {
        let travel = crate::input::nudge_travel(presses);
        self.field_nudge.0 += travel.0 * OVERLAY_SCALE;
        self.field_nudge.1 += travel.1 * OVERLAY_SCALE;
        if self.field_nudge != (0.0, 0.0) && !self.editor.is_busy() {
            let Some((page, boxes)) = self.chosen_boxes() else {
                self.field_nudge = (0.0, 0.0);
                return;
            };
            let wanted = self
                .chosen_fields
                .as_ref()
                .map_or(0, |chosen| chosen.widgets.len());
            if boxes.len() < wanted {
                return;
            }
            let by = std::mem::take(&mut self.field_nudge);
            let moved: Vec<(Reference, [f64; 4])> = boxes
                .iter()
                .map(|found| (found.field.widget, shifted(found.pixels, by)))
                .collect();
            self.set_the_boxes(page, &moved);
        }
    }

    fn paste_the_fields(&mut self) {
        let Some(clipboard) = self.field_clipboard.clone() else {
            return;
        };
        let page = clipboard.page;
        let fields = self.editor.fields_on(page);
        let originals: Vec<FieldBox> = clipboard
            .widgets
            .iter()
            .filter_map(|widget| {
                fields
                    .iter()
                    .find(|found| found.field.widget == *widget)
                    .cloned()
            })
            .collect();
        if originals.is_empty() {
            return;
        }
        let Some((width, height)) = self.editor.page_pixels(page, OVERLAY_SCALE) else {
            return;
        };
        let pixels: Vec<[f64; 4]> = originals.iter().map(|found| found.pixels).collect();
        let centred = arranged(
            &pixels,
            0,
            Arrangement::CentreVerticallyOnPage,
            (f64::from(width), f64::from(height)),
        );
        let centred = arranged(
            &centred,
            0,
            Arrangement::CentreHorizontallyOnPage,
            (f64::from(width), f64::from(height)),
        );
        #[allow(clippy::cast_precision_loss, reason = "a count of pastes")]
        let step = PASTE_STEP * self.pastes as f64;
        let copies: Vec<(Reference, [f64; 4])> = originals
            .iter()
            .zip(centred)
            .map(|(found, to)| (found.field.widget, shifted(to, (step, step))))
            .collect();
        let job = self.editor.begin_copy_fields(page, &copies);
        if job.is_none() {
            self.editor.say(Message::AnotherEditIsRunning);
            return;
        }
        self.send(job);
        self.pastes += 1;
        self.landing_fields = Some((page, copies.iter().map(|(_, to)| *to).collect()));
    }

    pub(crate) fn fields_as_the_form_tool_sees_them(&mut self, glass: &egui::Painter, laid: Laid) {
        let blue = egui::Color32::from_rgb(40, 90, 220);
        let orange = egui::Color32::from_rgb(230, 120, 0);
        let chosen: Vec<Reference> = self
            .chosen_fields
            .as_ref()
            .filter(|chosen| chosen.page == laid.page)
            .map(|chosen| chosen.widgets.clone())
            .unwrap_or_default();
        let only = chosen.len() == 1;
        for found in self.editor.fields_on(laid.page).iter() {
            let area = box_on_screen(laid.placed, found.pixels);
            let picked = chosen.contains(&found.field.widget);
            let colour = if picked { orange } else { blue };
            glass.rect_filled(area, 1.0, colour.gamma_multiply(0.10));
            glass.rect_stroke(
                area,
                1.0,
                egui::Stroke::new(if picked { 2.0 } else { 1.0 }, colour),
                egui::StrokeKind::Outside,
            );
            if picked && only {
                let handle =
                    egui::Rect::from_center_size(area.right_bottom(), egui::vec2(8.0, 8.0));
                glass.rect_filled(handle, 1.0, egui::Color32::WHITE);
                glass.rect_stroke(
                    handle,
                    1.0,
                    egui::Stroke::new(1.5, orange),
                    egui::StrokeKind::Inside,
                );
            }
            glass.text(
                area.left_top() + egui::vec2(0.0, -2.0),
                egui::Align2::LEFT_BOTTOM,
                &found.field.name,
                egui::FontId::proportional(11.0),
                colour,
            );
        }
    }

    pub(crate) fn form_tool_takes_hold(
        &mut self,
        page: usize,
        point: (f64, f64),
        adding: bool,
    ) -> Carrying {
        if self.form_tool.kind.is_some() {
            return Carrying::NewField;
        }
        let stretch = self
            .laid
            .iter()
            .find(|laid| laid.page == page)
            .map_or(1.0, |laid| laid.placed.stretch);
        let reach = f64::from(HANDLE_REACH / stretch.max(0.01));
        if let Some((chosen_page, boxes)) = self.chosen_boxes()
            && chosen_page == page
            && let [only] = boxes.as_slice()
        {
            let [_, _, x1, y1] = only.pixels;
            if (point.0 - x1).abs() <= reach && (point.1 - y1).abs() <= reach {
                return Carrying::MovingField {
                    fields: vec![(only.field.widget, only.pixels)],
                    resize: true,
                };
            }
        }
        let under = self
            .editor
            .fields_on(page)
            .iter()
            .rev()
            .find(|found| contains(found.pixels, point))
            .map(|found| found.field.widget);
        let Some(widget) = under else {
            return Carrying::FieldSweep { adding };
        };
        let already = self
            .chosen_fields
            .as_ref()
            .is_some_and(|chosen| chosen.page == page && chosen.widgets.contains(&widget));
        if !already {
            self.choose_field(page, Some(widget), adding);
        }
        let fields = self
            .chosen_boxes()
            .map(|(_, boxes)| {
                boxes
                    .iter()
                    .map(|found| (found.field.widget, found.pixels))
                    .collect()
            })
            .unwrap_or_default();
        Carrying::MovingField {
            fields,
            resize: false,
        }
    }

    fn fields_moved_to(&self, drag: &Drag) -> Option<Vec<(Reference, [f64; 4])>> {
        let Carrying::MovingField { fields, resize } = &drag.what else {
            return None;
        };
        let laid = self.laid.iter().find(|laid| laid.page == drag.page)?;
        let stretch = f64::from(laid.placed.stretch);
        if stretch <= 0.0 {
            return None;
        }
        let dx = f64::from(drag.to.x - drag.from.x) / stretch;
        let dy = f64::from(drag.to.y - drag.from.y) / stretch;
        Some(
            fields
                .iter()
                .map(|(widget, [x0, y0, x1, y1])| {
                    let to = if *resize {
                        [*x0, *y0, (x1 + dx).max(x0 + 1.0), (y1 + dy).max(y0 + 1.0)]
                    } else {
                        shifted([*x0, *y0, *x1, *y1], (dx, dy))
                    };
                    (*widget, to)
                })
                .collect(),
        )
    }

    pub(crate) fn show_the_field_moved(&self, painter: &egui::Painter, drag: &Drag) {
        let (Some(moved), Some(laid)) = (
            self.fields_moved_to(drag),
            self.laid
                .iter()
                .copied()
                .find(|laid| laid.page == drag.page),
        ) else {
            return;
        };
        let orange = egui::Color32::from_rgb(230, 120, 0);
        for (_, pixels) in moved {
            let area = box_on_screen(laid.placed, pixels);
            painter.rect_filled(area, 1.0, orange.gamma_multiply(0.15));
            painter.rect_stroke(
                area,
                1.0,
                egui::Stroke::new(1.5, orange),
                egui::StrokeKind::Inside,
            );
        }
    }

    pub(crate) fn take_the_field_moved(&mut self, drag: &Drag) {
        let Some(moved) = self.fields_moved_to(drag) else {
            return;
        };
        let travel = drag.to - drag.from;
        if travel.x.abs().max(travel.y.abs()) < 1.0 {
            return;
        }
        self.set_the_boxes(drag.page, &moved);
    }

    pub(crate) fn chosen_now(&mut self) -> Option<(usize, FieldBox)> {
        let chosen = self.chosen_fields.clone()?;
        let [widget] = chosen.widgets.as_slice() else {
            return None;
        };
        let now = self
            .editor
            .fields_on(chosen.page)
            .iter()
            .find(|found| found.field.widget == *widget)
            .cloned();
        match now {
            Some(found) => Some((chosen.page, found)),
            None if self.editor.is_busy() || self.editor.leaf(chosen.page).is_none() => None,
            None => {
                self.chosen_fields = None;
                None
            }
        }
    }
}

pub(crate) fn arrange_menu(ui: &mut egui::Ui, lang: Lang, count: usize) -> Option<Arrangement> {
    let mut asked = None;
    let button = icon_button(
        ui,
        Icon::Arrange,
        &Message::ArrangeFields.say(lang),
        false,
        true,
    );
    egui::Popup::menu(&button).show(|ui| {
        for (arrangement, name) in ARRANGEMENTS {
            if matches!(
                arrangement,
                Arrangement::DistributeHorizontally
                    | Arrangement::MatchWidth
                    | Arrangement::CentreHorizontallyOnPage
            ) {
                ui.separator();
            }
            if ui
                .add_enabled(
                    count >= arrangement.needs(),
                    egui::Button::new(name.say(lang)),
                )
                .clicked()
            {
                asked = Some(arrangement);
                ui.close();
            }
        }
    });
    asked
}
