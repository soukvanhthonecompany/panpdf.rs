use std::collections::BTreeMap;
use std::sync::Arc;

use eframe::egui;

use pdf_app::document::OVERLAY_SCALE;

const NEW_TEXT_LINE: f64 = 12.0 * 1.2;
use pdf_app::strip::{tile_box, tiles_over};
use pdf_app::tiles::{Held, Slot, TileId};
use pdf_app::view::{
    Placement, Quad, ROTATE_HANDLE, dashes, frame_quad, handles, rotate_stem, selection_quad,
    selection_rows, shown_caret, text_handles,
};
use pdf_app::wording::Message;

use crate::window_state::{
    AHEAD, COARSE, KEEP_PAGE_BYTES, KEEP_PAGES, Laid, Pointing, SPARE_TEXTURES, Scene, Tool,
    Window, ZOOMS,
};

pub(crate) const DESK_MARGIN: f32 = 16.0;

impl Window {
    pub(crate) fn retire_stale(&mut self, page: usize, region: Option<[f64; 4]>) {
        let epoch = self.editor.epoch();
        let touched: Option<BTreeMap<usize, [u32; 4]>> = region.map(|region| {
            ZOOMS
                .iter()
                .enumerate()
                .filter_map(|(rung, scale)| {
                    Some((rung, self.editor.region_in_pixels(page, *scale, region)?))
                })
                .collect()
        });
        let showing = self.rung();
        for (id, held, slot) in self.tiles.commit(page, touched.as_ref(), epoch, showing) {
            self.retire(id, held, slot);
        }
    }

    fn present(&mut self, laid: Laid, band: egui::Rect) {
        let visible: std::collections::BTreeSet<TileId> = self
            .fine_tiles(laid, band)
            .into_iter()
            .map(|(id, _, _)| id)
            .collect();
        let Some(swap) = self.tiles.swap_if_ready(laid.page, &visible) else {
            return;
        };
        for id in &swap.shown {
            if let Some(texture) = self.waiting_textures.remove(id)
                && let Some(old) = self.textures.insert(*id, texture)
            {
                self.keep_spare(old);
            }
        }
        let dropped = swap.dropped.len();
        for (id, held) in swap.dropped {
            self.retire(id, held, Slot::Shown);
        }
        let (frame, epoch) = (self.frame, self.editor.epoch());
        if let Some(trace) = self.trace.as_mut() {
            trace.note(
                frame,
                "swap",
                &format!(
                    "page={} epoch={epoch} shown={} dropped={dropped}",
                    laid.page,
                    swap.shown.len()
                ),
            );
        }
    }

    pub(crate) fn retire(&mut self, id: TileId, _held: Held, slot: Slot) {
        let texture = match slot {
            Slot::Shown => self.textures.remove(&id),
            Slot::Waiting => self.waiting_textures.remove(&id),
        };
        if let Some(texture) = texture {
            self.keep_spare(texture);
        }
    }

    pub(crate) fn keep_spare(&mut self, texture: egui::TextureHandle) {
        if self.spare.values().map(Vec::len).sum::<usize>() >= SPARE_TEXTURES {
            return;
        }
        let [width, height] = texture.size();
        self.spare.entry((width, height)).or_default().push(texture);
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the page loop and the glass over it, in the order they are drawn"
    )]
    pub(crate) fn document_area(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let zoom = self.zoom;
        let (strip_width, strip_height) = self.editor.strip().size();
        #[allow(clippy::cast_possible_truncation)]
        let content = egui::vec2((strip_width * zoom) as f32, (strip_height * zoom) as f32);
        let room = ui.available_size();
        let area = egui::vec2(
            content.x.max(room.x),
            (content.y + 2.0 * DESK_MARGIN).max(room.y),
        );
        let (whole, response) = ui.allocate_exact_size(area, egui::Sense::click_and_drag());
        let rect = egui::Rect::from_min_size(
            whole.min + egui::vec2((area.x - content.x) / 2.0, DESK_MARGIN),
            content,
        );
        let clip = ui.clip_rect().intersect(whole);
        let painter = ui.painter_at(clip);

        let band = clip.expand(AHEAD);
        let top = f64::from(band.min.y - rect.min.y) / zoom;
        let bottom = f64::from(band.max.y - rect.min.y) / zoom;
        let pages = self.editor.strip().between(top.max(0.0), bottom.max(0.0));

        self.laid.clear();
        let mut wanted: Vec<(f32, TileId, [u32; 4], f64)> = Vec::new();
        let mut drawn: Vec<TileId> = Vec::new();
        let middle = clip.center();
        let painting = crate::moment::Moment::now();
        for page in pages.clone() {
            let Some(laid) = self.lay_out(page, rect, zoom) else {
                continue;
            };
            self.laid.push(laid);
            self.present(laid, band);
            self.paint_page(&painter, laid, band, middle, &mut wanted, &mut drawn);
            self.paint_live(ctx, &painter, laid);
        }
        let now = self.frame;
        let (pages_drawn, tiles_drawn, tiles_wanted) = (self.laid.len(), drawn.len(), wanted.len());
        for id in drawn {
            self.tiles.touched_on(id, now);
        }
        let asking = crate::moment::Moment::now();
        self.ask_for(wanted);
        if let Some(meter) = self.meter.as_mut() {
            meter.span("pages", asking.since(painting));
            meter.span("ask", asking.elapsed());
            meter.count("pages", pages_drawn);
            meter.count("drawn", tiles_drawn);
            meter.count("wanted", tiles_wanted);
        }
        self.focus = self
            .laid
            .iter()
            .max_by(|one, other| {
                let seen = |laid: &Laid| laid.rect.intersect(clip).height();
                seen(one).total_cmp(&seen(other))
            })
            .map_or(self.focus, |laid| laid.page);

        let on_screen: Vec<usize> = self.laid.iter().map(|laid| laid.page).collect();
        self.scenes.retain(|page, _| on_screen.contains(page));
        let glass = ui.painter_at(ui.clip_rect());
        let overlaying = crate::moment::Moment::now();
        if self.show_frames {
            for laid in self.laid.clone() {
                let Some(scene) = self.scene_for(laid.page) else {
                    continue;
                };
                if !self.tool.draws_on_the_page() {
                    if self.framed.text {
                        Self::blocks_overlay(&glass, laid, &scene);
                    }
                    Self::objects_overlay(&glass, laid, &scene, self.framed);
                }
                if !self.live_frame(&glass, laid) {
                    Self::selected_block(&glass, laid, &scene);
                }
                self.caret_overlay(&glass, laid, &scene);
            }
        }
        for laid in self.laid.clone() {
            self.fields_on_the_page(&glass, laid);
            self.links_drawn_on_the_page(&glass, laid);
            if self.tool == Tool::Link {
                self.links_as_the_link_tool_sees_them(&glass, laid);
            }
            self.marks_on_the_page(&glass, laid);
            self.stamp_on_the_page(&glass, laid);
        }
        self.frame_drawn_for_text(&glass);
        self.pictures_in_hand(ctx, clip);
        if let Some(meter) = self.meter.as_mut() {
            meter.span("overlays", overlaying.elapsed());
        }
        let panels = crate::moment::Moment::now();
        self.reveal_the_caret(ui.clip_rect());
        self.pointer(ctx, &response, clip);
        self.field_being_filled(ctx);
        self.field_properties(ctx);
        self.link_panel(ctx);
        self.named_places_panel(ctx);
        self.stamp_panel(ctx);
        self.ocr_panel(ctx);
        self.scan_notice(ctx, ui.clip_rect());
        self.print_dialog(ctx);
        self.split_dialog(ctx);
        self.export_dialog(ctx);
        self.properties_window(ctx);
        self.bookmark_name_box(ctx);
        self.block_toolbar(ui);
        self.object_toolbar(ui);
        if let Some(meter) = self.meter.as_mut() {
            meter.span("panels", panels.elapsed());
        }
    }

    fn marks_on_the_page(&self, glass: &egui::Painter, laid: Laid) {
        let soft = egui::Color32::from_rgba_unmultiplied(255, 200, 0, 90);
        let strong = egui::Color32::from_rgba_unmultiplied(255, 140, 0, 140);
        let edge = egui::Color32::from_rgb(200, 90, 0);
        for (at, hit) in self.hits_on(laid.page).iter().enumerate() {
            let current = self.is_the_hit_in_hand(laid.page, at);
            for box_of in &hit.boxes {
                let area = box_on_screen(laid.placed, *box_of).expand(1.5);
                glass.rect_filled(area, 2.0, if current { strong } else { soft });
                if current {
                    glass.rect_stroke(
                        area,
                        2.0,
                        egui::Stroke::new(1.5, edge),
                        egui::StrokeKind::Outside,
                    );
                }
            }
        }
    }

    fn frame_drawn_for_text(&self, glass: &egui::Painter) {
        let Some((page, pixels)) = self.draft_pixels else {
            return;
        };
        let Some(laid) = self.laid.iter().copied().find(|laid| laid.page == page) else {
            return;
        };
        let blue = egui::Color32::from_rgb(0, 90, 200);
        let area = box_on_screen(laid.placed, pixels);
        glass.rect_stroke(
            area,
            0.0,
            egui::Stroke::new(1.0, blue),
            egui::StrokeKind::Inside,
        );
        let line = (pixels[3] - pixels[1]).min(NEW_TEXT_LINE) * f64::from(laid.placed.stretch);
        #[allow(clippy::cast_possible_truncation)]
        let down = egui::vec2(0.0, line as f32);
        glass.line_segment([area.left_top(), area.left_top() + down], (1.5, blue));
    }

    fn lay_out(&self, page: usize, rect: egui::Rect, zoom: f64) -> Option<Laid> {
        let (x, y) = self.editor.strip().origin(page)?;
        let size = self.editor.page_pixels(page, OVERLAY_SCALE)?;
        #[allow(clippy::cast_possible_truncation)]
        let corner = rect.min + egui::vec2((x * zoom) as f32, (y * zoom) as f32);
        #[allow(clippy::cast_possible_truncation)]
        let stretch = zoom as f32;
        let placed = Placement {
            origin: (corner.x, corner.y),
            size,
            stretch,
        };
        let (width, height) = placed.screen_size();
        Some(Laid {
            page,
            rect: egui::Rect::from_min_size(corner, egui::vec2(width, height)),
            placed,
        })
    }

    fn paint_page(
        &self,
        painter: &egui::Painter,
        laid: Laid,
        band: egui::Rect,
        middle: egui::Pos2,
        wanted: &mut Vec<(f32, TileId, [u32; 4], f64)>,
        drawn: &mut Vec<TileId>,
    ) {
        painter.rect_filled(laid.rect, 0.0, egui::Color32::WHITE);
        if let Some(reason) = self.failed.get(&laid.page) {
            painter.text(
                laid.rect.center(),
                egui::Align2::CENTER_CENTER,
                Message::PageWillNotOpen {
                    page: laid.page + 1,
                    why: reason.clone(),
                }
                .say(self.lang),
                egui::FontId::proportional(14.0),
                egui::Color32::from_rgb(150, 60, 60),
            );
            return;
        }
        let rung = self.rung();
        let covered = self
            .fine_tiles(laid, band)
            .into_iter()
            .all(|(id, _, _)| self.textures.contains_key(&id));
        let mut order: Vec<(usize, TileId, [u32; 4], bool)> = self
            .tiles
            .on_page(laid.page)
            .filter(|(id, _)| !covered || id.zoom == rung)
            .map(|(id, held)| (rung.abs_diff(id.zoom), id, held.box_pixels, held.stale))
            .collect();
        order.sort_by_key(|(distance, _, _, _)| std::cmp::Reverse(*distance));
        for (_, id, box_pixels, stale) in order {
            if stale && id.zoom != rung {
                continue;
            }
            let (Some(scale), Some(texture)) = (ZOOMS.get(id.zoom), self.textures.get(&id)) else {
                continue;
            };
            let Some(where_) = self.tile_rect(laid, box_pixels, *scale) else {
                continue;
            };
            if !where_.intersects(band) {
                continue;
            }
            painter.image(
                texture.id(),
                where_,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            drawn.push(id);
        }
        if rung != COARSE
            && let Some(coarse) = ZOOMS.get(COARSE)
            && let Some(page_pixels) = self.editor.page_pixels(laid.page, *coarse)
        {
            let epoch = self.editor.epoch();
            let urgency = if self.tiles.stale_on(laid.page) {
                f32::MAX
            } else {
                -1.0
            };
            let (across, down) = pdf_app::strip::tiles_across(page_pixels);
            for row in 0..down {
                for col in 0..across {
                    let id = TileId {
                        page: laid.page,
                        zoom: COARSE,
                        col,
                        row,
                    };
                    if !self.tiles.wants(id, epoch) {
                        continue;
                    }
                    if let Some(box_pixels) = tile_box(page_pixels, col, row) {
                        wanted.push((urgency, id, box_pixels, *coarse));
                    }
                }
            }
        }

        let Some(scale) = ZOOMS.get(rung) else { return };
        let epoch = self.editor.epoch();
        for (id, box_pixels, where_) in self.fine_tiles(laid, band) {
            if self.tiles.wants(id, epoch) {
                wanted.push((where_.center().distance(middle), id, box_pixels, *scale));
            }
        }
    }

    fn fine_tiles(&self, laid: Laid, band: egui::Rect) -> Vec<(TileId, [u32; 4], egui::Rect)> {
        let rung = self.rung();
        let Some(scale) = ZOOMS.get(rung) else {
            return Vec::new();
        };
        let Some(page_pixels) = self.editor.page_pixels(laid.page, *scale) else {
            return Vec::new();
        };
        let visible = laid.rect.intersect(band);
        if !visible.is_positive() {
            return Vec::new();
        }
        let to_device = f64::from(page_pixels.0) / f64::from(laid.rect.width().max(1.0));
        let window = [
            f64::from(visible.min.x - laid.rect.min.x) * to_device,
            f64::from(visible.min.y - laid.rect.min.y) * to_device,
            f64::from(visible.max.x - laid.rect.min.x) * to_device,
            f64::from(visible.max.y - laid.rect.min.y) * to_device,
        ];
        tiles_over(page_pixels, window)
            .into_iter()
            .filter_map(|(col, row)| {
                let id = TileId {
                    page: laid.page,
                    zoom: rung,
                    col,
                    row,
                };
                let box_pixels = tile_box(page_pixels, col, row)?;
                let where_ = self.tile_rect(laid, box_pixels, *scale)?;
                Some((id, box_pixels, where_))
            })
            .collect()
    }

    fn scene_for(&mut self, page: usize) -> Option<Scene> {
        if !self.tiles.stale_on(page)
            && let Some(leaf) = self.editor.leaf(page)
        {
            let bands = self
                .pointing
                .block_on(page)
                .and_then(|block| self.editor.free_bands(page, block));
            let scene = Scene {
                leaf: Arc::clone(leaf),
                pointing: self.pointing,
                chosen: self.chosen.clone(),
                frames: self.frames_of(page).to_vec(),
                bands,
            };
            self.scenes.insert(page, scene.clone());
            return Some(scene);
        }
        self.scenes.get(&page).cloned()
    }

    pub(crate) fn presenting(&self) -> bool {
        let Some(page) = self.pointing.page() else {
            return false;
        };
        self.laid.iter().any(|laid| laid.page == page)
            && !self.failed.contains_key(&page)
            && (self.tiles.stale_on(page) || self.editor.leaf(page).is_none())
    }

    fn tile_rect(&self, laid: Laid, box_pixels: [u32; 4], scale: f64) -> Option<egui::Rect> {
        let page_pixels = self.editor.page_pixels(laid.page, scale)?;
        if page_pixels.0 == 0 || page_pixels.1 == 0 {
            return None;
        }
        let kx = f64::from(laid.rect.width()) / f64::from(page_pixels.0);
        let ky = f64::from(laid.rect.height()) / f64::from(page_pixels.1);
        #[allow(clippy::cast_possible_truncation)]
        let at = |x: u32, y: u32| {
            laid.rect.min + egui::vec2((f64::from(x) * kx) as f32, (f64::from(y) * ky) as f32)
        };
        Some(egui::Rect::from_min_max(
            at(box_pixels[0], box_pixels[1]),
            at(box_pixels[2], box_pixels[3]),
        ))
    }

    fn ask_for(&mut self, mut wanted: Vec<(f32, TileId, [u32; 4], f64)>) {
        let epoch = self.editor.epoch();
        let pages: Vec<usize> = self.laid.iter().map(|laid| laid.page).collect();
        for page in pages {
            if self.editor.leaf(page).is_some()
                || self.painter.reading(page, epoch)
                || self.failed.contains_key(&page)
            {
                continue;
            }
            let Some(source) = self.editor.source().cloned() else {
                continue;
            };
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
        }
        let needed: std::collections::BTreeSet<usize> = self
            .laid
            .iter()
            .map(|laid| laid.page)
            .chain(wanted.iter().map(|(_, id, _, _)| id.page))
            .collect();
        self.editor.keep_pages(&needed, KEEP_PAGES, KEEP_PAGE_BYTES);
        wanted.sort_by(|one, other| other.0.total_cmp(&one.0));
        let keeping: std::collections::BTreeSet<TileId> =
            wanted.iter().map(|(_, id, _, _)| *id).collect();
        self.painter
            .forget(&|id| id.zoom == crate::pages::THUMB_RUNG || keeping.contains(&id));
        for (_, id, window, scale) in wanted {
            if self.painter.drawing(id, epoch) {
                continue;
            }
            let Some(leaf) = self.editor.leaf(id.page) else {
                continue;
            };
            let view = Arc::clone(&leaf.view);
            self.painter.draw(id, view, scale, window, epoch);
        }
    }

    fn blocks_overlay(painter: &egui::Painter, laid: Laid, scene: &Scene) {
        let overlay = &scene.leaf.overlay;
        let selected = scene.pointing.block_on(laid.page);
        let faint = egui::Color32::from_rgba_unmultiplied(0, 90, 200, 110);
        let frames = &scene.frames;
        let selected_frame = selected;
        let turned = |index: usize| {
            overlay
                .blocks
                .get(index)
                .is_some_and(|block| !Quad::from_pixels(block.quad).upright())
        };
        for (index, frame) in frames.iter().enumerate() {
            if selected_frame == Some(index)
                || turned(index)
                || !scene.pointing.block_stands(overlay, laid.page, index)
            {
                continue;
            }
            let outline = quad_on_screen(laid.placed, &frame_quad(&Quad::of(*frame)));
            Self::dashed_frame(painter, outline, 1.0, faint);
        }
        for (index, block) in overlay.blocks.iter().enumerate() {
            if selected == Some(index)
                || (frames.get(index).is_some() && !turned(index))
                || !scene.pointing.block_stands(overlay, laid.page, index)
            {
                continue;
            }
            let outline = quad_on_screen(laid.placed, &frame_quad(&Quad::from_pixels(block.quad)));
            Self::dashed_frame(painter, outline, 1.0, faint);
        }
    }

    fn selected_block(painter: &egui::Painter, laid: Laid, scene: &Scene) {
        if scene.chosen.is_a_group() && scene.chosen.page == laid.page {
            Self::group_outline(painter, laid, scene);
            if !scene.pointing.editing() {
                return;
            }
        }
        let Some(block) = scene
            .pointing
            .block_on(laid.page)
            .and_then(|at| scene.leaf.overlay.blocks.get(at))
        else {
            return;
        };
        let blue = egui::Color32::from_rgb(0, 90, 200);
        let quad = Quad::from_pixels(block.quad);
        let frame = quad_on_screen(laid.placed, &frame_quad(&quad));
        if let Some(bands) = scene
            .bands
            .as_ref()
            .filter(|bands| !bands.is_empty() && quad.upright())
        {
            Self::free_frame(painter, laid, bands, scene.pointing.editing());
            if scene.pointing.editing() {
                return;
            }
        } else if scene.pointing.editing() {
            stroke_quad(painter, frame, 1.5, blue);
            return;
        } else {
            fill_quad(
                painter,
                frame,
                egui::Color32::from_rgba_unmultiplied(0, 90, 200, 18),
            );
            Self::dashed_frame(painter, frame, 1.5, blue);
        }
        let radius = 3.5 * laid.placed.stretch.clamp(0.5, 2.0);
        for (handle, (x, y)) in text_handles(&quad) {
            let at = box_on_screen(laid.placed, [x, y, x, y]).min;
            if handle == ROTATE_HANDLE {
                let (leaves, _) = rotate_stem(&quad);
                let foot = box_on_screen(laid.placed, [leaves.0, leaves.1, leaves.0, leaves.1]).min;
                painter.line_segment([foot, at], egui::Stroke::new(1.0, blue));
                painter.circle_filled(at, radius + 2.5, egui::Color32::WHITE);
                painter.circle_stroke(at, radius + 1.0, egui::Stroke::new(1.5, blue));
                continue;
            }
            painter.circle_filled(at, radius + 1.0, egui::Color32::WHITE);
            painter.circle_filled(at, radius, blue);
        }
    }

    fn free_frame(painter: &egui::Painter, laid: Laid, bands: &[[f64; 4]], editing: bool) {
        let blue = egui::Color32::from_rgb(0, 90, 200);
        if !editing {
            for band in bands {
                painter.rect_filled(
                    box_on_screen(laid.placed, *band),
                    0.0,
                    egui::Color32::from_rgba_unmultiplied(0, 90, 200, 18),
                );
            }
        }
        let mut outline: Vec<egui::Pos2> = Vec::with_capacity(bands.len() * 4);
        for band in bands {
            let rect = box_on_screen(laid.placed, *band);
            outline.push(egui::pos2(rect.max.x, rect.min.y));
            outline.push(egui::pos2(rect.max.x, rect.max.y));
        }
        for band in bands.iter().rev() {
            let rect = box_on_screen(laid.placed, *band);
            outline.push(egui::pos2(rect.min.x, rect.max.y));
            outline.push(egui::pos2(rect.min.x, rect.min.y));
        }
        let stroke = egui::Stroke::new(1.5, blue);
        if editing {
            painter.add(egui::Shape::closed_line(outline, stroke));
            return;
        }
        for pair in 0..outline.len() {
            let (from, to) = (outline[pair], outline[(pair + 1) % outline.len()]);
            let length = f64::from((to - from).length());
            for (start, end) in dashes(length) {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "a fraction along a screen edge, which is f32's own space"
                )]
                let along = |fraction: f64| from + (to - from) * (fraction as f32);
                painter.line_segment([along(start), along(end)], stroke);
            }
        }
    }

    fn group_outline(painter: &egui::Painter, laid: Laid, scene: &Scene) {
        let overlay = &scene.leaf.overlay;
        let blue = egui::Color32::from_rgb(0, 90, 200);
        let quads =
            scene
                .chosen
                .blocks
                .iter()
                .filter_map(|at| overlay.blocks.get(*at).map(|block| (block.quad, true)))
                .chain(
                    scene.chosen.objects.iter().filter_map(|at| {
                        overlay.objects.get(*at).map(|object| (object.quad, false))
                    }),
                );
        for (quad, is_text) in quads {
            let frame = quad_on_screen(laid.placed, &frame_quad(&Quad::from_pixels(quad)));
            fill_quad(
                painter,
                frame,
                egui::Color32::from_rgba_unmultiplied(0, 90, 200, 18),
            );
            if is_text {
                Self::dashed_frame(painter, frame, 1.5, blue);
            } else {
                stroke_quad(painter, frame, 1.5, blue);
            }
        }
    }

    fn objects_overlay(
        painter: &egui::Painter,
        laid: Laid,
        scene: &Scene,
        framed: crate::window_state::Framed,
    ) {
        let overlay = &scene.leaf.overlay;
        let green = egui::Color32::from_rgb(0, 130, 90);
        let selected = scene.pointing.object_on(laid.page);
        for (index, object) in overlay.objects.iter().enumerate() {
            let quad = Quad::from_pixels(object.quad);
            let frame = quad_on_screen(laid.placed, &frame_quad(&quad));
            let in_a_group = scene.chosen.is_a_group()
                && scene.chosen.page == laid.page
                && !scene.pointing.editing();
            if selected == Some(index) && !in_a_group {
                fill_quad(
                    painter,
                    frame,
                    egui::Color32::from_rgba_unmultiplied(0, 130, 90, 18),
                );
                stroke_quad(painter, frame, 1.5, green);
                let radius = 3.5 * laid.placed.stretch.clamp(0.5, 2.0);
                let (leaves, turn) = rotate_stem(&quad);
                let foot = box_on_screen(laid.placed, [leaves.0, leaves.1, leaves.0, leaves.1]).min;
                let turn_at = box_on_screen(laid.placed, [turn.0, turn.1, turn.0, turn.1]).min;
                painter.line_segment([foot, turn_at], egui::Stroke::new(1.0, green));
                for (x, y) in handles(&quad).into_iter().chain(std::iter::once(turn)) {
                    let at = box_on_screen(laid.placed, [x, y, x, y]).min;
                    painter.circle_filled(at, radius + 1.0, egui::Color32::WHITE);
                    painter.circle_filled(at, radius, green);
                }
            } else if if object.kind == pdf_semantics::ObjectKind::Path {
                framed.drawings
            } else {
                framed.pictures
            } {
                stroke_quad(
                    painter,
                    frame,
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(0, 130, 90, 110),
                );
            }
        }
    }

    fn dashed_frame(
        painter: &egui::Painter,
        frame: [egui::Pos2; 4],
        width: f32,
        colour: egui::Color32,
    ) {
        let stroke = egui::Stroke::new(width, colour);
        let corners = [
            (frame[0], frame[1]),
            (frame[1], frame[2]),
            (frame[2], frame[3]),
            (frame[3], frame[0]),
        ];
        for (from, to) in corners {
            let length = f64::from((to - from).length());
            for (start, end) in dashes(length) {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "a fraction along a screen edge, which is f32's own space"
                )]
                let along = |fraction: f64| from + (to - from) * (fraction as f32);
                painter.line_segment([along(start), along(end)], stroke);
            }
        }
    }

    fn caret_overlay(&self, painter: &egui::Painter, laid: Laid, scene: &Scene) {
        let overlay = &scene.leaf.overlay;
        if self.show_clusters {
            let stroke =
                egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(200, 60, 0, 70));
            for box_pixels in overlay.clusters.iter().filter_map(|c| c.box_pixels) {
                painter.rect_stroke(
                    box_on_screen(laid.placed, box_pixels),
                    0.0,
                    stroke,
                    egui::StrokeKind::Inside,
                );
            }
        }
        if scene.pointing.page() != Some(laid.page) {
            return;
        }
        let typing_live = self
            .live
            .as_ref()
            .is_some_and(|live| !live.written && live.page == laid.page);
        let spans = match scene.pointing {
            Pointing::Text { block, caret, .. } if !typing_live => overlay
                .blocks
                .get(block)
                .map(|owner| selection_rows(&overlay.carets, &owner.lines, caret.anchor, caret.at))
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        for (line, from, to) in spans {
            if let Some(band) = selection_quad(&overlay.carets, &overlay.clusters, line, from, to) {
                fill_quad(
                    painter,
                    quad_on_screen(laid.placed, &band),
                    egui::Color32::from_rgba_unmultiplied(0, 90, 200, 105),
                );
            }
        }
        if self.live_caret(painter, laid) {
            return;
        }
        let Some(caret) = scene.pointing.caret() else {
            return;
        };
        let frame = match scene.pointing {
            Pointing::Text { block, .. } => scene.frames.get(block).copied(),
            _ => None,
        };
        let Some(stop) = shown_caret(&overlay.carets, &overlay.clusters, caret.at, frame) else {
            return;
        };
        let (top, bottom) = laid.placed.caret_line(&stop);
        let corner = egui::pos2(laid.placed.origin.0, laid.placed.origin.1);
        draw_caret(
            painter,
            corner + egui::vec2(top[0], top[1]),
            corner + egui::vec2(bottom[0], bottom[1]),
        );
    }
}

const CARET_WIDTH: f32 = 1.0;

const CARET_UPRIGHT: f32 = 0.01;

fn draw_caret(painter: &egui::Painter, top: egui::Pos2, bottom: egui::Pos2) {
    let color = egui::Color32::from_rgb(0, 90, 200);
    let rise = bottom - top;
    let height = rise.length();
    if height <= f32::EPSILON {
        return;
    }
    if rise.x.abs() <= rise.y.abs() * CARET_UPRIGHT {
        let left = (top.x - CARET_WIDTH / 2.0).round();
        let (y0, y1) = if top.y <= bottom.y {
            (top.y, bottom.y)
        } else {
            (bottom.y, top.y)
        };
        painter.rect_filled(
            egui::Rect::from_min_max(egui::pos2(left, y0), egui::pos2(left + CARET_WIDTH, y1)),
            0.0,
            color,
        );
        return;
    }
    let across = egui::vec2(-rise.y, rise.x) / height * (CARET_WIDTH / 2.0);
    fill_quad(
        painter,
        [top - across, top + across, bottom + across, bottom - across],
        color,
    );
}

pub(crate) fn box_on_screen(placed: Placement, bounds: [f64; 4]) -> egui::Rect {
    let [x0, y0, x1, y1] = placed.screen_box(bounds);
    let corner = egui::pos2(placed.origin.0, placed.origin.1);
    egui::Rect::from_min_max(corner + egui::vec2(x0, y0), corner + egui::vec2(x1, y1))
}

#[allow(clippy::cast_possible_truncation)]
pub(crate) fn quad_on_screen(placed: Placement, quad: &Quad) -> [egui::Pos2; 4] {
    let corner = egui::pos2(placed.origin.0, placed.origin.1);
    quad.corners
        .map(|(x, y)| corner + egui::vec2(x as f32 * placed.stretch, y as f32 * placed.stretch))
}

pub(crate) fn stroke_quad(
    painter: &egui::Painter,
    corners: [egui::Pos2; 4],
    width: f32,
    color: egui::Color32,
) {
    let middle = corners
        .iter()
        .fold(egui::Vec2::ZERO, |sum, corner| sum + corner.to_vec2())
        / 4.0;
    let inset = corners.map(|corner| {
        let out = corner.to_vec2() - middle;
        let length = out.length();
        if length <= f32::EPSILON {
            corner
        } else {
            corner - out / length * (width / 2.0)
        }
    });
    painter.add(egui::Shape::closed_line(
        inset.to_vec(),
        egui::Stroke::new(width, color),
    ));
}

pub(crate) fn fill_quad(painter: &egui::Painter, corners: [egui::Pos2; 4], color: egui::Color32) {
    painter.add(egui::Shape::convex_polygon(
        corners.to_vec(),
        color,
        egui::Stroke::NONE,
    ));
}
