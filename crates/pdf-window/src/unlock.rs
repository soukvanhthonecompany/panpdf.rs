use eframe::egui;

use pdf_app::wording::Message;

use crate::app::name_of;
use crate::window_state::Window;

const WIDTH: f32 = 420.0;

pub(crate) struct Unlock {
    pub(crate) path: std::path::PathBuf,
    pub(crate) page: usize,
    pub(crate) source: pdf_bytes::ByteStore,
    pub(crate) typed: String,
    pub(crate) tried: bool,
    pub(crate) shown: bool,
    pub(crate) focus: bool,
    pub(crate) for_pages: Option<bool>,
}

impl Window {
    pub(crate) fn asks_for_a_password(&self) -> bool {
        self.unlocking.is_some()
    }

    pub(crate) fn ask_for_the_password(&mut self, ctx: &egui::Context) {
        let Some(mut unlock) = self.unlocking.take() else {
            return;
        };
        let lang = self.lang;
        let mut answered = false;
        let mut gave_up = false;
        let modal = egui::Modal::new(egui::Id::new("document-locked")).show(ctx, |ui| {
            ui.set_width(WIDTH);
            ui.heading(Message::DocumentIsLocked(name_of(&unlock.path)).say(lang));
            ui.add_space(4.0);
            ui.label(if unlock.tried {
                Message::PasswordRefused.say(lang)
            } else {
                Message::AskForThePassword.say(lang)
            });
            ui.add_space(6.0);
            let box_ = ui.add(
                egui::TextEdit::singleline(&mut unlock.typed)
                    .password(!unlock.shown)
                    .desired_width(f32::INFINITY)
                    .hint_text("••••••••"),
            );
            if unlock.focus {
                box_.request_focus();
                unlock.focus = false;
            }
            answered = box_.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            ui.add_space(4.0);
            ui.checkbox(&mut unlock.shown, Message::ShowThePassword.say(lang));
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(Message::EitherPasswordOpensIt.say(lang))
                    .small()
                    .color(ui.visuals().weak_text_color()),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let ready = !unlock.typed.is_empty();
                answered |= ui
                    .add_enabled(ready, egui::Button::new(Message::Unlock.say(lang)))
                    .clicked();
                gave_up |= ui.button(Message::CancelLeaving.say(lang)).clicked();
            });
        });
        if gave_up || modal.should_close() {
            self.editor.say(match unlock.for_pages {
                None => Message::LeftLocked,
                Some(_) => Message::PagesNotRead {
                    name: name_of(&unlock.path),
                    why: "it asks for a password".to_owned(),
                },
            });
            return;
        }
        if answered && !unlock.typed.is_empty() {
            self.try_the_password(unlock);
            return;
        }
        self.unlocking = Some(unlock);
    }

    pub(crate) fn try_the_password(&mut self, unlock: Unlock) {
        if let Some(before) = unlock.for_pages {
            if pdf_edit::info::lock(&unlock.source, unlock.typed.as_bytes())
                == pdf_edit::info::Lock::Refused
            {
                self.unlocking = Some(Unlock {
                    typed: String::new(),
                    tried: true,
                    focus: true,
                    ..unlock
                });
                return;
            }
            let Unlock {
                path,
                source,
                typed,
                ..
            } = unlock;
            let bytes: std::sync::Arc<[u8]> = std::sync::Arc::from(source.to_vec());
            self.insert_pages_opened(&path, (bytes, typed.into_bytes()), before);
            return;
        }
        let Unlock {
            path,
            page,
            source,
            typed,
            ..
        } = unlock;
        self.editor.say(Message::Opening(name_of(&path)));
        let credential = typed.into_bytes();
        self.loading = Some(crate::window_state::Opening {
            path,
            page,
            changed_protection: false,
            tried_a_password: true,
            handle: std::thread::spawn(move || crate::chrome::open_bytes(source, &credential)),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::Unlock;
    use crate::window_state::{Opened, Window};
    use pdf_bytes::{ByteStore, SourceId};

    fn locked() -> ByteStore {
        ByteStore::new(
            SourceId::new(0),
            &include_bytes!("../../pdf-edit/tests/data/modifiable-r3.pdf")[..],
        )
    }

    fn window() -> Window {
        Window::new(
            pdf_app::Editor::stand_in().expect("the stand-in document opens"),
            std::path::PathBuf::new(),
            Vec::new(),
        )
    }

    fn settle(window: &mut Window) {
        let ctx = eframe::egui::Context::default();
        for _ in 0..2_000 {
            window.collect_open(&ctx);
            if window.loading.is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the open never finished");
    }

    #[test]
    fn a_locked_document_is_asked_about_and_a_broken_one_is_refused() {
        assert!(
            matches!(crate::chrome::open_bytes(locked(), b""), Opened::Locked(_)),
            "the empty password every open starts with is refused, so the person is asked"
        );
        assert!(matches!(
            crate::chrome::open_bytes(locked(), b"view"),
            Opened::Document(_)
        ));
        assert!(
            matches!(
                crate::chrome::open_bytes(locked(), b"master"),
                Opened::Document(_)
            ),
            "the owner's password opens it too"
        );
        let rubbish = ByteStore::new(SourceId::new(1), b"not a document at all".to_vec());
        assert!(
            matches!(crate::chrome::open_bytes(rubbish, b""), Opened::Refused(_)),
            "a file no password would mend is refused, not asked about"
        );
    }

    #[test]
    fn a_wrong_password_asks_again_and_the_right_one_opens_the_document() {
        let mut window = window();
        let path = std::path::PathBuf::from("/wherever/locked.pdf");
        window.unlocking = Some(Unlock {
            path: path.clone(),
            page: 0,
            source: locked(),
            typed: "not it".to_owned(),
            tried: false,
            shown: false,
            focus: true,
            for_pages: None,
        });
        let unlock = window.unlocking.take().expect("the question is up");
        window.try_the_password(unlock);
        settle(&mut window);

        let again = window
            .unlocking
            .as_ref()
            .expect("a wrong password asks again rather than giving up");
        assert!(
            again.tried,
            "the second asking says the password was wrong, not that one is wanted"
        );
        assert!(
            again.typed.is_empty(),
            "the refused password is not left in the box"
        );
        assert!(again.focus, "and the box takes the typing again");
        assert_eq!(
            again.path, path,
            "it is still the same file being asked about"
        );
        assert!(!window.has_document(), "nothing was opened");

        let mut unlock = window.unlocking.take().expect("the question is up");
        unlock.typed = "view".to_owned();
        window.try_the_password(unlock);
        settle(&mut window);

        assert!(
            window.unlocking.is_none(),
            "the question is answered and gone"
        );
        assert!(window.has_document());
        assert_eq!(
            window.opened, path,
            "the document is the file that was locked"
        );
    }
}
