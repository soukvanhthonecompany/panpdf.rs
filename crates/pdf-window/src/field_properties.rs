use eframe::egui;

use pdf_app::wording::{Lang, Message};
use pdf_edit::field_settings::FieldSettings;
use pdf_edit::form::{
    BorderStyle, ButtonStyle, FieldKind, FieldValue, FormField, Quadding, Visibility, flags,
};

use crate::canvas::box_on_screen;
use crate::window_state::{FieldDraft, PropertiesTab, Tool, Window};

const PANEL_WIDTH: f32 = 380.0;

pub(crate) fn beside_or_under(area: egui::Rect, screen: egui::Rect) -> egui::Pos2 {
    if area.right() + 16.0 + PANEL_WIDTH <= screen.right() {
        area.right_top() + egui::vec2(16.0, 0.0)
    } else {
        area.left_bottom() + egui::vec2(0.0, 16.0)
    }
}

fn to_f32(colour: [f64; 3]) -> [f32; 3] {
    #[allow(
        clippy::cast_possible_truncation,
        reason = "a colour component from 0 to 1"
    )]
    colour.map(|part| part as f32)
}

fn to_f64(colour: [f32; 3]) -> [f64; 3] {
    colour.map(|part| (f64::from(part) * 1000.0).round() / 1000.0)
}

fn same(one: [f32; 3], other: [f32; 3]) -> bool {
    one.iter().zip(other).all(|(a, b)| (a - b).abs() < 0.000_5)
}

const fn is_texty(kind: FieldKind) -> bool {
    matches!(kind, FieldKind::Text | FieldKind::Combo | FieldKind::List)
}

pub(crate) fn draft_of(field: &FormField) -> FieldDraft {
    let default_value = match &field.default_value {
        FieldValue::Text(text) => text.clone(),
        FieldValue::State(_) | FieldValue::Empty => String::new(),
    };
    FieldDraft {
        widget: field.widget,
        name: field.name.clone(),
        tooltip: field.tooltip.clone(),
        visibility: Visibility::of(field.annotation_flags),
        required: field.required,
        read_only: field.read_only,
        border_on: field.border.is_some(),
        border: to_f32(field.border.unwrap_or([0.0; 3])),
        fill_on: field.background.is_some(),
        fill: to_f32(field.background.unwrap_or([1.0; 3])),
        border_width: field.border_width,
        border_style: field.border_style,
        text_colour: to_f32(field.text_colour),
        fits: field.text_size().is_none(),
        size: field.text_size().unwrap_or(12.0),
        quadding: field.quadding,
        default_value,
        limit_on: field.max_len.is_some(),
        limit: field.max_len.unwrap_or(10),
        flags: field.flags,
        items: field
            .option_exports
            .iter()
            .cloned()
            .zip(field.options.iter().cloned())
            .collect(),
        new_item: String::new(),
        new_export: String::new(),
        picked_item: None,
        button_style: field.button_style,
        export_value: field
            .states
            .first()
            .cloned()
            .unwrap_or_else(|| "Yes".to_owned()),
        checked_by_default: matches!(field.default_value, FieldValue::State(_)),
        caption: field.caption.clone(),
        link: field.link.clone().unwrap_or_default(),
        date_format: field.date_format.clone().unwrap_or_default(),
    }
}

pub(crate) fn settings_changed(field: &FormField, draft: &FieldDraft) -> FieldSettings {
    let was = draft_of(field);
    let mut settings = FieldSettings::default();
    let name = draft.name.trim();
    if name != field.name {
        settings.name = Some(name.to_owned());
    }
    if draft.tooltip != was.tooltip {
        settings.tooltip = Some(draft.tooltip.clone());
    }
    if draft.visibility != was.visibility {
        settings.visibility = Some(draft.visibility);
    }
    if draft.required != was.required {
        settings.required = Some(draft.required);
    }
    if draft.read_only != was.read_only {
        settings.read_only = Some(draft.read_only);
    }
    if draft.border_on != was.border_on || (draft.border_on && !same(draft.border, was.border)) {
        settings.border = Some(draft.border_on.then(|| to_f64(draft.border)));
    }
    if draft.fill_on != was.fill_on || (draft.fill_on && !same(draft.fill, was.fill)) {
        settings.fill = Some(draft.fill_on.then(|| to_f64(draft.fill)));
    }
    if (draft.border_width - was.border_width).abs() > f64::EPSILON {
        settings.border_width = Some(draft.border_width);
    }
    if draft.border_style != was.border_style {
        settings.border_style = Some(draft.border_style);
    }
    if !same(draft.text_colour, was.text_colour) {
        settings.text_colour = Some(to_f64(draft.text_colour));
    }
    if is_texty(field.kind) {
        let size = (!draft.fits).then_some(draft.size);
        if size != field.text_size() {
            settings.text_size = Some(size);
        }
        if draft.quadding != was.quadding {
            settings.quadding = Some(draft.quadding);
        }
        if draft.default_value != was.default_value {
            settings.default_value = Some(draft.default_value.clone());
        }
    }
    if field.kind == FieldKind::Text
        && (draft.limit_on != was.limit_on || (draft.limit_on && draft.limit != was.limit))
    {
        settings.max_len = Some(draft.limit_on.then_some(draft.limit));
    }
    settings.set_flags = draft.flags & !field.flags;
    settings.clear_flags = field.flags & !draft.flags;
    if matches!(field.kind, FieldKind::Combo | FieldKind::List) && draft.items != was.items {
        settings.options = Some(draft.items.clone());
    }
    if field.kind == FieldKind::Push {
        if draft.caption.trim() != was.caption {
            settings.caption = Some(draft.caption.trim().to_owned());
        }
        let link = draft.link.trim();
        if link != was.link {
            settings.link = Some((!link.is_empty()).then(|| link.to_owned()));
        }
    }
    if field.kind == FieldKind::Text {
        let format = draft.date_format.trim();
        if format != was.date_format {
            settings.date_format = Some((!format.is_empty()).then(|| format.to_owned()));
        }
    }
    if field.kind.is_a_button() {
        if draft.button_style != was.button_style {
            settings.button_style = Some(draft.button_style);
        }
        if draft.export_value.trim() != was.export_value {
            settings.export_value = Some(draft.export_value.trim().to_owned());
        }
        if draft.checked_by_default != was.checked_by_default {
            settings.checked_by_default = Some(draft.checked_by_default);
        }
    }
    settings
}

fn flag(ui: &mut egui::Ui, bits: &mut u32, bit: u32, label: String) {
    let mut on = *bits & bit != 0;
    if ui.checkbox(&mut on, label).changed() {
        if on {
            *bits |= bit;
        } else {
            *bits &= !bit;
        }
    }
}

fn flag_off(ui: &mut egui::Ui, bits: &mut u32, bit: u32, label: String) {
    let mut on = *bits & bit == 0;
    if ui.checkbox(&mut on, label).changed() {
        if on {
            *bits &= !bit;
        } else {
            *bits |= bit;
        }
    }
}

fn general(ui: &mut egui::Ui, draft: &mut FieldDraft, lang: Lang) {
    egui::Grid::new("field general")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label(Message::FieldName.say(lang));
            ui.add(egui::TextEdit::singleline(&mut draft.name).desired_width(220.0));
            ui.end_row();
            ui.label(Message::FieldTooltip.say(lang));
            ui.add(egui::TextEdit::singleline(&mut draft.tooltip).desired_width(220.0));
            ui.end_row();
            ui.label(Message::FieldVisibility.say(lang));
            egui::ComboBox::from_id_salt("field visibility")
                .selected_text(visibility_name(draft.visibility).say(lang))
                .show_ui(ui, |ui| {
                    for choice in [
                        Visibility::Visible,
                        Visibility::Hidden,
                        Visibility::VisibleNotPrinted,
                        Visibility::HiddenPrinted,
                    ] {
                        ui.selectable_value(
                            &mut draft.visibility,
                            choice,
                            visibility_name(choice).say(lang),
                        );
                    }
                });
            ui.end_row();
            ui.label("");
            ui.checkbox(&mut draft.read_only, Message::FieldReadOnly.say(lang));
            ui.end_row();
            ui.label("");
            ui.checkbox(&mut draft.required, Message::FieldRequired.say(lang));
            ui.end_row();
        });
}

const fn visibility_name(visibility: Visibility) -> Message {
    match visibility {
        Visibility::Visible => Message::FieldVisible,
        Visibility::Hidden => Message::FieldHidden,
        Visibility::VisibleNotPrinted => Message::FieldVisibleNotPrinted,
        Visibility::HiddenPrinted => Message::FieldHiddenPrinted,
    }
}

fn appearance(ui: &mut egui::Ui, draft: &mut FieldDraft, kind: FieldKind, lang: Lang) {
    egui::Grid::new("field appearance")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label(Message::FieldBorderColour.say(lang));
            ui.horizontal(|ui| {
                ui.checkbox(&mut draft.border_on, "");
                ui.add_enabled_ui(draft.border_on, |ui| {
                    ui.color_edit_button_rgb(&mut draft.border)
                });
            });
            ui.end_row();
            ui.label(Message::FieldFillColour.say(lang));
            ui.horizontal(|ui| {
                ui.checkbox(&mut draft.fill_on, "");
                ui.add_enabled_ui(draft.fill_on, |ui| {
                    ui.color_edit_button_rgb(&mut draft.fill)
                });
            });
            ui.end_row();
            ui.label(Message::FieldLineThickness.say(lang));
            egui::ComboBox::from_id_salt("field thickness")
                .selected_text(thickness_name(draft.border_width).say(lang))
                .show_ui(ui, |ui| {
                    for width in [1.0, 2.0, 3.0] {
                        ui.selectable_value(
                            &mut draft.border_width,
                            width,
                            thickness_name(width).say(lang),
                        );
                    }
                });
            ui.end_row();
            ui.label(Message::FieldLineStyle.say(lang));
            egui::ComboBox::from_id_salt("field line style")
                .selected_text(style_name(draft.border_style).say(lang))
                .show_ui(ui, |ui| {
                    for style in [
                        BorderStyle::Solid,
                        BorderStyle::Dashed,
                        BorderStyle::Beveled,
                        BorderStyle::Inset,
                        BorderStyle::Underline,
                    ] {
                        ui.selectable_value(
                            &mut draft.border_style,
                            style,
                            style_name(style).say(lang),
                        );
                    }
                });
            ui.end_row();
            if is_texty(kind) {
                ui.label(Message::FieldTextSize.say(lang));
                ui.horizontal(|ui| {
                    ui.checkbox(&mut draft.fits, Message::FieldAutoSize.say(lang));
                    ui.add_enabled(
                        !draft.fits,
                        egui::DragValue::new(&mut draft.size)
                            .range(4.0..=72.0)
                            .suffix(" pt"),
                    );
                });
                ui.end_row();
            }
            ui.label(Message::TextColour.say(lang));
            ui.color_edit_button_rgb(&mut draft.text_colour);
            ui.end_row();
        });
}

fn thickness_name(width: f64) -> Message {
    if width >= 2.5 {
        Message::FieldThick
    } else if width >= 1.5 {
        Message::FieldMedium
    } else {
        Message::FieldThin
    }
}

const fn style_name(style: BorderStyle) -> Message {
    match style {
        BorderStyle::Solid => Message::FieldSolid,
        BorderStyle::Dashed => Message::FieldDashed,
        BorderStyle::Beveled => Message::FieldBeveled,
        BorderStyle::Inset => Message::FieldInset,
        BorderStyle::Underline => Message::Underline,
    }
}

const fn mark_name(style: ButtonStyle) -> Message {
    match style {
        ButtonStyle::Check => Message::MarkCheck,
        ButtonStyle::Circle => Message::MarkCircle,
        ButtonStyle::Cross => Message::MarkCross,
        ButtonStyle::Diamond => Message::MarkDiamond,
        ButtonStyle::Square => Message::MarkSquare,
        ButtonStyle::Star => Message::MarkStar,
    }
}

fn text_options(ui: &mut egui::Ui, draft: &mut FieldDraft, lang: Lang) {
    egui::Grid::new("field text options")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label(Message::FieldAlignment.say(lang));
            ui.horizontal(|ui| {
                for (quadding, word) in [
                    (Quadding::Left, Message::AlignLeft),
                    (Quadding::Centre, Message::AlignCentre),
                    (Quadding::Right, Message::AlignRight),
                ] {
                    ui.selectable_value(&mut draft.quadding, quadding, word.say(lang));
                }
            });
            ui.end_row();
            ui.label(Message::FieldDefaultValue.say(lang));
            ui.add(egui::TextEdit::singleline(&mut draft.default_value).desired_width(200.0));
            ui.end_row();
            ui.label(Message::FieldDateFormat.say(lang));
            egui::ComboBox::from_id_salt("field date format")
                .selected_text(if draft.date_format.is_empty() {
                    Message::FieldNone.say(lang)
                } else {
                    draft.date_format.clone()
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut draft.date_format,
                        String::new(),
                        Message::FieldNone.say(lang),
                    );
                    for format in ["dd/mm/yyyy", "mm/dd/yyyy", "yyyy-mm-dd", "d mmm yyyy"] {
                        ui.selectable_value(&mut draft.date_format, format.to_owned(), format);
                    }
                });
            ui.end_row();
        });
    let comb = draft.flags & flags::COMB != 0;
    ui.add_enabled_ui(!comb, |ui| {
        flag(
            ui,
            &mut draft.flags,
            flags::MULTILINE,
            Message::FieldMultiline.say(lang),
        );
        flag(
            ui,
            &mut draft.flags,
            flags::PASSWORD,
            Message::FieldPassword.say(lang),
        );
    });
    flag_off(
        ui,
        &mut draft.flags,
        flags::DO_NOT_SCROLL,
        Message::FieldScroll.say(lang),
    );
    flag_off(
        ui,
        &mut draft.flags,
        flags::DO_NOT_SPELL_CHECK,
        Message::FieldSpellCheck.say(lang),
    );
    ui.horizontal(|ui| {
        ui.checkbox(&mut draft.limit_on, Message::FieldLimit.say(lang));
        ui.add_enabled(
            draft.limit_on,
            egui::DragValue::new(&mut draft.limit).range(1..=32_768),
        );
    });
    let plain = draft.flags & (flags::MULTILINE | flags::PASSWORD) == 0;
    ui.add_enabled_ui(plain && draft.limit_on, |ui| {
        flag(
            ui,
            &mut draft.flags,
            flags::COMB,
            Message::FieldComb.say(lang),
        );
    });
}

fn push_options(ui: &mut egui::Ui, draft: &mut FieldDraft, lang: Lang) {
    egui::Grid::new("field push options")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label(Message::FieldCaption.say(lang));
            ui.add(egui::TextEdit::singleline(&mut draft.caption).desired_width(200.0));
            ui.end_row();
            ui.label(Message::FieldLink.say(lang));
            ui.add(egui::TextEdit::singleline(&mut draft.link).desired_width(240.0));
            ui.end_row();
        });
}

fn button_options(ui: &mut egui::Ui, draft: &mut FieldDraft, kind: FieldKind, lang: Lang) {
    egui::Grid::new("field button options")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label(Message::FieldMarkStyle.say(lang));
            egui::ComboBox::from_id_salt("field mark")
                .selected_text(mark_name(draft.button_style).say(lang))
                .show_ui(ui, |ui| {
                    for style in ButtonStyle::ALL {
                        ui.selectable_value(
                            &mut draft.button_style,
                            style,
                            mark_name(style).say(lang),
                        );
                    }
                });
            ui.end_row();
            ui.label(Message::FieldExportValue.say(lang));
            ui.add(egui::TextEdit::singleline(&mut draft.export_value).desired_width(160.0));
            ui.end_row();
        });
    ui.checkbox(
        &mut draft.checked_by_default,
        Message::FieldCheckedByDefault.say(lang),
    );
    if kind == FieldKind::Radio {
        flag(
            ui,
            &mut draft.flags,
            flags::RADIOS_IN_UNISON,
            Message::FieldInUnison.say(lang),
        );
    }
}

fn item_editor(ui: &mut egui::Ui, draft: &mut FieldDraft, lang: Lang) {
    egui::Grid::new("field list item")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label(Message::FieldItem.say(lang));
            ui.add(egui::TextEdit::singleline(&mut draft.new_item).desired_width(200.0));
            ui.end_row();
            ui.label(Message::FieldExportValue.say(lang));
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut draft.new_export).desired_width(130.0));
                let item = draft.new_item.trim().to_owned();
                if ui
                    .add_enabled(
                        !item.is_empty(),
                        egui::Button::new(Message::FieldAddItem.say(lang)),
                    )
                    .clicked()
                {
                    let export = draft.new_export.trim();
                    let export = if export.is_empty() {
                        item.clone()
                    } else {
                        export.to_owned()
                    };
                    draft.items.push((export, item));
                    draft.new_item.clear();
                    draft.new_export.clear();
                }
            });
            ui.end_row();
        });
    ui.horizontal(|ui| {
        egui::ScrollArea::vertical()
            .max_height(110.0)
            .min_scrolled_height(110.0)
            .show(ui, |ui| {
                ui.set_min_width(220.0);
                for (at, (export, shown)) in draft.items.iter().enumerate() {
                    let label = if export == shown {
                        shown.clone()
                    } else {
                        format!("{shown}  ({export})")
                    };
                    if ui
                        .selectable_label(draft.picked_item == Some(at), label)
                        .clicked()
                    {
                        draft.picked_item = Some(at);
                    }
                }
            });
        ui.vertical(|ui| {
            let picked = draft.picked_item.filter(|at| *at < draft.items.len());
            if ui
                .add_enabled(
                    picked.is_some(),
                    egui::Button::new(Message::FieldDeleteItem.say(lang)),
                )
                .clicked()
                && let Some(at) = picked
            {
                draft.items.remove(at);
                draft.picked_item = None;
            }
            if ui
                .add_enabled(
                    picked.is_some_and(|at| at > 0),
                    egui::Button::new(Message::FieldItemUp.say(lang)),
                )
                .clicked()
                && let Some(at) = picked
            {
                draft.items.swap(at, at - 1);
                draft.picked_item = Some(at - 1);
            }
            if ui
                .add_enabled(
                    picked.is_some_and(|at| at + 1 < draft.items.len()),
                    egui::Button::new(Message::FieldItemDown.say(lang)),
                )
                .clicked()
                && let Some(at) = picked
            {
                draft.items.swap(at, at + 1);
                draft.picked_item = Some(at + 1);
            }
        });
    });
}

fn list_options(ui: &mut egui::Ui, draft: &mut FieldDraft, kind: FieldKind, lang: Lang) {
    item_editor(ui, draft, lang);
    flag(
        ui,
        &mut draft.flags,
        flags::SORT,
        Message::FieldSort.say(lang),
    );
    if kind == FieldKind::Combo {
        flag(
            ui,
            &mut draft.flags,
            flags::EDIT,
            Message::FieldCustomText.say(lang),
        );
        flag_off(
            ui,
            &mut draft.flags,
            flags::DO_NOT_SPELL_CHECK,
            Message::FieldSpellCheck.say(lang),
        );
    } else {
        flag(
            ui,
            &mut draft.flags,
            flags::MULTI_SELECT,
            Message::FieldMultiSelect.say(lang),
        );
    }
    flag(
        ui,
        &mut draft.flags,
        flags::COMMIT_ON_SEL_CHANGE,
        Message::FieldCommitAtOnce.say(lang),
    );
}

impl Window {
    pub(crate) fn field_properties(&mut self, ctx: &egui::Context) {
        if self.tool != Tool::Form {
            self.field_draft = None;
            return;
        }
        self.settle_the_landing();
        let Some((page, found)) = self.chosen_now() else {
            self.field_draft = None;
            return;
        };
        let Some(laid) = self.laid.iter().copied().find(|laid| laid.page == page) else {
            return;
        };
        let field = found.field.clone();
        let fresh = draft_of(&field);
        if self.field_draft.is_none() || self.field_draft_origin.as_ref() != Some(&fresh) {
            self.field_draft = Some(fresh.clone());
            self.field_draft_origin = Some(fresh);
        }
        let lang = self.lang;
        let area = box_on_screen(laid.placed, found.pixels);
        let placed = beside_or_under(area, ctx.content_rect());
        let mut apply = false;
        let mut delete = false;
        let mut tab = self.properties_tab;
        let Some(draft) = self.field_draft.as_mut() else {
            return;
        };
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a position on screen, rounded to a whole pixel to name the window"
        )]
        egui::Window::new(Message::FieldProperties.say(lang))
            .id(egui::Id::new((
                "form field properties",
                placed.x as i32,
                placed.y as i32,
            )))
            .collapsible(false)
            .resizable(false)
            .default_pos(placed)
            .constrain(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    for (choice, word) in [
                        (PropertiesTab::General, Message::FieldGeneral),
                        (PropertiesTab::Appearance, Message::FieldAppearance),
                        (PropertiesTab::Options, Message::FieldOptions),
                    ] {
                        ui.selectable_value(&mut tab, choice, word.say(lang));
                    }
                });
                ui.separator();
                match tab {
                    PropertiesTab::General => general(ui, draft, lang),
                    PropertiesTab::Appearance => appearance(ui, draft, field.kind, lang),
                    PropertiesTab::Options => match field.kind {
                        FieldKind::Text => text_options(ui, draft, lang),
                        FieldKind::Checkbox | FieldKind::Radio => {
                            button_options(ui, draft, field.kind, lang);
                        }
                        FieldKind::Combo | FieldKind::List => {
                            list_options(ui, draft, field.kind, lang);
                        }
                        FieldKind::Push => push_options(ui, draft, lang),
                        FieldKind::Signature => {
                            ui.label(Message::FieldNoOptions.say(lang));
                        }
                    },
                }
                ui.separator();
                ui.horizontal(|ui| {
                    apply = ui.button(Message::Apply.say(lang)).clicked();
                    delete = ui.button(Message::DeleteField.say(lang)).clicked();
                });
            });
        self.properties_tab = tab;
        if delete {
            self.remove_the_chosen_fields();
            return;
        }
        if !apply {
            return;
        }
        let Some(draft) = self.field_draft.clone() else {
            return;
        };
        let settings = settings_changed(&field, &draft);
        if settings.is_empty() {
            return;
        }
        let job = self
            .editor
            .begin_set_field_settings(page, field.widget, settings);
        if job.is_none() {
            self.editor.say(Message::AnotherEditIsRunning);
            return;
        }
        self.send(job);
    }
}

#[cfg(test)]
mod tests {
    use super::{PANEL_WIDTH, beside_or_under, draft_of, settings_changed};
    use pdf_edit::form::{
        BorderStyle, ButtonStyle, FieldKind, FieldValue, FormField, Quadding, flags,
    };
    use pdf_syntax::Reference;

    fn a_list() -> FormField {
        FormField {
            field: Reference::new(4, 0),
            widget: Reference::new(4, 0),
            kind: FieldKind::Combo,
            name: "Country".to_owned(),
            rect: [0.0, 0.0, 100.0, 20.0],
            value: FieldValue::Empty,
            read_only: false,
            required: false,
            multiline: false,
            password: false,
            max_len: None,
            quadding: Quadding::Left,
            appearance: b"/Helv 0 Tf 0 g".to_vec(),
            states: Vec::new(),
            options: vec!["Thailand".to_owned(), "Laos".to_owned()],
            shown_state: None,
            border: Some([0.0, 0.0, 0.0]),
            background: None,
            flags: flags::COMBO,
            tooltip: String::new(),
            annotation_flags: 4,
            border_width: 1.0,
            border_style: BorderStyle::Solid,
            button_style: ButtonStyle::Check,
            default_value: FieldValue::Empty,
            option_exports: vec!["TH".to_owned(), "Laos".to_owned()],
            text_colour: [0.0, 0.0, 0.0],
            rotation: 0,
            date_format: None,
            caption: String::new(),
            link: None,
        }
    }

    #[test]
    fn a_window_left_alone_changes_nothing() {
        let field = a_list();
        assert!(settings_changed(&field, &draft_of(&field)).is_empty());
    }

    #[test]
    fn a_window_asks_only_for_what_changed() {
        let field = a_list();
        let mut draft = draft_of(&field);
        draft.required = true;
        draft.fits = false;
        draft.size = 10.0;
        draft.flags |= flags::SORT;
        draft.items.push(("VN".to_owned(), "Vietnam".to_owned()));
        draft.border_on = false;
        let settings = settings_changed(&field, &draft);
        assert_eq!(settings.required, Some(true));
        assert_eq!(settings.text_size, Some(Some(10.0)));
        assert_eq!(settings.set_flags, flags::SORT);
        assert_eq!(settings.clear_flags, 0);
        assert_eq!(settings.border, Some(None));
        assert_eq!(
            settings.options.as_ref().map(|items| items[0].0.clone()),
            Some("TH".to_owned())
        );
        assert_eq!(settings.name, None);
        assert_eq!(settings.read_only, None);
        assert_eq!(settings.quadding, None);
        assert_eq!(settings.fill, None);
    }

    #[test]
    fn the_window_never_covers_its_field() {
        let screen = eframe::egui::Rect::from_min_max(
            eframe::egui::pos2(0.0, 0.0),
            eframe::egui::pos2(1250.0, 900.0),
        );
        for area in [
            eframe::egui::Rect::from_min_max(
                eframe::egui::pos2(100.0, 100.0),
                eframe::egui::pos2(300.0, 120.0),
            ),
            eframe::egui::Rect::from_min_max(
                eframe::egui::pos2(540.0, 340.0),
                eframe::egui::pos2(900.0, 362.0),
            ),
        ] {
            let at = beside_or_under(area, screen);
            let window =
                eframe::egui::Rect::from_min_size(at, eframe::egui::vec2(PANEL_WIDTH, 300.0));
            assert!(!window.intersects(area), "{area:?} {window:?}");
        }
    }
}
