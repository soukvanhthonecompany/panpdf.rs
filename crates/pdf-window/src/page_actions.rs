use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use eframe::egui;

use pdf_app::wording::{Command, Message};

use crate::page_motion::Renumber;
use crate::window_state::{Pointing, Window};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) enum Choosing {
    #[default]
    Open,
    Pages {
        before: bool,
    },
    Picture,
    Copy,
    PagesOut(Vec<usize>),
    Pieces(Vec<Vec<usize>>),
    PagesAsPictures {
        pages: Vec<usize>,
        dpi: usize,
    },
    PicturesIn {
        before: Option<bool>,
    },
    PdfOfPictures(Vec<std::sync::Arc<[u8]>>),
    ChatAttachment,
}

impl Window {
    pub(crate) fn pages_acted_on(&self) -> Vec<usize> {
        let count = self.editor.page_count();
        let chosen: Vec<usize> = self
            .chosen_pages
            .iter()
            .copied()
            .filter(|page| *page < count)
            .collect();
        if chosen.is_empty() {
            vec![self.focus]
        } else {
            chosen
        }
    }

    pub(crate) fn choose_page(&mut self, page: usize, modifiers: egui::Modifiers) {
        if modifiers.command {
            if self.chosen_pages.is_empty() {
                self.chosen_pages.insert(self.focus);
            }
            if !self.chosen_pages.remove(&page) {
                self.chosen_pages.insert(page);
            }
        } else if modifiers.shift {
            let (from, to) = (self.focus.min(page), self.focus.max(page));
            self.chosen_pages = (from..=to).collect();
            return;
        } else {
            self.chosen_pages = BTreeSet::from([page]);
        }
        self.goto(page);
    }

    pub(crate) fn pages_in_menu(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let say = |command| Message::Command(command).say(lang);
        for (before, command) in [
            (true, Command::BlankPageBefore),
            (false, Command::BlankPageAfter),
        ] {
            ui.menu_button(say(command), |ui| {
                if let Some(size) = self.page_size_menu(ui) {
                    self.add_blank_page(before, size);
                    ui.close();
                }
            });
        }
        for (before, command) in [
            (true, Command::PagesFromFileBefore),
            (false, Command::PagesFromFileAfter),
        ] {
            if ui.button(say(command)).clicked() {
                self.choosing_for = Choosing::Pages { before };
                self.asking_to_open = true;
                ui.close();
            }
        }
        for (before, command) in [
            (true, Command::PicturesAsPagesBefore),
            (false, Command::PicturesAsPagesAfter),
        ] {
            if ui.button(say(command)).clicked() {
                self.choose_pictures(Some(before));
                ui.close();
            }
        }
    }

    pub(crate) fn page_menu(&mut self, ui: &mut egui::Ui, working: bool) {
        let lang = self.lang;
        let say = |command| Message::Command(command).say(lang);
        ui.add_enabled_ui(working, |ui| {
            let _ = ui.checkbox(&mut self.landscape, say(Command::Landscape));
            ui.separator();
            self.pages_in_menu(ui);
            ui.separator();
            let pages = self.pages_acted_on();
            let count = self.editor.page_count();
            let first = pages[0];
            let block = pages.len();
            let last_place = count.saturating_sub(block);
            for (command, to, possible) in [
                (Command::MovePageFirst, 0, first > 0),
                (Command::MovePageEarlier, first.saturating_sub(1), first > 0),
                (Command::MovePageLater, first + 1, first < last_place),
                (Command::MovePageLast, last_place, first < last_place),
            ] {
                if ui
                    .add_enabled(possible, egui::Button::new(say(command)))
                    .clicked()
                {
                    self.move_pages(&pages, to);
                    ui.close();
                }
            }
            ui.separator();
            for (command, quarter_turns) in [
                (Command::RotateClockwise, 1),
                (Command::RotateCounterClockwise, -1),
            ] {
                if ui.button(say(command)).clicked() {
                    self.rotate_pages(&pages, quarter_turns);
                    ui.close();
                }
            }
            ui.separator();
            if ui.button(say(Command::DuplicatePage)).clicked() {
                self.duplicate_pages();
                ui.close();
            }
            if ui.button(say(Command::PagesToNewFile)).clicked() {
                self.take_these_pages_out();
                ui.close();
            }
            ui.separator();
            if ui
                .add_enabled(block < count, egui::Button::new(say(Command::DeletePage)))
                .clicked()
            {
                self.remove_pages(&pages);
                ui.close();
            }
        });
    }

    pub(crate) fn before_pages_change(&mut self, going_to: usize) {
        self.put_the_drag_down();
        self.point_at(Pointing::Nothing);
        self.chosen_pages.clear();
        self.turning_to = Some(going_to);
    }

    pub(crate) fn aim_at_gap(&mut self, gap: usize) -> bool {
        let count = self.editor.page_count();
        if gap < count {
            self.chosen_pages = BTreeSet::from([gap]);
            true
        } else {
            self.chosen_pages = BTreeSet::from([count.saturating_sub(1)]);
            false
        }
    }

    pub(crate) fn add_blank_page(&mut self, before: bool, size: [f64; 2]) {
        let pages = self.pages_acted_on();
        let beside = if before {
            pages[0]
        } else {
            pages[pages.len() - 1]
        };
        let job = self.editor.begin_add_page(beside, before, size);
        if job.is_some() {
            let first = if before { beside } else { beside + 1 };
            self.renumber = Some(Renumber::inserted(self.editor.page_count(), first, 1));
            self.before_pages_change(first);
        }
        self.send(job);
    }

    pub(crate) fn remove_pages(&mut self, pages: &[usize]) {
        let job = self.editor.begin_remove_pages(pages);
        if job.is_some() {
            self.renumber = Some(Renumber::removed(self.editor.page_count(), pages));
            self.before_pages_change(pages[0]);
        }
        self.send(job);
    }

    pub(crate) fn rotate_pages(&mut self, pages: &[usize], quarter_turns: i32) {
        let job = self.editor.begin_rotate_pages(pages, quarter_turns);
        if job.is_some() {
            self.renumber = Some(Renumber::turned(
                self.editor.page_count(),
                pages,
                quarter_turns,
            ));
            let kept: BTreeSet<usize> = self.chosen_pages.clone();
            self.before_pages_change(pages[0]);
            self.chosen_pages = kept;
        }
        self.send(job);
    }

    pub(crate) fn move_pages(&mut self, pages: &[usize], to: usize) {
        let job = self.editor.begin_move_pages(pages, to);
        if job.is_some() {
            let renumber = Renumber::moved(self.editor.page_count(), pages, to);
            self.page_preview = Some(renumber.was.iter().flatten().copied().collect());
            self.renumber = Some(renumber);
            self.before_pages_change(to);
            self.chosen_pages = (to..to + pages.len()).collect();
        }
        self.send(job);
    }

    pub(crate) fn duplicate_pages(&mut self) {
        let pages = self.pages_acted_on();
        let Some(bytes) = self.editor.bytes_now() else {
            return;
        };
        let beside = pages[pages.len() - 1];
        let job = self
            .editor
            .begin_insert_pages((beside, false), bytes, &pages);
        if job.is_some() {
            let first = beside + 1;
            self.renumber = Some(Renumber::copied(self.editor.page_count(), &pages, first));
            self.before_pages_change(first);
            self.chosen_pages = (first..first + pages.len()).collect();
        }
        self.send(job);
    }

    pub(crate) fn insert_pages_from(&mut self, path: &Path, before: bool) -> usize {
        let name = path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        let bytes: Arc<[u8]> = match std::fs::read(path) {
            Ok(bytes) => Arc::from(bytes),
            Err(error) => {
                self.editor.say(Message::PagesNotRead {
                    name,
                    why: error.to_string(),
                });
                return 0;
            }
        };
        let source = pdf_bytes::ByteStore::new(pdf_bytes::SourceId::new(0), Arc::clone(&bytes));
        let count = match pdf_content::count_pages_recovering(
            &source,
            pdf_content::PageContentLimits::default(),
            pdf_content::RecoverLimits::default(),
            b"",
        )
        .map(|recovered| recovered.into_parts().0)
        {
            Ok(count) if count > 0 => count,
            Ok(_) => {
                self.editor.say(Message::PagesNotRead {
                    name,
                    why: "it has no pages".to_owned(),
                });
                return 0;
            }
            Err(error) => {
                self.editor.say(Message::PagesNotRead {
                    name,
                    why: error.to_string(),
                });
                return 0;
            }
        };
        let pages = self.pages_acted_on();
        let beside = if before {
            pages[0]
        } else {
            pages[pages.len() - 1]
        };
        let all: Vec<usize> = (0..count).collect();
        let job = self
            .editor
            .begin_insert_pages((beside, before), bytes, &all);
        let sent = job.is_some();
        if sent {
            let first = if before { beside } else { beside + 1 };
            self.renumber = Some(Renumber::inserted(self.editor.page_count(), first, count));
            self.before_pages_change(first);
            self.chosen_pages = (first..first + count).collect();
        }
        self.send(job);
        if sent { count } else { 0 }
    }
}
