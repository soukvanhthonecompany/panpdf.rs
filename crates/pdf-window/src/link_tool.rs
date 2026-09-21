use eframe::egui;

use pdf_app::arrange::{Arrangement, arranged, shifted};
use pdf_app::document::{LinkBox, OVERLAY_SCALE};
use pdf_app::wording::{Control, Lang, Message};
use pdf_edit::destination::Spot;
use pdf_edit::link::{Arrival, Highlight, LinkBorder, Look, Target};
use pdf_syntax::Reference;

use crate::canvas::box_on_screen;
use crate::field_properties::beside_or_under;
use crate::format::{icon_button, rule};
use crate::icons::Icon;
use crate::window_state::{
    Carrying, ChosenLinks, Drag, Goes, Laid, LinkDraft, LinkTab, Tool, Window,
};

const HANDLE_REACH: f32 = 9.0;

const SMALLEST: f64 = 3.0;

const PANEL_WIDTH: f32 = 320.0;

fn contains([x0, y0, x1, y1]: [f64; 4], (x, y): (f64, f64)) -> bool {
    (x0..=x1).contains(&x) && (y0..=y1).contains(&y)
}

fn draft_of(page: usize, found: &LinkBox) -> LinkDraft {
    let plain = LinkDraft {
        page,
        link: Some(found.link),
        pixels: None,
        goes: Goes::APage,
        page_number: (page + 1).to_string(),
        address: String::new(),
        look: found.look,
        arrival: Arrival::InheritZoom,
        percent: percent_of(Arrival::InheritZoom),
        file: String::new(),
        name: String::new(),
    };
    match &found.target {
        Some(Target::Page(to, arrival)) => LinkDraft {
            page_number: (to + 1).to_string(),
            arrival: *arrival,
            percent: percent_of(*arrival),
            ..plain
        },
        Some(Target::Address(address)) => LinkDraft {
            goes: Goes::AnAddress,
            address: address.clone(),
            ..plain
        },
        Some(Target::Name(name)) => LinkDraft {
            goes: Goes::AName,
            name: name.clone(),
            ..plain
        },
        Some(Target::Document {
            file,
            page: to,
            arrival,
        }) => LinkDraft {
            goes: Goes::ADocument,
            file: file.clone(),
            page_number: (to + 1).to_string(),
            arrival: *arrival,
            percent: percent_of(*arrival),
            ..plain
        },
        None => plain,
    }
}

fn target_of(draft: &LinkDraft, pages: usize) -> Result<Target, Message> {
    if draft.goes == Goes::ADocument {
        let file = draft.file.trim();
        if file.is_empty() || file.chars().any(char::is_control) {
            return Err(Message::LinkNeedsAFile);
        }
        let number: usize = draft
            .page_number
            .trim()
            .parse()
            .map_err(|_| Message::LinkNeedsAPageNumber)?;
        if number == 0 {
            return Err(Message::LinkNeedsAPageNumber);
        }
        return Ok(Target::Document {
            file: file.to_owned(),
            page: number - 1,
            arrival: arrival_asked(draft)?,
        });
    }
    if draft.goes == Goes::AName {
        let name = draft.name.trim();
        if name.is_empty() {
            return Err(Message::LinkNeedsAName);
        }
        return Ok(Target::Name(name.to_owned()));
    }
    if draft.goes == Goes::APage {
        let number: usize = draft
            .page_number
            .trim()
            .parse()
            .map_err(|_| Message::LinkNeedsAPageOfThis)?;
        if number == 0 || number > pages {
            return Err(Message::LinkNeedsAPageOfThis);
        }
        return Ok(Target::Page(number - 1, arrival_asked(draft)?));
    }
    let address = draft.address.trim();
    if !pdf_app::links::is_openable(address) {
        return Err(Message::LinkNeedsAWebAddress);
    }
    Ok(Target::Address(address.to_owned()))
}

impl Window {
    pub(crate) fn take_up_the_link_tool(&mut self) {
        self.finish_the_field();
        self.pictures.clear();
        self.ink = None;
        self.tool = Tool::Link;
        self.chosen_links = None;
        self.link_draft = None;
        self.editor.say(Message::DragOutALink);
    }

    pub(crate) fn links_as_the_link_tool_sees_them(&mut self, glass: &egui::Painter, laid: Laid) {
        let green = egui::Color32::from_rgb(20, 130, 90);
        let orange = egui::Color32::from_rgb(230, 120, 0);
        self.box_being_asked_about(glass, laid);
        let chosen = self.chosen_on(laid.page);
        let only = chosen.len() == 1;
        let pages = self.editor.page_count();
        for found in self.editor.links_on(laid.page).iter() {
            let area = box_on_screen(laid.placed, found.pixels);
            let picked = chosen.contains(&found.link);
            let colour = if picked { orange } else { green };
            glass.rect_filled(area, 1.0, colour.gamma_multiply(0.10));
            glass.rect_stroke(
                area,
                1.0,
                egui::Stroke::new(if picked { 2.0 } else { 1.0 }, colour),
                egui::StrokeKind::Outside,
            );
            if picked && only {
                let handle =
                    egui::Rect::from_center_size(area.right_bottom(), egui::vec2(8.0, 8.0));
                glass.rect_filled(handle, 1.0, egui::Color32::WHITE);
                glass.rect_stroke(
                    handle,
                    1.0,
                    egui::Stroke::new(1.5, orange),
                    egui::StrokeKind::Inside,
                );
            }
            glass.text(
                area.left_top() + egui::vec2(0.0, -2.0),
                egui::Align2::LEFT_BOTTOM,
                says_where(found, pages, self.lang),
                egui::FontId::proportional(11.0),
                colour,
            );
        }
    }

    fn box_being_asked_about(&self, glass: &egui::Painter, laid: Laid) {
        let Some(draft) = self.link_draft.as_ref() else {
            return;
        };
        let (Some(pixels), true) = (draft.pixels, draft.page == laid.page) else {
            return;
        };
        let green = egui::Color32::from_rgb(20, 130, 90);
        let area = box_on_screen(laid.placed, pixels);
        glass.rect_filled(area, 1.0, green.gamma_multiply(0.10));
        for (from, to) in [
            (area.left_top(), area.right_top()),
            (area.right_top(), area.right_bottom()),
            (area.right_bottom(), area.left_bottom()),
            (area.left_bottom(), area.left_top()),
        ] {
            glass.add(egui::Shape::dashed_line(
                &[from, to],
                egui::Stroke::new(1.5, green),
                4.0,
                3.0,
            ));
        }
    }

    pub(crate) fn links_drawn_on_the_page(&mut self, glass: &egui::Painter, laid: Laid) {
        let stretch = laid.placed.stretch;
        for found in self.editor.links_on(laid.page).iter() {
            if found.look.width <= 0.0 {
                continue;
            }
            #[expect(
                clippy::cast_possible_truncation,
                reason = "a border width in points, at the scale the page is drawn"
            )]
            let width = (found.look.width as f32 * stretch).max(0.5);
            let colour = drawn_colour(found.look.colour);
            let area = box_on_screen(laid.placed, found.pixels);
            let stroke = egui::Stroke::new(width, colour);
            match found.look.style {
                LinkBorder::Solid => {
                    glass.rect_stroke(area, 0.0, stroke, egui::StrokeKind::Inside);
                }
                LinkBorder::Dashed => {
                    for (from, to) in [
                        (area.left_top(), area.right_top()),
                        (area.right_top(), area.right_bottom()),
                        (area.right_bottom(), area.left_bottom()),
                        (area.left_bottom(), area.left_top()),
                    ] {
                        glass.add(egui::Shape::dashed_line(
                            &[from, to],
                            stroke,
                            DASH * stretch,
                            DASH * stretch,
                        ));
                    }
                }
                LinkBorder::Underline => {
                    glass.line_segment([area.left_bottom(), area.right_bottom()], stroke);
                }
            }
        }
    }

    pub(crate) fn link_tool_takes_hold(
        &mut self,
        page: usize,
        point: (f64, f64),
        adding: bool,
    ) -> Carrying {
        let stretch = self
            .laid
            .iter()
            .find(|laid| laid.page == page)
            .map_or(1.0, |laid| laid.placed.stretch);
        let reach = f64::from(HANDLE_REACH / stretch.max(0.01));
        let links = self.editor.links_on(page);
        let chosen = self.chosen_on(page);
        if let [only] = chosen.as_slice()
            && let Some(found) = links.iter().find(|found| found.link == *only)
        {
            let [_, _, x1, y1] = found.pixels;
            if (point.0 - x1).abs() <= reach && (point.1 - y1).abs() <= reach {
                return Carrying::MovingLinks {
                    links: vec![(found.link, found.pixels)],
                    resize: true,
                };
            }
        }
        let under = links
            .iter()
            .rev()
            .find(|found| contains(found.pixels, point))
            .cloned();
        let Some(found) = under else {
            return Carrying::NewLink;
        };
        if !chosen.contains(&found.link) {
            self.choose_link(page, &found, adding);
        }
        Carrying::MovingLinks {
            links: self.chosen_link_boxes(page),
            resize: false,
        }
    }

    fn chosen_on(&self, page: usize) -> Vec<Reference> {
        self.chosen_links
            .as_ref()
            .filter(|chosen| chosen.page == page)
            .map(|chosen| chosen.links.clone())
            .unwrap_or_default()
    }

    fn chosen_link_boxes(&mut self, page: usize) -> Vec<(Reference, [f64; 4])> {
        let chosen = self.chosen_on(page);
        let links = self.editor.links_on(page);
        chosen
            .iter()
            .filter_map(|link| {
                links
                    .iter()
                    .find(|found| found.link == *link)
                    .map(|found| (found.link, found.pixels))
            })
            .collect()
    }

    pub(crate) fn link_tool_clicked(&mut self, page: usize, point: (f64, f64), adding: bool) {
        let under = self
            .editor
            .links_on(page)
            .iter()
            .rev()
            .find(|found| contains(found.pixels, point))
            .cloned();
        if let Some(found) = under {
            self.choose_link(page, &found, adding);
        } else if !adding {
            self.chosen_links = None;
            self.link_draft = None;
        }
    }

    fn choose_link(&mut self, page: usize, found: &LinkBox, adding: bool) {
        match self.chosen_links.as_mut() {
            Some(chosen) if adding && chosen.page == page => {
                if let Some(at) = chosen.links.iter().position(|held| *held == found.link) {
                    chosen.links.remove(at);
                } else {
                    chosen.links.push(found.link);
                }
                if chosen.links.is_empty() {
                    self.chosen_links = None;
                }
            }
            _ => {
                self.chosen_links = Some(ChosenLinks {
                    page,
                    links: vec![found.link],
                });
            }
        }
        let one = self.chosen_on(page) == vec![found.link];
        let already = self
            .link_draft
            .as_ref()
            .is_some_and(|draft| draft.page == page && draft.link == Some(found.link));
        if one && !already {
            self.link_draft = Some(draft_of(page, found));
        } else if !one {
            self.link_draft = None;
        }
    }

    pub(crate) fn show_the_link_box(&self, painter: &egui::Painter, drag: &Drag) {
        let (Some(pixels), Some(laid)) = (
            self.box_dragged_out(drag),
            self.laid
                .iter()
                .copied()
                .find(|laid| laid.page == drag.page),
        ) else {
            return;
        };
        let area = box_on_screen(laid.placed, pixels);
        let green = egui::Color32::from_rgb(20, 130, 90);
        painter.rect_filled(area, 1.0, green.gamma_multiply(0.12));
        painter.rect_stroke(
            area,
            1.0,
            egui::Stroke::new(1.5, green),
            egui::StrokeKind::Inside,
        );
    }

    pub(crate) fn take_the_link_drawn(&mut self, drag: &Drag) {
        let Some(pixels) = self.box_dragged_out(drag) else {
            self.chosen_links = None;
            self.link_draft = None;
            return;
        };
        self.chosen_links = None;
        self.link_draft = Some(LinkDraft {
            page: drag.page,
            link: None,
            pixels: Some(pixels),
            goes: Goes::APage,
            page_number: (drag.page + 1).to_string(),
            address: String::new(),
            look: pdf_edit::link::Look::default(),
            arrival: Arrival::InheritZoom,
            percent: percent_of(Arrival::InheritZoom),
            file: String::new(),
            name: String::new(),
        });
    }

    fn links_moved_to(&self, drag: &Drag) -> Option<Vec<(Reference, [f64; 4])>> {
        let Carrying::MovingLinks { links, resize } = &drag.what else {
            return None;
        };
        let laid = self.laid.iter().find(|laid| laid.page == drag.page)?;
        let stretch = f64::from(laid.placed.stretch);
        if stretch <= 0.0 {
            return None;
        }
        let dx = f64::from(drag.to.x - drag.from.x) / stretch;
        let dy = f64::from(drag.to.y - drag.from.y) / stretch;
        Some(
            links
                .iter()
                .map(|(link, [x0, y0, x1, y1])| {
                    let to = if *resize {
                        [
                            *x0,
                            *y0,
                            (x1 + dx).max(x0 + SMALLEST),
                            (y1 + dy).max(y0 + SMALLEST),
                        ]
                    } else {
                        shifted([*x0, *y0, *x1, *y1], (dx, dy))
                    };
                    (*link, to)
                })
                .collect(),
        )
    }

    pub(crate) fn show_the_link_moved(&self, painter: &egui::Painter, drag: &Drag) {
        let (Some(moved), Some(laid)) = (
            self.links_moved_to(drag),
            self.laid
                .iter()
                .copied()
                .find(|laid| laid.page == drag.page),
        ) else {
            return;
        };
        let orange = egui::Color32::from_rgb(230, 120, 0);
        for (_, pixels) in moved {
            let area = box_on_screen(laid.placed, pixels);
            painter.rect_filled(area, 1.0, orange.gamma_multiply(0.15));
            painter.rect_stroke(
                area,
                1.0,
                egui::Stroke::new(1.5, orange),
                egui::StrokeKind::Inside,
            );
        }
    }

    pub(crate) fn take_the_link_moved(&mut self, drag: &Drag) {
        let Some(moved) = self.links_moved_to(drag) else {
            return;
        };
        let travel = drag.to - drag.from;
        if travel.x.abs().max(travel.y.abs()) < 1.0 {
            return;
        }
        self.set_the_link_boxes(drag.page, &moved);
    }

    fn set_the_link_boxes(&mut self, page: usize, boxes: &[(Reference, [f64; 4])]) {
        if boxes.is_empty() {
            return;
        }
        let job = if let [(link, rect)] = boxes {
            self.editor.begin_set_link_box(page, *link, *rect)
        } else {
            self.editor.begin_set_link_boxes(page, boxes)
        };
        if job.is_none() {
            self.editor.say(Message::AnotherEditIsRunning);
            return;
        }
        self.send(job);
    }

    pub(crate) fn remove_the_chosen_link(&mut self) -> bool {
        let Some(chosen) = self.chosen_links.clone() else {
            return false;
        };
        let job = if let [only] = chosen.links.as_slice() {
            self.editor.begin_remove_link(chosen.page, *only)
        } else {
            self.editor.begin_remove_links(chosen.page, &chosen.links)
        };
        if job.is_none() {
            self.editor.say(Message::AnotherEditIsRunning);
            return true;
        }
        self.chosen_links = None;
        self.link_draft = None;
        self.send(job);
        true
    }

    pub(crate) fn settle_the_landing_link(&mut self) {
        let Some((page, pixels)) = self.landing_link else {
            return;
        };
        if self.editor.is_busy() || self.editor.leaf(page).is_none() {
            return;
        }
        let close = |one: [f64; 4]| {
            one.iter()
                .zip(pixels)
                .all(|(edge, wanted)| (edge - wanted).abs() < 2.0)
        };
        if let Some(found) = self
            .editor
            .links_on(page)
            .iter()
            .find(|found| close(found.pixels))
        {
            self.chosen_links = Some(ChosenLinks {
                page,
                links: vec![found.link],
            });
            self.link_draft = Some(draft_of(page, found));
        }
        self.landing_link = None;
    }

    pub(crate) fn link_tool_choices(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let page = self.focus;
        let ready = !self.editor.is_busy() && self.editor.leaf(page).is_some();
        let hover = format!(
            "{}\n{}",
            Message::LinkTheAddressesWritten.say(lang),
            Message::LinkTheAddressesWrittenWhy.say(lang)
        );
        if icon_button(ui, Icon::LinkAddresses, &hover, false, ready).clicked() {
            self.link_the_addresses_written(page);
        }
        let hover = format!(
            "{}\n{}",
            Message::NamedPlaces.say(lang),
            Message::NamedPlacesWhy.say(lang)
        );
        if icon_button(
            ui,
            Icon::NamedPlaces,
            &hover,
            self.naming_draft.is_some(),
            true,
        )
        .clicked()
        {
            self.toggle_the_named_places();
        }
        let count = self
            .chosen_links
            .as_ref()
            .map_or(0, |chosen| chosen.links.len());
        if count == 0 {
            return;
        }
        rule(ui);
        ui.label(Message::LinksChosen(count).say(lang));
        let asked = crate::form_tool::arrange_menu(ui, lang, count);
        if icon_button(
            ui,
            Icon::Unlink,
            &Message::TakeTheLinkOff.say(lang),
            false,
            true,
        )
        .clicked()
        {
            self.remove_the_chosen_link();
            return;
        }
        if let Some(arrangement) = asked {
            self.arrange_the_chosen_links(arrangement);
        }
    }

    pub(crate) fn link_tool_hint(&self) -> Option<String> {
        self.chosen_links
            .as_ref()
            .is_some_and(|chosen| !chosen.links.is_empty())
            .then(|| Message::Control(Control::LinkChosen).say(self.lang))
    }

    fn arrange_the_chosen_links(&mut self, arrangement: Arrangement) {
        let Some(chosen) = self.chosen_links.clone() else {
            return;
        };
        let boxes = self.chosen_link_boxes(chosen.page);
        if boxes.len() < arrangement.needs() {
            return;
        }
        let Some((width, height)) = self.editor.page_pixels(chosen.page, OVERLAY_SCALE) else {
            return;
        };
        let pixels: Vec<[f64; 4]> = boxes.iter().map(|(_, pixels)| *pixels).collect();
        let moved = arranged(
            &pixels,
            pixels.len().saturating_sub(1),
            arrangement,
            (f64::from(width), f64::from(height)),
        );
        let changed: Vec<(Reference, [f64; 4])> = boxes
            .iter()
            .zip(moved)
            .filter(|((_, pixels), to)| {
                pixels
                    .iter()
                    .zip(to)
                    .any(|(before, after)| (before - after).abs() > f64::EPSILON)
            })
            .map(|((link, _), to)| (*link, to))
            .collect();
        if changed.is_empty() {
            self.editor.say(Message::LinkIsAlreadyThat);
            return;
        }
        self.set_the_link_boxes(chosen.page, &changed);
    }

    fn link_the_addresses_written(&mut self, page: usize) {
        let Some(leaf) = self.editor.leaf(page) else {
            return;
        };
        let written = pdf_app::addresses::found_in(&leaf.overlay.clusters);
        let already: Vec<[f64; 4]> = self
            .editor
            .links_on(page)
            .iter()
            .map(|found| found.pixels)
            .collect();
        let wanted: Vec<([f64; 4], Target, Look)> = written
            .into_iter()
            .filter(|found| !already.iter().any(|link| covers(*link, found.pixels)))
            .map(|found| {
                (
                    grown(found.pixels),
                    Target::Address(found.address),
                    Look::default(),
                )
            })
            .collect();
        if wanted.is_empty() {
            self.editor.say(Message::NoAddressesToLink);
            return;
        }
        let job = self.editor.begin_add_links(page, &wanted);
        if job.is_none() {
            self.editor.say(Message::AnotherEditIsRunning);
            return;
        }
        self.send(job);
    }

    pub(crate) fn link_tool_keys(&mut self, ctx: &egui::Context) {
        let (deleted, escaped) = ctx.input(|input| {
            (
                input.key_pressed(egui::Key::Delete) || input.key_pressed(egui::Key::Backspace),
                input.key_pressed(egui::Key::Escape),
            )
        });
        if deleted && !self.editor.is_busy() {
            self.remove_the_chosen_link();
            return;
        }
        if !escaped || ctx.any_popup_open() {
            return;
        }
        if self.chosen_links.is_some() || self.link_draft.is_some() {
            self.chosen_links = None;
            self.link_draft = None;
            return;
        }
        self.tool = Tool::Select;
    }

    pub(crate) fn link_panel(&mut self, ctx: &egui::Context) {
        if self.tool != Tool::Link {
            self.link_draft = None;
            return;
        }
        self.settle_the_landing_link();
        let Some(draft) = self.link_draft.clone() else {
            return;
        };
        let Some(laid) = self
            .laid
            .iter()
            .copied()
            .find(|laid| laid.page == draft.page)
        else {
            return;
        };
        let about = draft.pixels.or_else(|| {
            let link = draft.link?;
            self.editor
                .links_on(draft.page)
                .iter()
                .find(|found| found.link == link)
                .map(|found| found.pixels)
        });
        let Some(about) = about else {
            return;
        };
        let lang = self.lang;
        let pages = self.editor.page_count();
        let known = self.editor.named_places();
        let placed = kept_on_screen(
            beside_or_under(box_on_screen(laid.placed, about), ctx.content_rect()),
            ctx.content_rect(),
        );
        let Some(draft) = self.link_draft.as_mut() else {
            return;
        };
        let making = draft.link.is_none();
        let tab = self.link_tab;
        let (apply_word, close_word) = (
            if making {
                Message::PutTheLinkOn.say(lang)
            } else {
                Message::Apply.say(lang)
            },
            Message::Close.say(lang),
        );
        let answered = ask_where_it_goes(
            ctx,
            draft,
            &known,
            Asking {
                placed,
                pages,
                making,
                lang,
                apply_word,
                close_word,
                tab,
            },
        );
        let (apply, remove, close) = (answered.apply, answered.remove, answered.close);
        self.link_tab = answered.tab;
        if remove {
            self.remove_the_chosen_link();
            return;
        }
        if close {
            self.link_draft = None;
            self.chosen_links = None;
            return;
        }
        if apply {
            self.write_the_link(pages);
        }
    }

    fn write_the_link(&mut self, pages: usize) {
        let Some(draft) = self.link_draft.clone() else {
            return;
        };
        let target = match target_of(&draft, pages) {
            Ok(target) => target,
            Err(why) => {
                self.editor.say(why);
                return;
            }
        };
        let job = match (draft.link, draft.pixels) {
            (Some(link), _) => {
                let was = self
                    .editor
                    .links_on(draft.page)
                    .iter()
                    .find(|found| found.link == link)
                    .cloned();
                let sent =
                    was.as_ref().and_then(|found| found.target.clone()) != Some(target.clone());
                let drawn = was.as_ref().map(|found| found.look) != Some(draft.look);
                if !sent && !drawn {
                    self.editor.say(Message::LinkIsAlreadyThat);
                    return;
                }
                self.editor.begin_set_link_properties(
                    draft.page,
                    link,
                    (sent.then_some(target), drawn.then_some(draft.look)),
                )
            }
            (None, Some(pixels)) => {
                self.landing_link = Some((draft.page, pixels));
                self.editor
                    .begin_add_link(draft.page, pixels, (target, draft.look))
            }
            (None, None) => return,
        };
        if job.is_none() {
            self.editor.say(Message::AnotherEditIsRunning);
            return;
        }
        if draft.link.is_none() {
            self.link_draft = None;
        }
        self.send(job);
    }
}

struct Asking {
    placed: egui::Pos2,
    pages: usize,
    making: bool,
    lang: pdf_app::wording::Lang,
    apply_word: String,
    close_word: String,
    tab: LinkTab,
}

#[derive(Clone, Copy, Debug, Default)]
struct Answered {
    apply: bool,
    remove: bool,
    close: bool,
    tab: LinkTab,
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "a position on screen, rounded to a whole pixel to name the window"
)]
fn ask_where_it_goes(
    ctx: &egui::Context,
    draft: &mut LinkDraft,
    names: &[Spot],
    asking: Asking,
) -> Answered {
    let Asking {
        placed,
        pages,
        making,
        lang,
        apply_word,
        close_word,
        tab: _,
    } = asking;
    let mut answered = Answered {
        tab: asking.tab,
        ..Answered::default()
    };
    egui::Window::new(Message::LinkProperties.say(lang))
        .id(egui::Id::new((
            "link target",
            placed.x as i32,
            placed.y as i32,
        )))
        .collapsible(false)
        .resizable(false)
        .default_pos(placed)
        .fixed_size(egui::vec2(PANEL_WIDTH, 0.0))
        .constrain(false)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                for (choice, word) in [
                    (LinkTab::Goes, Message::LinkGoesTo),
                    (LinkTab::Appearance, Message::FieldAppearance),
                ] {
                    ui.selectable_value(&mut answered.tab, choice, word.say(lang));
                }
            });
            ui.separator();
            match answered.tab {
                LinkTab::Goes => where_it_goes(ui, draft, (pages, names), lang),
                LinkTab::Appearance => how_it_is_drawn(ui, &mut draft.look, lang),
            }
            ui.separator();
            ui.horizontal(|ui| {
                answered.apply = ui.button(apply_word).clicked();
                if !making {
                    answered.remove = ui.button(Message::TakeTheLinkOff.say(lang)).clicked();
                }
                answered.close = ui.button(close_word).clicked();
            });
        });
    answered
}

fn where_it_goes(
    ui: &mut egui::Ui,
    draft: &mut LinkDraft,
    (pages, names): (usize, &[Spot]),
    lang: Lang,
) {
    ui.radio_value(
        &mut draft.goes,
        Goes::APage,
        Message::LinkToAPageOfThis.say(lang),
    );
    let numbered = matches!(draft.goes, Goes::APage | Goes::ADocument);
    ui.horizontal(|ui| {
        ui.add_enabled_ui(numbered, |ui| {
            ui.label(Message::LinkPageNumber.say(lang));
            ui.add(egui::TextEdit::singleline(&mut draft.page_number).desired_width(60.0));
            if draft.goes == Goes::APage {
                ui.label(format!("/ {pages}"));
            }
        });
    });
    ui.horizontal(|ui| {
        ui.add_enabled_ui(numbered, |ui| {
            ui.label(Message::LinkZoom.say(lang));
            egui::ComboBox::from_id_salt("link zoom")
                .selected_text(zoom_word(draft.arrival).say(lang))
                .show_ui(ui, |ui| {
                    for choice in [
                        Arrival::InheritZoom,
                        Arrival::FitPage,
                        Arrival::FitWidth,
                        Arrival::FitHeight,
                        Arrival::FitVisible,
                        Arrival::ActualSize,
                        Arrival::Percent(DEFAULT_PERCENT),
                    ] {
                        let chosen = same_choice(draft.arrival, choice);
                        if ui
                            .selectable_label(chosen, zoom_word(choice).say(lang))
                            .clicked()
                        {
                            draft.arrival = choice;
                            draft.percent = percent_of(choice);
                        }
                    }
                });
            if matches!(draft.arrival, Arrival::Percent(_)) {
                ui.add(egui::TextEdit::singleline(&mut draft.percent).desired_width(48.0));
                ui.label("%");
            }
        });
    });
    somewhere_else(ui, draft, names, lang);
}

fn somewhere_else(ui: &mut egui::Ui, draft: &mut LinkDraft, names: &[Spot], lang: Lang) {
    ui.radio_value(
        &mut draft.goes,
        Goes::AnAddress,
        Message::LinkToAWebAddress.say(lang),
    );
    ui.add_enabled_ui(draft.goes == Goes::AnAddress, |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut draft.address)
                .hint_text("https://example.org")
                .desired_width(PANEL_WIDTH - 24.0),
        );
    });
    ui.radio_value(
        &mut draft.goes,
        Goes::AName,
        Message::LinkToANamedPlace.say(lang),
    );
    ui.add_enabled_ui(draft.goes == Goes::AName, |ui| {
        if names.is_empty() {
            ui.label(Message::NoNamedPlacesYet.say(lang));
        } else {
            egui::ComboBox::from_id_salt("link named place")
                .selected_text(if draft.name.is_empty() {
                    Message::ChooseANamedPlace.say(lang)
                } else {
                    shortened(&draft.name)
                })
                .width(PANEL_WIDTH - 24.0)
                .show_ui(ui, |ui| {
                    for place in names {
                        let chosen = draft.name == place.name;
                        let said = format!(
                            "{}  —  {} {}",
                            place.name,
                            Message::LinkPageNumber.say(lang),
                            place.page + 1
                        );
                        if ui.selectable_label(chosen, said).clicked() {
                            draft.name.clone_from(&place.name);
                        }
                    }
                });
        }
    });
    ui.radio_value(
        &mut draft.goes,
        Goes::ADocument,
        Message::LinkToAnotherDocument.say(lang),
    );
    ui.add_enabled_ui(draft.goes == Goes::ADocument, |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut draft.file)
                .hint_text(Message::LinkFileHint.say(lang))
                .desired_width(PANEL_WIDTH - 24.0),
        );
    });
}

fn how_it_is_drawn(ui: &mut egui::Ui, look: &mut Look, lang: Lang) {
    let mut visible = look.width > 0.0;
    ui.horizontal(|ui| {
        if ui
            .radio_value(&mut visible, false, Message::LinkInvisible.say(lang))
            .clicked()
        {
            look.width = 0.0;
        }
        if ui
            .radio_value(&mut visible, true, Message::LinkVisible.say(lang))
            .clicked()
            && look.width <= 0.0
        {
            look.width = THICKNESSES[0].0;
        }
    });
    ui.add_enabled_ui(visible, |ui| {
        ui.horizontal(|ui| {
            ui.label(Message::FieldLineThickness.say(lang));
            for (width, word) in THICKNESSES {
                let chosen = (look.width - width).abs() < 0.01;
                if ui.selectable_label(chosen, word.say(lang)).clicked() {
                    look.width = width;
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label(Message::FieldLineStyle.say(lang));
            for (style, word) in BORDERS {
                if ui
                    .selectable_label(look.style == style, word.say(lang))
                    .clicked()
                {
                    look.style = style;
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label(Message::FieldBorderColour.say(lang));
            let mut colour = colour_of(*look);
            if ui.color_edit_button_rgb(&mut colour).changed() {
                look.colour = Some(colour.map(|part| (f64::from(part) * 1000.0).round() / 1000.0));
            }
        });
    });
    ui.horizontal(|ui| {
        ui.label(Message::LinkHighlight.say(lang));
        egui::ComboBox::from_id_salt("link highlight")
            .selected_text(highlight_word(look.highlight).say(lang))
            .show_ui(ui, |ui| {
                for choice in [
                    Highlight::None,
                    Highlight::Invert,
                    Highlight::Outline,
                    Highlight::Inset,
                ] {
                    ui.selectable_value(
                        &mut look.highlight,
                        choice,
                        highlight_word(choice).say(lang),
                    );
                }
            });
    });
}

fn kept_on_screen(placed: egui::Pos2, screen: egui::Rect) -> egui::Pos2 {
    let x = placed
        .x
        .min(screen.right() - PANEL_WIDTH - MARGIN)
        .max(screen.left() + MARGIN);
    let y = placed
        .y
        .min(screen.bottom() - PANEL_HEIGHT - MARGIN)
        .max(screen.top() + MARGIN);
    egui::pos2(x, y)
}

const PANEL_HEIGHT: f32 = 350.0;

const MARGIN: f32 = 8.0;

fn arrival_asked(draft: &LinkDraft) -> Result<Arrival, Message> {
    let Arrival::Percent(_) = draft.arrival else {
        return Ok(draft.arrival);
    };
    let percent: f64 = draft
        .percent
        .trim()
        .trim_end_matches('%')
        .trim()
        .parse()
        .map_err(|_| Message::LinkNeedsAZoom)?;
    if !percent.is_finite() || !(1.0..=6400.0).contains(&percent) {
        return Err(Message::LinkNeedsAZoom);
    }
    Ok(Arrival::Percent(percent))
}

pub(crate) const fn same_choice(one: Arrival, other: Arrival) -> bool {
    matches!(
        (one, other),
        (Arrival::Percent(_), Arrival::Percent(_))
            | (Arrival::InheritZoom, Arrival::InheritZoom)
            | (Arrival::FitPage, Arrival::FitPage)
            | (Arrival::FitWidth, Arrival::FitWidth)
            | (Arrival::FitHeight, Arrival::FitHeight)
            | (Arrival::FitVisible, Arrival::FitVisible)
            | (Arrival::ActualSize, Arrival::ActualSize)
    )
}

pub(crate) const fn zoom_word(arrival: Arrival) -> Message {
    match arrival {
        Arrival::InheritZoom => Message::LinkInheritZoom,
        Arrival::FitPage => Message::LinkFitPage,
        Arrival::FitWidth => Message::LinkFitWidth,
        Arrival::FitHeight => Message::LinkFitHeight,
        Arrival::FitVisible => Message::LinkFitVisible,
        Arrival::ActualSize => Message::LinkActualSize,
        Arrival::Percent(_) => Message::LinkZoomTo,
    }
}

pub(crate) fn percent_of(arrival: Arrival) -> String {
    match arrival {
        Arrival::Percent(percent) => format!("{percent:.1}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned(),
        _ => DEFAULT_PERCENT.to_string(),
    }
}

pub(crate) const DEFAULT_PERCENT: f64 = 100.0;

fn covers(link: [f64; 4], box_of: [f64; 4]) -> bool {
    let middle = (
        f64::midpoint(box_of[0], box_of[2]),
        f64::midpoint(box_of[1], box_of[3]),
    );
    (link[0]..=link[2]).contains(&middle.0) && (link[1]..=link[3]).contains(&middle.1)
}

fn grown([x0, y0, x1, y1]: [f64; 4]) -> [f64; 4] {
    [x0 - ROOM, y0 - ROOM, x1 + ROOM, y1 + ROOM]
}

const ROOM: f64 = 1.0;

fn drawn_colour(colour: Option<[f64; 3]>) -> egui::Color32 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a colour component from zero to one, put on a scale of 255"
    )]
    match colour {
        Some(rgb) => {
            let [red, green, blue] = rgb.map(|part| (part.clamp(0.0, 1.0) * 255.0).round() as u8);
            egui::Color32::from_rgb(red, green, blue)
        }
        None => egui::Color32::BLACK,
    }
}

const DASH: f32 = 3.0;

fn colour_of(look: Look) -> [f32; 3] {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a colour component from zero to one"
    )]
    look.colour
        .map_or([0.0, 0.0, 0.0], |colour| colour.map(|part| part as f32))
}

const fn highlight_word(highlight: Highlight) -> Message {
    match highlight {
        Highlight::None => Message::FieldNone,
        Highlight::Invert => Message::LinkHighlightInvert,
        Highlight::Outline => Message::LinkHighlightOutline,
        Highlight::Inset => Message::FieldInset,
    }
}

const THICKNESSES: [(f64, Message); 3] = [
    (1.0, Message::FieldThin),
    (2.0, Message::FieldMedium),
    (3.0, Message::FieldThick),
];

const BORDERS: [(LinkBorder, Message); 3] = [
    (LinkBorder::Solid, Message::FieldSolid),
    (LinkBorder::Dashed, Message::FieldDashed),
    (LinkBorder::Underline, Message::Underline),
];

fn says_where(found: &LinkBox, pages: usize, lang: pdf_app::wording::Lang) -> String {
    match &found.target {
        Some(Target::Document { file, page, .. }) => {
            format!("{file} {} {}", Message::LinkPageNumber.say(lang), page + 1)
        }
        Some(Target::Page(to, _)) if *to < pages => {
            format!("{} {}", Message::LinkPageNumber.say(lang), to + 1)
        }
        Some(Target::Page(..)) => Message::LinkNeedsAPageOfThis.say(lang),
        Some(Target::Address(address)) => shortened(address),
        Some(Target::Name(name)) => shortened(name),
        None => Message::LinkPointsNowhere.say(lang),
    }
}

fn shortened(address: &str) -> String {
    let mut short: String = address.chars().take(40).collect();
    if address.chars().count() > 40 {
        short.push('…');
    }
    short
}

#[cfg(test)]
mod tests {
    use super::{draft_of, shortened, target_of};
    use crate::window_state::{Goes, LinkDraft};
    use eframe::egui;
    use pdf_app::document::LinkBox;
    use pdf_edit::link::{Arrival, Look, Target};
    use pdf_syntax::Reference;

    fn a_link(target: Option<Target>) -> LinkBox {
        LinkBox {
            link: Reference::new(9, 0),
            pixels: [10.0, 10.0, 90.0, 30.0],
            target,
            look: Look::default(),
        }
    }

    fn drafted(goes: Goes, number: &str, said: &str) -> LinkDraft {
        LinkDraft {
            page: 0,
            link: None,
            pixels: Some([0.0, 0.0, 10.0, 10.0]),
            goes,
            page_number: number.to_owned(),
            address: said.to_owned(),
            look: Look::default(),
            arrival: Arrival::InheritZoom,
            percent: "100".to_owned(),
            file: said.to_owned(),
            name: said.to_owned(),
        }
    }

    #[test]
    fn a_link_to_a_named_place_asks_for_the_name() {
        assert_eq!(
            target_of(&drafted(Goes::AName, "1", "chapter two"), 5),
            Ok(Target::Name("chapter two".to_owned()))
        );
        assert!(target_of(&drafted(Goes::AName, "1", "  "), 5).is_err());
    }

    #[test]
    fn the_panel_opens_on_the_name_a_link_says() {
        let draft = draft_of(2, &a_link(Some(Target::Name("appendix".to_owned()))));
        assert_eq!(draft.goes, Goes::AName);
        assert_eq!(draft.name, "appendix");
    }

    #[test]
    fn a_page_is_one_based_in_the_panel_and_zero_based_in_the_document() {
        assert_eq!(
            target_of(&drafted(Goes::APage, "3", ""), 5),
            Ok(Target::Page(2, Arrival::InheritZoom)),
            "page 3 of five is the third"
        );
        assert!(
            target_of(&drafted(Goes::APage, "0", ""), 5).is_err(),
            "no page 0"
        );
        assert!(
            target_of(&drafted(Goes::APage, "6", ""), 5).is_err(),
            "past the end"
        );
        assert!(
            target_of(&drafted(Goes::APage, "", ""), 5).is_err(),
            "nothing typed"
        );
        assert!(
            target_of(&drafted(Goes::APage, "two", ""), 5).is_err(),
            "not a number"
        );
    }

    #[test]
    fn only_addresses_this_window_would_follow_are_written() {
        assert_eq!(
            target_of(
                &drafted(Goes::AnAddress, "1", "  https://example.org/a  "),
                5
            ),
            Ok(Target::Address("https://example.org/a".to_owned())),
            "the spaces around it are not part of it"
        );
        assert_eq!(
            target_of(
                &drafted(Goes::AnAddress, "1", "mailto:someone@example.org"),
                5
            ),
            Ok(Target::Address("mailto:someone@example.org".to_owned()))
        );
        for refused in [
            "",
            "example.org",
            "file:///etc/passwd",
            "javascript:alert(1)",
        ] {
            assert!(
                target_of(&drafted(Goes::AnAddress, "1", refused), 5).is_err(),
                "this should not be written: {refused}"
            );
        }
    }

    #[test]
    fn the_panel_opens_on_what_the_link_says() {
        let page = draft_of(1, &a_link(Some(Target::Page(4, Arrival::InheritZoom))));
        assert_eq!(page.goes, Goes::APage);
        assert_eq!(page.page_number, "5");
        assert_eq!(
            target_of(&page, 9),
            Ok(Target::Page(4, Arrival::InheritZoom))
        );
        let address = draft_of(
            1,
            &a_link(Some(Target::Address("https://a.example/".to_owned()))),
        );
        assert_eq!(address.goes, Goes::AnAddress);
        assert_eq!(
            target_of(&address, 9),
            Ok(Target::Address("https://a.example/".to_owned()))
        );
        let other = draft_of(1, &a_link(None));
        assert_eq!(other.goes, Goes::APage);
        assert_eq!(other.page_number, "2");
    }

    #[test]
    fn a_link_to_another_document_needs_a_file_and_a_page() {
        assert_eq!(
            target_of(&drafted(Goes::ADocument, "12", " handbook.pdf "), 5),
            Ok(Target::Document {
                file: "handbook.pdf".to_owned(),
                page: 11,
                arrival: Arrival::InheritZoom,
            }),
            "page 12 of that document, whatever this one has"
        );
        assert!(
            target_of(&drafted(Goes::ADocument, "12", "  "), 5).is_err(),
            "a link to nothing is not a link"
        );
        assert!(
            target_of(&drafted(Goes::ADocument, "0", "a.pdf"), 5).is_err(),
            "there is no page zero"
        );
        let panel = draft_of(
            1,
            &a_link(Some(Target::Document {
                file: "a.pdf".to_owned(),
                page: 3,
                arrival: Arrival::FitPage,
            })),
        );
        assert_eq!(panel.goes, Goes::ADocument);
        assert_eq!(panel.file, "a.pdf");
        assert_eq!(panel.page_number, "4");
        assert_eq!(panel.arrival, Arrival::FitPage);
    }

    #[test]
    fn the_panel_is_kept_on_the_screen() {
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1400.0, 950.0));
        let low = super::kept_on_screen(egui::pos2(1300.0, 900.0), screen);
        assert!(low.x + super::PANEL_WIDTH <= screen.right(), "{low:?}");
        assert!(low.y + super::PANEL_HEIGHT <= screen.bottom(), "{low:?}");
        let fits = egui::pos2(300.0, 200.0);
        assert_eq!(super::kept_on_screen(fits, screen), fits);
    }

    #[test]
    fn a_long_address_is_cut_with_an_ellipsis() {
        assert_eq!(shortened("https://example.org/"), "https://example.org/");
        let long = shortened(&format!("https://example.org/{}", "a".repeat(60)));
        assert_eq!(long.chars().count(), 41);
        assert!(long.ends_with('…'));
    }
}
