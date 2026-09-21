use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;

use pdf_app::document::OVERLAY_SCALE;
use pdf_app::view::{Quad, SWEEP_ENOUGH};
use pdf_app::wording::Message;

use crate::window_state::{ChosenPicture, Drag, Pointing, Tool, Window};

const POINTS_A_PIXEL: f64 = 0.75;

const MOST_OF_THE_PAGE: f64 = 0.8;

const GAP: f64 = 8.0;

const MINI: f32 = 72.0;

const CARD_STEP: f32 = 7.0;

const CARDS_SHOWN: usize = 3;

const FROM_POINTER: f32 = 16.0;

fn is_a_picture(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        ["jpg", "jpeg", "png"]
            .iter()
            .any(|name| extension.eq_ignore_ascii_case(name))
    })
}

fn is_a_pdf(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}

impl Window {
    pub(crate) fn pictures_chosen_to_place(&mut self, paths: &[PathBuf]) {
        let pictures = self.read_pictures(paths);
        if pictures.is_empty() {
            return;
        }
        self.point_at(Pointing::Nothing);
        let count = pictures.len();
        self.pictures = pictures;
        self.picture_minis.clear();
        self.tool = Tool::Picture;
        self.editor.say(if count == 1 {
            Message::ClickToPlacePicture
        } else {
            Message::ClickToPlacePictures(count)
        });
    }

    fn read_pictures(&mut self, paths: &[PathBuf]) -> Vec<ChosenPicture> {
        let mut pictures = Vec::with_capacity(paths.len());
        for path in paths {
            let name = path
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
            let file: Arc<[u8]> = match std::fs::read(path) {
                Ok(bytes) => Arc::from(bytes),
                Err(error) => {
                    self.editor.say(Message::PictureNotRead {
                        name,
                        why: error.to_string(),
                    });
                    continue;
                }
            };
            let shown = pictures.len() < CARDS_SHOWN;
            let read = pdf_edit::image_file::ImageFile::read(&file).map(|image| {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "a thumbnail's side, a small whole number of screen points"
                )]
                let most = MINI as u32;
                (
                    image.upright(),
                    shown.then(|| image.thumbnail(most)).flatten(),
                )
            });
            match read {
                Ok((upright, mini)) => pictures.push(ChosenPicture {
                    file,
                    upright,
                    mini,
                }),
                Err(error) => self.editor.say(Message::PictureNotRead {
                    name,
                    why: error.reason().to_owned(),
                }),
            }
        }
        pictures
    }

    pub(crate) fn picture_boxes(&self, drag: &Drag) -> Option<Vec<[f64; 4]>> {
        let laid = self
            .laid
            .iter()
            .copied()
            .find(|laid| laid.page == drag.page)?;
        let from = laid.placed.point_in_page((drag.from.x, drag.from.y))?;
        let to = laid.placed.point_in_page((drag.to.x, drag.to.y))?;
        let travel = drag.to - drag.from;
        let swept = f64::from(travel.x.abs().max(travel.y.abs())) >= SWEEP_ENOUGH;
        self.boxes_on(drag.page, from, swept.then_some(to))
    }

    fn boxes_on(
        &self,
        page: usize,
        from: (f64, f64),
        to: Option<(f64, f64)>,
    ) -> Option<Vec<[f64; 4]>> {
        if self.pictures.is_empty() {
            return None;
        }
        let (width, height) = self.editor.page_pixels(page, OVERLAY_SCALE)?;
        let sizes: Vec<(f64, f64)> = self
            .pictures
            .iter()
            .map(|picture| (f64::from(picture.upright.0), f64::from(picture.upright.1)))
            .collect();
        Some(picture_boxes(
            &sizes,
            (f64::from(width), f64::from(height)),
            from,
            to,
        ))
    }

    pub(crate) fn place_the_picture(&mut self, drag: &Drag) {
        let Some(boxes) = self.picture_boxes(drag) else {
            return;
        };
        self.put_the_pictures_down(drag.page, &boxes);
    }

    fn put_the_pictures_down(&mut self, page: usize, boxes: &[[f64; 4]]) {
        let pictures: Vec<([f64; 4], Arc<[u8]>)> = boxes
            .iter()
            .zip(&self.pictures)
            .map(|(pixels, picture)| (*pixels, Arc::clone(&picture.file)))
            .collect();
        let job = self.editor.begin_place_images(page, pictures);
        if job.is_none() {
            self.editor.say(Message::AnotherEditIsRunning);
            return;
        }
        self.pictures.clear();
        self.tool = Tool::Select;
        if let [pixels] = boxes {
            self.reselect_object = Some((page, Quad::of(*pixels)));
        }
        self.send(job);
    }

    pub(crate) fn pictures_in_hand(&mut self, ctx: &egui::Context, over: egui::Rect) {
        if self.pictures.is_empty() || self.tool != Tool::Picture {
            self.picture_minis.clear();
            return;
        }
        if self.drag.is_some() {
            return;
        }
        let Some(at) = ctx.pointer_latest_pos() else {
            return;
        };
        if !over.contains(at) {
            return;
        }
        self.keep_the_minis(ctx);
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::new("pictures in hand"),
        ));
        let sizes: Vec<(f32, f32)> = self
            .pictures
            .iter()
            .map(|picture| {
                (
                    pixels(picture.upright.0).max(1.0),
                    pixels(picture.upright.1).max(1.0),
                )
            })
            .collect();
        let cards = deck_at((at.x, at.y), &sizes);
        let frame = egui::Color32::from_rgb(255, 255, 255);
        let edge = egui::Color32::from_rgb(120, 130, 145);
        for (at, card) in cards.iter().enumerate().rev() {
            let area = egui::Rect::from_min_max(
                egui::pos2(card[0], card[1]),
                egui::pos2(card[2], card[3]),
            );
            painter.rect_filled(area.expand(2.0), 3.0, frame);
            painter.rect_stroke(
                area.expand(2.0),
                3.0,
                egui::Stroke::new(1.0, edge),
                egui::StrokeKind::Inside,
            );
            if let Some(Some(texture)) = self.picture_minis.get(at) {
                painter.image(
                    texture.id(),
                    area,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            } else {
                painter.rect_filled(area, 0.0, egui::Color32::from_gray(225));
            }
        }
        if let (Some(front), true) = (cards.first(), self.pictures.len() > 1) {
            self.say_how_many(&painter, *front);
        }
    }

    fn say_how_many(&self, painter: &egui::Painter, front: [f32; 4]) {
        let at = egui::pos2(front[2], front[1]);
        let accent = egui::Color32::from_rgb(0, 90, 200);
        let galley = painter.layout_no_wrap(
            self.pictures.len().to_string(),
            egui::FontId::proportional(12.0),
            egui::Color32::WHITE,
        );
        let plate = egui::Rect::from_center_size(at, galley.size()).expand2(egui::vec2(6.0, 2.0));
        painter.rect_filled(plate, 9.0, accent);
        painter.galley(
            plate.center() - galley.size() / 2.0,
            galley,
            egui::Color32::WHITE,
        );
    }

    fn keep_the_minis(&mut self, ctx: &egui::Context) {
        if self.picture_minis.len() == self.pictures.len() {
            return;
        }
        self.picture_minis = self
            .pictures
            .iter()
            .enumerate()
            .map(|(at, picture)| {
                let mini = picture.mini.as_ref()?;
                let size = [mini.width as usize, mini.height as usize];
                let image = egui::ColorImage::from_rgba_unmultiplied(size, &mini.rgba);
                Some(ctx.load_texture(
                    format!("picture in hand {at}"),
                    image,
                    egui::TextureOptions::LINEAR,
                ))
            })
            .collect();
    }

    pub(crate) fn show_the_picture_box(&self, painter: &egui::Painter, drag: &Drag) {
        let (Some(boxes), Some(laid)) = (
            self.picture_boxes(drag),
            self.laid
                .iter()
                .copied()
                .find(|laid| laid.page == drag.page),
        ) else {
            return;
        };
        let blue = egui::Color32::from_rgb(0, 90, 200);
        for pixels in boxes {
            let area = crate::canvas::box_on_screen(laid.placed, pixels);
            painter.rect_filled(
                area,
                0.0,
                egui::Color32::from_rgba_unmultiplied(0, 90, 200, 24),
            );
            painter.rect_stroke(
                area,
                0.0,
                egui::Stroke::new(1.0, blue),
                egui::StrokeKind::Inside,
            );
            painter.line_segment([area.left_top(), area.right_bottom()], (1.0, blue));
            painter.line_segment([area.right_top(), area.left_bottom()], (1.0, blue));
        }
    }

    pub(crate) fn take_dropped_files(&mut self, ctx: &egui::Context) {
        let (hovered, dropped): (Vec<PathBuf>, Vec<PathBuf>) = ctx.input(|input| {
            (
                input
                    .raw
                    .hovered_files
                    .iter()
                    .filter_map(|file| file.path.clone())
                    .collect(),
                input
                    .raw
                    .dropped_files
                    .iter()
                    .filter_map(|file| file.path.clone())
                    .collect(),
            )
        });
        let panel_gap = self.panel_gap_under(ctx);
        let takes = |paths: &[PathBuf]| {
            paths
                .iter()
                .any(|path| is_a_pdf(path) || is_a_picture(path))
        };
        self.file_hover_gap = panel_gap.filter(|_| takes(&hovered));
        if !hovered.is_empty() {
            if self.file_hover_gap.is_some() {
                self.say_where_they_go(ctx);
            } else {
                self.show_what_a_drop_does(ctx, &hovered);
            }
        }
        let asking = self.chooser.is_some() || self.asks_for_a_password();
        if dropped.is_empty() || asking || self.loading.is_some() {
            return;
        }
        if let Some(gap) = panel_gap.filter(|_| takes(&dropped)) {
            for pdf in dropped.iter().filter(|path| is_a_pdf(path)) {
                self.arriving
                    .push_back((crate::page_motion::Arriving::Pages(pdf.clone()), gap));
            }
            let pictures: Vec<PathBuf> = dropped
                .iter()
                .filter(|path| is_a_picture(path))
                .cloned()
                .collect();
            if !pictures.is_empty() {
                self.arriving
                    .push_back((crate::page_motion::Arriving::Pictures(pictures), gap));
            }
            self.file_hover_gap = None;
            return;
        }
        if let Some(pdf) = dropped.iter().find(|path| is_a_pdf(path)) {
            self.open(pdf);
            return;
        }
        let pictures: Vec<PathBuf> = dropped
            .into_iter()
            .filter(|path| is_a_picture(path))
            .collect();
        if pictures.is_empty() {
            return;
        }
        if self.home || !self.has_document() {
            self.pictures_chosen(&pictures, None);
            return;
        }
        if self.editor.is_busy() {
            self.editor.say(Message::AnotherEditIsRunning);
            return;
        }
        let held = self.read_pictures(&pictures);
        if held.is_empty() {
            return;
        }
        self.point_at(Pointing::Nothing);
        self.drop_the_text_draft();
        let pointer = ctx.input(|input| input.pointer.latest_pos());
        let under = pointer.and_then(|at| {
            self.laid.iter().find_map(|laid| {
                let point = laid.placed.page_point((at.x, at.y))?;
                Some((laid.page, point))
            })
        });
        let middle = || {
            let (width, height) = self.editor.page_pixels(self.focus, OVERLAY_SCALE)?;
            Some((
                self.focus,
                (f64::from(width) / 2.0, f64::from(height) / 2.0),
            ))
        };
        let Some((page, at)) = under.or_else(middle) else {
            return;
        };
        self.pictures = held;
        let Some(boxes) = self.boxes_on(page, at, None) else {
            return;
        };
        self.put_the_pictures_down(page, &boxes);
    }

    fn panel_gap_under(&self, ctx: &egui::Context) -> Option<usize> {
        if self.home || !self.has_document() {
            return None;
        }
        let shape = self.page_panel_shape.as_ref()?;
        let pointer = ctx.input(|input| input.pointer.latest_pos())?;
        shape
            .rect
            .contains(pointer)
            .then(|| crate::page_motion::gap_at(shape.columns, &shape.pictures, pointer))
    }

    fn say_where_they_go(&self, ctx: &egui::Context) {
        let Some(held_at) = ctx.input(|input| input.pointer.latest_pos()) else {
            return;
        };
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::new("drop-note"),
        ));
        let accent = ctx.global_style().visuals.selection.stroke.color;
        let galley = painter.layout_no_wrap(
            Message::DropToInsertPages.say(self.lang),
            egui::FontId::proportional(13.0),
            egui::Color32::WHITE,
        );
        let plate = egui::Rect::from_min_size(held_at + egui::vec2(16.0, 12.0), galley.size())
            .expand2(egui::vec2(10.0, 5.0));
        painter.rect_filled(plate, 6.0, accent);
        painter.galley(
            plate.min + egui::vec2(10.0, 5.0),
            galley,
            egui::Color32::WHITE,
        );
    }

    fn show_what_a_drop_does(&self, ctx: &egui::Context, hovered: &[PathBuf]) {
        let pdf = hovered.iter().any(|path| is_a_pdf(path));
        if !pdf && !hovered.iter().any(|path| is_a_picture(path)) {
            return;
        }
        let open = !self.home && self.has_document();
        let said = if pdf {
            Message::DropToOpen
        } else if open {
            Message::DropToPlacePictures
        } else {
            Message::Command(pdf_app::wording::Command::PdfFromPictures)
        };
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("drop-veil"),
        ));
        let accent = ctx.global_style().visuals.selection.stroke.color;
        let screen = ctx.content_rect();
        let panel = self
            .page_panel_shape
            .as_ref()
            .map(|shape| shape.rect)
            .filter(|_| open);
        let rest = panel.map_or(screen, |panel| {
            egui::Rect::from_min_max(egui::pos2(panel.right(), screen.top()), screen.max)
        });
        let zone = |area: egui::Rect, text: String, size: f32| {
            painter.rect_filled(area, 0.0, accent.gamma_multiply(0.12));
            painter.rect_stroke(
                area.shrink(6.0),
                8.0,
                egui::Stroke::new(3.0, accent),
                egui::StrokeKind::Inside,
            );
            let galley = painter.layout(
                text.trim_end_matches('\u{2026}').to_owned(),
                egui::FontId::proportional(size),
                egui::Color32::WHITE,
                area.width() - 48.0,
            );
            let plate = egui::Rect::from_center_size(area.center(), galley.size())
                .expand2(egui::vec2(14.0, 8.0));
            painter.rect_filled(plate, 8.0, accent);
            painter.galley(
                plate.min + egui::vec2(14.0, 8.0),
                galley,
                egui::Color32::WHITE,
            );
        };
        zone(rest, said.say(self.lang), 20.0);
        if let Some(panel) = panel {
            zone(panel, Message::DropToInsertPages.say(self.lang), 14.0);
        }
    }
}

fn picture_boxes(
    pictures: &[(f64, f64)],
    page: (f64, f64),
    from: (f64, f64),
    to: Option<(f64, f64)>,
) -> Vec<[f64; 4]> {
    match pictures {
        [] => return Vec::new(),
        [one] => return vec![picture_box(*one, page, from, to)],
        _ => {}
    }
    let area = to.map_or(
        (page.0 * MOST_OF_THE_PAGE, page.1 * MOST_OF_THE_PAGE),
        |to| ((to.0 - from.0).abs(), (to.1 - from.1).abs()),
    );
    let natural: Vec<(f64, f64)> = pictures
        .iter()
        .map(|(width, height)| (width * POINTS_A_PIXEL, height * POINTS_A_PIXEL))
        .collect();
    let most = to.is_none().then(|| {
        natural.iter().fold((0.0_f64, 0.0_f64), |most, size| {
            (most.0.max(size.0), most.1.max(size.1))
        })
    });
    let count = pictures.len();
    let cell_for = |columns: usize| {
        let rows = count.div_ceil(columns);
        let mut cell = (
            (area.0 - GAP * float(columns - 1)) / float(columns),
            (area.1 - GAP * float(rows - 1)) / float(rows),
        );
        if let Some(most) = most {
            cell = (cell.0.min(most.0), cell.1.min(most.1));
        }
        (rows, (cell.0.max(1.0), cell.1.max(1.0)))
    };
    let fit = |size: (f64, f64), cell: (f64, f64)| {
        let mut scale = (cell.0 / size.0).min(cell.1 / size.1);
        if most.is_some() {
            scale = scale.min(1.0);
        }
        (size.0 * scale, size.1 * scale)
    };
    let covered = |columns: usize| {
        let (_, cell) = cell_for(columns);
        natural
            .iter()
            .map(|size| {
                let (width, height) = fit(*size, cell);
                width * height
            })
            .sum::<f64>()
    };
    let square = float(count).sqrt();
    let mut columns = 1;
    for trying in 2..=count {
        let (now, best) = (covered(trying), covered(columns));
        let nearer = (float(trying) - square).abs() < (float(columns) - square).abs();
        if now > best * (1.0 + 1e-9) || (now >= best * (1.0 - 1e-9) && nearer) {
            columns = trying;
        }
    }
    let (rows, cell) = cell_for(columns);
    let grid = (
        cell.0 * float(columns) + GAP * float(columns - 1),
        cell.1 * float(rows) + GAP * float(rows - 1),
    );
    let corner = match to {
        Some(to) => (from.0.min(to.0), from.1.min(to.1)),
        None => (
            (from.0 - grid.0 / 2.0).clamp(0.0, (page.0 - grid.0).max(0.0)),
            (from.1 - grid.1 / 2.0).clamp(0.0, (page.1 - grid.1).max(0.0)),
        ),
    };
    natural
        .iter()
        .enumerate()
        .map(|(at, size)| {
            let (column, row) = (at % columns, at / columns);
            let (width, height) = fit(*size, cell);
            let x0 = corner.0 + float(column) * (cell.0 + GAP) + (cell.0 - width) / 2.0;
            let y0 = corner.1 + float(row) * (cell.1 + GAP) + (cell.1 - height) / 2.0;
            [x0, y0, x0 + width, y0 + height]
        })
        .collect()
}

fn deck_at((x, y): (f32, f32), sizes: &[(f32, f32)]) -> Vec<[f32; 4]> {
    sizes
        .iter()
        .take(CARDS_SHOWN)
        .enumerate()
        .map(|(at, (width, height))| {
            let longest = width.max(*height).max(1.0);
            let scale = MINI / longest;
            let (wide, high) = (width * scale, height * scale);
            #[expect(clippy::cast_precision_loss, reason = "a count of at most CARDS_SHOWN")]
            let step = CARD_STEP * (at as f32);
            let (x0, y0) = (x + FROM_POINTER + step, y + FROM_POINTER + step);
            [x0, y0, x0 + wide, y0 + high]
        })
        .collect()
}

fn pixels(count: u32) -> f32 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "a picture's side in pixels, shown at a few dozen points"
    )]
    let float = count as f32;
    float
}

fn float(count: usize) -> f64 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "a count of pictures or of a grid's columns"
    )]
    let float = count as f64;
    float
}

fn picture_box(
    picture: (f64, f64),
    page: (f64, f64),
    from: (f64, f64),
    to: Option<(f64, f64)>,
) -> [f64; 4] {
    let ratio = picture.0 / picture.1;
    if let Some(to) = to {
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        let (mut width, mut height) = (dx.abs(), dy.abs());
        if width < height * ratio {
            height = width / ratio;
        } else {
            width = height * ratio;
        }
        let x0 = if dx < 0.0 { from.0 - width } else { from.0 };
        let y0 = if dy < 0.0 { from.1 - height } else { from.1 };
        return [x0, y0, x0 + width, y0 + height];
    }
    let mut width = picture.0 * POINTS_A_PIXEL;
    let mut height = picture.1 * POINTS_A_PIXEL;
    let shrink = (page.0 * MOST_OF_THE_PAGE / width)
        .min(page.1 * MOST_OF_THE_PAGE / height)
        .min(1.0);
    width *= shrink;
    height *= shrink;
    let x0 = (from.0 - width / 2.0).clamp(0.0, (page.0 - width).max(0.0));
    let y0 = (from.1 - height / 2.0).clamp(0.0, (page.1 - height).max(0.0));
    [x0, y0, x0 + width, y0 + height]
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    reason = "these boxes are halves and quarters, which arrive exactly"
)]
mod tests {
    use super::{
        CARD_STEP, CARDS_SHOWN, FROM_POINTER, GAP, MINI, deck_at, picture_box, picture_boxes,
    };

    #[test]
    fn one_picture_in_hand_is_one_card_of_its_own_shape() {
        let cards = deck_at((100.0, 200.0), &[(400.0, 200.0)]);
        assert_eq!(cards.len(), 1);
        let [x0, y0, x1, y1] = cards[0];
        assert_eq!((x0, y0), (100.0 + FROM_POINTER, 200.0 + FROM_POINTER));
        assert_eq!(x1 - x0, MINI);
        assert_eq!(y1 - y0, MINI / 2.0);
    }

    #[test]
    fn several_pictures_in_hand_are_a_deck_and_never_a_row() {
        let square = (300.0, 300.0);
        let cards = deck_at((0.0, 0.0), &[square; 6]);
        assert_eq!(cards.len(), CARDS_SHOWN, "the deck stops at {CARDS_SHOWN}");
        for (at, card) in cards.iter().enumerate() {
            #[expect(clippy::cast_precision_loss, reason = "a count of at most three")]
            let step = CARD_STEP * (at as f32);
            assert_eq!(card[0], FROM_POINTER + step);
            assert_eq!(card[1], FROM_POINTER + step);
        }
        let widest = cards
            .iter()
            .fold(0.0_f32, |widest, card| widest.max(card[2]))
            - cards[0][0];
        #[expect(clippy::cast_precision_loss, reason = "a count of at most three")]
        let shown = CARDS_SHOWN as f32;
        assert!(
            widest < CARD_STEP.mul_add(shown, MINI),
            "the deck is a deck: {widest}"
        );
    }

    #[test]
    fn a_picture_of_no_size_still_gets_a_card() {
        let cards = deck_at((0.0, 0.0), &[(0.0, 0.0)]);
        assert_eq!(cards.len(), 1);
        assert!(cards[0].iter().all(|value| value.is_finite()), "{cards:?}");
    }

    #[test]
    fn a_click_puts_a_picture_down_at_its_own_size_centred_on_the_click() {
        assert_eq!(
            picture_box((200.0, 100.0), (600.0, 800.0), (300.0, 400.0), None),
            [225.0, 362.5, 375.0, 437.5]
        );
    }

    #[test]
    fn a_click_near_an_edge_keeps_the_picture_on_the_page() {
        assert_eq!(
            picture_box((200.0, 100.0), (600.0, 800.0), (10.0, 790.0), None),
            [0.0, 725.0, 150.0, 800.0]
        );
    }

    #[test]
    fn a_picture_larger_than_the_page_is_made_to_fit_it() {
        assert_eq!(
            picture_box((4000.0, 2000.0), (600.0, 800.0), (300.0, 400.0), None),
            [60.0, 280.0, 540.0, 520.0]
        );
    }

    #[test]
    fn a_drag_gives_the_largest_box_of_its_shape_from_where_it_began() {
        assert_eq!(
            picture_box(
                (200.0, 100.0),
                (600.0, 800.0),
                (100.0, 100.0),
                Some((300.0, 400.0))
            ),
            [100.0, 100.0, 300.0, 200.0]
        );
        assert_eq!(
            picture_box(
                (200.0, 100.0),
                (600.0, 800.0),
                (300.0, 300.0),
                Some((0.0, 250.0))
            ),
            [200.0, 250.0, 300.0, 300.0]
        );
    }

    #[test]
    fn several_pictures_clicked_are_a_grid_of_their_own_sizes_centred_on_the_click() {
        let boxes = picture_boxes(&[(100.0, 100.0); 4], (600.0, 800.0), (300.0, 400.0), None);
        let side = 75.0;
        let left = 300.0 - side - GAP / 2.0;
        let top = 400.0 - side - GAP / 2.0;
        assert_eq!(
            boxes,
            vec![
                [left, top, left + side, top + side],
                [left + side + GAP, top, left + 2.0 * side + GAP, top + side],
                [left, top + side + GAP, left + side, top + 2.0 * side + GAP],
                [
                    left + side + GAP,
                    top + side + GAP,
                    left + 2.0 * side + GAP,
                    top + 2.0 * side + GAP
                ],
            ]
        );
    }

    #[test]
    fn several_pictures_dragged_fill_the_box_swept_and_keep_their_shape() {
        let boxes = picture_boxes(
            &[(200.0, 100.0), (200.0, 100.0)],
            (600.0, 800.0),
            (100.0, 100.0),
            Some((300.0, 500.0)),
        );
        assert_eq!(boxes.len(), 2);
        for [x0, y0, x1, y1] in &boxes {
            assert!((x1 - x0 - 2.0 * (y1 - y0)).abs() < 1e-9, "kept 2:1");
            assert!(*x0 >= 100.0 && *x1 <= 300.0 && *y0 >= 100.0 && *y1 <= 500.0);
        }
        assert_eq!(boxes[0][2] - boxes[0][0], 200.0, "as wide as the box");
        assert!(boxes[1][1] > boxes[0][3], "the second under the first");
    }

    #[test]
    fn many_pictures_clicked_near_an_edge_stay_on_the_page() {
        let boxes = picture_boxes(&[(4000.0, 3000.0); 9], (600.0, 800.0), (5.0, 795.0), None);
        assert_eq!(boxes.len(), 9);
        for [x0, y0, x1, y1] in boxes {
            assert!(x0 >= 0.0 && y0 >= 0.0 && x1 <= 600.0 && y1 <= 800.0);
        }
    }
}
