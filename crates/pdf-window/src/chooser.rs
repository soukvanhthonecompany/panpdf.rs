use std::path::{Path, PathBuf};

use eframe::egui;

use pdf_app::files::is_a_copy;
use pdf_app::wording::{Home, Lang, Message};

use crate::icons::Icon;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Entry {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) folder: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Offer {
    Pdfs,
    AnyPdf,
    Pictures,
}

impl Offer {
    fn takes(self, path: &Path) -> bool {
        let Some(extension) = path.extension() else {
            return false;
        };
        let is = |name: &str| extension.eq_ignore_ascii_case(name);
        match self {
            Self::Pdfs => is("pdf") && !is_a_copy(path),
            Self::AnyPdf => is("pdf"),
            Self::Pictures => is("jpg") || is("jpeg") || is("png"),
        }
    }
}

pub(crate) struct Chooser {
    offer: Offer,
    folder: PathBuf,
    entries: Vec<Entry>,
    unreadable: bool,
    picked: Option<PathBuf>,
    naming: Option<String>,
    spread: usize,
    ending: &'static str,
    ticked: Option<Vec<PathBuf>>,
    system: System,
}

enum System {
    Untried,
    Asking(crate::system_dialog::Asking),
    Declined,
}

pub(crate) enum Chose {
    Nothing,
    Cancelled,
    Open(PathBuf),
    Several(Vec<PathBuf>),
    Save(PathBuf),
}

impl Chooser {
    pub(crate) fn at(folder: Option<&Path>, offer: Offer) -> Self {
        let folder = folder
            .map(Path::to_path_buf)
            .or_else(home_folder)
            .unwrap_or_else(|| PathBuf::from("/"));
        let mut chooser = Self {
            offer,
            folder: PathBuf::new(),
            entries: Vec::new(),
            unreadable: false,
            picked: None,
            naming: None,
            spread: 1,
            ending: "pdf",
            ticked: None,
            system: System::Untried,
        };
        chooser.go(folder);
        chooser
    }

    pub(crate) fn saving(folder: Option<&Path>, suggested: &str, spread: usize) -> Self {
        Self::saving_as(folder, suggested, spread, "pdf")
    }

    pub(crate) fn saving_as(
        folder: Option<&Path>,
        suggested: &str,
        spread: usize,
        ending: &'static str,
    ) -> Self {
        let offer = if ending == "pdf" {
            Offer::AnyPdf
        } else {
            Offer::Pictures
        };
        Self {
            naming: Some(suggested.to_owned()),
            spread,
            ending,
            ..Self::at(folder, offer)
        }
    }

    pub(crate) fn several(folder: Option<&Path>) -> Self {
        Self {
            ticked: Some(Vec::new()),
            ..Self::at(folder, Offer::Pictures)
        }
    }

    fn go(&mut self, folder: PathBuf) {
        if let Ok(read) = std::fs::read_dir(&folder) {
            self.entries = listing(
                self.offer,
                read.flatten().map(|entry| {
                    let path = entry.path();
                    let folder = path.is_dir();
                    (path, folder)
                }),
            );
            self.unreadable = false;
        } else {
            self.entries.clear();
            self.unreadable = true;
        }
        self.folder = folder;
        self.picked = None;
    }

    pub(crate) fn show(&mut self, ctx: &egui::Context, lang: Lang, title: Home) -> Chose {
        if let Some(chose) = self.ask_the_desktop(ctx, lang, &title) {
            return chose;
        }
        self.show_own(ctx, lang, title)
    }

    fn ask_the_desktop(&mut self, ctx: &egui::Context, lang: Lang, title: &Home) -> Option<Chose> {
        if matches!(self.system, System::Untried) {
            let question = crate::system_dialog::Question {
                title: Message::Home(title.clone()).say(lang),
                folder: std::path::absolute(&self.folder).unwrap_or_else(|_| self.folder.clone()),
                kind: match self.offer {
                    Offer::Pdfs | Offer::AnyPdf => crate::system_dialog::Kind::Pdfs,
                    Offer::Pictures => crate::system_dialog::Kind::Pictures,
                },
                naming: self
                    .naming
                    .as_ref()
                    .map(|name| with_ending(name, self.ending)),
                several: self.ticked.is_some(),
            };
            self.system =
                crate::system_dialog::ask(&question).map_or(System::Declined, System::Asking);
        }
        let System::Asking(asking) = &self.system else {
            return None;
        };
        let Some(answer) = asking.answer() else {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            let cancelled = waiting_note(ctx, lang);
            if cancelled {
                self.system = System::Declined;
                return Some(Chose::Cancelled);
            }
            return Some(Chose::Nothing);
        };
        self.system = System::Declined;
        Some(match answer {
            crate::system_dialog::Answer::Cancelled => Chose::Cancelled,
            crate::system_dialog::Answer::Unavailable => return None,
            crate::system_dialog::Answer::Chose(paths) => self.took(paths)?,
        })
    }

    fn took(&mut self, mut paths: Vec<PathBuf>) -> Option<Chose> {
        if self.ticked.is_some() {
            return Some(Chose::Several(paths));
        }
        let path = paths.swap_remove(0);
        if self.naming.is_none() {
            return Some(Chose::Open(path));
        }
        let (folder, name) = crate::system_dialog::folder_and_name(&path)?;
        if let Ok(path) = where_to_write(&folder, &name, self.spread, self.ending) {
            return Some(Chose::Save(path));
        }
        self.go(folder);
        self.naming = Some(name);
        None
    }

    fn show_own(&mut self, ctx: &egui::Context, lang: Lang, title: Home) -> Chose {
        let say = |home: Home| Message::Home(home).say(lang);
        let mut chose = Chose::Nothing;
        let mut go = None;
        let mut naming = self.naming.take();
        let modal = egui::Modal::new(egui::Id::new("open-a-pdf")).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.heading(say(title));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let up = self.folder.parent().map(Path::to_path_buf);
                if ui
                    .add_enabled(up.is_some(), egui::Button::new("⬆"))
                    .on_hover_text(say(Home::UpOneFolder))
                    .clicked()
                {
                    go = up;
                }
                if let Some(home) = home_folder()
                    && ui.button(say(Home::HomeFolder)).clicked()
                {
                    go = Some(home);
                }
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(self.folder.display().to_string())
                            .color(ui.visuals().weak_text_color()),
                    )
                    .truncate(),
                );
            });
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height(380.0)
                .min_scrolled_height(380.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if self.unreadable {
                        let folder = self.folder.display().to_string();
                        ui.label(say(Home::FolderUnreadable(folder)));
                    } else if self.entries.is_empty() {
                        ui.label(say(match self.offer {
                            Offer::Pdfs | Offer::AnyPdf => Home::NoPdfHere,
                            Offer::Pictures => Home::NoPictureHere,
                        }));
                    }
                    for entry in &self.entries {
                        let picked = match &self.ticked {
                            Some(ticked) => ticked.contains(&entry.path),
                            None => self.picked.as_deref() == Some(entry.path.as_path()),
                        };
                        let response = entry_row(ui, entry, picked);
                        if entry.folder && (response.clicked() || response.double_clicked()) {
                            go = Some(entry.path.clone());
                        } else if let Some(ticked) = self.ticked.as_mut() {
                            if response.clicked() {
                                if let Some(at) = ticked.iter().position(|had| *had == entry.path) {
                                    ticked.remove(at);
                                } else {
                                    ticked.push(entry.path.clone());
                                }
                            }
                        } else if response.double_clicked() && naming.is_none() {
                            chose = Chose::Open(entry.path.clone());
                        } else if response.clicked() || response.double_clicked() {
                            if let Some(name) = naming.as_mut() {
                                name.clone_from(&entry.name);
                            }
                            self.picked = Some(entry.path.clone());
                        }
                    }
                });
            ui.separator();
            if let Some(answer) = finish(
                ui,
                Naming {
                    typed: &mut naming,
                    folder: &self.folder,
                    spread: self.spread,
                    ending: self.ending,
                },
                (self.picked.as_deref(), self.ticked.as_deref()),
                lang,
            ) {
                chose = answer;
            }
        });
        if modal.should_close() && matches!(chose, Chose::Nothing) {
            chose = Chose::Cancelled;
        }
        self.naming = naming;
        if let Some(folder) = go {
            self.go(folder);
        }
        chose
    }
}

struct Naming<'a> {
    typed: &'a mut Option<String>,
    folder: &'a Path,
    spread: usize,
    ending: &'static str,
}

fn finish(
    ui: &mut egui::Ui,
    naming: Naming<'_>,
    (picked, ticked): (Option<&Path>, Option<&[PathBuf]>),
    lang: Lang,
) -> Option<Chose> {
    let say = |home: Home| Message::Home(home).say(lang);
    let Naming {
        typed: naming,
        folder,
        spread,
        ending,
    } = naming;
    let destination = naming
        .as_ref()
        .map(|name| where_to_write(folder, name, spread, ending));
    if let Some(name) = naming.as_mut() {
        ui.horizontal(|ui| {
            ui.label(say(Home::FileName));
            let box_width = ui.available_width() - 8.0;
            let hint = format!(".{ending}");
            let typing = ui.add(
                egui::TextEdit::singleline(name)
                    .desired_width(box_width)
                    .hint_text(hint),
            );
            typing.request_focus();
        });
        if let Some(Err(trouble)) = &destination {
            let said = say(trouble.clone());
            ui.label(egui::RichText::new(said).color(ui.visuals().warn_fg_color));
        }
    }
    let mut chose = None;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        match &destination {
            None if ticked.is_some() => {
                let ticked = ticked.unwrap_or_default();
                let use_these = egui::Button::new(say(Home::UsePictures(ticked.len())));
                if ui.add_enabled(!ticked.is_empty(), use_these).clicked() {
                    chose = Some(Chose::Several(ticked.to_vec()));
                }
            }
            None => {
                let open = egui::Button::new(say(Home::OpenChosen));
                if ui.add_enabled(picked.is_some(), open).clicked()
                    && let Some(path) = picked
                {
                    chose = Some(Chose::Open(path.to_path_buf()));
                }
            }
            Some(destination) => {
                let save = egui::Button::new(say(Home::SaveHere));
                if ui.add_enabled(destination.is_ok(), save).clicked()
                    && let Ok(path) = destination
                {
                    chose = Some(Chose::Save(path.clone()));
                }
            }
        }
        if ui.button(say(Home::Cancel)).clicked() {
            chose = Some(Chose::Cancelled);
        }
    });
    chose
}

fn with_ending(name: &str, ending: &str) -> String {
    let dotted = format!(".{ending}");
    if name.to_ascii_lowercase().ends_with(&dotted) {
        name.to_owned()
    } else {
        format!("{name}{dotted}")
    }
}

fn waiting_note(ctx: &egui::Context, lang: Lang) -> bool {
    let mut cancelled = false;
    egui::Modal::new(egui::Id::new("file-window-open")).show(ctx, |ui| {
        ui.set_width(320.0);
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(Message::Home(Home::ChooseInTheFileWindow).say(lang));
        });
        ui.add_space(6.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(Message::Home(Home::Cancel).say(lang)).clicked() {
                cancelled = true;
            }
        });
    });
    cancelled
}

fn where_to_write(
    folder: &Path,
    typed: &str,
    spread: usize,
    ending: &str,
) -> Result<PathBuf, Home> {
    match pdf_app::files::named_file(folder, typed, ending) {
        Err(pdf_app::files::NameRefused::Empty) => Err(Home::FileName),
        Err(pdf_app::files::NameRefused::NotAName) => Err(Home::NameIsNotOne),
        Ok(path) => {
            if pdf_app::files::files_written_as(&path, spread, ending)
                .iter()
                .any(|file| std::fs::symlink_metadata(file).is_ok())
            {
                Err(Home::NameTaken)
            } else {
                Ok(path)
            }
        }
    }
}

fn entry_row(ui: &mut egui::Ui, entry: &Entry, picked: bool) -> egui::Response {
    let height = 26.0;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    if ui.is_rect_visible(rect) {
        let visuals = ui.visuals();
        if picked {
            ui.painter()
                .rect_filled(rect, 4.0, visuals.selection.bg_fill);
        } else if response.hovered() {
            ui.painter()
                .rect_filled(rect, 4.0, visuals.widgets.hovered.weak_bg_fill);
        }
        let colour = if picked {
            visuals.selection.stroke.color
        } else {
            visuals.text_color()
        };
        let icon = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 6.0, rect.center().y - 8.0),
            egui::vec2(16.0, 16.0),
        );
        let drawn = if entry.folder {
            Icon::Folder
        } else {
            Icon::Document
        };
        drawn.draw(ui.painter(), icon, colour);
        ui.painter().text(
            egui::pos2(rect.left() + 30.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            &entry.name,
            egui::FontId::proportional(14.0),
            colour,
        );
    }
    response
}

pub(crate) fn listing(offer: Offer, found: impl Iterator<Item = (PathBuf, bool)>) -> Vec<Entry> {
    let mut entries: Vec<Entry> = found
        .filter_map(|(path, folder)| {
            let name = path.file_name()?.to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            (folder || offer.takes(&path)).then_some(Entry { name, path, folder })
        })
        .collect();
    entries.sort_by(|a, b| {
        b.folder
            .cmp(&a.folder)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    entries
}

pub(crate) fn home_folder() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Offer, listing};

    #[test]
    fn folders_come_first_and_only_pdfs_are_offered() {
        let found = [
            ("/d/zeta.pdf", false),
            ("/d/Alpha.PDF", false),
            ("/d/notes.txt", false),
            ("/d/.hidden.pdf", false),
            ("/d/.config", true),
            ("/d/report-edited.pdf", false),
            ("/d/books", true),
            ("/d/Archive", true),
            ("/d/folder.pdf", true),
        ];
        let names: Vec<(String, bool)> = listing(
            Offer::Pdfs,
            found
                .iter()
                .map(|(path, folder)| (PathBuf::from(path), *folder)),
        )
        .into_iter()
        .map(|entry| (entry.name, entry.folder))
        .collect();
        assert_eq!(
            names,
            vec![
                ("Archive".to_owned(), true),
                ("books".to_owned(), true),
                ("folder.pdf".to_owned(), true),
                ("Alpha.PDF".to_owned(), false),
                ("zeta.pdf".to_owned(), false),
            ]
        );
    }

    #[test]
    fn a_picture_chooser_offers_jpeg_and_png_and_nothing_else() {
        let found = [
            ("/d/photo.JPG", false),
            ("/d/logo.png", false),
            ("/d/scan.jpeg", false),
            ("/d/report.pdf", false),
            ("/d/anim.gif", false),
        ];
        let names: Vec<String> = listing(
            Offer::Pictures,
            found
                .iter()
                .map(|(path, folder)| (PathBuf::from(path), *folder)),
        )
        .into_iter()
        .map(|entry| entry.name)
        .collect();
        assert_eq!(names, ["logo.png", "photo.JPG", "scan.jpeg"]);
    }
}
