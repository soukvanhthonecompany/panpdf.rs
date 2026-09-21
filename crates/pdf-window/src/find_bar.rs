use eframe::egui;

use pdf_app::find::Found;
use pdf_app::wording::{Command, Message};

use crate::window_state::{Finding, Pointing, Replacing, Window};

const PAGES_AT_ONCE: usize = 24;

impl Window {
    pub(crate) fn open_the_find_bar(&mut self) {
        if !self.has_document() {
            return;
        }
        let finding = self.finding.get_or_insert_with(Finding::default);
        finding.take_the_keyboard = true;
    }

    pub(crate) fn close_the_find_bar(&mut self) {
        self.finding = None;
    }

    pub(crate) fn find_bar(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let Some(finding) = self.finding.as_mut() else {
            return;
        };
        let say = |message: Message| message.say(lang);
        let mut walk = None;
        let mut close = false;
        let mut show_replace = None;
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            let field = egui::TextEdit::singleline(&mut finding.needle)
                .hint_text(say(Message::Command(Command::Find)))
                .desired_width(220.0);
            let response = ui.add(field);
            if std::mem::take(&mut finding.take_the_keyboard) {
                response.request_focus();
            }
            if response.changed() {
                finding.searched = Found::default();
                finding.asked.clear();
            }
            if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                walk = Some(!ui.input(|input| input.modifiers.shift));
                finding.take_the_keyboard = true;
            }
            let any = finding.searched.count() > 0;
            if ui
                .add_enabled(any, egui::Button::new("◀"))
                .on_hover_text(say(Message::Command(Command::FindPrevious)))
                .clicked()
            {
                walk = Some(false);
            }
            if ui
                .add_enabled(any, egui::Button::new("▶"))
                .on_hover_text(say(Message::Command(Command::FindNext)))
                .clicked()
            {
                walk = Some(true);
            }
            ui.label(count(finding, self.editor.page_count(), lang));
            let replacing = finding.replacement.is_some();
            if ui
                .selectable_label(replacing, say(Message::Command(Command::Replace)))
                .clicked()
            {
                show_replace = Some(!replacing);
            }
            if ui.button("✕").on_hover_text(say(Message::Close)).clicked() {
                close = true;
            }
        });
        if close {
            self.close_the_find_bar();
            return;
        }
        if let Some(show) = show_replace
            && let Some(finding) = self.finding.as_mut()
        {
            finding.replacement = show.then(String::new);
            finding.replacing = None;
        }
        if let Some(forwards) = walk {
            self.go_to_a_hit(forwards);
        }
        self.replace_row(ui);
    }

    pub(crate) fn open_the_replace_row(&mut self) {
        self.open_the_find_bar();
        if let Some(finding) = self.finding.as_mut()
            && finding.replacement.is_none()
        {
            finding.replacement = Some(String::new());
        }
    }

    fn replace_row(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let say = |message: Message| message.say(lang);
        let Some(finding) = self.finding.as_ref() else {
            return;
        };
        if finding.replacement.is_none() {
            return;
        }
        let mut asked = None;
        let mut one = false;
        let found = finding.searched.count();
        let here = finding.searched.current().is_some();
        let asking = finding.replacing.clone();
        let pages_with_hits = self
            .finding
            .as_ref()
            .map_or(0, |finding| finding.searched.pages());
        let Some(finding) = self.finding.as_mut() else {
            return;
        };
        let Some(replacement) = finding.replacement.as_mut() else {
            return;
        };
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.add(
                egui::TextEdit::singleline(replacement)
                    .hint_text(say(Message::Command(Command::ReplaceWith)))
                    .desired_width(220.0),
            );
            match asking {
                Some(Replacing {
                    confirmed: false,
                    wanted,
                    ..
                }) => {
                    ui.label(say(Message::ReplaceAllQuestion {
                        count: wanted,
                        pages: pages_with_hits,
                    }));
                    if ui.button(say(Message::ReplaceAllYes)).clicked() {
                        asked = Some(true);
                    }
                    if ui
                        .button(Message::Home(pdf_app::wording::Home::Cancel).say(lang))
                        .clicked()
                    {
                        asked = Some(false);
                    }
                }
                Some(Replacing { done, wanted, .. }) => {
                    ui.label(say(Message::Replacing {
                        done,
                        count: wanted,
                    }));
                }
                None => {
                    if ui
                        .add_enabled(
                            here,
                            egui::Button::new(say(Message::Command(Command::ReplaceThisOne))),
                        )
                        .clicked()
                    {
                        one = true;
                    }
                    if ui
                        .add_enabled(
                            found > 0,
                            egui::Button::new(say(Message::Command(Command::ReplaceAll))),
                        )
                        .clicked()
                    {
                        asked = Some(true);
                    }
                }
            }
        });
        match asked {
            Some(true) => self.ask_to_replace_them_all(),
            Some(false) => {
                if let Some(finding) = self.finding.as_mut() {
                    finding.replacing = None;
                }
            }
            None => {}
        }
        if one {
            self.replace_the_hit_in_hand();
        }
    }

    fn replace_the_hit_in_hand(&mut self) -> bool {
        if self.editor.is_busy() {
            return false;
        }
        let Some(finding) = self.finding.as_ref() else {
            return false;
        };
        let Some(replacement) = finding.replacement.clone() else {
            return false;
        };
        let Some(hit) = finding.searched.current() else {
            return false;
        };
        let (page, from, to) = (hit.page, hit.from, hit.to);
        if self.editor.leaf(page).is_none() {
            return false;
        }
        let job = self.editor.begin_replace(page, from, to, &replacement);
        if job.is_none() {
            self.editor.say(Message::NotReplaced(
                Message::Command(Command::Replace).say(self.lang),
            ));
            return false;
        }
        self.send(job);
        true
    }

    fn ask_to_replace_them_all(&mut self) {
        let lang = self.lang;
        let Some(finding) = self.finding.as_mut() else {
            return;
        };
        let replacement = finding.replacement.clone().unwrap_or_default();
        if !finding.needle.is_empty()
            && replacement
                .to_lowercase()
                .contains(&finding.needle.to_lowercase())
        {
            finding.replacing = None;
            self.editor.say(Message::ReplacementHoldsTheSearch);
            let _ = lang;
            return;
        }
        match finding.replacing.as_mut() {
            Some(replacing) => replacing.confirmed = true,
            None => {
                finding.replacing = Some(Replacing {
                    wanted: finding.searched.count(),
                    ..Replacing::default()
                });
            }
        }
    }

    pub(crate) fn keep_replacing(&mut self) {
        let Some(finding) = self.finding.as_ref() else {
            return;
        };
        let Some(replacing) = finding.replacing.clone() else {
            return;
        };
        if !replacing.confirmed || self.editor.is_busy() {
            return;
        }
        let searched_all = finding.searched.pages_answered() >= self.editor.page_count();
        let done = replacing.done + replacing.refused;
        if finding.searched.count() == 0 && searched_all || done >= replacing.wanted {
            let said = Message::ReplacedAll {
                done: replacing.done,
                refused: replacing.refused,
            };
            self.editor.say(said);
            if let Some(finding) = self.finding.as_mut() {
                finding.replacing = None;
            }
            return;
        }
        if finding.searched.count() == 0 {
            return;
        }
        if self
            .finding
            .as_ref()
            .and_then(|finding| finding.searched.current())
            .is_none()
        {
            self.go_to_a_hit(true);
            return;
        }
        let replaced = self.replace_the_hit_in_hand();
        if let Some(finding) = self.finding.as_mut()
            && let Some(replacing) = finding.replacing.as_mut()
            && replaced
        {
            replacing.done += 1;
        }
    }

    pub(crate) fn keep_searching(&mut self) {
        let Some(finding) = self.finding.as_mut() else {
            return;
        };
        if finding.needle.is_empty() {
            return;
        }
        let (Some(source), pages) = (self.editor.source().cloned(), self.editor.page_count())
        else {
            return;
        };
        let epoch = self.editor.epoch();
        if finding.epoch != epoch {
            finding.epoch = epoch;
            let answered = finding.searched.clone();
            finding.asked.retain(|page| answered.has_answered(*page));
        }
        let needle = finding.needle.clone();
        let here = self.focus.min(pages.saturating_sub(1));
        let mut asked = 0;
        for page in outwards(here, pages) {
            if asked >= PAGES_AT_ONCE {
                break;
            }
            if !finding.asked.insert(page) {
                continue;
            }
            self.painter.search(
                page,
                source.clone(),
                self.editor.credential(),
                self.editor.fonts(),
                &needle,
                epoch,
            );
            asked += 1;
        }
    }

    pub(crate) fn forget_what_was_found_on(&mut self, page: usize) {
        if let Some(finding) = self.finding.as_mut() {
            finding.searched.forget(page);
            finding.asked.remove(&page);
        }
    }

    pub(crate) fn search_the_document_again(&mut self) {
        if let Some(finding) = self.finding.as_mut() {
            finding.searched = Found::default();
            finding.asked.clear();
        }
    }

    pub(crate) fn page_searched(
        &mut self,
        page: usize,
        needle: &str,
        hits: Vec<pdf_app::find::Hit>,
    ) {
        let Some(finding) = self.finding.as_mut() else {
            return;
        };
        if finding.needle != needle {
            return;
        }
        finding.searched.take(page, hits);
        if finding.searched.current().is_none() && finding.searched.count() > 0 {
            self.go_to_a_hit(true);
        }
    }

    pub(crate) fn go_to_a_hit(&mut self, forwards: bool) {
        let here = self.focus;
        let Some(finding) = self.finding.as_mut() else {
            return;
        };
        let Some(hit) = finding.searched.walk(forwards, here) else {
            return;
        };
        let (page, bounds) = (hit.page, hit.bounds());
        self.point_at(Pointing::Nothing);
        self.show_the_page(page, bounds);
    }

    fn show_the_page(&mut self, page: usize, bounds: Option<[f64; 4]>) {
        self.goto(page);
        let (Some((_, top)), Some([_, y0, _, y1])) = (self.editor.strip().origin(page), bounds)
        else {
            return;
        };
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a strip is well inside what an f32 counts exactly"
        )]
        let middle = ((top + f64::midpoint(y0, y1)) * self.zoom) as f32;
        let y = crate::canvas::DESK_MARGIN + middle - self.view.y / 2.0;
        self.wanted_offset = Some(egui::vec2(self.offset.x, y.max(0.0)));
    }

    pub(crate) fn hits_on(&self, page: usize) -> &[pdf_app::find::Hit] {
        self.finding
            .as_ref()
            .map_or(&[], |finding| finding.searched.on(page))
    }

    pub(crate) fn is_the_hit_in_hand(&self, page: usize, at: usize) -> bool {
        self.finding
            .as_ref()
            .is_some_and(|finding| finding.searched.is_current(page, at))
    }
}

fn count(finding: &Finding, pages: usize, lang: pdf_app::wording::Lang) -> String {
    let found = finding.searched.count();
    let searching = finding.searched.pages_answered() < pages;
    if finding.needle.is_empty() {
        return String::new();
    }
    if found == 0 {
        return Message::NothingFound { searching }.say(lang);
    }
    Message::HitOf {
        at: finding.searched.ordinal().unwrap_or(0),
        count: found,
        searching,
    }
    .say(lang)
}

fn outwards(here: usize, pages: usize) -> Vec<usize> {
    let mut order = Vec::with_capacity(pages);
    for step in 0..pages {
        if here + step < pages {
            order.push(here + step);
        }
        if step > 0 && here >= step {
            order.push(here - step);
        }
    }
    order
}

#[cfg(test)]
mod tests {
    use super::outwards;

    #[test]
    fn pages_are_searched_outwards_from_the_one_on_screen() {
        assert_eq!(outwards(3, 8), [3, 4, 2, 5, 1, 6, 0, 7]);
        assert_eq!(outwards(0, 4), [0, 1, 2, 3]);
        assert_eq!(outwards(3, 4), [3, 2, 1, 0]);
        assert_eq!(outwards(0, 0), Vec::<usize>::new());
    }

    #[test]
    fn every_page_is_asked_for_once() {
        for pages in 1..12_usize {
            for here in 0..pages {
                let order = outwards(here, pages);
                let mut sorted = order.clone();
                sorted.sort_unstable();
                sorted.dedup();
                assert_eq!(sorted.len(), pages, "from {here} of {pages}: {order:?}");
            }
        }
    }
}
