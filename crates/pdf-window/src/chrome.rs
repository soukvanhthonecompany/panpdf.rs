use std::path::Path;
use std::sync::Arc;

use eframe::egui;

use pdf_app::Editor;
use pdf_app::wording::{Command, Lang, Message};
use pdf_bytes::{ByteStore, SourceId};

use crate::app::name_of;
use crate::icons::Icon;
use crate::room;
use crate::window_state::{
    LeaveChoice, Leaving, Opened, Opening, Pointing, Tool, Window, set_dark,
};

const MM_PER_POINT: f64 = 25.4 / 72.0;

pub(crate) const A4: [f64; 2] = [595.28, 841.89];

const PAPER_SIZES: &[(&str, [f64; 2])] = &[
    ("A3", [841.89, 1190.55]),
    ("A4", A4),
    ("A5", [419.53, 595.28]),
    ("B4", [708.66, 1000.63]),
    ("B5", [498.9, 708.66]),
    ("Letter", [612.0, 792.0]),
    ("Legal", [612.0, 1008.0]),
    ("Tabloid", [792.0, 1224.0]),
];

pub(crate) fn open_bytes(source: ByteStore, credential: &[u8]) -> Opened {
    if pdf_edit::info::lock(&source, credential) == pdf_edit::info::Lock::Refused {
        return Opened::Locked(source);
    }
    match Editor::open_with(source, credential) {
        Ok(editor) => Opened::Document(Box::new(editor)),
        Err(reason) => Opened::Refused(reason),
    }
}

impl Window {
    fn file_menu(&mut self, ui: &mut egui::Ui, (idle, working): (bool, bool)) {
        let lang = self.lang;
        let say = |command| Message::Command(command).say(lang);
        let home = self.has_document() && !self.home;
        if ui
            .add_enabled(home, egui::Button::new(say(Command::Home)))
            .clicked()
        {
            self.go_home();
            ui.close();
        }
        ui.separator();
        ui.add_enabled_ui(idle, |ui| {
            ui.menu_button(say(Command::NewDocument), |ui| {
                if let Some(size) = self.page_size_menu(ui) {
                    self.new_document(size);
                    ui.close();
                }
            });
        });
        if ui
            .add_enabled(idle, egui::Button::new(say(Command::Open)))
            .clicked()
        {
            self.asking_to_open = true;
            ui.close();
        }
        ui.add_enabled_ui(self.library.len() > 1, |ui| {
            ui.menu_button(say(Command::Documents), |ui| {
                for path in self.library.clone() {
                    let open = path == self.opened;
                    let button = egui::Button::selectable(open, name_of(&path));
                    if ui.add_enabled(idle || open, button).clicked() {
                        self.open(&path);
                        ui.close();
                    }
                }
            });
        });
        ui.separator();
        let save = egui::Button::new(say(Command::Save)).shortcut_text("Ctrl+S");
        if ui.add_enabled(working, save).clicked() {
            self.save();
            ui.close();
        }
        let save_as = egui::Button::new(say(Command::SaveAs)).shortcut_text("Ctrl+Shift+S");
        if ui.add_enabled(working, save_as).clicked() {
            self.save_a_copy_as();
            ui.close();
        }
        let split = egui::Button::new(say(Command::SplitDocument));
        if ui.add_enabled(working, split).clicked() {
            self.open_the_split_panel();
            ui.close();
        }
        ui.separator();
        let pictures = egui::Button::new(say(Command::PagesAsPictures));
        if ui.add_enabled(working, pictures).clicked() {
            self.open_the_export_panel();
            ui.close();
        }
        if ui.button(say(Command::PdfFromPictures)).clicked() {
            self.choose_pictures(None);
            ui.close();
        }
        ui.separator();
        let print = egui::Button::new(say(Command::Print)).shortcut_text("Ctrl+P");
        if ui.add_enabled(working, print).clicked() {
            self.open_the_print_dialog();
            ui.close();
        }
        ui.separator();
        let properties =
            egui::Button::new(pdf_app::wording::Fact::Properties.say(lang)).shortcut_text("Ctrl+D");
        if ui.add_enabled(working, properties).clicked() {
            self.open_the_properties();
            ui.close();
        }
    }

    pub(crate) fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let say = |command| Message::Command(command).say(lang);
        let idle = !self.editor.is_busy() && self.loading.is_none();
        let working = idle && self.has_document() && !self.home;
        egui::Panel::top("menu").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button(say(Command::File), |ui| {
                    self.file_menu(ui, (idle, working));
                });
                ui.menu_button(say(Command::Edit), |ui| {
                    let undo = egui::Button::new(say(Command::Undo)).shortcut_text("Ctrl+Z");
                    if ui
                        .add_enabled(working && self.editor.can_undo(), undo)
                        .clicked()
                    {
                        self.walk_history(true);
                        ui.close();
                    }
                    let redo = egui::Button::new(say(Command::Redo)).shortcut_text("Ctrl+Y");
                    if ui
                        .add_enabled(working && self.editor.can_redo(), redo)
                        .clicked()
                    {
                        self.walk_history(false);
                        ui.close();
                    }
                    ui.separator();
                    let copy = egui::Button::new(say(Command::Copy)).shortcut_text("Ctrl+C");
                    if ui.add_enabled(working && self.selected(), copy).clicked() {
                        let ctx = ui.ctx().clone();
                        let in_text = self.pointing.editing();
                        self.copy(&ctx, in_text);
                        ui.close();
                    }
                    let cut = egui::Button::new(say(Command::Cut)).shortcut_text("Ctrl+X");
                    if ui.add_enabled(working && self.selected(), cut).clicked() {
                        let ctx = ui.ctx().clone();
                        let in_text = self.pointing.editing();
                        self.cut(&ctx, in_text);
                        ui.close();
                    }
                    let holding = self.clipboard.is_some();
                    let paste = egui::Button::new(say(Command::Paste)).shortcut_text("Ctrl+V");
                    if ui.add_enabled(working && holding, paste).clicked() {
                        let ctx = ui.ctx().clone();
                        self.paste_the_clipboard(&ctx, false);
                        ui.close();
                    }
                    let in_place =
                        egui::Button::new(say(Command::PasteInPlace)).shortcut_text("Ctrl+Shift+V");
                    if ui.add_enabled(working && holding, in_place).clicked() {
                        let ctx = ui.ctx().clone();
                        self.paste_the_clipboard(&ctx, true);
                        ui.close();
                    }
                    let delete = egui::Button::new(say(Command::Delete)).shortcut_text("Del");
                    if ui.add_enabled(working && self.selected(), delete).clicked() {
                        self.delete();
                        ui.close();
                    }
                    ui.separator();
                    let ordering = working && self.can_order();
                    for (command, order) in [
                        (Command::BringToFront, pdf_edit::Stacking::ToFront),
                        (Command::BringForward, pdf_edit::Stacking::Forward),
                        (Command::SendBackward, pdf_edit::Stacking::Backward),
                        (Command::SendToBack, pdf_edit::Stacking::ToBack),
                    ] {
                        if ui
                            .add_enabled(ordering, egui::Button::new(say(command)))
                            .clicked()
                        {
                            self.put_in_order(order);
                            ui.close();
                        }
                    }
                    ui.separator();
                    let find = egui::Button::new(say(Command::Find)).shortcut_text("Ctrl+F");
                    if ui.add_enabled(working, find).clicked() {
                        self.open_the_find_bar();
                        ui.close();
                    }
                    if self.editor.editing_restricted() {
                        ui.separator();
                        let allow = egui::Button::new(say(Command::AllowEditing));
                        if ui.add_enabled(working, allow).clicked() {
                            self.restriction_answered = false;
                            ui.close();
                        }
                    }
                });
                ui.menu_button(say(Command::Insert), |ui| self.insert_menu(ui, working));
                ui.menu_button(say(Command::Page), |ui| self.page_menu(ui, working));
                ui.menu_button(say(Command::Tools), |ui| self.tools_menu(ui, working));
                ui.menu_button(say(Command::View), |ui| self.view_menu(ui, working));
                #[cfg(not(target_arch = "wasm32"))]
                ui.menu_button(say(Command::Help), |ui| self.help_menu(ui));
            });
        });
    }

    fn insert_menu(&mut self, ui: &mut egui::Ui, working: bool) {
        let lang = self.lang;
        let say = |command| Message::Command(command).say(lang);
        ui.add_enabled_ui(working, |ui| {
            if ui.button(say(Command::InsertText)).clicked() {
                self.pictures.clear();
                self.tool = Tool::Text;
                ui.close();
            }
            if ui.button(say(Command::InsertPictures)).clicked() {
                self.choosing_for = crate::page_actions::Choosing::Picture;
                self.asking_to_open = true;
                ui.close();
            }
            for (command, tool) in [
                (Command::Shape, Tool::Shape),
                (Command::Pen, Tool::Pen),
                (Command::Highlighter, Tool::Highlighter),
            ] {
                if ui.button(say(command)).clicked() {
                    self.take_up(tool);
                    ui.close();
                }
            }
            if ui.button(say(Command::Link)).clicked() {
                self.take_up_the_link_tool();
                ui.close();
            }
            if ui.button(say(Command::InsertField)).clicked() {
                self.take_up_the_form_tool();
                ui.close();
            }
            ui.separator();
            self.pages_in_menu(ui);
        });
    }

    fn tools_menu(&mut self, ui: &mut egui::Ui, working: bool) {
        let lang = self.lang;
        let say = |command| Message::Command(command).say(lang);
        ui.add_enabled_ui(working, |ui| {
            #[cfg(not(target_arch = "wasm32"))]
            {
                if ui.button(say(Command::AiAssistant)).clicked() {
                    self.open_ai_panel();
                    ui.close();
                }
                if ui.button(say(Command::ConnectAgents)).clicked() {
                    self.open_the_agents_window();
                    ui.close();
                }
                ui.separator();
            }
            if ui.button(say(Command::RecognizeText)).clicked() {
                self.open_the_ocr_panel();
                ui.close();
            }
            ui.separator();
            for (command, kind) in [
                (
                    Command::StampPageNumbers,
                    crate::stamp_tool::StampKind::PageNumbers,
                ),
                (
                    Command::StampHeaderFooter,
                    crate::stamp_tool::StampKind::HeaderFooter,
                ),
                (
                    Command::StampWatermark,
                    crate::stamp_tool::StampKind::Watermark,
                ),
            ] {
                if ui.button(say(command)).clicked() {
                    self.open_the_stamp_panel(kind);
                    ui.close();
                }
            }
        });
    }

    fn view_menu(&mut self, ui: &mut egui::Ui, working: bool) {
        let lang = self.lang;
        let say = |command| Message::Command(command).say(lang);
        let closer = egui::Button::new(say(Command::ZoomIn)).shortcut_text("Ctrl++");
        if ui.add_enabled(working, closer).clicked() {
            self.zoom_by(true, None);
        }
        let further = egui::Button::new(say(Command::ZoomOut)).shortcut_text("Ctrl+-");
        if ui.add_enabled(working, further).clicked() {
            self.zoom_by(false, None);
        }
        ui.separator();
        let mut pages = !self.pages_folded;
        if ui.checkbox(&mut pages, say(Command::Pages)).changed() {
            let now = ui.input(|input| input.time);
            self.fold_the_pages(!pages, now);
        }
        let _ = ui.checkbox(&mut self.show_contents, say(Command::Contents));
        ui.menu_button(say(Command::Frames), |ui| {
            let all = egui::Button::new(say(Command::ShowFrames))
                .selected(self.show_frames)
                .shortcut_text("F2");
            if ui.add(all).clicked() {
                self.show_frames = !self.show_frames;
            }
            ui.separator();
            ui.add_enabled_ui(self.show_frames, |ui| {
                let _ = ui.checkbox(&mut self.framed.text, say(Command::FramesOfText));
                let _ = ui.checkbox(&mut self.framed.pictures, say(Command::FramesOfPictures));
                let _ = ui.checkbox(&mut self.framed.drawings, say(Command::FramesOfDrawings));
            });
        });
        if ui
            .checkbox(&mut self.dark, say(Command::DarkMode))
            .changed()
        {
            set_dark(ui.ctx(), self.dark);
        }
        let _ = ui.checkbox(&mut self.show_speed, say(Command::ShowDrawingSpeed));
        ui.menu_button(say(Command::Language), |ui| {
            for (language, named) in language_rows() {
                let _ = ui.radio_value(&mut self.lang, language, named);
            }
        });
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn help_menu(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let say = |command| Message::Command(command).say(lang);
        if ui.button(say(Command::ReportAProblem)).clicked() {
            self.open_out(&crate::reporting::report_link());
            ui.close();
        }
        let folder = crate::reporting::log_folder();
        let show = egui::Button::new(say(Command::ShowTheLog));
        if ui.add_enabled(folder.is_some(), show).clicked() {
            if let Some(folder) = folder {
                self.open_out(&folder.to_string_lossy());
            }
            ui.close();
        }
    }

    pub(crate) fn page_size_menu(&self, ui: &mut egui::Ui) -> Option<[f64; 2]> {
        let say = |command| Message::Command(command).say(self.lang);
        let turned = |[width, height]: [f64; 2]| {
            if self.landscape == (width > height) {
                [width, height]
            } else {
                [height, width]
            }
        };
        let points = |[width, height]: [f64; 2]| {
            format!(
                "{:.0} × {:.0} mm",
                width * MM_PER_POINT,
                height * MM_PER_POINT
            )
        };
        let mut chosen = None;
        let showing = self.has_document() && !self.home;
        if let Some(geometry) = showing.then(|| self.editor.geometry(self.focus)).flatten() {
            let [x0, y0, x1, y1] = geometry.media_box;
            let size = [(x1 - x0).abs(), (y1 - y0).abs()];
            let label = format!("{} ({})", say(Command::SameSizeAsThisPage), points(size));
            if ui.button(label).clicked() {
                chosen = Some(size);
            }
            ui.separator();
        }
        for (name, size) in PAPER_SIZES {
            let size = turned(*size);
            if ui.button(format!("{name} ({})", points(size))).clicked() {
                chosen = Some(size);
            }
        }
        chosen
    }

    pub(crate) fn selected(&self) -> bool {
        matches!(
            self.pointing,
            Pointing::Text { caret, .. } if caret.at != caret.anchor
        ) || matches!(self.pointing, Pointing::Block { .. })
            || (self.tool == Tool::Form && self.chosen_fields.is_some())
            || self.pointing.object_on(self.focus).is_some()
    }

    pub(crate) fn toolbar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                let room = ui.available_width();
                let idle = !self.editor.is_busy() && self.loading.is_none();
                let lang = self.lang;
                let slots = self.bar_slots(ui, idle, lang);

                let bar = room::Bar {
                    buttons: slots.iter().filter(|slot| slot.is_button()).count(),
                    rules: slots.iter().filter(|slot| slot.is_rule()).count(),
                    choices: self.toolbar_choices,
                    readouts: slots.iter().map(Slot::readout_width).sum(),
                    slack: self.toolbar_slack,
                };
                let labels = room::labels_fit(room, &bar, !self.toolbar_compact);
                self.toolbar_compact = !labels;

                let mut pieces: Vec<room::Piece> =
                    slots.iter().map(|slot| slot.piece(labels)).collect();
                pieces.push(room::Piece {
                    width: self.toolbar_choices + room::EDGE + self.toolbar_slack,
                    rank: 0,
                });
                let cut = room::shed_above(room, &pieces, room::OVERFLOW_WIDTH);

                let start = ui.cursor().min.x;
                ui.add_space(4.0);
                let mut pressed = None;
                let mut choices_width = 0.0;
                let mut drawn = 0.0;
                let mut anything_yet = false;
                for slot in slots.iter().filter(|slot| slot.side == Side::Left) {
                    if let Some(command) =
                        self.draw_slot(ui, slot, cut, &mut anything_yet, &mut drawn)
                    {
                        pressed = Some(command);
                    }
                }
                if self.tool_has_choices() {
                    toolbar_separator(ui);
                    let before = ui.cursor().min.x;
                    self.tool_choices(ui);
                    choices_width = ui.cursor().min.x - before;
                }
                self.toolbar_choices = choices_width;

                let left_end = ui.cursor().min.x;
                let mut right_width = 0.0;
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.editor.is_busy() || self.loading.is_some() {
                        ui.spinner();
                    }
                    if cut != room::ALL
                        && let Some(command) = Self::overflow_menu(ui, &slots, cut, lang)
                    {
                        pressed = Some(command);
                    }
                    let mut yet = true;
                    for slot in slots.iter().filter(|slot| slot.side == Side::Right) {
                        if let Some(command) = self.draw_slot(ui, slot, cut, &mut yet, &mut drawn) {
                            pressed = Some(command);
                        }
                    }
                    right_width = ui.min_rect().width();
                });
                let measured = (left_end - start - choices_width) + right_width;
                self.toolbar_slack = (measured - drawn).clamp(0.0, 80.0);
                if let Some(command) = pressed {
                    let ctx = ui.ctx().clone();
                    self.run_from_the_bar(&ctx, command);
                }
                self.let_go_of_text_the_tool_may_not_hold();
            });
            if self.finding.is_some() {
                self.find_bar(ui);
            }
            ui.add_space(3.0);
        });
    }

    fn bar_slots(&self, ui: &egui::Ui, idle: bool, lang: Lang) -> Vec<Slot> {
        let button = |icon, command, enabled, on, rank, side| Slot {
            side,
            rank,
            what: What::Button {
                icon,
                command,
                enabled,
                on,
            },
        };
        let rule = |rank, side| Slot {
            side,
            rank,
            what: What::Rule,
        };
        let mut slots = vec![
            button(
                Icon::Home,
                Command::Home,
                idle,
                false,
                HOME_RANK,
                Side::Left,
            ),
            button(
                Icon::NewDocument,
                Command::NewDocument,
                idle,
                false,
                HOME_RANK,
                Side::Left,
            ),
            button(
                Icon::Open,
                Command::Open,
                idle,
                false,
                OPEN_RANK,
                Side::Left,
            ),
            button(
                Icon::Save,
                Command::Save,
                idle,
                false,
                SAVE_RANK,
                Side::Left,
            ),
            rule(HISTORY_RANK, Side::Left),
            button(
                Icon::Undo,
                Command::Undo,
                idle && self.editor.can_undo(),
                false,
                HISTORY_RANK,
                Side::Left,
            ),
            button(
                Icon::Redo,
                Command::Redo,
                idle && self.editor.can_redo(),
                false,
                HISTORY_RANK,
                Side::Left,
            ),
            rule(0, Side::Left),
        ];
        for (icon, command, tool) in TOOLS {
            slots.push(button(
                icon,
                command,
                if tool == Tool::Select { true } else { idle },
                self.tool == tool,
                0,
                Side::Left,
            ));
        }
        slots.push(button(
            Icon::Delete,
            Command::Delete,
            idle && self.selected(),
            false,
            DELETE_RANK,
            Side::Left,
        ));
        slots.extend(self.view_slots(ui, lang));
        slots
    }

    fn view_slots(&self, ui: &egui::Ui, lang: Lang) -> Vec<Slot> {
        let button = |icon, command, enabled, rank| Slot {
            side: Side::Right,
            rank,
            what: What::Button {
                icon,
                command,
                enabled,
                on: false,
            },
        };
        let rule = |rank| Slot {
            side: Side::Right,
            rank,
            what: What::Rule,
        };
        let readout = |text: String, rank| Slot {
            side: Side::Right,
            rank,
            what: What::Readout {
                width: text_width(ui, &text),
                text,
            },
        };
        let mut slots = Vec::new();
        slots.push(button(Icon::Theme, Command::Theme, true, THEME_RANK));
        slots.push(rule(THEME_RANK));
        slots.push(button(Icon::ZoomIn, Command::ZoomIn, true, ZOOM_RANK));
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a zoom is between 25% and 400%"
        )]
        let percent = (self.zoom * 100.0).round() as u32;
        slots.push(readout(Message::ZoomPercent(percent).say(lang), ZOOM_RANK));
        slots.push(button(Icon::ZoomOut, Command::ZoomOut, true, ZOOM_RANK));
        slots.push(rule(PAGING_RANK));
        let pages = self.editor.page_count();
        slots.push(button(
            Icon::Next,
            Command::NextPage,
            self.focus + 1 < pages,
            PAGING_RANK,
        ));
        slots.push(readout(
            Message::PageOf {
                page: self.focus + 1,
                count: pages,
            }
            .say(lang),
            PAGING_RANK,
        ));
        slots.push(button(
            Icon::Previous,
            Command::PreviousPage,
            self.focus > 0,
            PAGING_RANK,
        ));
        slots
    }

    fn draw_slot(
        &self,
        ui: &mut egui::Ui,
        slot: &Slot,
        cut: u8,
        anything_yet: &mut bool,
        drawn: &mut f32,
    ) -> Option<Command> {
        if slot.rank >= cut {
            return None;
        }
        *drawn += slot.piece(!self.toolbar_compact).width;
        match &slot.what {
            What::Rule => {
                if *anything_yet {
                    toolbar_separator(ui);
                }
                None
            }
            What::Readout { text, .. } => {
                *anything_yet = true;
                ui.label(text);
                None
            }
            What::Button {
                icon,
                command,
                enabled,
                on,
            } => {
                *anything_yet = true;
                self.tool_button(ui, *icon, *command, *enabled, *on)
                    .then_some(*command)
            }
        }
    }

    fn overflow_menu(ui: &mut egui::Ui, slots: &[Slot], cut: u8, lang: Lang) -> Option<Command> {
        let name = Message::Command(Command::MoreForPage).say(lang);
        let opened = crate::format::icon_button(ui, Icon::More, &name, false, true);
        let mut pressed = None;
        egui::Popup::menu(&opened).show(|ui| {
            ui.set_min_width(180.0);
            for slot in slots {
                if slot.rank < cut {
                    continue;
                }
                let What::Button {
                    command, enabled, ..
                } = slot.what
                else {
                    continue;
                };
                let label = Message::Command(command).say(lang);
                if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                    pressed = Some(command);
                    ui.close();
                }
            }
        });
        pressed
    }

    fn run_from_the_bar(&mut self, ctx: &egui::Context, command: Command) {
        match command {
            Command::Home => self.go_home(),
            Command::NewDocument => self.new_document(A4),
            Command::Open => self.asking_to_open = true,
            Command::Save => {
                self.save();
            }
            Command::Undo => {
                self.walk_history(true);
            }
            Command::Redo => {
                self.walk_history(false);
            }
            Command::Delete => {
                if !self.remove_the_chosen_fields() {
                    self.delete();
                }
            }
            Command::Select => {
                self.tool = Tool::Select;
                self.pictures.clear();
                self.ink = None;
            }
            Command::Text => {
                self.pictures.clear();
                self.tool = Tool::Text;
            }
            Command::Pen => self.take_up(Tool::Pen),
            Command::Highlighter => self.take_up(Tool::Highlighter),
            Command::Shape => self.take_up(Tool::Shape),
            Command::Form => self.take_up_the_form_tool(),
            Command::Link => self.take_up_the_link_tool(),
            Command::Picture => {
                self.choosing_for = crate::page_actions::Choosing::Picture;
                self.asking_to_open = true;
            }
            Command::Theme => {
                self.dark = !self.dark;
                set_dark(ctx, self.dark);
            }
            Command::ZoomIn => self.zoom_by(true, None),
            Command::ZoomOut => self.zoom_by(false, None),
            Command::NextPage => self.goto(self.focus + 1),
            Command::PreviousPage => self.goto(self.focus.saturating_sub(1)),
            _ => {}
        }
    }

    fn let_go_of_text_the_tool_may_not_hold(&mut self) {
        if self.tool.edits_text() {
            return;
        }
        let kept = self.pointing;
        self.point_at(kept);
        self.drop_the_text_draft();
    }

    fn tool_has_choices(&self) -> bool {
        self.tool.is_drawing() || matches!(self.tool, Tool::Form | Tool::Link)
    }

    fn tool_choices(&mut self, ui: &mut egui::Ui) {
        match self.tool {
            Tool::Form => self.form_tool_choices(ui),
            Tool::Link => self.link_tool_choices(ui),
            Tool::Shape => {
                self.shape_choices(ui);
                crate::format::rule(ui);
                self.pen_choices(ui);
            }
            Tool::Pen | Tool::Highlighter => self.pen_choices(ui),
            Tool::Select | Tool::Text | Tool::Picture => {}
        }
    }

    pub(crate) fn tool_hint(&self) -> Option<String> {
        match self.tool {
            Tool::Form => self.form_tool_hint(),
            Tool::Link => self.link_tool_hint(),
            Tool::Select
            | Tool::Text
            | Tool::Pen
            | Tool::Highlighter
            | Tool::Shape
            | Tool::Picture => None,
        }
    }

    fn tool_button(
        &self,
        ui: &mut egui::Ui,
        icon: Icon,
        command: Command,
        enabled: bool,
        on: bool,
    ) -> bool {
        let name = Message::Command(command).say(self.lang);
        let width = room::tool_width(!self.toolbar_compact);
        let size = egui::vec2(width, room::TOOL_HEIGHT);
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
        let response = response.on_hover_text(&name);
        let visuals = ui.visuals();
        let colour = if !enabled {
            visuals.weak_text_color()
        } else if on {
            visuals.selection.stroke.color
        } else {
            visuals.text_color()
        };
        if ui.is_rect_visible(rect) {
            if on {
                ui.painter()
                    .rect_filled(rect, 4.0, visuals.selection.bg_fill.gamma_multiply(0.35));
            } else if enabled && response.hovered() {
                ui.painter()
                    .rect_filled(rect, 4.0, visuals.widgets.hovered.bg_fill);
            }
            let top = if self.toolbar_compact {
                rect.center().y - room::ICON_SIDE / 2.0
            } else {
                rect.top() + 4.0
            };
            let glyph = egui::Rect::from_min_size(
                egui::pos2(rect.center().x - room::ICON_SIDE / 2.0, top),
                egui::vec2(room::ICON_SIDE, room::ICON_SIDE),
            );
            icon.draw_tinted(ui.painter(), glyph, colour, enabled);
            if self.toolbar_compact {
                return enabled && response.clicked();
            }
            ui.painter().text(
                egui::pos2(rect.center().x, rect.bottom() - 3.0),
                egui::Align2::CENTER_BOTTOM,
                &name,
                egui::FontId::proportional(10.0),
                colour,
            );
        }
        enabled && response.clicked()
    }

    pub(crate) fn open(&mut self, path: &Path) {
        self.open_at(path, 0);
    }

    pub(crate) fn open_at(&mut self, path: &Path, page: usize) {
        if self.editor.is_busy() || self.loading.is_some() || self.unlocking.is_some() {
            return;
        }
        if *path == self.opened {
            self.home = false;
            self.goto(page.min(self.editor.page_count().saturating_sub(1)));
            return;
        }
        if self.unsaved() {
            self.leaving = Some(Leaving::Open(path.to_path_buf(), page));
            return;
        }
        self.open_now(path, page);
    }

    fn open_now(&mut self, path: &Path, page: usize) {
        #[cfg(not(target_arch = "wasm32"))]
        crate::reporting::say(
            pdf_app::trouble::Kind::Document,
            &format!("opening {}", path.display()),
        );
        self.remember_here();
        let wanted = path.to_path_buf();
        let reading = wanted.clone();
        let opening = Message::Opening(name_of(&wanted));
        self.editor.say(opening);
        self.loading = Some(Opening {
            path: wanted,
            page,
            changed_protection: false,
            tried_a_password: false,
            handle: std::thread::spawn(move || match std::fs::read(&reading) {
                Ok(bytes) => open_bytes(
                    ByteStore::new(SourceId::new(0), Arc::<[u8]>::from(bytes)),
                    b"",
                ),
                Err(error) => Opened::Refused(format!("{}: {error}", reading.display())),
            }),
        });
    }

    fn take_the_document(
        &mut self,
        editor: pdf_app::Editor,
        path: std::path::PathBuf,
        page: usize,
    ) {
        self.editor = editor;
        self.saved_epoch = self.editor.epoch();
        self.saved_digest = None;
        self.protection_changed = false;
        self.untitled = path.as_os_str().is_empty();
        self.destination = if self.untitled {
            std::path::PathBuf::new()
        } else {
            crate::save_file::unused_copy(&path)
        };
        self.title = if self.untitled {
            pdf_app::wording::Home::Untitled.say(self.lang)
        } else {
            path.display().to_string()
        };
        self.opened = path;
        self.resume = None;
        self.clipboard = None;
        self.input = pdf_app::draft::Input::default();
        self.restriction_answered = false;
        self.point_at(Pointing::Nothing);
        self.drag = None;
        self.landing = None;
        for (id, held, slot) in self.tiles.clear() {
            self.retire(id, held, slot);
        }
        self.scenes.clear();
        self.thumbs.clear();
        self.failed.clear();
        self.chosen_pages.clear();
        self.focus = 0;
        self.wanted_offset = Some(egui::Vec2::ZERO);
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(folder) = self.opened.parent() {
            self.library = crate::hub::pdfs_in(folder);
        }
        let last = self.editor.page_count().saturating_sub(1);
        if page > 0 {
            self.goto(page.min(last));
            self.focus = page.min(last);
        }
        self.home = false;
        self.remember_here();
    }

    pub(crate) fn read_the_new_document_key(&mut self, ctx: &egui::Context) {
        if self.leaving.is_none()
            && self.loading.is_none()
            && self.chooser.is_none()
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::N))
        {
            self.new_document(A4);
        }
    }

    pub(crate) fn new_document(&mut self, size: [f64; 2]) {
        if self.editor.is_busy() || self.loading.is_some() {
            return;
        }
        if self.unsaved() {
            self.leaving = Some(Leaving::New(size));
            return;
        }
        self.start_a_new_document(size);
    }

    fn start_a_new_document(&mut self, size: [f64; 2]) {
        match Editor::blank(size) {
            Ok(editor) => {
                self.remember_here();
                self.take_the_document(editor, std::path::PathBuf::new(), 0);
                self.editor.say(Message::StartedANewDocument);
            }
            Err(reason) => self.editor.say(Message::Plain(reason)),
        }
    }

    pub(crate) fn collect_open(&mut self, ctx: &egui::Context) {
        let Some(loading) = &self.loading else { return };
        if !loading.handle.is_finished() {
            ctx.request_repaint();
            return;
        }
        let Some(loading) = self.loading.take() else {
            return;
        };
        match loading.handle.join() {
            Ok(Opened::Document(editor)) => {
                self.take_the_document(*editor, loading.path, loading.page);
                if loading.changed_protection {
                    self.protection_changed = true;
                    self.editor.say(Message::Plain(
                        pdf_app::wording::Fact::ProtectionChanged.say(self.lang),
                    ));
                }
            }
            Ok(Opened::Locked(source)) => {
                self.editor.say(Message::Quiet);
                self.unlocking = Some(crate::unlock::Unlock {
                    path: loading.path,
                    page: loading.page,
                    source,
                    typed: String::new(),
                    tried: loading.tried_a_password,
                    shown: false,
                    focus: true,
                });
            }
            Ok(Opened::Refused(reason)) => self.editor.say(Message::Plain(reason)),
            Err(_) => {
                let failed = Message::OpeningFailed;
                self.editor.say(failed);
            }
        }
    }

    pub(crate) fn save(&mut self) -> bool {
        if self.input.pending() || self.input.draft().is_some() {
            self.editor.say(Message::ResolveDraftBeforeSaving);
            return false;
        }
        if self.untitled && self.destination.as_os_str().is_empty() {
            self.save_a_copy_as();
            return false;
        }
        match self.editor.export() {
            Ok(export) => match crate::save_file::save(
                &self.opened,
                &self.destination,
                &export.bytes,
                self.saved_digest.as_deref(),
            ) {
                Ok(digest) => {
                    self.saved_digest = Some(digest);
                    self.saved_epoch = self.editor.epoch();
                    self.protection_changed = false;
                    if self.untitled {
                        self.title = self.destination.display().to_string();
                        let saved = self.destination.clone();
                        self.remember_file(&saved);
                    }
                    let frame = self.frame;
                    let revision = self
                        .editor
                        .revision()
                        .map_or_else(|| "unknown".to_owned(), |revision| format!("r{revision}"));
                    let digest = pdf_content::sha256_hex(&export.bytes);
                    if let Some(trace) = self.trace.as_mut() {
                        trace.note(
                            frame,
                            "saved",
                            &format!(
                                "path={} bytes={} revision={revision} sha256={digest}",
                                self.destination.display(),
                                export.bytes.len()
                            ),
                        );
                    }
                    #[cfg(not(target_arch = "wasm32"))]
                    crate::reporting::say(
                        pdf_app::trouble::Kind::Document,
                        &format!(
                            "saved {} ({} bytes)",
                            self.destination.display(),
                            export.bytes.len()
                        ),
                    );
                    let said = Message::SavedTo {
                        name: self.destination.display().to_string(),
                        bytes: export.bytes.len() as u64,
                    };
                    self.editor.say(said);
                    true
                }
                Err(error) => {
                    #[cfg(not(target_arch = "wasm32"))]
                    crate::reporting::say(
                        pdf_app::trouble::Kind::Failed,
                        &format!("could not save {}: {error}", self.destination.display()),
                    );
                    let said = Message::CouldNotSave {
                        name: self.destination.display().to_string(),
                        why: error.to_string(),
                    };
                    self.editor.say(said);
                    false
                }
            },
            Err(reason) => {
                self.editor.say(Message::Plain(reason));
                false
            }
        }
    }

    pub(crate) fn unsaved(&self) -> bool {
        self.editor.is_busy()
            || self.protection_changed
            || self.editor.epoch() != self.saved_epoch
            || self.input.pending()
            || self.input.draft().is_some()
    }

    pub(crate) fn guard_close(&mut self, ctx: &egui::Context) {
        if ctx.input(|input| input.viewport().close_requested())
            && !self.close_confirmed
            && (self.unsaved() || self.loading.is_some())
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.leaving = Some(Leaving::Close);
        }
    }

    pub(crate) fn asks_about_restrictions(&self) -> bool {
        !self.restriction_answered
            && !self.home
            && self.leaving.is_none()
            && self.loading.is_none()
            && self.unlocking.is_none()
            && self.editor.editing_restricted()
    }

    pub(crate) fn warn_of_restrictions(&mut self, ctx: &egui::Context) {
        if !self.asks_about_restrictions() {
            return;
        }
        let idle = !self.editor.is_busy();
        let mut edit = false;
        let mut read = false;
        let modal = egui::Modal::new(egui::Id::new("editing-restricted")).show(ctx, |ui| {
            ui.set_max_width(460.0);
            ui.heading(Message::EditingRestricted.say(self.lang));
            ui.label(Message::EditingRestrictedWarning.say(self.lang));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                edit = ui
                    .add_enabled(idle, egui::Button::new(Message::EditAnyway.say(self.lang)))
                    .clicked();
                read = ui.button(Message::ReadOnly.say(self.lang)).clicked();
            });
        });
        if (edit && self.editor.set_aside_restrictions()) || read || modal.should_close() {
            self.restriction_answered = true;
        }
    }

    pub(crate) fn confirm_leaving(&mut self, ctx: &egui::Context) {
        if self.leaving.is_none() {
            return;
        }
        let idle = !self.editor.is_busy() && self.loading.is_none() && !self.input.pending();
        let mut choice = None;
        egui::Modal::new(egui::Id::new("unsaved-document")).show(ctx, |ui| {
            ui.heading(Message::UnsavedChanges.say(self.lang));
            ui.label(Message::SaveBeforeLeaving.say(self.lang));
            if self.input.draft().is_some() {
                ui.label(Message::ResolveDraftBeforeSaving.say(self.lang));
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        idle && self.input.draft().is_none(),
                        egui::Button::new(Message::Command(Command::Save).say(self.lang)),
                    )
                    .clicked()
                {
                    choice = Some(LeaveChoice::Save);
                }
                if ui
                    .add_enabled(
                        idle,
                        egui::Button::new(Message::DiscardChanges.say(self.lang)),
                    )
                    .clicked()
                {
                    choice = Some(LeaveChoice::Discard);
                }
                if ui.button(Message::CancelLeaving.say(self.lang)).clicked() {
                    choice = Some(LeaveChoice::Cancel);
                }
            });
        });
        if let Some(choice) = choice {
            self.resolve_leaving(choice, ctx);
        }
    }

    pub(crate) fn resolve_leaving(&mut self, choice: LeaveChoice, ctx: &egui::Context) {
        match choice {
            LeaveChoice::Cancel => self.leaving = None,
            LeaveChoice::Save if !self.save() => {
                if self.chooser.is_some() {
                    self.leaving = None;
                }
            }
            LeaveChoice::Save | LeaveChoice::Discard => match self.leaving.take() {
                Some(Leaving::Open(path, page)) => self.open_now(&path, page),
                Some(Leaving::New(size)) => self.start_a_new_document(size),
                Some(Leaving::Close) => {
                    self.close_confirmed = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                None => {}
            },
        }
    }

    pub(crate) fn status_bar(&self, ui: &mut egui::Ui) {
        let status = self.editor.status().say(self.lang);
        let hint = self.tool_hint();
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                if let Some(hint) = hint {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(egui::RichText::new(hint).color(ui.visuals().weak_text_color()));
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.add(egui::Label::new(&status).truncate())
                                .on_hover_text(&status);
                        });
                    });
                } else {
                    ui.add(egui::Label::new(&status).truncate())
                        .on_hover_text(&status);
                }
            });
        });
    }

    pub(crate) fn draft_bar(&mut self, ui: &mut egui::Ui) {
        let Some(draft) = self.input.draft() else {
            return;
        };
        let text = draft.text.clone();
        let reason = draft.reason.say(self.lang);
        let retryable = draft.can_retry(self.editor.epoch());
        let idle = !self.editor.is_busy() && self.resume.is_none();
        let preview: String = {
            let mut shown: String = text
                .chars()
                .take(40)
                .map(|character| match character {
                    '\n' => '⏎',
                    pdf_edit::LINE_BREAK => '↵',
                    other => other,
                })
                .collect();
            if text.chars().count() > 40 {
                shown.push('…');
            }
            shown
        };
        let lang = self.lang;
        let (mut retry, mut copy, mut discard) = (false, false, false);
        egui::Panel::top("draft").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(Message::DraftBarTitle.say(lang)).strong());
                ui.label(format!("“{preview}”")).on_hover_text(&text);
                retry = ui
                    .add_enabled(
                        retryable && idle,
                        egui::Button::new(Message::DraftRetry.say(lang)),
                    )
                    .on_disabled_hover_text(
                        if retryable {
                            Message::DraftWaitForTheEditToLand
                        } else {
                            Message::DraftPlaceNoLongerUsable
                        }
                        .say(lang),
                    )
                    .clicked();
                copy = ui.button(Message::DraftCopy.say(lang)).clicked();
                discard = ui.button(Message::DraftDiscard.say(lang)).clicked();
                ui.add(egui::Label::new(&reason).truncate())
                    .on_hover_text(&reason);
            });
        });
        if copy {
            ui.ctx().copy_text(text);
            self.editor.say(Message::DraftCopied);
        }
        if retry {
            self.retry_draft();
        }
        if discard {
            self.input.discard();
            let said = Message::DraftDiscarded;
            self.editor.say(said);
        }
    }
}

fn toolbar_separator(ui: &mut egui::Ui) {
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);
}

const THEME_RANK: u8 = 7;
const HOME_RANK: u8 = 6;
const OPEN_RANK: u8 = 5;
const ZOOM_RANK: u8 = 5;
const SAVE_RANK: u8 = 4;
const PAGING_RANK: u8 = 4;
const DELETE_RANK: u8 = 3;
const HISTORY_RANK: u8 = 2;

const TOOLS: [(Icon, Command, Tool); 8] = [
    (Icon::Select, Command::Select, Tool::Select),
    (Icon::Text, Command::Text, Tool::Text),
    (Icon::Pen, Command::Pen, Tool::Pen),
    (Icon::Highlighter, Command::Highlighter, Tool::Highlighter),
    (Icon::Shape, Command::Shape, Tool::Shape),
    (Icon::Form, Command::Form, Tool::Form),
    (Icon::Link, Command::Link, Tool::Link),
    (Icon::Picture, Command::Picture, Tool::Picture),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Side {
    Left,
    Right,
}

#[derive(Clone, Debug)]
enum What {
    Button {
        icon: Icon,
        command: Command,
        enabled: bool,
        on: bool,
    },
    Rule,
    Readout {
        text: String,
        width: f32,
    },
}

#[derive(Clone, Debug)]
struct Slot {
    side: Side,
    rank: u8,
    what: What,
}

impl Slot {
    fn is_button(&self) -> bool {
        matches!(self.what, What::Button { .. })
    }

    fn is_rule(&self) -> bool {
        matches!(self.what, What::Rule)
    }

    fn readout_width(&self) -> f32 {
        match self.what {
            What::Readout { width, .. } => width,
            _ => 0.0,
        }
    }

    fn piece(&self, labels: bool) -> room::Piece {
        let width = match self.what {
            What::Button { .. } => room::tool_width(labels) + room::GAP,
            What::Rule => room::RULE_WIDTH,
            What::Readout { width, .. } => width + room::GAP,
        };
        room::Piece {
            width,
            rank: self.rank,
        }
    }
}

fn text_width(ui: &egui::Ui, text: &str) -> f32 {
    let font = egui::TextStyle::Body.resolve(ui.style());
    ui.painter()
        .layout_no_wrap(text.to_owned(), font, egui::Color32::PLACEHOLDER)
        .size()
        .x
}

fn language_rows() -> Vec<(Lang, &'static str)> {
    Lang::ALL
        .iter()
        .map(|language| (*language, language.endonym()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{Lang, language_rows};

    #[test]
    fn the_picker_lists_exactly_the_languages_the_window_speaks() {
        let rows = language_rows();
        assert_eq!(rows.len(), Lang::ALL.len());
        for (at, (language, named)) in rows.iter().enumerate() {
            assert_eq!(*language, Lang::ALL[at]);
            assert_eq!(*named, language.endonym());
            assert!(!named.is_empty());
            assert_eq!(Lang::of_tag(language.tag()), Some(*language));
        }
        let english = rows
            .iter()
            .find(|(language, _)| *language == Lang::English)
            .expect("the window speaks English");
        assert_eq!(english.1, "English");
    }
}
