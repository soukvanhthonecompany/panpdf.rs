use std::sync::Arc;

use eframe::egui;

use pdf_app::tiles::TileId;
use pdf_app::wording::{Command, Message};

use crate::icons::Icon;
use crate::page_motion::{self, Grid, PanelShape};
use crate::room;
use crate::window_state::{PageDrag, Window};

pub(crate) const THUMB_RUNG: usize = usize::MAX;

const READING_AT_ONCE: usize = 2;

const EDGE: f32 = 40.0;

const CARD_AIR: f32 = 8.0;

const SPLITTER: f32 = 8.0;

const SPLITTER_ID: &str = "pages-splitter-handle";

const PANEL_MARGIN: egui::Vec2 = egui::vec2(8.0, 2.0);

const SHADOW_ROOM: f32 = 24.0;
const SHADOW_DEPTH: u8 = 18;

const BAR_BUTTON: f32 = 24.0;
const BAR_HEIGHT: f32 = 30.0;

const SEAM_REACH: f32 = 18.0;

const SEAM_RADIUS: f32 = 10.0;

const END_ROOM: f32 = page_motion::ROW_GAP / 2.0 + SEAM_RADIUS + 2.0;

#[derive(Clone)]
pub(crate) struct Thumb {
    pub(crate) texture: egui::TextureHandle,
    pub(crate) fresh: bool,
    pub(crate) width: f32,
    pub(crate) turn: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PanelMenu {
    Seam(usize),
    Page(usize),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Slot {
    Page(usize),
    Hole,
}

struct Laid<'a> {
    origin: egui::Pos2,
    now: f64,
    targets: &'a [egui::Rect],
    columns: usize,
    panel: egui::Rect,
    floating: bool,
}

impl Window {
    pub(crate) fn page_panel(&mut self, ui: &mut egui::Ui) {
        let window = ui.ctx().content_rect().width();
        let now = ui.input(|input| input.time);
        self.fold_the_pages_on_a_narrow_window(window, now);
        let held = ui.ctx().is_being_dragged(egui::Id::new(SPLITTER_ID));
        let settled = room::panel_now(self.pages_folded, held, self.pages_width, window);
        let (width, flowing) = self.page_motion.panel_at(settled, now);
        if flowing {
            ui.ctx().request_repaint();
        }
        if width < room::PANEL_GONE {
            self.folded_handle(ui, window, now);
            return;
        }
        if let Some(meter) = self.meter.as_mut() {
            meter.note(&format!(
                "panel width {width:.2} settled {settled:.2} held {held} flowing {flowing}"
            ));
        }
        let stage = room::panel_layout(width, window);
        let working = !self.editor.is_busy() && self.loading.is_none() && self.has_document();
        let mut visible = Vec::new();
        let reserved = egui::Panel::left("pages")
            .resizable(false)
            .exact_size(stage.reserved)
            .show(ui, |ui| {
                if !stage.floating {
                    let panel = ui.max_rect();
                    visible = self.panel_body(ui, working, panel);
                }
            })
            .response
            .rect;
        let panel =
            egui::Rect::from_min_size(reserved.min, egui::vec2(stage.width, reserved.height()));
        if stage.floating {
            visible = self.floating_pages(ui.ctx(), panel, working);
        }
        self.panel_splitter(ui.ctx(), panel, window, now);
        self.forget_far_away_pictures(&visible);
        if let Some(meter) = self.meter.as_mut() {
            meter.count("cards", visible.len());
            meter.count("pictures", self.thumbs.len());
        }
        self.thumbs_wanted = if held { Vec::new() } else { visible };
        self.panel_menu_popup(ui.ctx(), working);
        self.take_in_what_arrived();
    }

    fn panel_body(&mut self, ui: &mut egui::Ui, working: bool, panel: egui::Rect) -> Vec<usize> {
        let mut visible = Vec::new();
        self.page_panel_header(ui, working);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                visible = self.page_cards(ui, working, panel);
            });
        visible
    }

    fn floating_pages(
        &mut self,
        ctx: &egui::Context,
        panel: egui::Rect,
        working: bool,
    ) -> Vec<usize> {
        let mut visible = Vec::new();
        egui::Area::new(egui::Id::new("pages-grid"))
            .order(egui::Order::Middle)
            .fixed_pos(panel.min)
            .show(ctx, |ui| {
                ui.set_clip_rect(panel.expand(SHADOW_ROOM));
                let fill = ui.visuals().panel_fill;
                let edge = ui.visuals().widgets.noninteractive.bg_stroke.color;
                let painter = ui.painter();
                painter.add(lift(panel, SHADOW_DEPTH));
                painter.rect_filled(panel, 0.0, fill);
                painter.vline(panel.right(), panel.y_range(), egui::Stroke::new(1.0, edge));
                let inner = panel.shrink2(PANEL_MARGIN);
                ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
                    ui.set_clip_rect(panel);
                    visible = self.panel_body(ui, working, panel);
                });
            });
        visible
    }

    fn panel_splitter(&mut self, ctx: &egui::Context, panel: egui::Rect, window: f32, now: f64) {
        let strip = egui::Rect::from_min_max(
            egui::pos2(panel.right() - SPLITTER / 2.0, panel.top()),
            egui::pos2(panel.right() + SPLITTER / 2.0, panel.bottom()),
        );
        let mut let_go = None;
        egui::Area::new(egui::Id::new("pages-splitter"))
            .order(egui::Order::Foreground)
            .fixed_pos(strip.min)
            .show(ctx, |ui| {
                let handle = ui.interact(
                    strip,
                    egui::Id::new(SPLITTER_ID),
                    egui::Sense::click_and_drag(),
                );
                let live = handle.hovered() || handle.dragged();
                if live {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                }
                let colour = if live {
                    ui.visuals().selection.stroke.color
                } else {
                    ui.visuals().widgets.noninteractive.bg_stroke.color
                };
                ui.painter().vline(
                    panel.right(),
                    panel.y_range(),
                    egui::Stroke::new(if live { 2.0 } else { 1.0 }, colour),
                );
                if handle.dragged()
                    && let Some(at) = ui.ctx().pointer_interact_pos()
                {
                    let raw = (at.x - panel.left())
                        .clamp(room::PANEL_FOLD_FLOOR, room::panel_widest(window));
                    let want = match room::dragged_to(raw, window) {
                        room::Dragged::Wide(width) => width,
                        room::Dragged::Fold => raw,
                    };
                    if (want - raw).abs() <= room::SAME_WIDTH {
                        self.page_motion.settle_panel();
                    } else if (self.pages_width - want).abs() > room::SAME_WIDTH {
                        self.page_motion.flow_panel(self.pages_width, want, now);
                    }
                    self.pages_width = want;
                }
                if handle.drag_stopped() {
                    let_go = Some(room::dragged_to(self.pages_width, window));
                }
            });
        match let_go {
            Some(room::Dragged::Fold) => {
                self.pages_width = room::PANEL_WIDTH;
                self.fold_the_pages(true, now);
            }
            Some(room::Dragged::Wide(width)) => {
                let from = self.pages_width;
                self.pages_width = width;
                if (from - width).abs() > room::SAME_WIDTH {
                    self.page_motion.flow_panel(from, width, now);
                }
                self.remember_the_view();
            }
            None => {}
        }
    }

    fn folded_handle(&mut self, ui: &egui::Ui, window: f32, now: f64) {
        let space = ui.max_rect();
        let tall = room::handle_tall(space.height());
        if tall <= 0.0 {
            return;
        }
        let strip = egui::Rect::from_min_size(
            egui::pos2(space.left(), space.center().y - tall / 2.0),
            egui::vec2(room::HANDLE_WIDE, tall),
        );
        let ctx = ui.ctx().clone();
        egui::Area::new(egui::Id::new("pages-handle"))
            .order(egui::Order::Foreground)
            .fixed_pos(strip.min)
            .show(&ctx, |ui| {
                let handle = ui.interact(
                    strip,
                    egui::Id::new(SPLITTER_ID),
                    egui::Sense::click_and_drag(),
                );
                let live = handle.hovered() || handle.dragged();
                if live {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                }
                let visuals = ui.visuals();
                let ink = if live {
                    visuals.selection.stroke.color
                } else {
                    visuals.weak_text_color()
                };
                let fill = if live {
                    visuals.widgets.hovered.bg_fill
                } else {
                    visuals.widgets.inactive.bg_fill
                };
                let painter = ui.painter();
                painter.rect_filled(strip, 4.0, fill);
                painter.rect_stroke(
                    strip,
                    4.0,
                    egui::Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color),
                    egui::StrokeKind::Inside,
                );
                let middle = strip.center();
                let arm = egui::Stroke::new(1.6, ink);
                painter.line_segment(
                    [
                        egui::pos2(middle.x - 2.0, middle.y - 4.0),
                        egui::pos2(middle.x + 2.0, middle.y),
                    ],
                    arm,
                );
                painter.line_segment(
                    [
                        egui::pos2(middle.x + 2.0, middle.y),
                        egui::pos2(middle.x - 2.0, middle.y + 4.0),
                    ],
                    arm,
                );
                if handle.clicked() {
                    self.fold_the_pages(false, now);
                }
                if handle.dragged()
                    && let Some(at) = ui.ctx().pointer_interact_pos()
                    && let room::Pulled::Open(width) =
                        room::handle_pulled_to(at.x - space.left(), window)
                {
                    self.pages_width = width;
                    self.pages_folded = false;
                    self.page_motion.settle_panel();
                }
            });
    }

    pub(crate) fn fold_the_pages(&mut self, folded: bool, now: f64) {
        if self.pages_folded == folded {
            return;
        }
        self.pages_folded = folded;
        let open = room::panel_width(self.pages_window_width, self.pages_width);
        let (from, to) = if folded { (open, 0.0) } else { (0.0, open) };
        self.page_motion.flow_panel(from, to, now);
        self.remember_the_view();
    }

    fn back_to_the_sidebar(&mut self, now: f64) {
        let window = self.pages_window_width;
        let room::FoldPress::BackToSidebar(width) = room::fold_press(self.pages_width, window)
        else {
            return;
        };
        let from = room::panel_width(window, self.pages_width);
        self.pages_width = width;
        self.page_motion.flow_panel(from, width, now);
        self.remember_the_view();
    }

    fn fold_the_pages_on_a_narrow_window(&mut self, window: f32, now: f64) {
        let was = self.pages_window_width;
        self.pages_window_width = window;
        if window > 0.0 && was >= room::PANEL_FOLDS_BELOW && room::panel_starts_folded(window) {
            self.fold_the_pages(true, now);
        }
    }

    fn page_panel_header(&mut self, ui: &mut egui::Ui, working: bool) {
        let lang = self.lang;
        let now = ui.input(|input| input.time);
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let press = room::fold_press(self.pages_width, self.pages_window_width);
            let (icon, said) = match press {
                room::FoldPress::BackToSidebar(_) => (Icon::Previous, Command::BackToReading),
                room::FoldPress::FoldAway => (Icon::Close, Command::HidePages),
            };
            let fold = crate::format::icon_button(
                ui,
                icon,
                &Message::Command(said).say(lang),
                false,
                true,
            );
            if fold.clicked() {
                match press {
                    room::FoldPress::BackToSidebar(_) => self.back_to_the_sidebar(now),
                    room::FoldPress::FoldAway => self.fold_the_pages(true, now),
                }
            }
            ui.label(
                egui::RichText::new(Message::Command(Command::Pages).say(lang))
                    .size(11.0)
                    .color(ui.visuals().weak_text_color()),
            );
            let chosen = self.chosen_pages.len();
            if chosen > 1 {
                ui.label(
                    egui::RichText::new(Message::PagesChosen(chosen).say(lang))
                        .size(11.0)
                        .color(ui.visuals().selection.stroke.color),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let add = crate::format::icon_button(
                    ui,
                    Icon::Plus,
                    &Message::Command(Command::AddPages).say(lang),
                    false,
                    working,
                );
                egui::Popup::menu(&add).show(|ui| {
                    ui.set_min_width(220.0);
                    self.pages_in_menu(ui);
                });
            });
        });
    }

    fn page_cards(&mut self, ui: &mut egui::Ui, working: bool, panel: egui::Rect) -> Vec<usize> {
        let count = self.editor.page_count();
        let now = ui.input(|input| input.time);
        let grid = Grid::for_width(ui.available_width() - CARD_AIR);
        let glide = room::pictures_glide(ui.ctx().is_being_dragged(egui::Id::new(SPLITTER_ID)));
        self.thumb_width = grid.thumb;
        let slots = self.slots(count);
        let hole_page = self
            .page_drag
            .as_ref()
            .and_then(|drag| drag.pages.first().copied())
            .unwrap_or(self.focus);
        let heights: Vec<f32> = slots
            .iter()
            .map(|slot| {
                let page = match slot {
                    Slot::Page(page) => *page,
                    Slot::Hole => hole_page,
                };
                grid.thumb * self.tallness(page)
            })
            .collect();
        let (places, total) = page_motion::lay_out(&grid, &heights);
        if let Some(meter) = self.meter.as_mut() {
            let first = places.first().map_or(0.0, |place| place.min.x);
            meter.note(&format!(
                "grid columns {} cell {:.2} thumb {:.2} first {first:.2}",
                grid.columns, grid.cell, grid.thumb
            ));
        }
        let (area, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), total + END_ROOM * 2.0),
            egui::Sense::hover(),
        );
        let origin = area.min + egui::vec2((area.width() - grid.width()) / 2.0, END_ROOM);
        let mut visible = Vec::new();
        let mut targets = Vec::with_capacity(count);
        let mut cards = Vec::with_capacity(count);
        let mut moving = false;
        for (slot, place) in slots.iter().zip(&places) {
            let target = place.translate(origin.to_vec2());
            let Slot::Page(page) = *slot else {
                paint_hole(ui, target);
                continue;
            };
            targets.push(target);
            let (at, on_its_way) = if glide {
                self.page_motion.at(page, place.min.to_vec2(), now)
            } else {
                self.page_motion
                    .set_off_from(page, place.min.to_vec2(), now);
                (place.min.to_vec2(), false)
            };
            moving |= on_its_way;
            let shown = egui::Rect::from_min_size(origin + at, place.size());
            if !ui.is_rect_visible(shown.expand2(egui::vec2(4.0, page_motion::LABEL))) {
                continue;
            }
            visible.push(page);
            self.paint_card(ui, shown, page);
            let response = ui.interact(
                shown.expand(4.0),
                egui::Id::new(("page-card", page)),
                egui::Sense::click_and_drag(),
            );
            cards.push((page, shown, response));
        }
        if moving {
            ui.ctx().request_repaint();
        }
        if let Some(meter) = self.meter.as_mut() {
            let where_each = cards
                .iter()
                .map(|(page, shown, _)| {
                    format!(
                        "{page}:{:.2},{:.2},{:.2}",
                        shown.min.x,
                        shown.min.y,
                        shown.width()
                    )
                })
                .collect::<Vec<_>>()
                .join(" ");
            meter.note(&format!("cards moving {moving} {where_each}"));
        }
        let laid = Laid {
            origin,
            now,
            targets: &targets,
            columns: grid.columns,
            panel,
            floating: room::panel_layout(panel.width(), self.pages_window_width).floating,
        };
        self.handle_cards(ui, &cards, working, &laid);
        self.page_panel_shape = Some(PanelShape {
            rect: panel,
            pictures: targets,
            columns: grid.columns,
        });
        visible
    }

    fn slots(&self, count: usize) -> Vec<Slot> {
        if let Some(drag) = &self.page_drag {
            let mut rest: Vec<Slot> = (0..count)
                .filter(|page| !drag.pages.contains(page))
                .map(Slot::Page)
                .collect();
            let home = drag.pages.first().map_or(0, |first| {
                (0..*first)
                    .filter(|page| !drag.pages.contains(page))
                    .count()
            });
            let gap = drag.gap.unwrap_or(home).min(rest.len());
            rest.insert(gap, Slot::Hole);
            return rest;
        }
        if let Some(order) = &self.page_preview
            && order.len() == count
        {
            return order.iter().copied().map(Slot::Page).collect();
        }
        let mut all: Vec<Slot> = (0..count).map(Slot::Page).collect();
        if let Some(gap) = self.file_hover_gap {
            all.insert(gap.min(count), Slot::Hole);
        }
        all
    }

    fn tallness(&self, page: usize) -> f32 {
        let (wide, high) = self
            .editor
            .strip()
            .page_size(page)
            .unwrap_or((612.0, 792.0));
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a page's proportion, well inside f32"
        )]
        let tall = (high / wide.max(1.0)) as f32;
        tall
    }

    fn paint_card(&self, ui: &egui::Ui, rect: egui::Rect, page: usize) {
        let visuals = ui.visuals();
        let painter = ui.painter();
        let accent = visuals.selection.stroke.color;
        let current = page == self.focus;
        let chosen = self.chosen_pages.contains(&page);
        let hovered = self.page_drag.is_none() && ui.rect_contains_pointer(rect.expand(4.0));
        if chosen {
            painter.rect_filled(
                rect.expand(5.0),
                4.0,
                visuals.selection.bg_fill.gamma_multiply(0.35),
            );
        }
        painter.add(lift(rect, if hovered { 10 } else { 4 }));
        paint_picture(painter, rect, self.thumbs.get(&page));
        painter.rect_stroke(
            rect,
            1.0,
            egui::Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color),
            egui::StrokeKind::Outside,
        );
        if current || chosen {
            painter.rect_stroke(
                rect.expand(4.0),
                4.0,
                egui::Stroke::new(if current { 2.0 } else { 1.5 }, accent),
                egui::StrokeKind::Outside,
            );
        } else if hovered {
            painter.rect_stroke(
                rect.expand(4.0),
                4.0,
                egui::Stroke::new(1.0, visuals.widgets.hovered.bg_stroke.color),
                egui::StrokeKind::Outside,
            );
        }
        painter.text(
            egui::pos2(rect.center().x, rect.bottom() + 5.0),
            egui::Align2::CENTER_TOP,
            (page + 1).to_string(),
            egui::FontId::proportional(11.0),
            if current {
                accent
            } else {
                visuals.weak_text_color()
            },
        );
    }

    fn handle_cards(
        &mut self,
        ui: &mut egui::Ui,
        cards: &[(usize, egui::Rect, egui::Response)],
        working: bool,
        laid: &Laid<'_>,
    ) {
        let modifiers = ui.input(|input| input.modifiers);
        for (page, rect, card) in cards {
            if card.clicked() {
                self.choose_page(*page, modifiers);
                if laid.floating && !modifiers.command && !modifiers.shift {
                    self.back_to_the_sidebar(laid.now);
                }
            }
            if card.secondary_clicked() && !self.chosen_pages.contains(page) {
                self.chosen_pages = std::collections::BTreeSet::from([*page]);
            }
            card.context_menu(|ui| self.page_menu(ui, working));
            if card.drag_started() && working && self.page_drag.is_none() {
                let pages = if self.chosen_pages.contains(page) {
                    self.pages_acted_on()
                } else {
                    vec![*page]
                };
                let grab = card
                    .interact_pointer_pos()
                    .map_or(egui::Vec2::ZERO, |at| at - rect.min);
                self.panel_menu = None;
                self.page_drag = Some(PageDrag {
                    pages,
                    gap: None,
                    grab,
                });
            }
        }
        if self.page_drag.is_some() {
            self.carry_pages(ui, laid);
            return;
        }
        if working && self.panel_menu.is_none() {
            self.page_bar(ui, cards);
            self.seam_button(ui, laid);
        }
    }

    fn carry_pages(&mut self, ui: &mut egui::Ui, laid: &Laid<'_>) {
        let Some(mut drag) = self.page_drag.take() else {
            return;
        };
        let pointer = ui.input(|input| input.pointer.latest_pos());
        let over = pointer.filter(|at| laid.panel.expand(24.0).x_range().contains(at.x));
        if let Some(at) = over {
            drag.gap = Some(page_motion::gap_at(laid.columns, laid.targets, at));
        }
        if let Some(at) = pointer {
            let clip = ui.clip_rect();
            let speed = 8.0;
            if at.y < clip.top() + EDGE {
                ui.scroll_with_delta(egui::vec2(0.0, speed));
            } else if at.y > clip.bottom() - EDGE {
                ui.scroll_with_delta(egui::vec2(0.0, -speed));
            }
            ui.ctx().request_repaint();
        }
        let first = drag.pages.first().copied().unwrap_or(0);
        let size = egui::vec2(self.thumb_width, self.thumb_width * self.tallness(first));
        let ghost = pointer.map(|at| egui::Rect::from_min_size(at - drag.grab, size));
        if let Some(ghost) = ghost {
            self.paint_ghost(ui.ctx(), ghost, &drag.pages);
        }
        let held = ui.input(|input| input.pointer.primary_down());
        if held {
            self.page_drag = Some(drag);
            return;
        }
        if let Some(ghost) = ghost {
            let from = ghost.min - laid.origin;
            for page in &drag.pages {
                self.page_motion.set_off_from(*page, from, laid.now);
            }
        }
        let (Some(_), Some(gap)) = (over, drag.gap) else {
            return;
        };
        let rest = self.editor.page_count() - drag.pages.len();
        let to = gap.min(rest);
        let already = drag
            .pages
            .iter()
            .enumerate()
            .all(|(offset, page)| *page == to + offset);
        if !already {
            self.move_pages(&drag.pages, to);
        }
    }

    fn paint_ghost(&self, ctx: &egui::Context, ghost: egui::Rect, pages: &[usize]) {
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::new("page-ghost"),
        ));
        let visuals = ctx.global_style().visuals.clone();
        if pages.len() > 1 {
            let behind = ghost.translate(egui::vec2(5.0, 5.0));
            painter.rect_filled(behind, 1.0, egui::Color32::WHITE);
            painter.rect_stroke(
                behind,
                1.0,
                egui::Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color),
                egui::StrokeKind::Outside,
            );
        }
        painter.add(lift(ghost, 18));
        paint_picture(
            &painter,
            ghost,
            pages.first().and_then(|page| self.thumbs.get(page)),
        );
        painter.rect_stroke(
            ghost.expand(3.0),
            4.0,
            egui::Stroke::new(2.0, visuals.selection.stroke.color),
            egui::StrokeKind::Outside,
        );
        if pages.len() > 1 {
            let badge = egui::Rect::from_center_size(ghost.right_top(), egui::vec2(24.0, 24.0));
            painter.circle_filled(badge.center(), 12.0, visuals.selection.stroke.color);
            painter.text(
                badge.center(),
                egui::Align2::CENTER_CENTER,
                pages.len().to_string(),
                egui::FontId::proportional(12.0),
                egui::Color32::WHITE,
            );
        }
    }

    fn page_bar(&mut self, ui: &mut egui::Ui, cards: &[(usize, egui::Rect, egui::Response)]) {
        let Some((page, rect)) = cards
            .iter()
            .find(|(_, rect, _)| ui.rect_contains_pointer(rect.expand(4.0)))
            .map(|(page, rect, _)| (*page, *rect))
        else {
            return;
        };
        let lang = self.lang;
        let say = |command| Message::Command(command).say(lang);
        let count = self.editor.page_count();
        let acting = if self.chosen_pages.contains(&page) {
            self.pages_acted_on()
        } else {
            vec![page]
        };
        let buttons = [
            (Icon::RotateLeft, say(Command::RotateCounterClockwise), true),
            (Icon::RotateRight, say(Command::RotateClockwise), true),
            (Icon::Delete, say(Command::DeletePage), acting.len() < count),
            (Icon::More, say(Command::MoreForPage), true),
        ];
        #[expect(clippy::cast_precision_loss, reason = "four buttons")]
        let width = buttons.len() as f32 * BAR_BUTTON + 10.0;
        let bar = egui::Rect::from_center_size(
            egui::pos2(rect.center().x, rect.bottom() - BAR_HEIGHT / 2.0 - 6.0),
            egui::vec2(width, BAR_HEIGHT),
        );
        let visuals = ui.visuals().clone();
        let painter = ui.painter().clone();
        painter.add(lift(bar, 8));
        painter.rect_filled(bar, BAR_HEIGHT / 2.0, visuals.window_fill);
        painter.rect_stroke(
            bar,
            BAR_HEIGHT / 2.0,
            egui::Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color),
            egui::StrokeKind::Inside,
        );
        let mut pressed = None;
        for (index, (icon, tip, enabled)) in buttons.iter().enumerate() {
            #[expect(clippy::cast_precision_loss, reason = "four buttons")]
            let left = bar.left() + 5.0 + index as f32 * BAR_BUTTON;
            let spot = egui::Rect::from_min_size(
                egui::pos2(left, bar.center().y - BAR_BUTTON / 2.0),
                egui::vec2(BAR_BUTTON, BAR_BUTTON),
            );
            let sense = if *enabled {
                egui::Sense::click()
            } else {
                egui::Sense::hover()
            };
            let response = ui
                .interact(spot, egui::Id::new(("page-bar", page, index)), sense)
                .on_hover_text(tip.trim_end_matches('\u{2026}'));
            if *enabled && response.hovered() {
                painter.circle_filled(
                    spot.center(),
                    BAR_BUTTON / 2.0,
                    visuals.widgets.hovered.weak_bg_fill,
                );
            }
            let colour = if *enabled {
                visuals.text_color()
            } else {
                visuals.weak_text_color()
            };
            icon.draw_tinted(
                &painter,
                spot.shrink(5.0),
                colour,
                *enabled && response.hovered(),
            );
            if response.clicked() {
                pressed = Some((index, spot));
            }
        }
        match pressed {
            Some((0, _)) => self.rotate_pages(&acting, -1),
            Some((1, _)) => self.rotate_pages(&acting, 1),
            Some((2, _)) => self.remove_pages(&acting),
            Some((_, spot)) => {
                if !self.chosen_pages.contains(&page) {
                    self.chosen_pages = std::collections::BTreeSet::from([page]);
                }
                self.panel_menu = Some((PanelMenu::Page(page), spot.left_bottom(), self.frame));
            }
            None => {}
        }
    }

    fn seam_button(&mut self, ui: &mut egui::Ui, laid: &Laid<'_>) {
        let Some(pointer) = ui.input(|input| input.pointer.hover_pos()) else {
            return;
        };
        if !laid.panel.contains(pointer)
            || !ui.clip_rect().contains(pointer)
            || laid
                .targets
                .iter()
                .any(|picture| picture.expand(4.0).contains(pointer))
        {
            return;
        }
        let half = self.thumb_width / 2.0;
        let near = page_motion::seams(laid.columns, laid.targets)
            .into_iter()
            .enumerate()
            .map(|(gap, seam)| {
                let (along, across) = if laid.columns <= 1 {
                    ((pointer.x - seam.x).abs(), (pointer.y - seam.y).abs())
                } else {
                    ((pointer.y - seam.y).abs(), (pointer.x - seam.x).abs())
                };
                (
                    gap,
                    seam,
                    if along <= half { across } else { f32::INFINITY },
                )
            })
            .filter(|(_, _, distance)| *distance < SEAM_REACH)
            .min_by(|one, other| one.2.total_cmp(&other.2));
        let Some((gap, seam, _)) = near else {
            return;
        };
        let visuals = ui.visuals().clone();
        let accent = visuals.selection.stroke.color;
        let button = egui::Rect::from_center_size(seam, egui::Vec2::splat(SEAM_RADIUS * 2.0));
        let response = ui
            .interact(button, egui::Id::new("page-seam"), egui::Sense::click())
            .on_hover_text(Message::Command(Command::InsertHere).say(self.lang));
        let pen = ui.painter();
        let line = if laid.columns <= 1 {
            [
                egui::pos2(seam.x - half, seam.y),
                egui::pos2(seam.x + half, seam.y),
            ]
        } else {
            [
                egui::pos2(seam.x, seam.y - half),
                egui::pos2(seam.x, seam.y + half),
            ]
        };
        pen.line_segment(line, egui::Stroke::new(2.0, accent.gamma_multiply(0.6)));
        pen.circle_filled(seam, SEAM_RADIUS, accent);
        Icon::Plus.draw(pen, button.shrink(4.0), egui::Color32::WHITE);
        if response.clicked() {
            self.panel_menu = Some((PanelMenu::Seam(gap), button.right_bottom(), self.frame));
        }
    }

    fn panel_menu_popup(&mut self, ctx: &egui::Context, working: bool) {
        let Some((menu, at, opened)) = self.panel_menu else {
            return;
        };
        let lang = self.lang;
        let say = |command| Message::Command(command).say(lang);
        let mut done = false;
        let shown = egui::Area::new(egui::Id::new("page-panel-menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(at)
            .show(ctx, |ui| {
                egui::Frame::menu(ui.style()).show(ui, |ui| {
                    ui.set_min_width(200.0);
                    ui.style_mut().visuals.button_frame = false;
                    match menu {
                        PanelMenu::Page(_) => {
                            self.page_menu(ui, working);
                        }
                        PanelMenu::Seam(gap) => {
                            ui.menu_button(say(Command::InsertBlankPage), |ui| {
                                if let Some(size) = self.page_size_menu(ui) {
                                    let before = self.aim_at_gap(gap);
                                    self.add_blank_page(before, size);
                                    done = true;
                                }
                            });
                            if ui.button(say(Command::InsertPagesFromPdf)).clicked() {
                                let before = self.aim_at_gap(gap);
                                self.choosing_for = crate::page_actions::Choosing::Pages { before };
                                self.asking_to_open = true;
                                done = true;
                            }
                            if ui.button(say(Command::InsertPicturesAsPages)).clicked() {
                                let before = self.aim_at_gap(gap);
                                self.choose_pictures(Some(before));
                                done = true;
                            }
                        }
                    }
                });
            });
        let escape = ctx.input(|input| input.key_pressed(egui::Key::Escape));
        let elsewhere = opened != self.frame && shown.response.clicked_elsewhere();
        if done || escape || elsewhere || shown.response.should_close() || self.editor.is_busy() {
            self.panel_menu = None;
        }
    }

    fn take_in_what_arrived(&mut self) {
        if self.arriving.is_empty()
            || self.editor.is_busy()
            || self.making.is_some()
            || self.loading.is_some()
            || self.chooser.is_some()
        {
            return;
        }
        let Some((next, gap)) = self.arriving.pop_front() else {
            return;
        };
        let before = self.aim_at_gap(gap);
        let added = match next {
            page_motion::Arriving::Pages(path) => self.insert_pages_from(&path, before),
            page_motion::Arriving::Pictures(paths) => {
                self.pictures_chosen(&paths, Some(before));
                paths.len()
            }
        };
        for (_, later) in &mut self.arriving {
            *later += added;
        }
    }

    fn forget_far_away_pictures(&mut self, shown: &[usize]) {
        let span = shown.iter().copied().min().zip(shown.iter().copied().max());
        let Some(kept) = room::thumbs_kept(span) else {
            return;
        };
        self.thumbs.retain(|page, _| kept.contains(page));
    }

    pub(crate) fn ask_for_thumbnails(&mut self, pixels_per_point: f32) {
        let epoch = self.editor.epoch();
        let mut reading = 0;
        let wanted = std::mem::take(&mut self.thumbs_wanted);
        for page in wanted.iter().copied() {
            if self
                .thumbs
                .get(&page)
                .is_some_and(|thumb| thumb.fresh && thumb.width >= self.thumb_width * 0.9)
                || self.failed.contains_key(&page)
            {
                continue;
            }
            let id = TileId {
                page,
                zoom: THUMB_RUNG,
                col: 0,
                row: 0,
            };
            if self.painter.drawing(id, epoch) {
                continue;
            }
            if let Some(leaf) = self.editor.leaf(page) {
                let Some((wide, _)) = self.editor.page_pixels(page, 1.0) else {
                    continue;
                };
                let scale = f64::from(self.thumb_width * pixels_per_point) / f64::from(wide.max(1));
                let Some((width, height)) = self.editor.page_pixels(page, scale) else {
                    continue;
                };
                let view = Arc::clone(&leaf.view);
                self.painter
                    .draw(id, view, scale, [0, 0, width, height], epoch);
            } else if self.painter.reading(page, epoch) {
                reading += 1;
            } else if reading < READING_AT_ONCE
                && let Some(source) = self.editor.source().cloned()
            {
                let credential = self.editor.credential().to_vec();
                let grouping = self.editor.grouping(page);
                self.painter.read(
                    page,
                    source,
                    &credential,
                    grouping,
                    self.editor.fonts(),
                    epoch,
                );
                reading += 1;
            }
        }
        self.thumbs_wanted = wanted;
    }

    pub(crate) fn thumbnail_arrived(
        &mut self,
        ctx: &egui::Context,
        page: usize,
        pixels: &pdf_app::painter::TilePixels,
    ) {
        let size = [pixels.width as usize, pixels.height as usize];
        let image = egui::ColorImage::from_rgba_unmultiplied(size, &pixels.rgba);
        let texture = ctx.load_texture(
            format!("page-picture-{page}"),
            image,
            egui::TextureOptions::LINEAR,
        );
        self.thumbs.insert(
            page,
            Thumb {
                texture,
                fresh: true,
                width: self.thumb_width,
                turn: 0,
            },
        );
    }
}

fn lift(rect: egui::Rect, blur: u8) -> egui::Shape {
    egui::Shadow {
        offset: [0, 2],
        blur,
        spread: 0,
        color: egui::Color32::from_black_alpha(if blur > 12 { 70 } else { 36 }),
    }
    .as_shape(rect, 2.0)
    .into()
}

fn paint_picture(painter: &egui::Painter, rect: egui::Rect, thumb: Option<&Thumb>) {
    painter.rect_filled(rect, 0.0, egui::Color32::WHITE);
    let Some(thumb) = thumb else {
        return;
    };
    let corners = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
    ];
    let uvs = [
        egui::pos2(0.0, 0.0),
        egui::pos2(1.0, 0.0),
        egui::pos2(1.0, 1.0),
        egui::pos2(0.0, 1.0),
    ];
    let shift = usize::try_from(thumb.turn.rem_euclid(4)).unwrap_or(0);
    let mut mesh = egui::Mesh::with_texture(thumb.texture.id());
    for (corner, at) in corners.iter().enumerate() {
        mesh.vertices.push(egui::epaint::Vertex {
            pos: *at,
            uv: uvs[(corner + 4 - shift) % 4],
            color: egui::Color32::WHITE,
        });
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
}

fn paint_hole(ui: &egui::Ui, rect: egui::Rect) {
    let visuals = ui.visuals();
    ui.painter()
        .rect_filled(rect, 4.0, visuals.selection.bg_fill.gamma_multiply(0.25));
    ui.painter().rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(1.5, visuals.selection.stroke.color.gamma_multiply(0.7)),
        egui::StrokeKind::Inside,
    );
}

fn view_file() -> Option<std::path::PathBuf> {
    if cfg!(test) {
        return None;
    }
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|home| std::path::PathBuf::from(home).join(".local").join("state"))
        })?;
    Some(state.join("panpdf").join("view"))
}

pub(crate) fn remembered_view() -> room::View {
    view_file()
        .and_then(|file| std::fs::read_to_string(file).ok())
        .map(|text| room::View::read(&text))
        .unwrap_or_default()
}

impl Window {
    pub(crate) fn remember_the_view(&self) {
        let Some(file) = view_file() else {
            return;
        };
        let view = room::View {
            pages_folded: self.pages_folded,
            pages_width: self.pages_width,
        };
        if let Some(folder) = file.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        let _ = std::fs::write(file, view.write());
    }
}
