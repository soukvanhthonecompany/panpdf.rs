use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use eframe::egui;

use pdf_app::files::is_a_copy;
use pdf_app::recent::{self, Recent};
use pdf_app::wording::{Home, Message};

use crate::app::name_of;
use crate::chooser::{Chooser, Chose};
use crate::icons::Icon;
use crate::window_state::{Window, desk};

const COLUMN: f32 = 760.0;

fn list_file() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("state"))
        })?;
    Some(state.join("panpdf").join("recent"))
}

pub(crate) fn load_recent() -> Vec<Recent> {
    list_file()
        .and_then(|file| std::fs::read_to_string(file).ok())
        .map(|text| recent::read(&text))
        .unwrap_or_default()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

impl Window {
    pub(crate) fn has_document(&self) -> bool {
        self.untitled || !self.opened.as_os_str().is_empty()
    }

    pub(crate) fn remember_here(&mut self) {
        if self.opened.as_os_str().is_empty() {
            return;
        }
        let here = self.opened.clone();
        self.remember_file(&here);
    }

    pub(crate) fn remember_file(&mut self, path: &Path) {
        self.recent = recent::remember(&self.recent, path, self.focus, now());
        self.write_recent();
    }

    pub(crate) fn keep_the_page_in_the_list(&mut self) {
        let listed = self
            .recent
            .first()
            .is_some_and(|head| head.path == self.opened && head.page == self.focus);
        if !listed && self.loading.is_none() {
            self.remember_here();
        }
    }

    fn write_recent(&self) {
        let Some(file) = list_file() else { return };
        if let Some(folder) = file.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        let temporary = file.with_extension("new");
        if std::fs::write(&temporary, recent::write(&self.recent)).is_ok() {
            let _ = std::fs::rename(&temporary, &file);
        }
    }

    pub(crate) fn go_home(&mut self) {
        self.remember_here();
        self.put_the_drag_down();
        self.home = true;
    }

    pub(crate) fn choose_a_file(&mut self, ctx: &egui::Context) {
        if self.asking_to_open && self.chooser.is_none() {
            let start = self
                .has_document()
                .then(|| self.opened.parent().map(Path::to_path_buf))
                .flatten();
            self.chooser = Some(
                if self.choosing_for == crate::page_actions::Choosing::Picture {
                    Chooser::several(start.as_deref())
                } else {
                    Chooser::at(start.as_deref(), crate::chooser::Offer::Pdfs)
                },
            );
        }
        self.asking_to_open = false;
        let Some(chooser) = self.chooser.as_mut() else {
            return;
        };
        let title = match self.choosing_for {
            crate::page_actions::Choosing::Open => pdf_app::wording::Home::OpenFile,
            crate::page_actions::Choosing::Pages { .. } => pdf_app::wording::Home::PagesFromFile,
            crate::page_actions::Choosing::Picture => pdf_app::wording::Home::PictureFromFile,
            crate::page_actions::Choosing::Copy
                if self.untitled && self.destination.as_os_str().is_empty() =>
            {
                pdf_app::wording::Home::SaveNewDocument
            }
            crate::page_actions::Choosing::Copy => pdf_app::wording::Home::SaveACopy,
            crate::page_actions::Choosing::PagesOut(_) => pdf_app::wording::Home::PagesToFile,
            crate::page_actions::Choosing::Pieces(_) => pdf_app::wording::Home::SplitToFiles,
            crate::page_actions::Choosing::PagesAsPictures { .. } => {
                pdf_app::wording::Home::PagesToPictures
            }
            crate::page_actions::Choosing::PicturesIn { before: Some(_) } => {
                pdf_app::wording::Home::PicturesToInsert
            }
            crate::page_actions::Choosing::PicturesIn { before: None }
            | crate::page_actions::Choosing::PdfOfPictures(_) => {
                pdf_app::wording::Home::PicturesToPages
            }
        };
        match chooser.show(ctx, self.lang, title) {
            Chose::Nothing => {}
            Chose::Cancelled => {
                self.chooser = None;
                self.choosing_for = crate::page_actions::Choosing::Open;
            }
            Chose::Open(path) => {
                self.chooser = None;
                match std::mem::take(&mut self.choosing_for) {
                    crate::page_actions::Choosing::Open => self.open_at(&path, 0),
                    crate::page_actions::Choosing::Pages { before } => {
                        self.insert_pages_from(&path, before);
                    }
                    crate::page_actions::Choosing::Picture => {
                        self.pictures_chosen_to_place(&[path]);
                    }
                    crate::page_actions::Choosing::Copy
                    | crate::page_actions::Choosing::PagesOut(_)
                    | crate::page_actions::Choosing::Pieces(_)
                    | crate::page_actions::Choosing::PagesAsPictures { .. }
                    | crate::page_actions::Choosing::PicturesIn { .. }
                    | crate::page_actions::Choosing::PdfOfPictures(_) => {}
                }
            }
            Chose::Several(paths) => {
                self.chooser = None;
                match std::mem::take(&mut self.choosing_for) {
                    crate::page_actions::Choosing::PicturesIn { before } => {
                        self.pictures_chosen(&paths, before);
                    }
                    crate::page_actions::Choosing::Picture => {
                        self.pictures_chosen_to_place(&paths);
                    }
                    _ => {}
                }
            }
            Chose::Save(path) => {
                self.chooser = None;
                self.write_what_was_chosen(&path);
            }
        }
    }

    pub(crate) fn home_screen(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let say = |home: Home| Message::Home(home).say(lang);
        let idle = !self.editor.is_busy() && self.loading.is_none();
        let ground = egui::Frame::NONE.fill(desk(self.dark));
        egui::CentralPanel::no_frame().frame(ground).show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let room = ui.available_width();
                    let wide = room.min(COLUMN);
                    let side = ((room - wide) / 2.0).max(0.0);
                    ui.add_space(36.0);
                    ui.horizontal(|ui| {
                        ui.add_space(side);
                        ui.vertical(|ui| {
                            ui.set_width(wide - 32.0);
                            self.home_column(ui, idle, &say);
                        });
                    });
                    ui.add_space(36.0);
                });
        });
    }

    fn home_column(&mut self, ui: &mut egui::Ui, idle: bool, say: &dyn Fn(Home) -> String) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("PanPDF").size(26.0).strong());
            ui.label(
                egui::RichText::new(env!("CARGO_PKG_VERSION"))
                    .size(13.0)
                    .color(ui.visuals().weak_text_color()),
            );
            if self.has_document() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let back = format!("{}  →", say(Home::BackToDocument));
                    if ui
                        .button(back)
                        .on_hover_text(name_of(&self.opened))
                        .clicked()
                    {
                        self.home = false;
                    }
                });
            }
        });
        ui.add_space(20.0);
        let between = 12.0;
        let tile =
            ((ui.available_width() - between - 2.0 * ui.spacing().item_spacing.x) / 2.0).max(200.0);
        ui.horizontal(|ui| {
            if action_tile(
                ui,
                tile,
                Icon::NewDocument,
                &say(Home::BlankDocument),
                &say(Home::BlankDocumentHelp),
                idle,
            ) {
                self.new_document(crate::chrome::A4);
            }
            ui.add_space(between);
            if action_tile(
                ui,
                tile,
                Icon::Open,
                &say(Home::OpenFile),
                &say(Home::OpenFileHelp),
                idle,
            ) {
                self.asking_to_open = true;
            }
        });
        ui.add_space(28.0);
        ui.label(
            egui::RichText::new(say(Home::Recent))
                .size(13.0)
                .strong()
                .color(ui.visuals().weak_text_color()),
        );
        ui.add_space(6.0);
        if self.recent.is_empty() {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(say(Home::NothingRecent)).color(ui.visuals().weak_text_color()),
            );
            return;
        }
        let now = now();
        let mut open = None;
        let mut forget = None;
        for entry in &self.recent {
            match recent_row(ui, entry, now, idle, say) {
                RowAction::Nothing => {}
                RowAction::Open => open = Some((entry.path.clone(), entry.page)),
                RowAction::Forget => forget = Some(entry.path.clone()),
            }
        }
        if let Some(path) = forget {
            self.recent = recent::forget(&self.recent, &path);
            self.write_recent();
        }
        if let Some((path, page)) = open {
            self.open_at(&path, page);
        }
    }
}

fn action_tile(
    ui: &mut egui::Ui,
    wide: f32,
    icon: Icon,
    name: &str,
    help: &str,
    enabled: bool,
) -> bool {
    let size = egui::vec2(wide, 84.0);
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    if ui.is_rect_visible(rect) {
        let visuals = ui.visuals();
        let fill = if enabled && response.hovered() {
            visuals.widgets.hovered.weak_bg_fill
        } else {
            visuals.panel_fill
        };
        let border = if enabled && response.hovered() {
            visuals.selection.stroke.color
        } else {
            visuals.widgets.noninteractive.bg_stroke.color
        };
        let text = if enabled {
            visuals.text_color()
        } else {
            visuals.weak_text_color()
        };
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        painter.rect(
            rect,
            8.0,
            fill,
            egui::Stroke::new(1.0, border),
            egui::StrokeKind::Inside,
        );
        let accent = if enabled {
            visuals.selection.stroke.color
        } else {
            text
        };
        icon.draw(
            &painter,
            egui::Rect::from_min_size(
                egui::pos2(rect.left() + 18.0, rect.center().y - 16.0),
                egui::vec2(32.0, 32.0),
            ),
            accent,
        );
        painter.text(
            egui::pos2(rect.left() + 66.0, rect.center().y - 10.0),
            egui::Align2::LEFT_CENTER,
            name,
            egui::FontId::proportional(16.0),
            text,
        );
        painter.text(
            egui::pos2(rect.left() + 66.0, rect.center().y + 12.0),
            egui::Align2::LEFT_CENTER,
            help,
            egui::FontId::proportional(12.0),
            visuals.weak_text_color(),
        );
    }
    enabled && response.clicked()
}

enum RowAction {
    Nothing,
    Open,
    Forget,
}

const NAME_SIZE: f32 = 15.0;
const DETAIL_SIZE: f32 = 12.0;

fn fitted_words(painter: &egui::Painter, room: f32, name: &str, detail: &str) -> (String, String) {
    let measured = |size: f32| {
        move |text: &str| {
            painter
                .layout_no_wrap(
                    text.to_owned(),
                    egui::FontId::proportional(size),
                    egui::Color32::PLACEHOLDER,
                )
                .size()
                .x
        }
    };
    (
        crate::room::elide_middle(name, room, measured(NAME_SIZE)),
        crate::room::elide_middle(detail, room, measured(DETAIL_SIZE)),
    )
}

fn detail_line(entry: &Recent, there: bool, say: &dyn Fn(Home) -> String) -> String {
    let folder = entry
        .path
        .parent()
        .map(|folder| folder.display().to_string())
        .unwrap_or_default();
    let where_it_was = if there {
        say(Home::ContinueAt(entry.page + 1))
    } else {
        say(Home::Missing)
    };
    format!("{where_it_was}  \u{00b7}  {folder}")
}

fn recent_row(
    ui: &mut egui::Ui,
    entry: &Recent,
    now: u64,
    idle: bool,
    say: &dyn Fn(Home) -> String,
) -> RowAction {
    let there = entry.path.is_file();
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 52.0), egui::Sense::click());
    let mut action = RowAction::Nothing;
    let forget_rect = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 20.0, rect.center().y),
        egui::vec2(26.0, 26.0),
    );
    let hover = ui.ctx().pointer_hover_pos();
    let over_forget = hover.is_some_and(|at| forget_rect.contains(at));
    if ui.is_rect_visible(rect) {
        let visuals = ui.visuals();
        let painter = ui.painter();
        if response.hovered() {
            painter.rect_filled(rect, 6.0, visuals.widgets.hovered.weak_bg_fill);
        }
        let strong = if there {
            visuals.text_color()
        } else {
            visuals.weak_text_color()
        };
        let weak = visuals.weak_text_color();
        Icon::Document.draw(
            painter,
            egui::Rect::from_min_size(
                egui::pos2(rect.left() + 12.0, rect.center().y - 12.0),
                egui::vec2(24.0, 24.0),
            ),
            strong,
        );
        let detail = detail_line(entry, there, say);
        let name_room = (rect.right() - 170.0) - (rect.left() + 50.0);
        let (name, detail) = fitted_words(painter, name_room, &name_of(&entry.path), &detail);
        painter.text(
            egui::pos2(rect.left() + 50.0, rect.top() + 16.0),
            egui::Align2::LEFT_CENTER,
            name,
            egui::FontId::proportional(NAME_SIZE),
            strong,
        );
        let detail_rect = egui::Rect::from_min_max(
            egui::pos2(rect.left() + 50.0, rect.top() + 28.0),
            egui::pos2(rect.right() - 170.0, rect.bottom() - 6.0),
        );
        painter.with_clip_rect(detail_rect).text(
            egui::pos2(detail_rect.left(), rect.top() + 36.0),
            egui::Align2::LEFT_CENTER,
            detail,
            egui::FontId::proportional(DETAIL_SIZE),
            weak,
        );
        painter.text(
            egui::pos2(rect.right() - 44.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            say(Home::WorkedOn(recent::ago(entry.when, now))),
            egui::FontId::proportional(12.0),
            weak,
        );
        if response.hovered() {
            let cross = if over_forget { strong } else { weak };
            if over_forget {
                painter.rect_filled(forget_rect, 4.0, visuals.widgets.active.weak_bg_fill);
            }
            let c = forget_rect.center();
            let arm = 5.0;
            let stroke = egui::Stroke::new(1.5, cross);
            painter.line_segment(
                [c + egui::vec2(-arm, -arm), c + egui::vec2(arm, arm)],
                stroke,
            );
            painter.line_segment(
                [c + egui::vec2(-arm, arm), c + egui::vec2(arm, -arm)],
                stroke,
            );
        }
    }
    let response = if over_forget {
        response.on_hover_text(say(Home::RemoveFromList))
    } else {
        response.on_hover_text(entry.path.display().to_string())
    };
    if response.clicked() {
        if over_forget {
            action = RowAction::Forget;
        } else if there && idle {
            action = RowAction::Open;
        }
    }
    action
}

#[must_use]
pub fn pdfs_in(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
                && !is_a_copy(path)
        })
        .collect();
    found.sort();
    found
}
