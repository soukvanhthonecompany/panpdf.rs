use eframe::egui;
use egui::Key;
use pdf_app::draft::{Intent, Target};
use pdf_app::live::Pending;
use pdf_app::wording::Message;

use crate::canvas::box_on_screen;
use crate::window_state::{Laid, Pointing, Window, ZOOMS};

pub(crate) struct LiveTyping {
    pub(crate) page: usize,
    pub(crate) block: usize,
    reading: pdf_edit::BlockReading,
    pending: Pending,
    laid: pdf_edit::LiveBlock,
    history: Vec<Pending>,
    future: Vec<Pending>,
    places: std::collections::BTreeMap<pdf_semantics::ClusterKey, (usize, usize)>,
    picture: Option<Picture>,
    drawn: bool,
    pub(crate) written: bool,
    written_at: u64,
}

struct Picture {
    rung: usize,
    region: [u32; 4],
    covered: [f64; 4],
    texture: egui::TextureHandle,
}

pub(crate) enum LiveStep {
    Took,
    NotMine,
}

impl Window {
    pub(crate) fn live_take(&mut self) -> LiveStep {
        let Some(front) = self.input.front().cloned() else {
            return LiveStep::NotMine;
        };
        let active = self.live.as_ref().is_some_and(|live| !live.written);
        match front {
            Intent::Insert(_) | Intent::Delete { .. } => {}
            Intent::Undo | Intent::Redo if active => {
                let back = front == Intent::Undo;
                if self.live_history(back) {
                    self.input.hold_front();
                    return LiveStep::Took;
                }
                self.write_live(true);
                return LiveStep::NotMine;
            }
            Intent::Caret { .. } | Intent::Undo | Intent::Redo => {
                if active {
                    self.write_live(true);
                }
                return LiveStep::NotMine;
            }
        }
        let Some((page, block, at, anchor)) = self.text_positions() else {
            return LiveStep::NotMine;
        };
        let continuing = self
            .live
            .as_ref()
            .is_some_and(|live| !live.written && live.page == page && live.block == block);
        if !continuing && self.typing.next.is_some() {
            return LiveStep::NotMine;
        }
        if !continuing {
            if active {
                self.write_live(true);
                return LiveStep::NotMine;
            }
            if !self.start_live(page, block, (at, anchor)) {
                return LiveStep::NotMine;
            }
        }
        self.apply_live(&front, (page, block))
    }

    fn start_live(
        &mut self,
        page: usize,
        block: usize,
        (at, anchor): ((usize, usize), (usize, usize)),
    ) -> bool {
        let Some(reading) = self.editor.block_reading(page, block) else {
            return false;
        };
        let pending = Pending::over(anchor, at);
        let Ok(laid) = self
            .editor
            .live_block(page, block, pending.range(), &pending.text)
        else {
            return false;
        };
        let places = laid
            .before
            .iter()
            .enumerate()
            .flat_map(|(row, keys)| {
                keys.iter()
                    .enumerate()
                    .map(move |(stop, key)| (*key, (row, stop)))
            })
            .collect();
        let picture = self
            .live
            .take()
            .filter(|live| live.page == page)
            .and_then(|live| live.picture);
        self.live = Some(LiveTyping {
            page,
            block,
            reading,
            pending,
            laid,
            history: Vec::new(),
            future: Vec::new(),
            places,
            picture,
            drawn: false,
            written: false,
            written_at: 0,
        });
        true
    }

    fn apply_live(&mut self, front: &Intent, (page, block): (usize, usize)) -> LiveStep {
        let Some(live) = self.live.as_ref() else {
            return LiveStep::NotMine;
        };
        let mut next = live.pending.clone();
        match front {
            Intent::Insert(text) => next.insert(text),
            Intent::Delete { .. } if live.history.is_empty() && next.from != next.to => {}
            Intent::Delete { backwards, count } => {
                for _ in 0..*count {
                    let took = if *backwards {
                        next.back(&live.reading)
                    } else {
                        next.forward(&live.reading)
                    };
                    if !took {
                        break;
                    }
                }
            }
            Intent::Caret { .. } | Intent::Undo | Intent::Redo => {}
        }
        let Ok(laid) = self
            .editor
            .live_block(page, block, next.range(), &next.text)
        else {
            self.write_live(true);
            return LiveStep::NotMine;
        };
        if let Some(live) = self.live.as_mut() {
            let before = std::mem::replace(&mut live.pending, next);
            live.history.push(before);
            live.future.clear();
            live.laid = laid;
            live.drawn = false;
        }
        self.input.hold_front();
        self.reveal_caret = true;
        LiveStep::Took
    }

    fn live_history(&mut self, back: bool) -> bool {
        let Some(live) = self.live.as_mut() else {
            return false;
        };
        let (from, to) = if back {
            (&mut live.history, &mut live.future)
        } else {
            (&mut live.future, &mut live.history)
        };
        let Some(step) = from.pop() else {
            return false;
        };
        let Ok(laid) = self
            .editor
            .live_block(live.page, live.block, step.range(), &step.text)
        else {
            from.push(step);
            return false;
        };
        let was = std::mem::replace(&mut live.pending, step);
        to.push(was);
        live.laid = laid;
        live.drawn = false;
        true
    }

    pub(crate) fn write_live(&mut self, keep_caret: bool) {
        let Some(live) = self.live.as_mut() else {
            return;
        };
        if live.written {
            return;
        }
        live.written = true;
        if live.pending.is_empty() || self.editor.is_busy() {
            self.live = None;
            return;
        }
        let (page, block) = (live.page, live.block);
        let pending = live.pending.clone();
        let Some(job) = self
            .editor
            .begin_edit(page, block, pending.range(), &pending.text)
        else {
            self.live = None;
            return;
        };
        self.resume = keep_caret.then_some((page, block, pending.from.0, pending.from.1));
        let applied = self.took_back(job.run());
        if let Some(live) = self.live.as_mut() {
            live.written_at = self.editor.epoch();
        }
        if let pdf_app::Applied::Refused(reason) = applied {
            self.live = None;
            let target = Target {
                page,
                block,
                from: pending.from,
                to: pending.to,
            };
            let said = Message::Refused(reason);
            self.input
                .live_refused(pending.text, &said, target, self.editor.epoch());
            self.editor.say(said);
        }
    }

    pub(crate) fn keep_live(&mut self, ctx: &egui::Context) {
        let Some(live) = self.live.as_ref() else {
            return;
        };
        if live.written {
            let (page, at) = (live.page, live.written_at);
            if self.editor.epoch() != at
                || (self.editor.leaf(page).is_some() && !self.tiles.stale_on(page))
            {
                self.live = None;
            }
            return;
        }
        let (page, block) = (live.page, live.block);
        let (pressed, leaving_key) = ctx.input(|input| {
            (
                input.pointer.any_pressed(),
                input.events.iter().any(leaves_the_block),
            )
        });
        let still_here = matches!(self.pointing, Pointing::Text { page: at, block: which, .. } if at == page && which == block);
        if pressed || !still_here {
            self.write_live(false);
        } else if leaving_key {
            self.write_live(true);
        }
    }

    pub(crate) fn paint_live(&mut self, ctx: &egui::Context, painter: &egui::Painter, laid: Laid) {
        let rung = self.rung();
        let Some(typing) = self.live.as_ref() else {
            return;
        };
        if typing.page != laid.page {
            return;
        }
        let stale = !typing.drawn
            || typing
                .picture
                .as_ref()
                .is_none_or(|picture| picture.rung != rung);
        if stale && !typing.written {
            self.draw_live(ctx, rung);
        }
        let Some(typing) = self.live.as_ref() else {
            return;
        };
        let Some(picture) = typing.picture.as_ref() else {
            return;
        };
        let Some(drawn_at) = ZOOMS.get(picture.rung) else {
            return;
        };
        let Some(page_pixels) = self.editor.page_pixels(laid.page, *drawn_at) else {
            return;
        };
        #[allow(clippy::cast_precision_loss)]
        let (across, down) = (
            laid.rect.width() / page_pixels.0.max(1) as f32,
            laid.rect.height() / page_pixels.1.max(1) as f32,
        );
        #[allow(clippy::cast_precision_loss)]
        let at = |x: u32, y: u32| laid.rect.min + egui::vec2(x as f32 * across, y as f32 * down);
        let region = picture.region;
        painter.image(
            picture.texture.id(),
            egui::Rect::from_min_max(at(region[0], region[1]), at(region[2], region[3])),
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }

    pub(crate) fn live_caret(&self, painter: &egui::Painter, laid: Laid) -> bool {
        let Some(live) = self.live.as_ref() else {
            return false;
        };
        if live.written || live.page != laid.page {
            return false;
        }
        let Some((x, y, em)) = caret_of(live) else {
            return true;
        };
        let Some(device) = self.editor.geometry(laid.page).and_then(|geometry| {
            pdf_render::DeviceTransform::for_page(
                geometry,
                pdf_app::document::OVERLAY_SCALE,
                pdf_render::RenderLimits::default(),
            )
            .ok()
        }) else {
            return true;
        };
        let foot = device
            .matrix
            .transform(pdf_paint::Point { x, y: y - 0.2 * em });
        let head = device.matrix.transform(pdf_paint::Point {
            x,
            y: y + 0.85 * em,
        });
        let point = |p: pdf_paint::Point| box_on_screen(laid.placed, [p.x, p.y, p.x, p.y]).min;
        painter.line_segment(
            [point(foot), point(head)],
            egui::Stroke::new(1.5, egui::Color32::from_rgb(0, 90, 200)),
        );
        true
    }

    pub(crate) fn live_reach(&self, page: usize, block: usize) -> Option<[f64; 4]> {
        let live = self.live.as_ref()?;
        if live.written || live.page != page || live.block != block {
            return None;
        }
        let device = pdf_render::DeviceTransform::for_page(
            self.editor.geometry(page)?,
            pdf_app::document::OVERLAY_SCALE,
            pdf_render::RenderLimits::default(),
        )
        .ok()?;
        let mut reach = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        for line in &live.laid.lines {
            let right = line.pieces.last().map_or(line.origin.0, |piece| piece.to);
            for (x, y) in [
                (line.origin.0, line.origin.1 - 0.25 * line.em),
                (right, line.origin.1 + 0.85 * line.em),
            ] {
                let at = device.matrix.transform(pdf_paint::Point { x, y });
                reach = [
                    reach[0].min(at.x),
                    reach[1].min(at.y),
                    reach[2].max(at.x),
                    reach[3].max(at.y),
                ];
            }
        }
        reach.iter().all(|value| value.is_finite()).then_some(reach)
    }

    pub(crate) fn live_frame(&self, painter: &egui::Painter, laid: Laid) -> bool {
        let Some(live) = self.live.as_ref() else {
            return false;
        };
        let Some(reach) = self.live_reach(laid.page, live.block) else {
            return false;
        };
        let held = self
            .overlay(laid.page)
            .and_then(|overlay| overlay.blocks.get(live.block))
            .map_or(reach, |block| block.box_pixels);
        let frame = [
            held[0].min(reach[0]),
            held[1].min(reach[1]),
            held[2].max(reach[2]),
            held[3].max(reach[3]),
        ];
        let on_screen = box_on_screen(laid.placed, frame);
        painter.rect_stroke(
            on_screen,
            0.0,
            egui::Stroke::new(1.5, egui::Color32::from_rgb(0, 90, 200)),
            egui::StrokeKind::Inside,
        );
        true
    }

    fn draw_live(&mut self, ctx: &egui::Context, rung: usize) {
        let Some(scale) = ZOOMS.get(rung).copied() else {
            return;
        };
        let Some(live) = self.live.as_mut() else {
            return;
        };
        let Some(leaf) = self.editor.leaf(live.page).cloned() else {
            return;
        };
        let view = &leaf.view;
        let hidden: Vec<usize> = live.laid.hidden.iter().copied().collect();
        let before = pdf_paint::decipher_fonts::region_of(&view.graph, &hidden);
        let mut covered = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        for bounds in [
            live.laid.extent,
            before,
            live.picture.as_ref().map(|picture| picture.covered),
        ]
        .into_iter()
        .flatten()
        {
            covered = [
                covered[0].min(bounds[0]),
                covered[1].min(bounds[1]),
                covered[2].max(bounds[2]),
                covered[3].max(bounds[3]),
            ];
        }
        if !covered.iter().all(|value| value.is_finite()) {
            return;
        }
        let Some(region) = self.editor.region_in_pixels(live.page, scale, covered) else {
            return;
        };
        let typed = pdf_paint::PaintGraph {
            atoms: live.laid.atoms.clone(),
            ..pdf_paint::PaintGraph::default()
        };
        let above: Vec<&pdf_paint::PaintGraph> = view
            .annotations
            .iter()
            .map(|annotation| &annotation.graph)
            .collect();
        let Ok((canvas, _)) = pdf_render::render_region_replacing(
            (&view.graph, &live.laid.hidden, &typed),
            &above,
            &view.program.geometry,
            pdf_render::RenderOptions {
                scale,
                ..pdf_render::RenderOptions::default()
            },
            region,
        ) else {
            return;
        };
        let size = [canvas.width as usize, canvas.height as usize];
        let image =
            egui::ColorImage::from_rgba_unmultiplied(size, &pdf_app::painter::rgba(&canvas));
        let options = egui::TextureOptions::LINEAR;
        match live.picture.as_mut() {
            Some(picture) => {
                picture.texture.set(image, options);
                picture.rung = rung;
                picture.region = region;
                picture.covered = covered;
            }
            None => {
                live.picture = Some(Picture {
                    rung,
                    region,
                    covered,
                    texture: ctx.load_texture("live block", image, options),
                });
            }
        }
        live.drawn = true;
    }
}

fn caret_of(typing: &LiveTyping) -> Option<(f64, f64, f64)> {
    let from = typing.pending.from;
    let before = |piece: &pdf_edit::LivePiece| match piece.kept {
        None => true,
        Some(key) => typing
            .places
            .get(&key)
            .is_some_and(|(row, stop)| (*row, stop + 1) <= from),
    };
    let rows = &typing.laid.lines;
    let mut at: Option<(usize, f64)> = None;
    for (index, row) in rows.iter().enumerate() {
        for piece in &row.pieces {
            if before(piece) {
                at = Some((index, piece.to));
            }
        }
    }
    let next = rows.iter().enumerate().find_map(|(index, row)| {
        let after = row.pieces.iter().position(|piece| !before(piece))?;
        Some((index, after, row))
    });
    if let Some((below, 0, row)) = next
        && at.is_none_or(|(above, _)| above < below)
    {
        return Some((row.pieces.first()?.from, row.origin.1, row.em));
    }
    let breaks = typing
        .pending
        .text
        .chars()
        .rev()
        .take_while(|character| *character == '\n' || *character == pdf_edit::LINE_BREAK)
        .count();
    let last = rows.len().saturating_sub(1);
    let (on, x) = match at {
        Some((after, x)) if breaks == 0 => (after, x),
        Some((after, _)) => {
            let below = (after + breaks).min(last);
            (below, rows.get(below)?.origin.0)
        }
        None => {
            let below = (from.0.min(last) + breaks).min(last);
            (below, rows.get(below)?.origin.0)
        }
    };
    let found = rows.get(on)?;
    Some((x, found.origin.1, found.em))
}

fn leaves_the_block(event: &egui::Event) -> bool {
    let egui::Event::Key {
        key,
        pressed: true,
        modifiers,
        ..
    } = event
    else {
        return false;
    };
    if modifiers.command || modifiers.ctrl || modifiers.alt {
        return !matches!(key, Key::Z | Key::Y | Key::V);
    }
    matches!(
        key,
        Key::ArrowUp
            | Key::ArrowDown
            | Key::ArrowLeft
            | Key::ArrowRight
            | Key::Home
            | Key::End
            | Key::PageUp
            | Key::PageDown
            | Key::Escape
            | Key::Tab
    )
}
