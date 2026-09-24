use eframe::egui;
use pdf_app::wording::Message;

use crate::window_state::Window;

const SLOW_EDIT_MS: f64 = 100.0;

impl Window {
    pub(crate) fn show_the_drawing_speed(&mut self, ctx: &egui::Context) {
        if !self.show_speed {
            return;
        }
        ctx.request_repaint();
        let Some(summary) = self.speed.summary() else {
            return;
        };
        let mut said = Message::DrawingSpeed(summary).say(self.lang);
        if let Some(edit) = self.last_edit {
            said.push('\n');
            said.push_str(&Message::EditSpeed(edit).say(self.lang));
        }
        egui::Area::new(egui::Id::new("drawing-speed"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -16.0))
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .inner_margin(egui::Margin::symmetric(8, 4))
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(egui::RichText::new(said).monospace().size(12.0))
                                .wrap_mode(egui::TextWrapMode::Extend),
                        );
                    });
            });
    }

    pub(crate) fn take_in_the_frames_cost(&mut self, took: std::time::Duration) {
        self.speed.saw(took.as_secs_f64() * 1e3);
        self.take_in_the_edits_cost();
    }

    fn take_in_the_edits_cost(&mut self) {
        let Some(edit) = self.editor.take_last_edit() else {
            return;
        };
        self.last_edit = Some(edit);
        if edit.total < SLOW_EDIT_MS {
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        crate::reporting::say(pdf_app::trouble::Kind::Session, &edit.logged());
        #[cfg(target_arch = "wasm32")]
        web_sys::console::log_1(&edit.logged().into());
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn log_the_drawing_speed(&self) {
        if let Some(summary) = self.speed.summary() {
            crate::reporting::say(pdf_app::trouble::Kind::Session, &summary.logged());
        }
    }
}
