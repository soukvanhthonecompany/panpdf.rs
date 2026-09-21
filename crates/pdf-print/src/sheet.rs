use pdf_render::Canvas;

use crate::layout::{Placement, Sheet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SheetImage {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

impl SheetImage {
    fn blank(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            rgb: vec![255; width as usize * height as usize * 3],
        }
    }

    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 3] {
        let at = (y as usize * self.width as usize + x as usize) * 3;
        [self.rgb[at], self.rgb[at + 1], self.rgb[at + 2]]
    }

    fn set(&mut self, x: u32, y: u32, colour: [u8; 3]) {
        let at = (y as usize * self.width as usize + x as usize) * 3;
        self.rgb[at..at + 3].copy_from_slice(&colour);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SheetError {
    BadResolution,
    Page { page: usize, why: String },
}

impl std::fmt::Display for SheetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadResolution => {
                formatter.write_str("the sheet cannot be drawn at that resolution")
            }
            Self::Page { page, why } => {
                write!(formatter, "page {} cannot be drawn: {why}", page + 1)
            }
        }
    }
}

impl std::error::Error for SheetError {}

const MOST_PIXELS: f64 = 80_000_000.0;

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn pixels(points: f64, per_point: f64) -> i64 {
    (points * per_point).round() as i64
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn extent(points: f64, scale: f64) -> i64 {
    ((points * scale).ceil() as i64).max(1)
}

pub fn draw_sheet(
    sheet: &Sheet,
    dpi: f64,
    borders: bool,
    mut draw: impl FnMut(usize, f64, [u32; 4]) -> Result<Canvas, String>,
) -> Result<SheetImage, SheetError> {
    let per_point = dpi / 72.0;
    if !(per_point.is_finite() && per_point > 0.0)
        || sheet.size[0] * sheet.size[1] * per_point * per_point > MOST_PIXELS
    {
        return Err(SheetError::BadResolution);
    }
    let width = u32::try_from(pixels(sheet.size[0], per_point).max(1))
        .map_err(|_| SheetError::BadResolution)?;
    let height = u32::try_from(pixels(sheet.size[1], per_point).max(1))
        .map_err(|_| SheetError::BadResolution)?;
    let mut image = SheetImage::blank(width, height);
    for placement in &sheet.placements {
        place(&mut image, sheet, placement, per_point, &mut draw)?;
        if borders {
            border(&mut image, sheet, placement, per_point);
        }
    }
    Ok(image)
}

fn on_grid(sheet: &Sheet, bounds: [f64; 4], per_point: f64) -> [i64; 4] {
    [
        pixels(bounds[0], per_point),
        pixels(sheet.size[1] - bounds[3], per_point),
        pixels(bounds[2], per_point),
        pixels(sheet.size[1] - bounds[1], per_point),
    ]
}

fn place(
    image: &mut SheetImage,
    sheet: &Sheet,
    placement: &Placement,
    per_point: f64,
    draw: &mut impl FnMut(usize, f64, [u32; 4]) -> Result<Canvas, String>,
) -> Result<(), SheetError> {
    let scale = placement.scale * per_point;
    let (across, down) = (
        extent(placement.size[0], scale),
        extent(placement.size[1], scale),
    );
    let landing = on_grid(sheet, placement.landing(), per_point);
    let (left, top) = (landing[0], landing[1]);
    let (wide, high) = if placement.turned {
        (down, across)
    } else {
        (across, down)
    };
    let clip = on_grid(sheet, placement.clip, per_point);
    let shown = [
        left.max(clip[0]).max(0),
        top.max(clip[1]).max(0),
        (left + wide).min(clip[2]).min(i64::from(image.width)),
        (top + high).min(clip[3]).min(i64::from(image.height)),
    ];
    if shown[2] <= shown[0] || shown[3] <= shown[1] {
        return Ok(());
    }
    let region = if placement.turned {
        [
            across - (shown[3] - top),
            shown[0] - left,
            across - (shown[1] - top),
            shown[2] - left,
        ]
    } else {
        [
            shown[0] - left,
            shown[1] - top,
            shown[2] - left,
            shown[3] - top,
        ]
    };
    let region = region.map(|value| u32::try_from(value).unwrap_or(0));
    let canvas = draw(placement.page, scale, region).map_err(|why| SheetError::Page {
        page: placement.page,
        why,
    })?;
    for v in region[1]..region[3] {
        for u in region[0]..region[2] {
            let [red, green, blue] = canvas.pixel(u, v);
            let (x, y) = if placement.turned {
                (left + i64::from(v), top + across - 1 - i64::from(u))
            } else {
                (left + i64::from(u), top + i64::from(v))
            };
            if let (Ok(x), Ok(y)) = (u32::try_from(x), u32::try_from(y)) {
                image.set(x, y, [byte(red), byte(green), byte(blue)]);
            }
        }
    }
    Ok(())
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn border(image: &mut SheetImage, sheet: &Sheet, placement: &Placement, per_point: f64) {
    let [left, top, right, bottom] = on_grid(sheet, placement.landing(), per_point);
    let clip = on_grid(sheet, placement.clip, per_point);
    let thick = pixels(0.5, per_point).max(1);
    let (width, height) = (i64::from(image.width), i64::from(image.height));
    let mut paint = |x0: i64, y0: i64, x1: i64, y1: i64| {
        for y in y0.max(clip[1]).max(0)..y1.min(clip[3]).min(height) {
            for x in x0.max(clip[0]).max(0)..x1.min(clip[2]).min(width) {
                if let (Ok(x), Ok(y)) = (u32::try_from(x), u32::try_from(y)) {
                    image.set(x, y, [0, 0, 0]);
                }
            }
        }
    };
    paint(left, top, right, top + thick);
    paint(left, bottom - thick, right, bottom);
    paint(left, top, left + thick, bottom);
    paint(right - thick, top, right, bottom);
}
