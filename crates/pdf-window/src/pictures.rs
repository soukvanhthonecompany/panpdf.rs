use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;

use pdf_app::wording::{Command, Home, Message};

use crate::page_actions::Choosing;
use crate::take_out::{Piece, Wrote};
use crate::window_state::Window;

const RESOLUTIONS: [usize; 5] = [72, 96, 150, 300, 600];

pub(crate) struct ExportChoices {
    pub(crate) all: bool,
    pub(crate) pages: String,
    pub(crate) dpi: usize,
}

impl Default for ExportChoices {
    fn default() -> Self {
        Self {
            all: true,
            pages: String::new(),
            dpi: 150,
        }
    }
}

pub(crate) struct Making {
    pub(crate) handle: std::thread::JoinHandle<Result<Vec<u8>, String>>,
    pub(crate) then: AfterPictures,
}

pub(crate) enum AfterPictures {
    Insert {
        beside: usize,
        before: bool,
        pages: usize,
    },
    Write(PathBuf),
}

pub(crate) fn export_pages(choices: &ExportChoices, count: usize) -> Result<Vec<usize>, Message> {
    if choices.all {
        return Ok((0..count).collect());
    }
    if choices.pages.trim().is_empty() {
        return Ok(Vec::new());
    }
    pdf_edit::stamp::pages_of(&choices.pages, count, pdf_edit::stamp::Only::Every)
        .map_err(|error| Message::refusal(&error))
}

impl Window {
    pub(crate) fn open_the_export_panel(&mut self) {
        self.exporting = Some(ExportChoices::default());
    }

    pub(crate) fn choose_pictures(&mut self, before: Option<bool>) {
        let folder = self
            .has_document()
            .then(|| self.opened.parent().map(Path::to_path_buf))
            .flatten();
        self.chooser = Some(crate::chooser::Chooser::several(folder.as_deref()));
        self.choosing_for = Choosing::PicturesIn { before };
    }

    pub(crate) fn export_dialog(&mut self, ctx: &egui::Context) {
        if self.exporting.is_none() || !self.has_document() {
            return;
        }
        let (lang, count) = (self.lang, self.editor.page_count());
        let shown = self.focus;
        let pixels = |dpi: usize, page: usize| {
            #[expect(
                clippy::cast_precision_loss,
                reason = "a resolution is at most three digits"
            )]
            let scale = dpi as f64 / 72.0;
            self.editor.page_pixels(page, scale)
        };
        let Some(choices) = self.exporting.as_mut() else {
            return;
        };
        let pages = export_pages(choices, count);
        let (mut close, mut go_on) = (false, false);
        let title = Message::Command(Command::PagesAsPictures).say(lang);
        let modal = egui::Modal::new(egui::Id::new("export-dialog")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.heading(title.trim_end_matches('\u{2026}'));
            ui.label(
                egui::RichText::new(Message::ExportWhy.say(lang))
                    .size(11.0)
                    .color(ui.visuals().weak_text_color()),
            );
            ui.separator();
            ui.horizontal(|ui| {
                ui.radio_value(&mut choices.all, true, Message::AllPages.say(lang));
                ui.radio_value(&mut choices.all, false, Message::SomePages.say(lang));
                let typed = ui.add(
                    egui::TextEdit::singleline(&mut choices.pages)
                        .hint_text("1-5, 8")
                        .desired_width(120.0),
                );
                if typed.changed() {
                    choices.all = false;
                }
            });
            ui.horizontal(|ui| {
                ui.label(Message::ExportResolution.say(lang));
                for resolution in RESOLUTIONS {
                    ui.radio_value(&mut choices.dpi, resolution, resolution.to_string());
                }
                ui.label(Message::ExportDpi.say(lang));
            });
            ui.separator();
            match &pages {
                Ok(pages) => {
                    let first = pages.first().copied().unwrap_or(shown);
                    let (width, height) = pixels(choices.dpi, first).unwrap_or((0, 0));
                    ui.label(
                        Message::ExportSize {
                            pages: pages.len(),
                            width,
                            height,
                        }
                        .say(lang),
                    );
                }
                Err(why) => {
                    let said = why.say(lang);
                    ui.label(egui::RichText::new(said).color(ui.visuals().warn_fg_color));
                }
            }
            ui.separator();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let ready = pages.as_ref().is_ok_and(|pages| !pages.is_empty());
                let on = egui::Button::new(Message::SplitWhere.say(lang));
                if ui.add_enabled(ready, on).clicked() {
                    go_on = true;
                }
                if ui.button(Message::Home(Home::Cancel).say(lang)).clicked() {
                    close = true;
                }
            });
        });
        if modal.should_close() {
            close = true;
        }
        if go_on && let Ok(pages) = pages {
            let dpi = choices.dpi;
            self.exporting = None;
            let suggested = pdf_app::files::named_for_pages(&self.opened, &pages);
            let spread = pages.len();
            self.choosing_for = Choosing::PagesAsPictures { pages, dpi };
            let folder = self.opened.parent().map(Path::to_path_buf);
            self.chooser = Some(crate::chooser::Chooser::saving_as(
                folder.as_deref(),
                suggested.trim_end_matches(".pdf"),
                spread,
                "png",
            ));
        } else if close {
            self.exporting = None;
        }
    }

    pub(crate) fn write_pages_as_pictures(&mut self, base: &Path, pages: &[usize], dpi: usize) {
        if self.writing.is_some() {
            return;
        }
        let Some(source) = self.editor.source().cloned() else {
            return;
        };
        let pieces: Vec<Piece> = pdf_app::files::files_written_as(base, pages.len(), "png")
            .into_iter()
            .zip(pages)
            .map(|(path, page)| Piece {
                path,
                pages: vec![*page],
            })
            .collect();
        let credential = self.editor.credential().to_vec();
        let fonts = self.editor.fonts();
        let original = self.opened.clone();
        let handle = std::thread::spawn(move || {
            let mut wrote = Wrote {
                files: Vec::with_capacity(pieces.len()),
                bytes: 0,
            };
            for piece in &pieces {
                let page = *piece.pages.first().ok_or("a picture is one page")?;
                let picture = drawn(&source, (&credential, fonts.clone()), page, dpi)?;
                crate::save_file::save(&original, &piece.path, &picture, None)
                    .map_err(|error| format!("{}: {error}", piece.path.display()))?;
                wrote.bytes += picture.len() as u64;
                wrote.files.push(piece.path.clone());
            }
            Ok(wrote)
        });
        self.writing = Some(crate::take_out::Writing {
            handle,
            pictures: true,
        });
    }

    pub(crate) fn pictures_chosen(&mut self, paths: &[PathBuf], before: Option<bool>) {
        let mut pictures: Vec<Arc<[u8]>> = Vec::with_capacity(paths.len());
        for path in paths {
            match std::fs::read(path) {
                Ok(bytes) => pictures.push(Arc::from(bytes)),
                Err(error) => {
                    self.editor.say(Message::PictureNotRead {
                        name: path
                            .file_name()
                            .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
                        why: error.to_string(),
                    });
                    return;
                }
            }
        }
        if let Some(before) = before {
            let pages = self.pages_acted_on();
            let beside = if before {
                pages[0]
            } else {
                pages[pages.len() - 1]
            };
            self.make_pages(
                pictures,
                AfterPictures::Insert {
                    beside,
                    before,
                    pages: paths.len(),
                },
            );
            return;
        }
        let suggested = paths
            .first()
            .and_then(|path| path.file_stem())
            .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
        let folder = paths.first().and_then(|path| path.parent());
        self.choosing_for = Choosing::PdfOfPictures(pictures);
        self.chooser = Some(crate::chooser::Chooser::saving(folder, &suggested, 1));
    }

    pub(crate) fn make_pages(&mut self, pictures: Vec<Arc<[u8]>>, then: AfterPictures) {
        if self.making.is_some() {
            return;
        }
        let handle = std::thread::spawn(move || {
            pdf_session::pictures_into_pdf(&pictures).map_err(|error| error.to_string())
        });
        self.making = Some(Making { handle, then });
        self.editor.say(Message::MakingPages);
    }

    pub(crate) fn collect_making(&mut self, ctx: &egui::Context) {
        let Some(making) = &self.making else { return };
        if !making.handle.is_finished() {
            ctx.request_repaint();
            return;
        }
        let Some(making) = self.making.take() else {
            return;
        };
        let made = match making.handle.join() {
            Ok(made) => made,
            Err(_) => Err("the work stopped".to_owned()),
        };
        let bytes = match made {
            Ok(bytes) => bytes,
            Err(why) => {
                self.editor.say(Message::CouldNotMakePages(why));
                return;
            }
        };
        match making.then {
            AfterPictures::Insert {
                beside,
                before,
                pages,
            } => {
                let all: Vec<usize> = (0..pages).collect();
                let job = self.editor.begin_insert_pages(
                    (beside, before),
                    (Arc::from(bytes), pdf_edit::Password::default()),
                    &all,
                );
                if job.is_some() {
                    let first = if before { beside } else { beside + 1 };
                    self.renumber = Some(crate::page_motion::Renumber::inserted(
                        self.editor.page_count(),
                        first,
                        pages,
                    ));
                    self.before_pages_change(first);
                    self.chosen_pages = (first..first + pages).collect();
                }
                self.send(job);
            }
            AfterPictures::Write(path) => {
                match crate::save_file::save(&self.opened, &path, &bytes, None) {
                    Ok(_) => {
                        self.editor.say(Message::SavedTo {
                            name: path.display().to_string(),
                            bytes: bytes.len() as u64,
                        });
                        self.open_at(&path, 0);
                    }
                    Err(error) => {
                        self.editor
                            .say(Message::CouldNotMakePages(error.to_string()));
                    }
                }
            }
        }
    }
}

fn drawn(
    source: &pdf_bytes::ByteStore,
    (credential, fonts): (&[u8], Option<Arc<dyn pdf_content::FontProvider>>),
    page: usize,
    dpi: usize,
) -> Result<Vec<u8>, String> {
    #[expect(
        clippy::cast_precision_loss,
        reason = "a resolution is at most three digits"
    )]
    let dpi = dpi as f64;
    let view = pdf_session::interpret_page_for_display(source, page, credential, None, fonts)
        .map_err(|error| error.to_string())?;
    let (canvas, _) =
        pdf_app::painter::draw_page(&view, dpi / 72.0).map_err(|error| error.to_string())?;
    let metre = pdf_edit::png::per_metre(dpi);
    pdf_edit::png::write(
        (canvas.width, canvas.height),
        &canvas.to_rgb8(),
        Some((metre, metre)),
    )
    .map_err(str::to_owned)
}

#[cfg(test)]
mod tests {
    use pdf_app::wording::{Lang, Message};

    use super::{ExportChoices, export_pages};

    #[test]
    fn the_panel_names_the_pages() {
        let all = ExportChoices::default();
        assert!(all.all);
        assert_eq!(export_pages(&all, 3).expect("every page"), vec![0, 1, 2]);
        let some = ExportChoices {
            all: false,
            pages: "1-2, 4".to_owned(),
            dpi: 150,
        };
        assert_eq!(export_pages(&some, 6).expect("named"), vec![0, 1, 3]);
        let nothing = ExportChoices {
            all: false,
            pages: String::new(),
            dpi: 150,
        };
        assert_eq!(export_pages(&nothing, 6).expect("none"), Vec::new());
    }

    #[test]
    fn a_page_the_document_has_not_got_is_refused() {
        let beyond = ExportChoices {
            all: false,
            pages: "9".to_owned(),
            dpi: 300,
        };
        let why = export_pages(&beyond, 6).expect_err("refused");
        assert!(!why.say(Lang::English).trim().is_empty());
        let nonsense = ExportChoices {
            all: false,
            pages: "every other one".to_owned(),
            dpi: 300,
        };
        assert!(matches!(
            export_pages(&nonsense, 6),
            Err(Message::Refused(_))
        ));
    }
}
