use eframe::egui;

use pdf_app::wording::Message;
use pdf_edit::outline::{Bookmark, Change};
use pdf_syntax::Reference;

use crate::window_state::Window;

const PANEL_WIDTH: f32 = 250.0;
const STEP: f32 = 14.0;

impl Window {
    pub(crate) fn contents_panel(&mut self, ui: &mut egui::Ui) {
        if !self.show_contents || !self.has_document() {
            return;
        }
        let lang = self.lang;
        let bookmarks = self.editor.bookmarks();
        let mut asked: Option<Change> = None;
        let mut go_to = None;
        egui::Panel::left("contents")
            .resizable(false)
            .exact_size(PANEL_WIDTH)
            .show(ui, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(Message::Contents.say(lang))
                            .size(11.0)
                            .color(ui.visuals().weak_text_color()),
                    );
                });
                ui.add_space(4.0);
                asked = self.contents_buttons(ui, &bookmarks);
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let mut hidden_under: Option<usize> = None;
                    for book in bookmarks.iter() {
                        match hidden_under {
                            Some(depth) if book.depth > depth => continue,
                            Some(_) => hidden_under = None,
                            None => {}
                        }
                        if !book.open && book.children > 0 {
                            hidden_under = Some(book.depth);
                        }
                        ui.horizontal(|ui| {
                            #[allow(
                                clippy::cast_precision_loss,
                                reason = "a bookmark is a few levels deep"
                            )]
                            ui.add_space(book.depth as f32 * STEP);
                            if book.children > 0 {
                                let mark = if book.open { "▾" } else { "▸" };
                                if ui.small_button(mark).clicked() {
                                    asked = Some(Change::Fold {
                                        bookmark: book.reference,
                                        open: !book.open,
                                    });
                                }
                            } else {
                                ui.add_space(STEP);
                            }
                            let chosen = self.chosen_bookmark == Some(book.reference);
                            let row = ui.selectable_label(chosen, &book.title);
                            if row.clicked() {
                                self.chosen_bookmark = Some(book.reference);
                                go_to = book.page;
                            }
                        });
                    }
                });
            });
        if let Some(page) = go_to {
            self.goto(page);
        }
        if let Some(change) = asked {
            self.change_the_outline(change);
        }
    }

    fn contents_buttons(&mut self, ui: &mut egui::Ui, bookmarks: &[Bookmark]) -> Option<Change> {
        let lang = self.lang;
        let idle = !self.editor.is_busy();
        let chosen = self
            .chosen_bookmark
            .and_then(|reference| bookmarks.iter().find(|book| book.reference == reference));
        let mut asked = None;
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(idle, egui::Button::new(Message::FieldAddItem.say(lang)))
                .on_hover_text(Message::AddBookmarkSaid.say(lang))
                .clicked()
            {
                self.renaming = Some((None, self.title_for_this_page()));
            }
            let has = chosen.is_some();
            if ui
                .add_enabled(
                    idle && has,
                    egui::Button::new(Message::RenameBookmark.say(lang)),
                )
                .clicked()
                && let Some(book) = chosen
            {
                self.renaming = Some((Some(book.reference), book.title.clone()));
            }
            if ui
                .add_enabled(
                    idle && has,
                    egui::Button::new(Message::FieldDeleteItem.say(lang)),
                )
                .clicked()
                && let Some(book) = chosen
            {
                asked = Some(Change::Remove {
                    bookmark: book.reference,
                });
            }
        });
        ui.horizontal_wrapped(|ui| {
            for (mark, said, step) in [
                ("▲", Message::BookmarkUp, Step::Up),
                ("▼", Message::BookmarkDown, Step::Down),
                ("◀", Message::BookmarkOut, Step::Out),
                ("▶", Message::BookmarkIn, Step::In),
            ] {
                if ui
                    .add_enabled(idle && chosen.is_some(), egui::Button::new(mark))
                    .on_hover_text(said.say(lang))
                    .clicked()
                    && let Some(book) = chosen
                {
                    asked = moved(bookmarks, book, step);
                }
            }
        });
        asked
    }

    fn title_for_this_page(&mut self) -> String {
        let page = self.focus;
        self.editor.first_line_of(page).unwrap_or_else(|| {
            Message::PageOf {
                page: page + 1,
                count: self.editor.page_count(),
            }
            .say(self.lang)
        })
    }

    pub(crate) fn change_the_outline(&mut self, change: Change) {
        let job = self.editor.begin_change_outline(self.focus, change);
        if job.is_none() {
            self.editor.say(Message::AnotherEditIsRunning);
            return;
        }
        self.send(job);
    }

    pub(crate) fn bookmark_name_box(&mut self, ctx: &egui::Context) {
        let Some((bookmark, mut title)) = self.renaming.clone() else {
            return;
        };
        let lang = self.lang;
        let mut done = false;
        let mut cancelled = false;
        egui::Window::new(Message::BookmarkName.say(lang))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                let box_ = ui.add(egui::TextEdit::singleline(&mut title).desired_width(280.0));
                box_.request_focus();
                ui.horizontal(|ui| {
                    done = ui.button(Message::Apply.say(lang)).clicked()
                        || (box_.lost_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter)));
                    cancelled = ui.button(Message::CancelLeaving.say(lang)).clicked()
                        || ui.input(|input| input.key_pressed(egui::Key::Escape));
                });
            });
        self.renaming = Some((bookmark, title.clone()));
        if cancelled {
            self.renaming = None;
            return;
        }
        if !done {
            return;
        }
        self.renaming = None;
        let change = match bookmark {
            Some(bookmark) => Change::Rename { bookmark, title },
            None => Change::Add {
                title,
                page: self.focus,
                after: self.chosen_bookmark,
                inside: false,
            },
        };
        self.change_the_outline(change);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Step {
    Up,
    Down,
    In,
    Out,
}

fn moved(bookmarks: &[Bookmark], book: &Bookmark, step: Step) -> Option<Change> {
    let siblings: Vec<&Bookmark> = bookmarks
        .iter()
        .filter(|other| other.parent == book.parent && other.depth == book.depth)
        .collect();
    let at = siblings
        .iter()
        .position(|other| other.reference == book.reference)?;
    let move_to = |after: Option<Reference>, inside: bool, before: bool| {
        Some(Change::Move {
            bookmark: book.reference,
            after,
            inside,
            before,
        })
    };
    match step {
        Step::Up => {
            let above = siblings.get(at.checked_sub(1)?)?;
            move_to(Some(above.reference), false, true)
        }
        Step::Down => {
            let below = siblings.get(at + 1)?;
            move_to(Some(below.reference), false, false)
        }
        Step::In => {
            let above = siblings.get(at.checked_sub(1)?)?;
            move_to(Some(above.reference), true, false)
        }
        Step::Out => move_to(Some(book.parent?), false, false),
    }
}

#[cfg(test)]
mod tests {
    use super::{Step, moved};
    use pdf_edit::outline::{Bookmark, Change};
    use pdf_syntax::Reference;

    fn at(number: u32, depth: usize, parent: Option<u32>) -> Bookmark {
        Bookmark {
            reference: Reference::new(number, 0),
            title: format!("Book {number}"),
            page: Some(0),
            depth,
            open: true,
            children: 0,
            previous: None,
            parent: parent.map(|number| Reference::new(number, 0)),
        }
    }

    fn a_list() -> Vec<Bookmark> {
        vec![
            at(1, 0, None),
            at(2, 1, Some(1)),
            at(3, 1, Some(1)),
            at(4, 0, None),
        ]
    }

    #[test]
    fn a_bookmark_moves_within_its_level() {
        let list = a_list();
        assert_eq!(
            moved(&list, &list[2], Step::Up),
            Some(Change::Move {
                bookmark: Reference::new(3, 0),
                after: Some(Reference::new(2, 0)),
                inside: false,
                before: true,
            })
        );
        assert_eq!(
            moved(&list, &list[0], Step::Down),
            Some(Change::Move {
                bookmark: Reference::new(1, 0),
                after: Some(Reference::new(4, 0)),
                inside: false,
                before: false,
            })
        );
        assert_eq!(
            moved(&list, &list[0], Step::Up),
            None,
            "the first cannot go up"
        );
        assert_eq!(
            moved(&list, &list[3], Step::Down),
            None,
            "nor the last down"
        );
    }

    #[test]
    fn a_bookmark_moves_in_and_out() {
        let list = a_list();
        assert_eq!(
            moved(&list, &list[2], Step::In),
            Some(Change::Move {
                bookmark: Reference::new(3, 0),
                after: Some(Reference::new(2, 0)),
                inside: true,
                before: false,
            })
        );
        assert_eq!(
            moved(&list, &list[1], Step::Out),
            Some(Change::Move {
                bookmark: Reference::new(2, 0),
                after: Some(Reference::new(1, 0)),
                inside: false,
                before: false,
            })
        );
        assert_eq!(moved(&list, &list[0], Step::In), None, "nothing above it");
        assert_eq!(
            moved(&list, &list[3], Step::Out),
            None,
            "already at the top"
        );
    }
}
