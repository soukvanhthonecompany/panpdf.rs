use eframe::egui;

use pdf_app::document::OVERLAY_SCALE;
use pdf_app::draft::Intent;
use pdf_app::view::{
    Proportions, Quad, ROTATE_HANDLE, block_at, caret_at_in, covering, handle_at, object_at,
    resized, run_at, shaped, standing_block_at, text_handle_at,
};
use pdf_app::view::{Step, frame_quad};
use pdf_app::wording::Message;

use crate::app::{FAR_STEP, NEAR_STEP, kind_name};
use crate::canvas::{fill_quad, quad_on_screen, stroke_quad};
use crate::window_state::{
    Around, ArrowPress, Caret, Carrying, Chosen, Drag, HOLD_FOR, Laid, Landing, Pointing,
    STILL_ENOUGH, TextDraft, Tool, TypedKey, Window, resize_cursor,
};

const DRAWN_TEXT_WIDTH: f64 = 200.0;

const NEW_TEXT_SIZE: f64 = 12.0;
pub(crate) const NEW_TEXT_FAMILY: &str = "DejaVu Sans";

impl Window {
    pub(crate) fn put_the_drag_down(&mut self) {
        self.drag = None;
        self.landing = None;
        self.reselect = None;
    }

    fn handle_held(&self, page: usize, point: (f64, f64)) -> Option<(Carrying, Quad)> {
        if self.pointing.editing() {
            return None;
        }
        let (frame, handle, quad) = self.handle_under(page, point)?;
        if handle == ROTATE_HANDLE {
            return Some((Carrying::BlockTurn(self.anchors_of(page, frame)?), quad));
        }
        let held = Quad::of(*self.frames_of(page).get(frame)?);
        Some((Carrying::Handle { frame, handle }, held))
    }

    fn handle_under(&self, page: usize, point: (f64, f64)) -> Option<(usize, usize, Quad)> {
        if self.pointing.editing() {
            return None;
        }
        let block = self.pointing.block_on(page)?;
        let quad = self.quad_of_block(page, block)?;
        let handle = text_handle_at(&quad, point)?;
        Some((block, handle, quad))
    }

    fn object_handle_held(&self, page: usize, point: (f64, f64)) -> Option<(Carrying, Quad)> {
        let object = self.pointing.object_on(page)?;
        let served = self.overlay(page)?.objects.get(object)?;
        let quad = Quad::from_pixels(served.quad);
        let handle = handle_at(&quad, point)?;
        Some((
            Carrying::ObjectHandle {
                anchor: served.anchor.clone(),
                handle,
            },
            quad,
        ))
    }

    fn quad_of_block(&self, page: usize, block: usize) -> Option<Quad> {
        Some(Quad::from_pixels(
            self.overlay(page)?.blocks.get(block)?.quad,
        ))
    }

    fn resize(
        &mut self,
        page: usize,
        frame: usize,
        handle: usize,
        started_at: [f64; 4],
        travel: egui::Vec2,
    ) {
        let Some(laid) = self.laid.iter().copied().find(|laid| laid.page == page) else {
            return;
        };
        let (dx, dy) = (
            f64::from(travel.x / laid.placed.stretch),
            f64::from(travel.y / laid.placed.stretch),
        );
        let held = self.layout_in(page, frame);
        let want = resized(started_at, handle, dx, dy);
        let supported = held.map_or(want, |laid| covering(want, laid));
        self.editor.preview_frame(page, frame, supported);
        if supported
            .iter()
            .zip(want)
            .any(|(one, other)| (one - other).abs() > 1e-8)
        {
            self.editor.say(Message::FrameStopsAtText);
        }
    }

    pub(crate) fn page_point(&self, at: egui::Pos2) -> Option<(usize, (f64, f64))> {
        self.laid
            .iter()
            .find_map(|laid| Some((laid.page, laid.placed.page_point((at.x, at.y))?)))
    }

    pub(crate) fn pointer(
        &mut self,
        ctx: &egui::Context,
        response: &egui::Response,
        clip: egui::Rect,
    ) {
        if self.leaving.is_some() || self.loading.is_some() {
            return;
        }
        if self.resume.is_some() || self.input.has_queued() {
            if !(response.clicked() || response.drag_started()) {
                return;
            }
            self.resume = None;
            self.input.set_aside(&Message::ClickedAwayFromDraft);
        }
        if self.left_the_drawn_frame(ctx, response) && self.tool != Tool::Text {
            self.drag = None;
            return;
        }
        if response.clicked()
            && !self.editor.is_busy()
            && self.tool.places_on_click()
            && let Some(at) = response.interact_pointer_pos()
            && let Some((page, _)) = self.page_point(at)
        {
            let press = Drag {
                what: Carrying::NewPicture,
                page,
                from: at,
                to: at,
                started_at: Quad::of([0.0; 4]),
                straight: false,
            };
            self.drag = None;
            if self.tool == Tool::Picture {
                self.place_the_picture(&press);
            } else {
                self.take_the_frame_drawn(&press);
            }
            return;
        }
        if response.clicked() && self.input.collecting() {
            self.input.park();
        }
        if response.secondary_clicked()
            && !self.editor.is_busy()
            && let Some(at) = response.interact_pointer_pos()
        {
            let pointing = self
                .page_point(at)
                .map_or(Pointing::Nothing, |(page, point)| {
                    self.clicked_at(page, point, false)
                });
            self.point_at(pointing);
            self.context = (!matches!(pointing, Pointing::Nothing)).then_some(at);
        }
        if response.clicked()
            && !self.editor.is_busy()
            && ctx.input(|input| input.modifiers.command)
            && let Some(at) = response.interact_pointer_pos()
            && let Some((page, point)) = self.page_point(at)
            && let Some(link) = self.editor.link_at(page, point)
        {
            self.follow(&link);
            return;
        }
        if self.clicked_a_field(ctx, response) {
            return;
        }
        if response.clicked() && !self.editor.is_busy() {
            self.context = None;
            let extend = ctx.input(|input| input.modifiers.shift);
            if let Some(at) = response.interact_pointer_pos() {
                match self.page_point(at) {
                    Some((page, point)) if extend && !self.pointing.editing() => {
                        self.add_to_group(page, point);
                    }
                    Some((page, point)) => {
                        let pointing = self.clicked_at(page, point, extend);
                        self.point_at(pointing);
                    }
                    None => self.point_at(Pointing::Nothing),
                }
            }
        }
        if response.double_clicked()
            && !self.editor.is_busy()
            && let Some(at) = response.interact_pointer_pos()
            && let Some((page, point)) = self.page_point(at)
        {
            let was_editing = self.pointing.editing();
            let pointing = match self.clicked_at(page, point, false) {
                Pointing::Block { page, block } => self.enter(page, block, point, false),
                other => other,
            };
            self.point_at(pointing);
            if was_editing {
                self.select_around(Around::Word);
            }
        }
        if response.triple_clicked() && !self.editor.is_busy() {
            self.select_around(Around::Row);
        }
        self.hold_still_hint(ctx, response);
        self.dragging(ctx, response, clip);
    }

    fn select_around(&mut self, around: Around) {
        let Pointing::Text { page, block, caret } = self.pointing else {
            return;
        };
        let Some(overlay) = self.overlay(page) else {
            return;
        };
        let Some(stop) = overlay.carets.get(caret.at) else {
            return;
        };
        let line = stop.line;
        let span = match around {
            Around::Word => pdf_app::view::word_at(&overlay.clusters, line, stop.offset),
            Around::Row => pdf_app::view::row_at(&overlay.carets, line),
        };
        let Some((from, to)) = span else { return };
        let (Some(anchor), Some(at)) = (
            pdf_app::view::stop_at(&overlay.carets, line, from),
            pdf_app::view::stop_at(&overlay.carets, line, to),
        ) else {
            return;
        };
        self.pointing = Pointing::Text {
            page,
            block,
            caret: Caret { at, anchor },
        };
    }

    fn add_to_group(&mut self, page: usize, point: (f64, f64)) {
        let mut picked = match self.clicked_at(page, point, false) {
            Pointing::Block { block, .. } | Pointing::Text { block, .. } => Some((true, block)),
            Pointing::Object { object, .. } => Some((false, object)),
            Pointing::Nothing => None,
        };
        if let Some((true, block)) = picked
            && self.chosen.page == page
            && self.chosen.blocks.contains(&block)
            && let Some(overlay) = self.overlay(page)
            && let Some(object) = object_at(&overlay.objects, point)
            && !self.chosen.objects.contains(&object)
        {
            picked = Some((false, object));
        }
        let Some((is_a_block, index)) = picked else {
            return;
        };
        if self.chosen.count() > 0 && self.chosen.page != page {
            self.chosen = Chosen::default();
        }
        let mut blocks = std::mem::take(&mut self.chosen.blocks);
        let mut objects = std::mem::take(&mut self.chosen.objects);
        if self.chosen.count() == 0 {
            match self.pointing {
                Pointing::Block { page: at, block }
                | Pointing::Text {
                    page: at, block, ..
                } if at == page => {
                    blocks.push(block);
                }
                Pointing::Object { page: at, object } if at == page => objects.push(object),
                _ => {}
            }
        }
        let list = if is_a_block {
            &mut blocks
        } else {
            &mut objects
        };
        if let Some(place) = list.iter().position(|at| *at == index) {
            list.remove(place);
        } else {
            list.push(index);
        }
        blocks.sort_unstable();
        objects.sort_unstable();
        self.choose(page, blocks, objects);
    }

    fn clicked_at(&self, page: usize, point: (f64, f64), extend: bool) -> Pointing {
        let Some(overlay) = self.overlay(page) else {
            return Pointing::Nothing;
        };
        let quads: Vec<Quad> = overlay
            .blocks
            .iter()
            .map(|block| Quad::from_pixels(block.quad))
            .collect();
        let hit = standing_block_at(
            &quads,
            |index| self.pointing.block_stands(overlay, page, index),
            point,
        );
        match (hit, self.pointing) {
            (
                Some(block),
                Pointing::Text {
                    page: was,
                    block: had,
                    ..
                }
                | Pointing::Block {
                    page: was,
                    block: had,
                },
            ) if block == had && page == was => self.enter(page, block, point, extend),
            (Some(block), _) => Pointing::Block { page, block },
            (None, _) => match object_at(&overlay.objects, point) {
                Some(object) => Pointing::Object { page, object },
                None => Pointing::Nothing,
            },
        }
    }

    fn enter(&self, page: usize, block: usize, point: (f64, f64), extend: bool) -> Pointing {
        let Some(rows) = self.rows_of(page, block) else {
            return Pointing::Block { page, block };
        };
        let Some(overlay) = self.overlay(page) else {
            return Pointing::Block { page, block };
        };
        let Some(at) = caret_at_in(&overlay.carets, &rows, point) else {
            return Pointing::Block { page, block };
        };
        Pointing::Text {
            page,
            block,
            caret: Caret {
                at,
                anchor: match (extend, self.pointing.caret()) {
                    (true, Some(caret)) => caret.anchor,
                    _ => at,
                },
            },
        }
    }

    fn dragging(&mut self, ctx: &egui::Context, response: &egui::Response, clip: egui::Rect) {
        if response.drag_started() && !self.editor.is_busy() {
            self.take_hold(ctx, response);
        }
        self.show_the_pointer(ctx, response);
        let straight = ctx.input(|input| input.modifiers.shift);
        if let (Some(drag), Some(at)) = (self.drag.as_mut(), response.interact_pointer_pos()) {
            drag.to = at;
            drag.straight = straight;
        }
        let inking = match &self.drag {
            Some(Drag {
                what: Carrying::NewLine,
                page,
                to,
                straight,
                ..
            }) => Some((*page, *to, *straight)),
            _ => None,
        };
        if let Some((page, to, straight)) = inking {
            self.carry_the_ink(page, to, straight);
        }
        let sweeping = match &self.drag {
            Some(Drag {
                what: Carrying::Selection { block },
                page,
                from,
                to,
                ..
            }) => Some((*page, *block, *from, *to)),
            _ => None,
        };
        let resizing = match &self.drag {
            Some(Drag {
                what: Carrying::Handle { frame, handle },
                page,
                from,
                to,
                started_at,
                ..
            }) => Some((*page, *frame, *handle, *started_at, *to - *from)),
            _ => None,
        };
        let panning = match &self.drag {
            Some(Drag {
                what: Carrying::Panning { was },
                from,
                to,
                ..
            }) => Some(*was - (*to - *from)),
            _ => None,
        };
        if let Some(offset) = panning {
            self.wanted_offset = Some(offset);
        }
        if let Some((page, frame, handle, started_at, travel)) = resizing {
            self.resize(page, frame, handle, started_at.bounds(), travel);
        } else if let Some((page, block, from, to)) = sweeping {
            self.sweep(page, block, from, to);
        } else if let Some(drag) = &self.drag {
            self.carry(ctx, drag, clip);
        } else {
            self.show_landing(ctx, clip);
        }
        if response.drag_stopped() {
            self.let_go();
        }
    }

    fn show_the_pointer(&self, ctx: &egui::Context, response: &egui::Response) {
        let held = match &self.drag {
            Some(Drag {
                what: Carrying::Handle { handle, .. } | Carrying::ObjectHandle { handle, .. },
                ..
            }) => Some(*handle),
            Some(Drag {
                what: Carrying::BlockTurn(_),
                ..
            }) => Some(ROTATE_HANDLE),
            Some(_) => None,
            None => response
                .hover_pos()
                .and_then(|at| self.page_point(at))
                .and_then(|(page, point)| {
                    self.handle_under(page, point)
                        .map(|(_, handle, _)| handle)
                        .or_else(|| {
                            let object = self.pointing.object_on(page)?;
                            let served = self.overlay(page)?.objects.get(object)?;
                            handle_at(&Quad::from_pixels(served.quad), point)
                        })
                }),
        };
        if let Some(handle) = held {
            ctx.set_cursor_icon(resize_cursor(handle));
            return;
        }
        let over_the_page = self.drag.is_some()
            || response
                .hover_pos()
                .is_some_and(|at| self.page_point(at).is_some());
        if !over_the_page {
            return;
        }
        let placing = match self.tool {
            Tool::Select => false,
            Tool::Form => self.form_tool.kind.is_some(),
            Tool::Pen => {
                if let Some(at) = ctx.pointer_latest_pos() {
                    ctx.set_cursor_icon(egui::CursorIcon::None);
                    let painter = ctx.layer_painter(egui::LayerId::new(
                        egui::Order::Tooltip,
                        egui::Id::new("pen pointer"),
                    ));
                    crate::draw_pen::pen_pointer(
                        &painter,
                        at,
                        crate::draw_pen::on_screen_colour(self.pen.colour),
                    );
                }
                return;
            }
            Tool::Text | Tool::Picture | Tool::Highlighter | Tool::Shape | Tool::Link => true,
        };
        if placing {
            ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
        }
    }

    fn take_hold_of_the_page(&mut self, ctx: &egui::Context, at: egui::Pos2) -> bool {
        if !ctx.input(|input| input.key_down(egui::Key::Space) || input.pointer.middle_down()) {
            return false;
        }
        self.drag = Some(Drag {
            what: Carrying::Panning { was: self.offset },
            page: self.focus,
            from: at,
            to: at,
            started_at: Quad::of([0.0; 4]),
            straight: false,
        });
        true
    }

    fn take_hold_of_the_text(&mut self, page: usize, point: (f64, f64), at: egui::Pos2) -> bool {
        let Pointing::Text {
            page: was, block, ..
        } = self.pointing
        else {
            return false;
        };
        let quad = self
            .overlay(page)
            .filter(|_| was == page)
            .and_then(|overlay| overlay.blocks.get(block))
            .map(|held| Quad::from_pixels(held.quad));
        let Some(quad) = quad.filter(|quad| block_at(&[*quad], point).is_some()) else {
            return false;
        };
        self.drag = Some(Drag {
            what: Carrying::Selection { block },
            page,
            from: at,
            to: at,
            started_at: quad,
            straight: false,
        });
        true
    }

    fn take_hold_of_a_picture(&mut self, page: usize, point: (f64, f64), at: egui::Pos2) -> bool {
        if let Some(taken) = self.object_handle_held(page, point) {
            self.drag = Some(Drag {
                what: taken.0,
                page,
                from: at,
                to: at,
                started_at: taken.1,
                straight: false,
            });
            return true;
        }
        let picked = self.overlay(page).and_then(|overlay| {
            let index = object_at(&overlay.objects, point)?;
            let object = overlay.objects.get(index)?;
            Some((index, object.anchor.clone(), Quad::from_pixels(object.quad)))
        });
        let Some((index, anchor, quad)) = picked else {
            return false;
        };
        self.drag = Some(Drag {
            what: Carrying::Object(anchor),
            page,
            from: at,
            to: at,
            started_at: quad,
            straight: false,
        });
        self.point_at(Pointing::Object {
            page,
            object: index,
        });
        true
    }

    fn left_the_drawn_frame(&mut self, ctx: &egui::Context, response: &egui::Response) -> bool {
        if self.text_draft.is_none()
            || !(response.clicked() || response.drag_started() || response.secondary_clicked())
        {
            return false;
        }
        let Some(at) = ctx
            .input(|input| input.pointer.press_origin())
            .or_else(|| response.interact_pointer_pos())
        else {
            return false;
        };
        let inside = self.page_point(at).is_some_and(|(page, point)| {
            self.draft_pixels.is_some_and(|(drawn, [x0, y0, x1, y1])| {
                drawn == page && (x0..=x1).contains(&point.0) && (y0..=y1).contains(&point.1)
            })
        });
        if inside {
            return false;
        }
        self.drop_the_text_draft();
        true
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one press, one arm per thing it can land on"
    )]
    fn take_hold(&mut self, ctx: &egui::Context, response: &egui::Response) {
        let Some(at) = ctx
            .input(|input| input.pointer.press_origin())
            .or_else(|| response.interact_pointer_pos())
        else {
            self.drag = None;
            return;
        };
        if self.take_hold_of_the_page(ctx, at) {
            return;
        }
        let Some((page, point)) = self.page_point(at) else {
            self.drag = None;
            return;
        };
        if self.overlay(page).is_none() {
            self.drag = None;
            return;
        }
        if self.tool == Tool::Link {
            self.drag = Some(Drag {
                what: self.link_tool_takes_hold(
                    page,
                    point,
                    ctx.input(|input| input.modifiers.shift || input.modifiers.command),
                ),
                page,
                from: at,
                to: at,
                started_at: Quad::of([0.0; 4]),
                straight: false,
            });
            return;
        }
        if self.tool == Tool::Form {
            self.drag = Some(Drag {
                what: self.form_tool_takes_hold(
                    page,
                    point,
                    ctx.input(|input| input.modifiers.shift || input.modifiers.command),
                ),
                page,
                from: at,
                to: at,
                started_at: Quad::of([0.0; 4]),
                straight: false,
            });
            return;
        }
        if self.tool.drags_a_box() {
            self.drag = Some(Drag {
                what: Carrying::NewShape,
                page,
                from: at,
                to: at,
                started_at: Quad::of([0.0; 4]),
                straight: false,
            });
            return;
        }
        if self.tool.is_freehand() {
            let drag = Drag {
                what: Carrying::NewLine,
                page,
                from: at,
                to: at,
                started_at: Quad::of([0.0; 4]),
                straight: false,
            };
            self.start_the_ink(&drag);
            self.drag = Some(drag);
            return;
        }
        if self.tool == Tool::Picture {
            self.drag = Some(Drag {
                what: Carrying::NewPicture,
                page,
                from: at,
                to: at,
                started_at: Quad::of([0.0; 4]),
                straight: false,
            });
            return;
        }
        if self.tool == Tool::Text {
            self.drag = Some(Drag {
                what: Carrying::NewText,
                page,
                from: at,
                to: at,
                started_at: Quad::of([0.0; 4]),
                straight: false,
            });
            return;
        }
        if let Some(taken) = self.handle_held(page, point) {
            self.drag = Some(Drag {
                what: taken.0,
                page,
                from: at,
                to: at,
                started_at: taken.1,
                straight: false,
            });
            return;
        }
        if self.take_hold_of_the_text(page, point, at) {
            return;
        }
        let Some(overlay) = self.overlay(page) else {
            self.drag = None;
            return;
        };
        let quads: Vec<Quad> = overlay
            .blocks
            .iter()
            .map(|block| Quad::from_pixels(block.quad))
            .collect();
        if let Some(index) = standing_block_at(
            &quads,
            |index| self.pointing.block_stands(overlay, page, index),
            point,
        ) {
            let Some(block) = overlay.blocks.get(index) else {
                return;
            };
            self.drag = Some(Drag {
                what: Carrying::Block(block.anchors.clone()),
                page,
                from: at,
                to: at,
                started_at: quads[index],
                straight: false,
            });
            self.point_at(Pointing::Block { page, block: index });
            return;
        }
        if self.take_hold_of_a_picture(page, point, at) {
            return;
        }
        let Some(overlay) = self.overlay(page) else {
            return;
        };
        self.drag = Some(
            run_at(&overlay.runs, point)
                .and_then(|index| overlay.runs.get(index))
                .map_or(
                    Drag {
                        what: Carrying::Marquee,
                        page,
                        from: at,
                        to: at,
                        started_at: Quad::of([0.0; 4]),
                        straight: false,
                    },
                    |run| Drag {
                        what: Carrying::Run(run.anchor.clone()),
                        page,
                        from: at,
                        to: at,
                        started_at: Quad::of(run.bounds),
                        straight: false,
                    },
                ),
        );
    }

    #[expect(clippy::too_many_lines, reason = "one release, one arm per drag")]
    fn let_go(&mut self) {
        let Some(drag) = self.drag.take() else { return };
        if matches!(drag.what, Carrying::Selection { .. }) {
            return;
        }
        if let Carrying::Handle { frame, .. } = drag.what {
            self.editor
                .finish_frame_resize(drag.page, frame, drag.started_at.bounds());
            if let Some(at) = self.frames_of(drag.page).get(frame).copied() {
                let said = Message::FrameDeclared {
                    wide: at[2] - at[0],
                    high: at[3] - at[1],
                };
                self.editor.say(said);
            }
            return;
        }
        if matches!(drag.what, Carrying::NewText) {
            self.take_the_frame_drawn(&drag);
            return;
        }
        if matches!(drag.what, Carrying::NewPicture) {
            self.place_the_picture(&drag);
            return;
        }
        if matches!(drag.what, Carrying::NewLine) {
            self.take_the_ink_drawn();
            return;
        }
        if matches!(drag.what, Carrying::NewShape) {
            self.take_the_shape_drawn(&drag);
            return;
        }
        if matches!(drag.what, Carrying::NewField) {
            self.take_the_field_drawn(&drag);
            return;
        }
        if matches!(drag.what, Carrying::NewLink) {
            self.take_the_link_drawn(&drag);
            return;
        }
        if matches!(drag.what, Carrying::MovingLinks { .. }) {
            self.take_the_link_moved(&drag);
            return;
        }
        if matches!(drag.what, Carrying::MovingField { .. }) {
            self.take_the_field_moved(&drag);
            return;
        }
        if matches!(drag.what, Carrying::FieldSweep { .. }) {
            self.take_the_fields_swept(&drag);
            return;
        }
        let Some((_, (dx, dy), proportions)) = self.asked_for(&drag) else {
            return;
        };
        if dx.abs() < 0.5 && dy.abs() < 0.5 {
            return;
        }
        let (job, target) = match &drag.what {
            Carrying::Block(anchors) => {
                let group = self.chosen.is_a_group() && self.chosen.page == drag.page;
                let job = if group {
                    self.begin_group_move(drag.page, (dx, dy))
                } else {
                    self.editor.begin_move_block_pointed(
                        drag.page,
                        anchors,
                        self.pointing.block(),
                        (dx, dy),
                    )
                };
                (job, self.pointing.block().map(|block| (drag.page, block)))
            }
            Carrying::Run(anchor) => (self.editor.begin_move(drag.page, anchor, dx, dy), None),
            Carrying::Object(_) if self.chosen.is_a_group() && self.chosen.page == drag.page => {
                (self.begin_group_move(drag.page, (dx, dy)), None)
            }
            Carrying::Object(anchor) => {
                self.reselect_object = Some((drag.page, drag.started_at.shifted(dx, dy)));
                (self.editor.begin_place(drag.page, anchor, dx, dy), None)
            }
            Carrying::ObjectHandle { anchor, handle } => {
                let Some((matrix, about)) =
                    shaped(&drag.started_at, *handle, (dx, dy), proportions)
                else {
                    return;
                };
                self.reselect_object =
                    Some((drag.page, drag.started_at.transformed(matrix, about)));
                (
                    self.editor.begin_shape(drag.page, anchor, matrix, about),
                    None,
                )
            }
            Carrying::BlockTurn(anchors) => {
                let Some((matrix, about)) =
                    shaped(&drag.started_at, ROTATE_HANDLE, (dx, dy), proportions)
                else {
                    return;
                };
                (
                    self.editor
                        .begin_shape_block(drag.page, anchors, matrix, about),
                    self.pointing.block().map(|block| (drag.page, block)),
                )
            }
            Carrying::Marquee => {
                self.take_what_was_swept(&drag);
                return;
            }
            Carrying::NewText
            | Carrying::NewPicture
            | Carrying::NewLine
            | Carrying::NewShape
            | Carrying::NewField
            | Carrying::NewLink
            | Carrying::MovingLinks { .. }
            | Carrying::MovingField { .. }
            | Carrying::FieldSweep { .. }
            | Carrying::Selection { .. }
            | Carrying::Handle { .. }
            | Carrying::Panning { .. } => {
                return;
            }
        };
        if job.is_some() {
            self.reselect = target;
            self.landing = self.going(&drag).map(|going| Landing {
                page: drag.page,
                source: drag.started_at,
                going,
            });
        }
        self.send(job);
    }

    fn hold_still_hint(&mut self, ctx: &egui::Context, response: &egui::Response) {
        let now = ctx.input(|input| input.time);
        let Some(at) = response
            .interact_pointer_pos()
            .filter(|_| response.is_pointer_button_down_on())
        else {
            self.held_still = None;
            return;
        };
        match self.held_still {
            Some((was, _)) if (at - was).length() > STILL_ENOUGH => {
                self.held_still = Some((at, now));
                return;
            }
            Some((_, since)) if now - since >= HOLD_FOR => {}
            Some(_) => return,
            None => {
                self.held_still = Some((at, now));
                return;
            }
        }
        if self.drag.is_some() {
            return;
        }
        let Some((page, point)) = self.page_point(at) else {
            return;
        };
        let page_count = self.editor.page_count();
        let link = self
            .editor
            .link_at(page, point)
            .map(|link| pdf_app::links::describe(&link, page_count));
        let Some(says) = link.or_else(|| self.what_is_at(page, point)) else {
            return;
        };
        let says = says.say(self.lang);
        egui::Area::new(egui::Id::new("held still"))
            .order(egui::Order::Foreground)
            .fixed_pos(at + egui::vec2(12.0, 18.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| ui.label(says));
            });
    }

    fn what_is_at(&self, page: usize, point: (f64, f64)) -> Option<Message> {
        let overlay = self.overlay(page)?;
        let quads: Vec<Quad> = overlay
            .blocks
            .iter()
            .map(|block| Quad::from_pixels(block.quad))
            .collect();
        if let Some(index) = standing_block_at(
            &quads,
            |index| self.pointing.block_stands(overlay, page, index),
            point,
        ) {
            let block = overlay.blocks.get(index)?;
            return Some(Message::ParagraphOf {
                rows: block.lines.len(),
                runs: block.anchors.len(),
            });
        }
        let index = object_at(&overlay.objects, point)?;
        let object = overlay.objects.get(index)?;
        let bounds = Quad::from_pixels(object.quad).bounds();
        Some(Message::ObjectSized {
            kind: kind_name(object.kind),
            wide: bounds[2] - bounds[0],
            high: bounds[3] - bounds[1],
        })
    }

    fn take_what_was_swept(&mut self, drag: &Drag) {
        let travel = drag.to - drag.from;
        let Some(laid) = self
            .laid
            .iter()
            .copied()
            .find(|laid| laid.page == drag.page)
        else {
            return;
        };
        if f64::from(travel.x.abs().max(travel.y.abs())) < pdf_app::view::SWEEP_ENOUGH {
            self.point_at(Pointing::Nothing);
            return;
        }
        let (Some(from), Some(to)) = (
            laid.placed.page_point((drag.from.x, drag.from.y)),
            laid.placed.page_point((drag.to.x, drag.to.y)),
        ) else {
            return;
        };
        let sweep = pdf_app::view::sweep_box(from, to);
        let Some(overlay) = self.overlay(drag.page) else {
            return;
        };
        let blocks: Vec<Quad> = overlay
            .blocks
            .iter()
            .map(|block| Quad::from_pixels(block.quad))
            .collect();
        let objects: Vec<Quad> = overlay
            .objects
            .iter()
            .map(|object| Quad::from_pixels(object.quad))
            .collect();
        let (mut blocks, objects) = (
            pdf_app::view::swept(&blocks, sweep),
            pdf_app::view::swept(&objects, sweep),
        );
        blocks.retain(|block| self.block_is_there(drag.page, *block));
        if blocks.is_empty() && objects.is_empty() {
            self.point_at(Pointing::Nothing);
            return;
        }
        let caught = blocks.len() + objects.len();
        self.choose(drag.page, blocks, objects);
        let said = Message::Selected(caught);
        self.editor.say(said);
    }

    fn take_the_frame_drawn(&mut self, drag: &Drag) {
        self.tool = Tool::Select;
        let Some(laid) = self
            .laid
            .iter()
            .copied()
            .find(|laid| laid.page == drag.page)
        else {
            return;
        };
        let (Some(from), Some(to)) = (
            laid.placed.page_point((drag.from.x, drag.from.y)),
            laid.placed.page_point((drag.to.x, drag.to.y)),
        ) else {
            return;
        };
        let travel = drag.to - drag.from;
        let drawn = f64::from(travel.x.abs().max(travel.y.abs())) >= pdf_app::view::SWEEP_ENOUGH;
        let pixels = if drawn {
            [
                from.0.min(to.0),
                from.1.min(to.1),
                from.0.max(to.0),
                from.1.max(to.1),
            ]
        } else {
            [
                from.0,
                from.1,
                from.0 + DRAWN_TEXT_WIDTH,
                from.1 + NEW_TEXT_SIZE * 1.2,
            ]
        };
        let Some(frame) = self.editor.frame_in_user_space(drag.page, pixels) else {
            return;
        };
        self.point_at(Pointing::Nothing);
        self.text_draft = Some(TextDraft {
            page: drag.page,
            frame,
            family: NEW_TEXT_FAMILY.to_owned(),
            size: NEW_TEXT_SIZE,
            bold: false,
            italic: false,
            fill: None,
            alignment: pdf_edit::Alignment::Start,
            flows_round: false,
        });
        self.draft_pixels = Some((drag.page, pixels));
        self.editor.say(Message::TypeHereToAddText);
    }

    pub(crate) fn show_the_sweep(painter: &egui::Painter, drag: &Drag) {
        let blue = egui::Color32::from_rgb(0, 90, 200);
        let area = egui::Rect::from_two_pos(drag.from, drag.to);
        painter.rect_filled(
            area,
            0.0,
            egui::Color32::from_rgba_unmultiplied(0, 90, 200, 14),
        );
        painter.rect_stroke(
            area,
            0.0,
            egui::Stroke::new(1.0, blue),
            egui::StrokeKind::Inside,
        );
    }

    fn sweep(&mut self, page: usize, block: usize, from: egui::Pos2, to: egui::Pos2) {
        let Some(laid) = self.laid.iter().copied().find(|laid| laid.page == page) else {
            return;
        };
        let (Some(start), Some(end)) = (
            laid.placed.page_point((from.x, from.y)),
            laid.placed.page_point((to.x, to.y)),
        ) else {
            return;
        };
        let Some(rows) = self.rows_of(page, block) else {
            return;
        };
        let Some(overlay) = self.overlay(page) else {
            return;
        };
        let (Some(anchor), Some(at)) = (
            caret_at_in(&overlay.carets, &rows, start),
            caret_at_in(&overlay.carets, &rows, end),
        ) else {
            return;
        };
        self.pointing = Pointing::Text {
            page,
            block,
            caret: Caret { at, anchor },
        };
    }

    fn carry(&self, ctx: &egui::Context, drag: &Drag, clip: egui::Rect) {
        if matches!(
            drag.what,
            Carrying::Marquee
                | Carrying::NewText
                | Carrying::NewPicture
                | Carrying::NewLine
                | Carrying::NewShape
                | Carrying::NewField
                | Carrying::NewLink
                | Carrying::MovingLinks { .. }
                | Carrying::MovingField { .. }
                | Carrying::FieldSweep { .. }
        ) {
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("marquee"),
            ));
            if matches!(drag.what, Carrying::NewPicture) {
                self.show_the_picture_box(&painter.with_clip_rect(clip), drag);
                return;
            }
            if matches!(drag.what, Carrying::NewLine) {
                self.show_the_ink(&painter.with_clip_rect(clip), drag);
                return;
            }
            if matches!(drag.what, Carrying::NewShape) {
                self.show_the_shape(&painter.with_clip_rect(clip), drag);
                return;
            }
            if matches!(drag.what, Carrying::NewField) {
                self.show_the_field_box(&painter.with_clip_rect(clip), drag);
                return;
            }
            if matches!(drag.what, Carrying::NewLink) {
                self.show_the_link_box(&painter.with_clip_rect(clip), drag);
                return;
            }
            if matches!(drag.what, Carrying::MovingLinks { .. }) {
                self.show_the_link_moved(&painter.with_clip_rect(clip), drag);
                return;
            }
            if matches!(drag.what, Carrying::MovingField { .. }) {
                self.show_the_field_moved(&painter.with_clip_rect(clip), drag);
                return;
            }
            if matches!(drag.what, Carrying::FieldSweep { .. }) {
                Self::show_the_sweep(&painter.with_clip_rect(clip), drag);
                return;
            }
            Self::show_the_sweep(&painter.with_clip_rect(clip), drag);
            return;
        }
        let Some(going) = self.going(drag) else {
            return;
        };
        self.show_the_move(ctx, clip, drag.page, drag.started_at, going);
    }

    fn asked_for(&self, drag: &Drag) -> Option<(Laid, (f64, f64), Proportions)> {
        let laid = self
            .laid
            .iter()
            .copied()
            .find(|laid| laid.page == drag.page)?;
        let travel = drag.to - drag.from;
        let (dx, dy) = (
            f64::from(travel.x / laid.placed.stretch),
            f64::from(travel.y / laid.placed.stretch),
        );
        let corner = matches!(
            drag.what,
            Carrying::ObjectHandle { handle, .. }
                if handle <= 3
        );
        let proportions = if drag.straight && corner {
            Proportions::Free
        } else {
            Proportions::Kept
        };
        let handle = matches!(
            drag.what,
            Carrying::ObjectHandle { .. } | Carrying::BlockTurn(_)
        );
        let travel = if drag.straight && !handle {
            pdf_app::view::along_one_axis((dx, dy))
        } else {
            (dx, dy)
        };
        Some((laid, travel, proportions))
    }

    fn going(&self, drag: &Drag) -> Option<Quad> {
        let (_, (dx, dy), proportions) = self.asked_for(drag)?;
        let handle = match &drag.what {
            Carrying::ObjectHandle { handle, .. } => Some(*handle),
            Carrying::BlockTurn(_) => Some(ROTATE_HANDLE),
            _ => None,
        };
        if let Some(handle) = handle {
            let (matrix, about) = shaped(&drag.started_at, handle, (dx, dy), proportions)?;
            return Some(drag.started_at.transformed(matrix, about));
        }
        Some(drag.started_at.shifted(dx, dy))
    }

    fn show_landing(&self, ctx: &egui::Context, clip: egui::Rect) {
        let Some(landing) = &self.landing else { return };
        self.show_the_move(ctx, clip, landing.page, landing.source, landing.going);
    }

    fn show_the_move(
        &self,
        ctx: &egui::Context,
        clip: egui::Rect,
        page: usize,
        source: Quad,
        going: Quad,
    ) {
        let Some(laid) = self.laid.iter().copied().find(|laid| laid.page == page) else {
            return;
        };
        let from = quad_on_screen(laid.placed, &frame_quad(&source));
        let painter = ctx
            .layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("carried"),
            ))
            .with_clip_rect(clip);
        stroke_quad(&painter, from, 1.0, egui::Color32::from_rgb(160, 160, 160));
        let to = quad_on_screen(laid.placed, &frame_quad(&going));
        fill_quad(
            &painter,
            to,
            egui::Color32::from_rgba_unmultiplied(0, 90, 200, 40),
        );
        stroke_quad(&painter, to, 1.5, egui::Color32::from_rgb(0, 90, 200));
    }

    fn record_keys(&mut self, ctx: &egui::Context) {
        if self.trace.is_none() {
            return;
        }
        let pressed: Vec<String> = ctx.input(|input| {
            input
                .events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        repeat,
                        modifiers,
                        ..
                    } => Some(format!(
                        "{key:?}{}{}",
                        if modifiers.any() {
                            format!(" +{modifiers:?}")
                        } else {
                            String::new()
                        },
                        if *repeat { " repeat" } else { "" }
                    )),
                    _ => None,
                })
                .collect()
        });
        let context = self.input_context();
        let frame = self.frame;
        for key in pressed {
            if let Some(trace) = self.trace.as_mut() {
                trace.event(frame, "key", &key, &context);
            }
        }
    }

    pub(crate) fn keys(&mut self, ctx: &egui::Context) {
        self.record_keys(ctx);
        self.record_text_events(ctx);
        let presses = ctx.input(arrow_presses);
        if !ctx.egui_wants_keyboard_input() && self.tool == Tool::Link {
            self.link_tool_keys(ctx);
        } else if !ctx.egui_wants_keyboard_input() && self.tool == Tool::Form {
            self.form_tool_keys(ctx, &presses);
        } else if !ctx.egui_wants_keyboard_input() {
            let in_text = self.pointing.editing()
                || self.resume.is_some()
                || self.input.pending()
                || self.text_draft.is_some()
                || self.entering.is_some();
            for key in ctx.input(|input| typed_keys(&input.events)) {
                self.take_key(ctx, key, in_text);
            }
            self.leave(ctx);
            self.walk_the_page(ctx);
            self.nudge(&presses);
        }
        self.history(ctx);
        self.pump();
        if ctx.input(|input| input.key_pressed(egui::Key::F2)) {
            self.show_frames = !self.show_frames;
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
            self.open_the_find_bar();
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::H)) {
            self.open_the_replace_row();
        }
        if self.finding.is_some() {
            let (next, back) = ctx.input_mut(|input| {
                (
                    input.consume_key(egui::Modifiers::NONE, egui::Key::F3)
                        || input.consume_key(egui::Modifiers::COMMAND, egui::Key::G),
                    input.consume_key(egui::Modifiers::SHIFT, egui::Key::F3)
                        || input.consume_key(
                            egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT),
                            egui::Key::G,
                        ),
                )
            });
            if next || back {
                self.go_to_a_hit(next);
            }
        }
        self.keep_searching();
        self.keep_replacing();
        let (closer, further, reset) = ctx.input(|input| {
            let held = input.modifiers.command;
            (
                held && (input.key_pressed(egui::Key::Plus)
                    || input.key_pressed(egui::Key::Equals)),
                held && input.key_pressed(egui::Key::Minus),
                held && input.key_pressed(egui::Key::Num0),
            )
        });
        if closer || further || reset {
            let pointer = self.pointer_in_view(ctx);
            if closer {
                self.zoom_by(true, pointer);
            }
            if further {
                self.zoom_by(false, pointer);
            }
            if reset {
                self.zoom_to(1.0, pointer);
            }
        }
        self.page_keys(ctx);
        let (wheel, pointer) = ctx.input(|input| {
            let zoom = input.zoom_delta();
            (
                (zoom - 1.0).abs().gt(&1e-3).then_some(zoom),
                input.pointer.hover_pos(),
            )
        });
        if let Some(zoom) = wheel {
            let pointer = pointer.map(|at| at - self.view_corner).filter(|at| {
                at.x >= 0.0 && at.y >= 0.0 && at.x < self.view.x && at.y < self.view.y
            });
            self.zoom_to(self.zoom * f64::from(zoom), pointer);
        }
    }

    fn page_keys(&mut self, ctx: &egui::Context) {
        let turn = ctx.input(|input| {
            if input.key_pressed(egui::Key::PageDown) {
                Some(1_isize)
            } else if input.key_pressed(egui::Key::PageUp) {
                Some(-1)
            } else {
                None
            }
        });
        let Some(turn) = turn else { return };
        let next = if turn > 0 {
            self.focus.checked_add(1)
        } else {
            self.focus.checked_sub(1)
        };
        if let Some(next) = next.filter(|next| *next < self.editor.page_count()) {
            self.goto(next);
        }
    }

    fn leave(&mut self, ctx: &egui::Context) {
        if !ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            return;
        }
        if self.drag.take().is_some() {
            return;
        }
        if self.finding.is_some() {
            self.close_the_find_bar();
            return;
        }
        if self.context.take().is_some() {
            return;
        }
        if self.text_draft.is_some() {
            self.drop_the_text_draft();
            return;
        }
        if self.tool != Tool::Select {
            self.tool = Tool::Select;
            self.pictures.clear();
            self.ink = None;
            return;
        }
        match self.pointing {
            Pointing::Text { page, block, .. } => {
                self.point_at(Pointing::Block { page, block });
                self.input.set_aside(&Message::LeftTheTextUnderDraft);
            }
            Pointing::Block { .. } | Pointing::Object { .. } => {
                self.point_at(Pointing::Nothing);
            }
            Pointing::Nothing => {}
        }
    }

    fn walk_the_page(&mut self, ctx: &egui::Context) {
        if self.pointing.editing()
            || !ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Tab))
        {
            return;
        }
        let page = self.pointing.page().unwrap_or(self.focus);
        let Some(overlay) = self.overlay(page) else {
            return;
        };
        let anchors: Vec<String> = overlay
            .blocks
            .iter()
            .map(|block| block.anchors.first().cloned().unwrap_or_default())
            .chain(overlay.objects.iter().map(|object| object.anchor.clone()))
            .collect();
        let blocks = overlay.blocks.len();
        let mut order = pdf_app::view::in_file_order(&anchors);
        order.retain(|at| *at >= blocks || self.block_is_there(page, *at));
        let from = match self.pointing {
            Pointing::Block { page: at, block }
            | Pointing::Text {
                page: at, block, ..
            } if at == page => Some(block),
            Pointing::Object { page: at, object } if at == page => Some(blocks + object),
            _ => None,
        };
        let Some(next) = pdf_app::view::next_in_order(&order, from) else {
            return;
        };
        self.point_at(if next < blocks {
            Pointing::Block { page, block: next }
        } else {
            Pointing::Object {
                page,
                object: next - blocks,
            }
        });
    }

    fn nudge(&mut self, presses: &[ArrowPress]) {
        if self.pointing.editing() || self.editor.is_busy() {
            return;
        }
        let travel = nudge_travel(presses);
        if travel == (0.0, 0.0) {
            return;
        }
        self.move_selection((travel.0 * OVERLAY_SCALE, travel.1 * OVERLAY_SCALE));
    }
}

fn arrow_presses(input: &egui::InputState) -> Vec<ArrowPress> {
    input
        .events
        .iter()
        .filter_map(|event| match event {
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                let step = match key {
                    egui::Key::ArrowLeft => Step::Left,
                    egui::Key::ArrowRight => Step::Right,
                    egui::Key::ArrowUp => Step::Up,
                    egui::Key::ArrowDown => Step::Down,
                    _ => return None,
                };
                Some(ArrowPress {
                    step,
                    shift: modifiers.shift,
                })
            }
            _ => None,
        })
        .collect()
}

fn typed_keys(events: &[egui::Event]) -> Vec<TypedKey> {
    events
        .iter()
        .filter_map(|event| match event {
            egui::Event::Text(text)
            | egui::Event::Paste(text)
            | egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                let text = text.replace("\r\n", "\n").replace('\r', "\n");
                (!text.is_empty()).then_some(TypedKey::Intent(Intent::Insert(text)))
            }
            egui::Event::Copy => Some(TypedKey::Copy),
            egui::Event::Cut => Some(TypedKey::Cut),
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                let caret = |step| {
                    Some(TypedKey::Intent(Intent::Caret {
                        step,
                        extend: modifiers.shift,
                    }))
                };
                match key {
                    egui::Key::Enter if modifiers.shift => Some(TypedKey::Intent(Intent::Insert(
                        pdf_edit::LINE_BREAK.to_string(),
                    ))),
                    egui::Key::Enter => Some(TypedKey::Intent(Intent::Insert("\n".to_owned()))),
                    egui::Key::Backspace => Some(TypedKey::Intent(Intent::Delete {
                        backwards: true,
                        count: 1,
                    })),
                    egui::Key::Delete => Some(TypedKey::Intent(Intent::Delete {
                        backwards: false,
                        count: 1,
                    })),
                    egui::Key::ArrowLeft => caret(Step::Left),
                    egui::Key::ArrowRight => caret(Step::Right),
                    egui::Key::ArrowUp => caret(Step::Up),
                    egui::Key::ArrowDown => caret(Step::Down),
                    egui::Key::Home => caret(Step::RowStart),
                    egui::Key::End => caret(Step::RowEnd),
                    egui::Key::A if modifiers.command => Some(TypedKey::SelectAll),
                    _ => None,
                }
            }
            _ => None,
        })
        .collect()
}

pub(crate) fn nudge_travel(presses: &[ArrowPress]) -> (f64, f64) {
    let mut travel = (0.0_f64, 0.0_f64);
    for press in presses {
        let far = if press.shift { FAR_STEP } else { NEAR_STEP };
        let (dx, dy): (f64, f64) = match press.step {
            Step::Left => (-1.0, 0.0),
            Step::Right => (1.0, 0.0),
            Step::Up => (0.0, -1.0),
            Step::Down => (0.0, 1.0),
            Step::RowStart | Step::RowEnd => (0.0, 0.0),
        };
        travel = (dx.mul_add(far, travel.0), dy.mul_add(far, travel.1));
    }
    travel
}

pub(crate) fn history_keys(input: &mut egui::InputState) -> (bool, bool) {
    let shifted = input.consume_key(
        egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
        egui::Key::Z,
    );
    let forward = input.consume_key(egui::Modifiers::COMMAND, egui::Key::Y) || shifted;
    let back = input.consume_key(egui::Modifiers::COMMAND, egui::Key::Z);
    (back, forward)
}

#[cfg(test)]
mod shortcut_tests {
    use crate::window_state::{ArrowPress, TypedKey};
    use pdf_app::view::Step;

    use super::{Intent, arrow_presses, egui, history_keys, nudge_travel, typed_keys};

    #[test]
    fn text_enter_deletes_and_caret_keys_keep_the_order_of_their_events() {
        let key = |key, modifiers| egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: true,
            modifiers,
        };
        let events = vec![
            egui::Event::Text("a".to_owned()),
            key(egui::Key::Enter, egui::Modifiers::NONE),
            egui::Event::Text("b".to_owned()),
            key(egui::Key::Backspace, egui::Modifiers::NONE),
            key(egui::Key::Backspace, egui::Modifiers::NONE),
            key(egui::Key::Home, egui::Modifiers::SHIFT),
            key(egui::Key::Enter, egui::Modifiers::SHIFT),
            egui::Event::Paste("x\r\ny".to_owned()),
            egui::Event::Key {
                key: egui::Key::Delete,
                physical_key: Some(egui::Key::Delete),
                pressed: false,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
            key(egui::Key::F5, egui::Modifiers::NONE),
        ];
        let delete = TypedKey::Intent(Intent::Delete {
            backwards: true,
            count: 1,
        });
        assert_eq!(
            typed_keys(&events),
            vec![
                TypedKey::Intent(Intent::Insert("a".to_owned())),
                TypedKey::Intent(Intent::Insert("\n".to_owned())),
                TypedKey::Intent(Intent::Insert("b".to_owned())),
                delete.clone(),
                delete,
                TypedKey::Intent(Intent::Caret {
                    step: Step::RowStart,
                    extend: true
                }),
                TypedKey::Intent(Intent::Insert(pdf_edit::LINE_BREAK.to_string())),
                TypedKey::Intent(Intent::Insert("x\ny".to_owned())),
            ]
        );
    }

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn arrow_steps_come_from_each_events_own_modifiers() {
        let mut input = egui::InputState::default();
        input.events = (0..4)
            .map(|_| key(egui::Key::ArrowRight, egui::Modifiers::SHIFT))
            .collect();
        assert!(!input.modifiers.shift);
        let presses = arrow_presses(&input);
        assert_eq!(presses.len(), 4);
        assert!(presses.iter().all(|press| press.shift));
        assert_eq!(nudge_travel(&presses), (40.0, 0.0));

        let near = [ArrowPress {
            step: Step::Right,
            shift: false,
        }];
        assert_eq!(nudge_travel(&near), (1.0, 0.0));
        assert_eq!(
            nudge_travel(&[
                ArrowPress {
                    step: Step::Right,
                    shift: true
                },
                ArrowPress {
                    step: Step::Right,
                    shift: false
                },
            ]),
            (11.0, 0.0)
        );

        let mut input = egui::InputState::default();
        input.events = vec![
            key(egui::Key::ArrowRight, egui::Modifiers::SHIFT),
            key(egui::Key::ArrowDown, egui::Modifiers::NONE),
            key(egui::Key::ArrowUp, egui::Modifiers::SHIFT),
        ];
        assert_eq!(
            arrow_presses(&input),
            [
                ArrowPress {
                    step: Step::Right,
                    shift: true
                },
                ArrowPress {
                    step: Step::Down,
                    shift: false
                },
                ArrowPress {
                    step: Step::Up,
                    shift: true
                },
            ]
        );
        assert_eq!(nudge_travel(&arrow_presses(&input)), (10.0, -9.0));

        let mut input = egui::InputState::default();
        input.events = vec![
            key(egui::Key::ArrowLeft, egui::Modifiers::SHIFT),
            key(egui::Key::ArrowRight, egui::Modifiers::SHIFT),
            key(egui::Key::Z, egui::Modifiers::COMMAND),
            egui::Event::Key {
                key: egui::Key::ArrowRight,
                physical_key: Some(egui::Key::ArrowRight),
                pressed: false,
                repeat: false,
                modifiers: egui::Modifiers::SHIFT,
            },
        ];
        assert_eq!(arrow_presses(&input).len(), 2);
        assert_eq!(nudge_travel(&arrow_presses(&input)), (0.0, 0.0));
        assert_eq!(nudge_travel(&[]), (0.0, 0.0));
    }

    #[test]
    fn history_uses_the_key_events_modifiers_even_after_ctrl_was_released() {
        for (key, modifiers, expected) in [
            (egui::Key::Z, egui::Modifiers::COMMAND, (true, false)),
            (egui::Key::Y, egui::Modifiers::COMMAND, (false, true)),
            (
                egui::Key::Z,
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                (false, true),
            ),
            (egui::Key::Z, egui::Modifiers::NONE, (false, false)),
        ] {
            let mut input = egui::InputState::default();
            input.events = vec![egui::Event::Key {
                key,
                physical_key: Some(key),
                pressed: true,
                repeat: false,
                modifiers,
            }];
            assert!(
                !input.modifiers.command,
                "Ctrl is already released at frame end"
            );
            assert_eq!(history_keys(&mut input), expected);
            assert_eq!(
                history_keys(&mut input),
                (false, false),
                "one event is consumed once"
            );
        }
    }
}
