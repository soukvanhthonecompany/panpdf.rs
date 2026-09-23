use eframe::egui;

use pdf_app::wording::{Done, Message, Refusal};

use crate::window_state::{Clipboard, Window};

impl Window {
    pub(crate) fn copy_the_selection(&mut self, ctx: &egui::Context) -> bool {
        let Some(page) = self.pointing.page() else {
            return false;
        };
        let Some((anchors, bounds)) = self.what_to_copy(page) else {
            return false;
        };
        let copied = match self.editor.copy_objects(page, &anchors) {
            Ok(copied) => copied,
            Err(why) => {
                self.editor
                    .say(Message::Refused(Refusal::NothingToCopy { why }));
                return false;
            }
        };
        let objects = copied.objects.len();
        if objects == 0 {
            return false;
        }
        let marker = self
            .text_of_the_pointed_block(page)
            .unwrap_or_else(|| format!("PanPDF: {objects}"));
        ctx.copy_text(marker.clone());
        let Some(from) = self.editor.source().cloned() else {
            self.editor.say(Message::AnotherEditIsRunning);
            return false;
        };
        self.clipboard = Some(Clipboard {
            copied,
            marker,
            bounds,
            from,
        });
        self.editor.say(Message::Done(Done::Copied { objects }));
        true
    }

    pub(crate) fn copy(&mut self, ctx: &egui::Context, in_text: bool) {
        if in_text {
            self.copy_selection(ctx);
        } else {
            self.copy_the_selection(ctx);
        }
    }

    pub(crate) fn cut(&mut self, ctx: &egui::Context, in_text: bool) {
        if in_text {
            if self.copy_selection(ctx) {
                self.delete_direction(true);
            }
        } else if self.copy_the_selection(ctx) {
            self.delete_the_object();
        }
    }

    pub(crate) fn paste_the_clipboard(&mut self, ctx: &egui::Context, in_place: bool) {
        if self.editor.is_busy() {
            return;
        }
        let Some(clipboard) = self.clipboard.as_ref() else {
            self.editor.say(Message::Done(Done::NothingToPaste));
            return;
        };
        let pointer = ctx
            .input(|input| input.pointer.hover_pos())
            .and_then(|at| self.page_point(at));
        let (page, at) = match pointer {
            Some((page, at)) => (page, Some(at)),
            None => (self.focus, None),
        };
        let offset = if in_place {
            (0.0, 0.0)
        } else {
            pdf_app::put_down::offset(at, clipboard.bounds)
        };
        let copied = clipboard.copied.clone();
        let elsewhere = self
            .editor
            .source()
            .is_none_or(|into| into.id().get() != clipboard.from.id().get())
            .then(|| clipboard.from.clone());
        let job = self.editor.begin_paste(page, copied, offset, elsewhere);
        if job.is_none() {
            self.editor.say(Message::AnotherEditIsRunning);
        }
        self.send(job);
    }

    pub(crate) fn clipboard_is_ours(&self, text: &str) -> bool {
        self.clipboard
            .as_ref()
            .is_some_and(|clipboard| clipboard.marker == text)
    }

    fn what_to_copy(&self, page: usize) -> Option<(Vec<String>, [f64; 4])> {
        let overlay = self.overlay(page)?;
        if self.chosen.is_a_group() && self.chosen.page == page {
            let (mut anchors, objects) = self.group_members()?;
            anchors.extend(objects);
            let boxes =
                self.chosen
                    .blocks
                    .iter()
                    .filter_map(|block| overlay.blocks.get(*block).map(|block| block.box_pixels))
                    .chain(
                        self.chosen.objects.iter().filter_map(|object| {
                            overlay.objects.get(*object).map(|o| o.box_pixels)
                        }),
                    );
            let bounds = pdf_app::put_down::bounds_of(boxes)?;
            return (!anchors.is_empty()).then_some((anchors, bounds));
        }
        if let Some(block) = self.pointing.block() {
            let bounds = overlay.blocks.get(block)?.box_pixels;
            return Some((self.anchors_of(page, block)?, bounds));
        }
        let object = overlay.objects.get(self.pointing.object_on(page)?)?;
        Some((vec![object.anchor.clone()], object.box_pixels))
    }

    fn text_of_the_pointed_block(&self, page: usize) -> Option<String> {
        if self.chosen.is_a_group() {
            return None;
        }
        let block = self.pointing.block()?;
        let overlay = self.overlay(page)?;
        let rows = &overlay.blocks.get(block)?.lines;
        let last = *rows.last()?;
        let end = overlay
            .carets
            .iter()
            .filter(|stop| stop.line == last)
            .map(|stop| stop.offset)
            .max()?;
        self.editor
            .copy_text(page, block, (0, 0), (rows.len() - 1, end))
    }
}
