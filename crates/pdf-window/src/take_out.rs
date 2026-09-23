use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;

use pdf_app::wording::{Command, Message};

use crate::page_actions::Choosing;
use crate::window_state::Window;

pub(crate) struct Piece {
    pub(crate) path: PathBuf,
    pub(crate) pages: Vec<usize>,
}

pub(crate) struct Writing {
    pub(crate) handle: std::thread::JoinHandle<Result<Wrote, String>>,
    pub(crate) pictures: bool,
}

pub(crate) struct Wrote {
    pub(crate) files: Vec<PathBuf>,
    pub(crate) bytes: u64,
}

pub(crate) struct SplitChoices {
    pub(crate) by_count: bool,
    pub(crate) size: usize,
    pub(crate) starts: String,
}

impl Default for SplitChoices {
    fn default() -> Self {
        Self {
            by_count: true,
            size: 1,
            starts: String::new(),
        }
    }
}

pub(crate) fn split_pieces(
    choices: &SplitChoices,
    count: usize,
) -> Result<Vec<Vec<usize>>, Message> {
    if choices.by_count {
        return Ok(pdf_app::pieces::every(count, choices.size));
    }
    if choices.starts.trim().is_empty() {
        return Ok(pdf_app::pieces::at(count, &[]));
    }
    let starts = pdf_edit::stamp::pages_of(&choices.starts, count, pdf_edit::stamp::Only::Every)
        .map_err(|error| Message::refusal(&error))?;
    Ok(pdf_app::pieces::at(count, &starts))
}

impl Window {
    pub(crate) fn save_a_copy_as(&mut self) {
        let suggested = self.destination.file_name().map_or_else(
            || format!("{}.pdf", pdf_app::wording::Home::Untitled.say(self.lang)),
            |name| name.to_string_lossy().into_owned(),
        );
        self.ask_where(Choosing::Copy, &suggested, 1);
    }

    pub(crate) fn take_these_pages_out(&mut self) {
        let pages = self.pages_acted_on();
        let suggested = pdf_app::files::named_for_pages(&self.opened, &pages);
        self.ask_where(Choosing::PagesOut(pages), &suggested, 1);
    }

    pub(crate) fn open_the_split_panel(&mut self) {
        self.splitting = Some(SplitChoices::default());
    }

    fn ask_where(&mut self, what: Choosing, suggested: &str, spread: usize) {
        let folder = self.opened.parent().map(Path::to_path_buf);
        self.chooser = Some(crate::chooser::Chooser::saving(
            folder.as_deref(),
            suggested,
            spread,
        ));
        self.choosing_for = what;
    }

    pub(crate) fn split_dialog(&mut self, ctx: &egui::Context) {
        if self.splitting.is_none() || !self.has_document() {
            return;
        }
        let (lang, count) = (self.lang, self.editor.page_count());
        let Some(choices) = self.splitting.as_mut() else {
            return;
        };
        let pieces = split_pieces(choices, count);
        let (mut close, mut go_on) = (false, false);
        let title = Message::Command(Command::SplitDocument).say(lang);
        let modal = egui::Modal::new(egui::Id::new("split-dialog")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.heading(title.trim_end_matches('\u{2026}'));
            ui.label(
                egui::RichText::new(Message::SplitWhy.say(lang))
                    .size(11.0)
                    .color(ui.visuals().weak_text_color()),
            );
            ui.separator();
            ui.horizontal(|ui| {
                ui.radio_value(&mut choices.by_count, true, Message::SplitEvery.say(lang));
                let size = ui.add(egui::DragValue::new(&mut choices.size).range(1..=count.max(1)));
                if size.changed() {
                    choices.by_count = true;
                }
                ui.label(Message::SplitEveryPages.say(lang));
            });
            ui.horizontal(|ui| {
                ui.radio_value(
                    &mut choices.by_count,
                    false,
                    Message::SplitAtPages.say(lang),
                );
                let typed = ui.add(
                    egui::TextEdit::singleline(&mut choices.starts)
                        .hint_text("5, 12")
                        .desired_width(120.0),
                );
                if typed.changed() {
                    choices.by_count = false;
                }
            });
            ui.separator();
            match &pieces {
                Ok(pieces) => {
                    ui.label(
                        Message::SplitInto {
                            pages: count,
                            files: pieces.len(),
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
                let ready = pieces.as_ref().is_ok_and(|pieces| pieces.len() > 1);
                let on = egui::Button::new(Message::SplitWhere.say(lang));
                if ui.add_enabled(ready, on).clicked() {
                    go_on = true;
                }
                if ui
                    .button(Message::Home(pdf_app::wording::Home::Cancel).say(lang))
                    .clicked()
                {
                    close = true;
                }
            });
        });
        if modal.should_close() {
            close = true;
        }
        if go_on && let Ok(pieces) = pieces {
            self.splitting = None;
            let suggested = self
                .opened
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
            let spread = pieces.len();
            self.ask_where(Choosing::Pieces(pieces), &suggested, spread);
        } else if close {
            self.splitting = None;
        }
    }

    pub(crate) fn write_what_was_chosen(&mut self, path: &Path) {
        match std::mem::take(&mut self.choosing_for) {
            Choosing::Copy => self.save_a_copy_to(path),
            Choosing::PagesOut(pages) => self.write_pieces(vec![Piece {
                path: path.to_path_buf(),
                pages,
            }]),
            Choosing::Pieces(pieces) => {
                let of = pieces.len();
                let files = pieces
                    .into_iter()
                    .enumerate()
                    .map(|(at, pages)| Piece {
                        path: pdf_app::files::numbered(path, at + 1, of),
                        pages,
                    })
                    .collect();
                self.write_pieces(files);
            }
            Choosing::PagesAsPictures { pages, dpi } => {
                self.write_pages_as_pictures(path, &pages, dpi);
            }
            Choosing::PdfOfPictures(pictures) => {
                self.make_pages(
                    pictures,
                    crate::pictures::AfterPictures::Write(path.to_path_buf()),
                );
            }
            Choosing::Open
            | Choosing::Pages { .. }
            | Choosing::Picture
            | Choosing::ChatAttachment
            | Choosing::PicturesIn { .. } => {}
        }
    }

    fn save_a_copy_to(&mut self, path: &Path) {
        let (was, digest) = (self.destination.clone(), self.saved_digest.take());
        self.destination = path.to_path_buf();
        if !self.save() {
            self.destination = was;
            self.saved_digest = digest;
        }
    }

    fn write_pieces(&mut self, pieces: Vec<Piece>) {
        if self.writing.is_some() {
            return;
        }
        let bytes: Arc<[u8]> = match self.editor.export() {
            Ok(export) => Arc::from(export.bytes),
            Err(why) => {
                self.editor.say(Message::CouldNotTakePagesOut(why));
                return;
            }
        };
        let original = self.opened.clone();
        let handle = std::thread::spawn(move || write_each(&original, &bytes, &pieces));
        self.writing = Some(Writing {
            handle,
            pictures: false,
        });
    }

    pub(crate) fn collect_writing(&mut self, ctx: &egui::Context) {
        let Some(writing) = &self.writing else { return };
        if !writing.handle.is_finished() {
            ctx.request_repaint();
            return;
        }
        let Some(writing) = self.writing.take() else {
            return;
        };
        let pictures = writing.pictures;
        let refused = |why: String| {
            if pictures {
                Message::CouldNotWritePictures(why)
            } else {
                Message::CouldNotTakePagesOut(why)
            }
        };
        let said = match writing.handle.join() {
            Ok(Ok(wrote)) => match wrote.files.as_slice() {
                [] => refused("nothing was named".to_owned()),
                [one] => Message::SavedTo {
                    name: one.display().to_string(),
                    bytes: wrote.bytes,
                },
                [first, ..] if pictures => Message::WrotePictures {
                    files: wrote.files.len(),
                    first: first.display().to_string(),
                    bytes: wrote.bytes,
                },
                [first, ..] => Message::WroteFiles {
                    files: wrote.files.len(),
                    first: first.display().to_string(),
                    bytes: wrote.bytes,
                },
            },
            Ok(Err(why)) => refused(why),
            Err(_) => refused("the work stopped".to_owned()),
        };
        self.editor.say(said);
    }
}

fn write_each(original: &Path, bytes: &Arc<[u8]>, pieces: &[Piece]) -> Result<Wrote, String> {
    let mut wrote = Wrote {
        files: Vec::with_capacity(pieces.len()),
        bytes: 0,
    };
    for piece in pieces {
        let written = if piece.pages.is_empty() {
            bytes.to_vec()
        } else {
            pdf_session::extract_pages(bytes, &piece.pages).map_err(|error| error.to_string())?
        };
        crate::save_file::save(original, &piece.path, &written, None)
            .map_err(|error| format!("{}: {error}", piece.path.display()))?;
        wrote.bytes += written.len() as u64;
        wrote.files.push(piece.path.clone());
    }
    Ok(wrote)
}

#[cfg(test)]
mod tests {
    use pdf_app::wording::{Lang, Message};

    use super::{SplitChoices, split_pieces};

    #[test]
    fn the_rules_name_the_files() {
        let every = SplitChoices {
            by_count: true,
            size: 2,
            starts: "9".to_owned(),
        };
        assert_eq!(
            split_pieces(&every, 5).expect("every two pages"),
            vec![vec![0, 1], vec![2, 3], vec![4]]
        );
        let at = SplitChoices {
            by_count: false,
            size: 2,
            starts: "3, 5".to_owned(),
        };
        assert_eq!(
            split_pieces(&at, 6).expect("at pages 3 and 5"),
            vec![vec![0, 1], vec![2, 3], vec![4, 5]]
        );
        let whole = SplitChoices {
            by_count: false,
            size: 1,
            starts: String::new(),
        };
        assert_eq!(split_pieces(&whole, 3).expect("all"), vec![vec![0, 1, 2]]);
    }

    #[test]
    fn a_page_the_document_has_not_got_is_refused() {
        let at = SplitChoices {
            by_count: false,
            size: 1,
            starts: "99".to_owned(),
        };
        let why = split_pieces(&at, 6).expect_err("refused");
        assert!(!why.say(Lang::English).trim().is_empty());
        let nonsense = SplitChoices {
            by_count: false,
            size: 1,
            starts: "chapter two".to_owned(),
        };
        assert!(matches!(
            split_pieces(&nonsense, 6),
            Err(Message::Refused(_))
        ));
    }
}
