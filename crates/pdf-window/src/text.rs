use eframe::egui;

use pdf_app::EditJob;
use pdf_app::draft::{Intent, Landing as TextLanding, Target};
use pdf_app::view::{
    HANDLE_REACH, Quad, ROTATE_REACH_OUT, Step, caret_step_in, selection_between, toolbar_at,
};
use pdf_app::wording::Message;
use pdf_edit::BlockRange;

use crate::canvas::box_on_screen;
use crate::format::Set;
use crate::format::Wanted;
use crate::window_state::{Caret, Pointing, Regrouping, Window};

impl Window {
    pub(crate) fn selection(&self) -> Option<(usize, usize, usize)> {
        let Pointing::Text { page, caret, .. } = self.pointing else {
            return None;
        };
        selection_between(&self.overlay(page)?.carets, caret.anchor, caret.at)
    }

    pub(crate) fn restore_caret(&mut self, page: usize) {
        let Some((wanted, block, row, offset)) = self.resume else {
            return;
        };
        if wanted != page {
            return;
        }
        let Some(overlay) = self.overlay(page) else {
            return;
        };
        let at = overlay
            .blocks
            .get(block)
            .and_then(|block| block.lines.get(row))
            .and_then(|line| {
                overlay
                    .carets
                    .iter()
                    .position(|stop| stop.line == *line && stop.offset == offset)
            });
        let found = at.is_some();
        let stop_at = |(row, offset): (usize, usize)| {
            overlay
                .blocks
                .get(block)
                .and_then(|block| block.lines.get(row))
                .and_then(|line| {
                    overlay
                        .carets
                        .iter()
                        .position(|stop| stop.line == *line && stop.offset == offset)
                })
        };
        let held = self.resume_anchor.and_then(stop_at);
        self.point_at(
            at.map_or(Pointing::Block { page, block }, |at| Pointing::Text {
                page,
                block,
                caret: Caret {
                    at,
                    anchor: held.unwrap_or(at),
                },
            }),
        );
        self.reveal_caret = found;
        self.resume = None;
        self.resume_anchor = None;
        if !found {
            self.input.set_aside(&Message::CaretLostAfterEdit);
        }
    }

    pub(crate) fn delete(&mut self) {
        self.delete_direction(false);
    }

    pub(crate) fn delete_direction(&mut self, backwards: bool) {
        if !self.pointing.editing() && !self.input.pending() && self.resume.is_none() {
            self.delete_the_object();
            return;
        }
        if let Err(reason) = self.input.press(Intent::Delete {
            backwards,
            count: 1,
        }) {
            self.editor.say(reason);
        }
    }

    pub(crate) fn delete_the_object(&mut self) {
        if self.editor.is_busy() {
            return;
        }
        let Some(page) = self.pointing.page() else {
            return;
        };
        if self.chosen.is_a_group() && self.chosen.page == page {
            if let Some((anchors, objects)) = self.group_members() {
                let job = if objects.is_empty() {
                    self.editor.begin_delete_block(page, &anchors)
                } else {
                    self.editor.begin_delete_group(page, &anchors, &objects)
                };
                if job.is_some() {
                    self.point_at(Pointing::Nothing);
                }
                self.send(job);
            }
            return;
        }
        let anchors = self
            .pointing
            .block()
            .and_then(|at| self.anchors_of(page, at));
        let picture = self.pointing.object_on(page).and_then(|at| {
            let object = self.overlay(page)?.objects.get(at)?;
            Some((
                object.anchor.clone(),
                object.kind == pdf_semantics::ObjectKind::Path,
            ))
        });
        let job = match (anchors, picture) {
            (Some(anchors), _) => self.editor.begin_delete_block(page, &anchors),
            (None, Some((anchor, false))) => self.editor.begin_remove_object(page, &anchor),
            (None, Some((anchor, true))) => self.editor.begin_remove_drawing(page, &anchor),
            (None, None) => return,
        };
        if job.is_some() {
            self.point_at(Pointing::Nothing);
        }
        self.send(job);
    }

    pub(crate) fn group_members(&self) -> Option<(Vec<String>, Vec<String>)> {
        let overlay = self.overlay(self.chosen.page)?;
        let mut anchors: Vec<String> = Vec::new();
        for block in &self.chosen.blocks {
            for anchor in &overlay.blocks.get(*block)?.anchors {
                if !anchors.iter().any(|had| had == anchor) {
                    anchors.push(anchor.clone());
                }
            }
        }
        let mut objects: Vec<String> = Vec::new();
        for object in &self.chosen.objects {
            let anchor = &overlay.objects.get(*object)?.anchor;
            if !objects.iter().any(|had| had == anchor) {
                objects.push(anchor.clone());
            }
        }
        (!anchors.is_empty() || !objects.is_empty()).then_some((anchors, objects))
    }

    fn group_moved_to(&self, page: usize, (dx, dy): (f64, f64)) -> Option<Regrouping> {
        let overlay = self.overlay(page)?;
        let moved = |quad: pdf_cli::QuadPixels| Quad::from_pixels(quad).shifted(dx, dy);
        Some(Regrouping {
            page,
            blocks: self
                .chosen
                .blocks
                .iter()
                .filter_map(|at| overlay.blocks.get(*at).map(|block| moved(block.quad)))
                .collect(),
            objects: self
                .chosen
                .objects
                .iter()
                .filter_map(|at| overlay.objects.get(*at).map(|object| moved(object.quad)))
                .collect(),
        })
    }

    pub(crate) fn begin_group_move(
        &mut self,
        page: usize,
        (dx, dy): (f64, f64),
    ) -> Option<EditJob> {
        let (anchors, objects) = self.group_members()?;
        let going = self.group_moved_to(page, (dx, dy));
        let job = if objects.is_empty() {
            self.editor
                .begin_move_block_as_group(page, &anchors, dx, dy)
        } else {
            self.editor
                .begin_move_group(page, &anchors, &objects, (dx, dy))
        };
        if job.is_some() {
            self.regroup = going;
        }
        job
    }

    fn sense_toolbar_surface(&self, ui: &mut egui::Ui, nominal: egui::Rect) {
        let rect = self.toolbar_area.unwrap_or(nominal);
        ui.interact(
            rect,
            ui.id().with(TOOLBAR_SURFACE),
            egui::Sense::click_and_drag(),
        );
    }

    fn ordering_buttons(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let can_order = self.can_order();
        let say = |command| pdf_app::wording::Message::Command(command).say(lang);
        let mut asked = None;
        for (icon, command, order) in [
            (
                crate::icons::Icon::BringToFront,
                pdf_app::wording::Command::BringToFront,
                pdf_edit::Stacking::ToFront,
            ),
            (
                crate::icons::Icon::BringForward,
                pdf_app::wording::Command::BringForward,
                pdf_edit::Stacking::Forward,
            ),
            (
                crate::icons::Icon::SendBackward,
                pdf_app::wording::Command::SendBackward,
                pdf_edit::Stacking::Backward,
            ),
            (
                crate::icons::Icon::SendToBack,
                pdf_app::wording::Command::SendToBack,
                pdf_edit::Stacking::ToBack,
            ),
        ] {
            let clicked = crate::format::icon_button(ui, icon, &say(command), false, can_order)
                .on_disabled_hover_text(Message::OrderingWaitsForThePage.say(lang))
                .clicked();
            if clicked {
                asked = Some(order);
            }
        }
        if let Some(order) = asked {
            self.put_in_order(order);
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one popup, drawn in the three states it has: framed, selected, and being typed in"
    )]
    pub(crate) fn block_toolbar(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        self.toolbar_area = None;
        if !self.show_frames {
            return;
        }
        if self.text_draft.is_some() {
            self.drawn_frame_toolbar(ui);
            return;
        }
        if !self.pointing.editing()
            && self
                .pointing
                .page()
                .is_some_and(|page| self.offer_on(page) == Offer::Group)
        {
            self.group_toolbar(ui);
            return;
        }
        let at = self
            .pointing
            .page()
            .and_then(|page| Some((page, self.pointing.block()?)));
        self.typing.about(at);
        let Some(page) = self.pointing.page() else {
            return;
        };
        let Some(laid) = self.laid.iter().copied().find(|laid| laid.page == page) else {
            return;
        };
        let caret = match self.pointing {
            Pointing::Text { caret, .. } => Some(caret),
            _ => None,
        };
        if self.typing.next.as_ref().is_some_and(|(stop, _)| {
            caret.is_none_or(|caret| caret.at != *stop || caret.anchor != *stop)
        }) {
            self.typing.next = None;
        }
        let Some(overlay) = self.overlay(page) else {
            return;
        };
        let Some(block) = self.pointing.block().and_then(|at| overlay.blocks.get(at)) else {
            return;
        };
        let bounds = self
            .pointing
            .block()
            .and_then(|at| self.live_reach(page, at))
            .map_or(block.box_pixels, |reach| {
                let had = block.box_pixels;
                [
                    had[0].min(reach[0]),
                    had[1].min(reach[1]),
                    had[2].max(reach[2]),
                    had[3].max(reach[3]),
                ]
            });
        let anchors = block.anchors.clone();
        let editing = self.pointing.editing();
        let run = caret
            .and_then(|caret| overlay.carets.get(caret.at))
            .and_then(|stop| pdf_app::view::cluster_styling(&overlay.clusters, stop))
            .and_then(|cluster| overlay.clusters.get(cluster))
            .and_then(|cluster| overlay.runs.iter().find(|run| run.anchor == cluster.anchor));
        let next = self
            .typing
            .next
            .as_ref()
            .map(|(_, style)| style.clone())
            .unwrap_or_default();
        let (font, size_on_page, fill) = match run {
            Some(run) => (
                next.family.clone().or_else(|| run.family.clone()),
                next.size.or(Some(run.em.abs())),
                next.fill.or(run.fill),
            ),
            None => (
                block.font.clone(),
                block.shape.map(|shape| shape.size),
                block.anchors.first().and_then(|first| {
                    overlay
                        .runs
                        .iter()
                        .find(|run| run.anchor == *first)
                        .and_then(|run| run.fill)
                }),
            ),
        };
        let warning = editing
            && self.selection().is_some_and(|(line, from, to)| {
                !pdf_cli::selection_is_actionable(&overlay.clusters, line, from, to)
            });
        let rows_height = 2.0 * crate::format::CONTROL_HEIGHT + ROW_GAP + 2.0 * POPUP_MARGIN;
        let size = (
            760.0,
            if warning {
                rows_height + 18.0
            } else {
                rows_height
            },
        );
        let on_screen = box_on_screen(laid.placed, bounds);
        let view = ui.clip_rect();
        let (x, y) = toolbar_at(
            [
                on_screen.min.x,
                on_screen.min.y,
                on_screen.max.x,
                on_screen.max.y,
            ]
            .map(f64::from),
            (f64::from(size.0), f64::from(size.1)),
            [view.min.x, view.min.y, view.max.x, view.max.y].map(f64::from),
            stem_on_screen(laid.placed),
        );
        #[allow(clippy::cast_possible_truncation)]
        let corner = egui::pos2(x as f32, y as f32);
        let corner = self.context.unwrap_or(corner);
        let area = egui::Rect::from_min_size(corner, egui::vec2(size.0, size.1));
        let spacing = self
            .pointing
            .block()
            .and_then(|block| self.block_spacing(page, block));
        let paragraph = crate::format::ParagraphNow {
            alignment: self
                .pointing
                .block()
                .and_then(|block| self.block_alignment(page, block)),
            flows_round: self
                .pointing
                .block()
                .is_some_and(|block| self.editor.flows_round(page, block)),
        };
        let range = editing
            .then(|| self.text_positions())
            .flatten()
            .map(|(_, _, at, anchor)| (anchor, at));
        let pressed = self
            .pointing
            .block()
            .and_then(|block| self.block_faces(page, block))
            .map(|faces| crate::format::Pressed::of(&faces, range))
            .unwrap_or_default()
            .with(&next);
        let mut wanted = None;
        let mut delete = false;
        let showing = crate::format::Showing {
            font: font.as_deref(),
            size: size_on_page,
            spacing,
            fill,
            pressed,
            paragraph,
        };
        self.sense_toolbar_surface(ui, area);
        let drawn = ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
            toolbar_frame(ui).show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = ROW_GAP;
                    ui.horizontal(|ui| {
                        wanted = self.text_row(ui, &showing);
                    });
                    ui.horizontal(|ui| {
                        if let Some(asked) = self.paragraph_row(ui, &showing) {
                            wanted = Some(asked);
                        }
                        if offers_ordering(Some(Offer::Block), editing) {
                            crate::format::rule(ui);
                            self.ordering_buttons(ui);
                            crate::format::rule(ui);
                            delete = crate::format::icon_button(
                                ui,
                                crate::icons::Icon::Delete,
                                &Message::DeleteBlockHelp.say(lang),
                                false,
                                true,
                            )
                            .clicked();
                        }
                    });
                });
                if warning {
                    ui.label(
                        egui::RichText::new(Message::DeleteNotPlannableHere.say(lang))
                            .small()
                            .color(ui.visuals().warn_fg_color),
                    );
                }
            })
        });
        self.toolbar_area = Some(drawn.inner.response.rect);
        if delete {
            self.delete_the_object();
            return;
        }
        let at_caret = editing
            && self
                .text_positions()
                .is_some_and(|(_, _, at, anchor)| at == anchor);
        let asked = match &wanted {
            Some(Wanted::Style(style)) => Some(style.clone()),
            Some(Wanted::Size(points)) => Some(pdf_edit::TextStyle {
                size: Some(*points),
                ..pdf_edit::TextStyle::default()
            }),
            Some(Wanted::Align(_) | Wanted::FlowRound(_)) | None => None,
        };
        if at_caret
            && let (Some(asked), Some(caret)) = (asked, caret)
            && asked.line_spacing.is_none()
        {
            let held = self
                .typing
                .next
                .take()
                .map(|(_, style)| style)
                .unwrap_or_default();
            self.typing.next = Some((caret.at, pdf_app::view::laid_over(&held, &asked)));
            self.editor.say(Message::StyleForNextTyping);
            return;
        }
        match wanted {
            None => {}
            Some(Wanted::Align(alignment)) => {
                self.set_the_paragraph(page, Set::Align(alignment));
            }
            Some(Wanted::FlowRound(round)) => {
                self.set_the_paragraph(page, Set::FlowRound(round));
            }
            Some(Wanted::Style(style)) if editing => self.style_selection(style),
            Some(Wanted::Style(style)) => self.style_block(page, style),
            Some(Wanted::Size(points)) if editing => self.style_selection(pdf_edit::TextStyle {
                size: Some(points),
                ..pdf_edit::TextStyle::default()
            }),
            Some(Wanted::Size(points)) => self.set_size_from_toolbar(page, &anchors, points),
        }
    }

    pub(crate) fn style_selection(&mut self, style: pdf_edit::TextStyle) {
        let Some((page, block, at, anchor)) = self.text_positions() else {
            self.editor.say(Message::CaretGoneBeforeStyling);
            return;
        };
        let job = self.editor.begin_style(
            page,
            block,
            BlockRange::Between {
                from: anchor,
                to: at,
            },
            style,
        );
        if job.is_some() {
            let start = at.min(anchor);
            self.resume = Some((page, block, start.0, start.1));
        } else {
            let said = Message::AnotherEditIsRunning;
            self.editor.say(said);
        }
        self.send(job);
    }

    pub(crate) fn style_block(&mut self, page: usize, style: pdf_edit::TextStyle) {
        let Some(block) = self.pointing.block() else {
            return;
        };
        let end = self.overlay(page).and_then(|overlay| {
            let rows = &overlay.blocks.get(block)?.lines;
            let last = *rows.last()?;
            let offset = overlay
                .carets
                .iter()
                .filter(|stop| stop.line == last)
                .map(|stop| stop.offset)
                .max()?;
            Some((rows.len() - 1, offset))
        });
        let Some(end) = end else {
            self.editor.say(Message::BlockTextNotFound);
            return;
        };
        let job = self.editor.begin_style(
            page,
            block,
            BlockRange::Between {
                from: (0, 0),
                to: end,
            },
            style,
        );
        if job.is_some() {
            self.reselect = Some((page, block));
        } else {
            let said = Message::AnotherEditIsRunning;
            self.editor.say(said);
        }
        self.send(job);
    }

    pub(crate) fn set_the_paragraph(&mut self, page: usize, set: Set) {
        let Some(block) = self.pointing.block() else {
            return;
        };
        match set {
            Set::Align(alignment) => self.editor.set_alignment(page, block, alignment),
            Set::FlowRound(round) => {
                if round && !self.editor.anything_stands_in(page, block) {
                    self.editor.say(Message::NothingInTheWay);
                }
                self.editor.set_flow_round(page, block, round);
            }
        }
        self.style_block(page, pdf_edit::TextStyle::default());
    }

    pub(crate) fn set_size_from_toolbar(&mut self, page: usize, anchors: &[String], points: f64) {
        let target = self.pointing.block().map(|block| (page, block));
        let job = self.editor.begin_set_size(page, anchors, points);
        if job.is_some() {
            self.reselect = target;
        }
        self.send(job);
    }

    pub(crate) fn move_selection(&mut self, (dx, dy): (f64, f64)) {
        let Some(page) = self.pointing.page() else {
            return;
        };
        if self.chosen.is_a_group() && self.chosen.page == page {
            let job = self.begin_group_move(page, (dx, dy));
            self.send(job);
            return;
        }
        let (job, target) = match self.pointing {
            Pointing::Block { block, .. } | Pointing::Text { block, .. } => {
                if self.anchors_of(page, block).is_none() {
                    return;
                }
                (
                    self.editor.begin_move_text_block(page, block, dx, dy),
                    Some((page, block)),
                )
            }
            Pointing::Object { object, .. } => {
                let Some((anchor, quad)) = self
                    .overlay(page)
                    .and_then(|overlay| overlay.objects.get(object))
                    .map(|object| (object.anchor.clone(), Quad::from_pixels(object.quad)))
                else {
                    return;
                };
                self.reselect_object = Some((page, quad.shifted(dx, dy)));
                (self.editor.begin_place(page, &anchor, dx, dy), None)
            }
            Pointing::Nothing => return,
        };
        if job.is_some() {
            self.reselect = target;
        }
        self.send(job);
    }

    fn send_text_to_the_draft(&mut self) -> bool {
        let Some(draft) = self.text_draft.clone() else {
            return false;
        };
        let target = Target {
            page: draft.page,
            block: 0,
            from: (0, 0),
            to: (0, 0),
        };
        let Some(text) = self.input.send(target, self.editor.epoch()) else {
            return true;
        };
        let job = self.editor.begin_place_text(
            draft.page,
            draft.frame,
            &text,
            &pdf_app::NewTextStyle {
                family: draft.family.clone(),
                size: draft.size,
                bold: draft.bold,
                italic: draft.italic,
                fill: draft.fill,
                paragraph: pdf_edit::ParagraphLayout {
                    alignment: Some(draft.alignment),
                    flow_round: draft.flows_round,
                },
            },
        );
        if job.is_none() {
            self.input
                .landed(TextLanding::Refused(Message::AnotherEditIsRunning));
            return true;
        }
        self.text_draft = None;
        self.entering = self.draft_pixels;
        self.send(job);
        true
    }

    pub(crate) fn drop_the_text_draft(&mut self) {
        if self.text_draft.take().is_some() {
            self.draft_pixels = None;
            self.entering = None;
            self.editor.say(Message::NoTextWasTyped);
        }
    }

    pub(crate) fn enter_the_new_text(&mut self, page: usize) {
        let Some((wanted, pixels)) = self.entering else {
            return;
        };
        if wanted != page {
            return;
        }
        let Some(overlay) = self.overlay(page) else {
            return;
        };
        let Some(anchor) = self.editor.text_just_written() else {
            return;
        };
        let Some(block) = overlay
            .blocks
            .iter()
            .position(|block| block.anchors.iter().any(|named| named == anchor))
        else {
            return;
        };
        let Some(at) = overlay
            .blocks
            .get(block)
            .and_then(|owner| owner.lines.last().copied())
            .and_then(|line| {
                overlay
                    .carets
                    .iter()
                    .enumerate()
                    .filter(|(_, stop)| stop.line == line)
                    .max_by_key(|(_, stop)| stop.offset)
                    .map(|(at, _)| at)
            })
        else {
            return;
        };
        self.entering = None;
        self.draft_pixels = None;
        self.editor.preview_frame(page, block, pixels);
        self.editor.declare_frame(page, block);
        self.point_at(Pointing::Text {
            page,
            block,
            caret: Caret { at, anchor: at },
        });
    }

    pub(crate) fn object_toolbar(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        if !self.show_frames {
            return;
        }
        let Pointing::Object { page, object } = self.pointing else {
            return;
        };
        let offer = self.offer_on(page);
        if offer != Offer::Object {
            return;
        }
        let Some(laid) = self.laid.iter().copied().find(|laid| laid.page == page) else {
            return;
        };
        let Some(quad) = self
            .overlay(page)
            .and_then(|overlay| overlay.objects.get(object))
            .map(|object| object.quad)
        else {
            return;
        };
        let bounds = Quad::from_pixels(quad).bounds();
        #[allow(clippy::cast_precision_loss)]
        let size = (ONE_CONTROL * controls_in(offer) as f32, 44.0);
        let on_screen = box_on_screen(laid.placed, bounds);
        let view = ui.clip_rect();
        let (x, y) = toolbar_at(
            [
                on_screen.min.x,
                on_screen.min.y,
                on_screen.max.x,
                on_screen.max.y,
            ]
            .map(f64::from),
            (f64::from(size.0), f64::from(size.1)),
            [view.min.x, view.min.y, view.max.x, view.max.y].map(f64::from),
            stem_on_screen(laid.placed),
        );
        #[allow(clippy::cast_possible_truncation)]
        let corner = self
            .context
            .unwrap_or_else(|| egui::pos2(x as f32, y as f32));
        let area = egui::Rect::from_min_size(corner, egui::vec2(size.0, size.1));
        let mut asked = false;
        self.sense_toolbar_surface(ui, area);
        let drawn = ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
            toolbar_frame(ui).show(ui, |ui| {
                ui.horizontal(|ui| {
                    asked = crate::format::icon_button(
                        ui,
                        crate::icons::Icon::FlowRound,
                        &Message::TextFlowsRoundThis.say(lang),
                        false,
                        true,
                    )
                    .clicked();
                    if offers_ordering(Some(offer), false) {
                        crate::format::rule(ui);
                        self.ordering_buttons(ui);
                    }
                });
            })
        });
        self.toolbar_area = Some(drawn.inner.response.rect);
        if asked {
            let job = self.editor.begin_flow_round(page, bounds);
            self.send(job);
        }
    }

    fn group_toolbar(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let page = self.chosen.page;
        let Some(laid) = self.laid.iter().copied().find(|laid| laid.page == page) else {
            return;
        };
        let Some(bounds) = self.group_bounds(page) else {
            return;
        };
        #[allow(clippy::cast_precision_loss)]
        let size = (ONE_CONTROL * controls_in(Offer::Group) as f32, 44.0);
        let on_screen = box_on_screen(laid.placed, bounds);
        let view = ui.clip_rect();
        let (x, y) = toolbar_at(
            [
                on_screen.min.x,
                on_screen.min.y,
                on_screen.max.x,
                on_screen.max.y,
            ]
            .map(f64::from),
            (f64::from(size.0), f64::from(size.1)),
            [view.min.x, view.min.y, view.max.x, view.max.y].map(f64::from),
            0.0,
        );
        #[allow(clippy::cast_possible_truncation)]
        let corner = self
            .context
            .unwrap_or_else(|| egui::pos2(x as f32, y as f32));
        let area = egui::Rect::from_min_size(corner, egui::vec2(size.0, size.1));
        let mut asked = false;
        self.sense_toolbar_surface(ui, area);
        let drawn = ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
            toolbar_frame(ui).show(ui, |ui| {
                ui.horizontal(|ui| {
                    asked = crate::format::icon_button(
                        ui,
                        crate::icons::Icon::Delete,
                        &Message::DeleteBlockHelp.say(lang),
                        false,
                        true,
                    )
                    .clicked();
                });
            })
        });
        self.toolbar_area = Some(drawn.inner.response.rect);
        if asked {
            self.delete_the_object();
        }
    }

    pub(crate) fn offer_on(&self, page: usize) -> Offer {
        let members = if self.chosen.page == page {
            self.chosen.count()
        } else {
            0
        };
        offer_for(members, self.pointing.object_on(page).is_some())
    }

    fn group_bounds(&self, page: usize) -> Option<[f64; 4]> {
        let overlay = self.overlay(page)?;
        let quads = self
            .chosen
            .blocks
            .iter()
            .filter_map(|at| overlay.blocks.get(*at).map(|block| block.quad))
            .chain(
                self.chosen
                    .objects
                    .iter()
                    .filter_map(|at| overlay.objects.get(*at).map(|object| object.quad)),
            );
        let mut bounds: Option<[f64; 4]> = None;
        for quad in quads {
            let box_ = Quad::from_pixels(quad).bounds();
            bounds = Some(bounds.map_or(box_, |had: [f64; 4]| {
                [
                    had[0].min(box_[0]),
                    had[1].min(box_[1]),
                    had[2].max(box_[2]),
                    had[3].max(box_[3]),
                ]
            }));
        }
        bounds
    }

    fn drawn_frame_toolbar(&mut self, ui: &mut egui::Ui) {
        let Some(draft) = self.text_draft.clone() else {
            return;
        };
        let Some((page, pixels)) = self.draft_pixels else {
            return;
        };
        let Some(laid) = self.laid.iter().copied().find(|laid| laid.page == page) else {
            return;
        };
        let size = (700.0, 44.0);
        let on_screen = box_on_screen(laid.placed, pixels);
        let view = ui.clip_rect();
        let (x, y) = toolbar_at(
            [
                on_screen.min.x,
                on_screen.min.y,
                on_screen.max.x,
                on_screen.max.y,
            ]
            .map(f64::from),
            (f64::from(size.0), f64::from(size.1)),
            [view.min.x, view.min.y, view.max.x, view.max.y].map(f64::from),
            0.0,
        );
        #[allow(clippy::cast_possible_truncation)]
        let corner = self
            .context
            .unwrap_or_else(|| egui::pos2(x as f32, y as f32));
        let area = egui::Rect::from_min_size(corner, egui::vec2(size.0, size.1));
        let styling = draft.styling();
        let mut wanted = None;
        self.sense_toolbar_surface(ui, area);
        let drawn = ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
            toolbar_frame(ui).show(ui, |ui| {
                ui.horizontal(|ui| {
                    wanted = self.font_row(
                        ui,
                        &crate::format::Showing {
                            font: styling.family.as_deref(),
                            size: styling.size,
                            spacing: None,
                            fill: styling.fill,
                            pressed: crate::format::Pressed {
                                bold: styling.bold == Some(true),
                                italic: styling.italic == Some(true),
                                underline: false,
                            },
                            paragraph: crate::format::ParagraphNow {
                                alignment: Some(draft.alignment),
                                flows_round: draft.flows_round,
                            },
                        },
                    );
                });
            })
        });
        self.toolbar_area = Some(drawn.inner.response.rect);
        if let Some(wanted) = wanted
            && let Some(draft) = self.text_draft.as_mut()
        {
            draft.restyle(&wanted);
        }
    }

    pub(crate) fn send_text(&mut self) {
        if self.send_text_to_the_draft() {
            return;
        }
        if !self.pointing.editing() {
            self.input.set_aside(&Message::CaretIsNoLongerInText);
            return;
        }
        let Some((page, block, at, anchor)) = self.text_positions() else {
            self.input.set_aside(&Message::CaretIsNoLongerInText);
            self.point_at(Pointing::Nothing);
            self.editor.say(Message::CaretGoneBeforeTyping);
            return;
        };
        let target = Target {
            page,
            block,
            from: anchor,
            to: at,
        };
        let Some(text) = self.input.send(target, self.editor.epoch()) else {
            if let Some(draft) = self.input.draft() {
                self.editor.say(draft.reason.clone());
            }
            return;
        };
        self.type_at(target, &text);
    }

    fn type_at(&mut self, target: Target, text: &str) {
        let start = target.from.min(target.to);
        let range = BlockRange::Between {
            from: target.from,
            to: target.to,
        };
        let collapsed = target.from == target.to;
        let carried = self.typing.carry.take();
        let style = match self.typing.next.take() {
            Some((_, style)) if collapsed => Some(style),
            _ => carried.filter(|_| collapsed),
        };
        if let Some(style) = &style
            && text.chars().all(char::is_whitespace)
        {
            self.typing.carry = Some(style.clone());
        }
        let job = match style {
            Some(style) => {
                self.editor
                    .begin_edit_in_style(target.page, target.block, range, text, style)
            }
            None => self
                .editor
                .begin_edit(target.page, target.block, range, text),
        };
        if job.is_some() {
            self.resume = Some((target.page, target.block, start.0, start.1));
        } else {
            self.input
                .landed(TextLanding::Refused(Message::AnotherEditIsRunning));
        }
        self.send(job);
    }

    pub(crate) fn delete_at_caret(&mut self, backwards: bool, count: usize) {
        let Some((page, block, at, anchor)) = self.text_positions() else {
            self.input
                .landed(TextLanding::Refused(Message::CaretIsNoLongerInText));
            self.editor.say(Message::CaretGoneBeforeDeleting);
            return;
        };
        let range = if at == anchor {
            BlockRange::Units {
                at,
                backwards,
                count,
            }
        } else {
            BlockRange::Between {
                from: anchor,
                to: at,
            }
        };
        let start = at.min(anchor);
        let job = self.editor.begin_edit(page, block, range, "");
        if job.is_some() {
            self.resume = Some((page, block, start.0, start.1));
        } else {
            self.input
                .landed(TextLanding::Refused(Message::AnotherEditIsRunning));
        }
        self.send(job);
    }

    pub(crate) fn step_caret(&mut self, step: Step, extend: bool) {
        let Pointing::Text { page, block, caret } = self.pointing else {
            return;
        };
        let Some(rows) = self.rows_of(page, block) else {
            return;
        };
        let Some(overlay) = self.overlay(page) else {
            return;
        };
        let at = caret_step_in(&overlay.carets, &rows, caret.at, step);
        self.pointing = Pointing::Text {
            page,
            block,
            caret: Caret {
                at,
                anchor: if extend { caret.anchor } else { at },
            },
        };
        self.reveal_caret = true;
    }

    pub(crate) fn retry_draft(&mut self) {
        match self.input.retry(self.editor.epoch()) {
            Ok((target, text)) => self.type_at(target, &text),
            Err(reason) => self.editor.say(reason),
        }
    }
}

pub(crate) fn toolbar_frame(ui: &mut egui::Ui) -> egui::Frame {
    ui.spacing_mut().interact_size.y = crate::format::CONTROL_HEIGHT;
    ui.spacing_mut().item_spacing.x = 4.0;
    egui::Frame::popup(ui.style())
        .inner_margin(egui::Margin::symmetric(6, 6))
        .corner_radius(8.0)
}

const TOOLBAR_SURFACE: &str = "toolbar-surface";

pub(crate) fn press_belongs_to_the_toolbar(area: Option<egui::Rect>, at: egui::Pos2) -> bool {
    area.is_some_and(|area| area.contains(at))
}

pub(crate) fn toolbar_takes_the_pointer(
    area: Option<egui::Rect>,
    pressed_at: Option<egui::Pos2>,
    carrying: bool,
) -> bool {
    !carrying && pressed_at.is_some_and(|at| press_belongs_to_the_toolbar(area, at))
}

#[cfg(test)]
mod toolbar_hit_area_tests {
    use eframe::egui;

    use super::{press_belongs_to_the_toolbar, toolbar_takes_the_pointer};

    fn toolbar() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(68.0, 40.0))
    }

    #[test]
    fn a_press_on_a_button_belongs_to_the_toolbar() {
        let area = Some(toolbar());
        assert!(press_belongs_to_the_toolbar(area, egui::pos2(112.0, 120.0)));
    }

    #[test]
    fn a_press_in_the_padding_between_two_buttons_still_belongs_to_the_toolbar() {
        let area = Some(toolbar());
        assert!(
            press_belongs_to_the_toolbar(area, egui::pos2(137.0, 120.0)),
            "the whole popup must answer, not only its buttons"
        );
    }

    #[test]
    fn a_press_one_pixel_inside_the_edge_belongs_to_the_toolbar() {
        let area = Some(toolbar());
        assert!(press_belongs_to_the_toolbar(area, egui::pos2(167.0, 139.0)));
    }

    #[test]
    fn a_press_one_pixel_outside_the_edge_does_not_belong_to_the_toolbar() {
        let area = Some(toolbar());
        assert!(!press_belongs_to_the_toolbar(
            area,
            egui::pos2(169.0, 120.0)
        ));
    }

    #[test]
    fn with_no_toolbar_nothing_belongs_to_it() {
        assert!(!press_belongs_to_the_toolbar(
            None,
            egui::pos2(120.0, 120.0)
        ));
    }

    #[test]
    fn a_drag_let_go_over_the_toolbar_stays_the_pages() {
        let area = Some(toolbar());
        let over = Some(egui::pos2(137.0, 120.0));
        assert!(!toolbar_takes_the_pointer(area, over, true));
        assert!(toolbar_takes_the_pointer(area, over, false), "the control");
        assert!(
            press_belongs_to_the_toolbar(area, egui::pos2(137.0, 120.0)),
            "where the pointer is, alone, would have given the drag away"
        );
    }

    #[test]
    fn a_press_is_judged_where_it_began() {
        let area = Some(toolbar());
        assert!(!toolbar_takes_the_pointer(
            area,
            Some(egui::pos2(80.0, 120.0)),
            false
        ));
        assert!(toolbar_takes_the_pointer(
            area,
            Some(egui::pos2(112.0, 120.0)),
            false
        ));
        assert!(!toolbar_takes_the_pointer(area, None, false), "no press");
    }
}

const ROW_GAP: f32 = 10.0;

const POPUP_MARGIN: f32 = 6.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Offer {
    Block,
    Object,
    Group,
}

pub(crate) const fn offer_for(members: usize, on_an_object: bool) -> Offer {
    if members > 1 {
        Offer::Group
    } else if on_an_object {
        Offer::Object
    } else {
        Offer::Block
    }
}

pub(crate) const fn offers_ordering(offer: Option<Offer>, editing: bool) -> bool {
    !editing && matches!(offer, Some(Offer::Block | Offer::Object))
}

pub(crate) const fn controls_in(offer: Offer) -> usize {
    match offer {
        Offer::Group => 1,
        Offer::Object => 1 + ORDER_CONTROLS,
        Offer::Block => BLOCK_CONTROLS,
    }
}

const BLOCK_CONTROLS: usize = 20;

const ORDER_CONTROLS: usize = 4;

const ONE_CONTROL: f32 = 52.0;

fn stem_on_screen(placed: pdf_app::view::Placement) -> f64 {
    let rise = box_on_screen(placed, [0.0, 0.0, 0.0, ROTATE_REACH_OUT + HANDLE_REACH]);
    f64::from((rise.max.y - rise.min.y).abs())
}

#[cfg(test)]
mod offer_tests {
    use super::{BLOCK_CONTROLS, ORDER_CONTROLS, Offer, controls_in, offer_for};

    #[test]
    fn several_things_chosen_are_offered_one_control() {
        for members in [2, 3, 6, 49] {
            assert_eq!(offer_for(members, false), Offer::Group, "{members} chosen");
            assert_eq!(
                offer_for(members, true),
                Offer::Group,
                "a picture among them changes nothing"
            );
            assert_eq!(
                controls_in(offer_for(members, false)),
                1,
                "{members} chosen were offered more than Delete"
            );
        }
    }

    #[test]
    fn one_thing_chosen_is_still_offered_everything() {
        assert_eq!(offer_for(1, false), Offer::Block);
        assert_eq!(controls_in(offer_for(1, false)), BLOCK_CONTROLS);
        assert_eq!(offer_for(1, true), Offer::Object);
        assert_eq!(controls_in(offer_for(1, true)), 1 + ORDER_CONTROLS);
        assert_ne!(
            controls_in(offer_for(1, false)),
            controls_in(offer_for(6, false)),
            "one thing and six were offered the same toolbar"
        );
    }
}

#[cfg(test)]
mod ordering_offer_tests {
    use super::{Offer, offers_ordering};

    #[test]
    fn only_one_block_or_one_object_not_being_typed_in_offers_ordering() {
        assert!(
            offers_ordering(Some(Offer::Block), false),
            "a block, at rest"
        );
        assert!(offers_ordering(Some(Offer::Object), false), "a picture");
        assert!(
            !offers_ordering(Some(Offer::Block), true),
            "a caret in the block -- the owner: \"while we are typing that \
             button should not be there\""
        );
        assert!(
            !offers_ordering(Some(Offer::Group), false),
            "several things chosen keep Delete alone"
        );
        assert!(
            !offers_ordering(None, false),
            "nothing pointed at offers nothing"
        );
    }

    #[test]
    fn editing_is_the_one_thing_that_changes_the_answer_for_a_block() {
        assert_ne!(
            offers_ordering(Some(Offer::Block), false),
            offers_ordering(Some(Offer::Block), true)
        );
    }
}
