use pdf_content::PageGeometry;
use pdf_paint::{Matrix, Path, PathSegment, Point};

use crate::{RenderError, RenderLimits};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeviceTransform {
    pub matrix: Matrix,
    pub width: u32,
    pub height: u32,
    pub scale: f64,
}

impl DeviceTransform {
    #[must_use]
    pub fn user_point(&self, pixel: [f64; 2]) -> Option<[f64; 2]> {
        let point = self.matrix.inverse()?.transform(pdf_paint::Point {
            x: pixel[0],
            y: pixel[1],
        });
        Some([point.x, point.y])
    }

    #[must_use]
    pub fn device_box(&self, bounds: [f64; 4]) -> [f64; 4] {
        let corners = [
            (bounds[0], bounds[1]),
            (bounds[2], bounds[1]),
            (bounds[2], bounds[3]),
            (bounds[0], bounds[3]),
        ]
        .map(|(x, y)| {
            let point = self.matrix.transform(Point { x, y });
            [point.x, point.y]
        });
        let xs = corners.map(|corner| corner[0]);
        let ys = corners.map(|corner| corner[1]);
        [
            xs.iter().copied().fold(f64::INFINITY, f64::min),
            ys.iter().copied().fold(f64::INFINITY, f64::min),
            xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            ys.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        ]
    }

    #[must_use]
    pub fn pixel_box(&self, bounds: [f64; 4]) -> Option<[u32; 4]> {
        let [x0, y0, x1, y1] = self.device_box(bounds);
        if !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite()) {
            return None;
        }
        let low = |value: f64, limit: u32| -> u32 {
            let floored = value.floor().max(0.0);
            if floored >= f64::from(limit) {
                limit
            } else {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                {
                    floored as u32
                }
            }
        };
        let high = |value: f64, limit: u32| -> u32 {
            let ceiled = value.ceil().max(0.0);
            if ceiled >= f64::from(limit) {
                limit
            } else {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                {
                    ceiled as u32
                }
            }
        };
        let box_ = [
            low(x0, self.width),
            low(y0, self.height),
            high(x1, self.width),
            high(y1, self.height),
        ];
        if box_[2] <= box_[0] || box_[3] <= box_[1] {
            return None;
        }
        Some(box_)
    }

    pub fn for_page(
        geometry: &PageGeometry,
        scale: f64,
        limits: RenderLimits,
    ) -> Result<Self, RenderError> {
        if !scale.is_finite() || scale <= 0.0 {
            return Err(RenderError::InvalidScale);
        }
        let [x0, y0, x1, y1] = geometry.crop_box;
        let (pixels_wide, pixels_high) = geometry.rotated_size();
        let width = grid_extent(pixels_wide * scale)?;
        let height = grid_extent(pixels_high * scale)?;
        if (width as usize).saturating_mul(height as usize) > limits.max_pixels {
            return Err(RenderError::PageTooLarge);
        }
        let matrix = match geometry.rotate {
            90 => Matrix {
                a: 0.0,
                b: scale,
                c: scale,
                d: 0.0,
                e: -scale * y0,
                f: -scale * x0,
            },
            180 => Matrix {
                a: -scale,
                b: 0.0,
                c: 0.0,
                d: scale,
                e: scale * x1,
                f: -scale * y0,
            },
            270 => Matrix {
                a: 0.0,
                b: -scale,
                c: -scale,
                d: 0.0,
                e: scale * y1,
                f: scale * x1,
            },
            _ => Matrix {
                a: scale,
                b: 0.0,
                c: 0.0,
                d: -scale,
                e: -scale * x0,
                f: scale * y1,
            },
        };
        Ok(Self {
            matrix,
            width,
            height,
            scale,
        })
    }
}

fn grid_extent(pixels: f64) -> Result<u32, RenderError> {
    let rounded = pixels.ceil();
    if !rounded.is_finite() || rounded < 1.0 || rounded > f64::from(u32::MAX) {
        return Err(RenderError::PageTooLarge);
    }
    let mut extent = 1_u32;
    while f64::from(extent) < rounded {
        extent = extent.saturating_mul(2);
    }
    let mut low = 1_u32;
    let mut high = extent;
    while low < high {
        let middle = low + (high - low) / 2;
        if f64::from(middle) < rounded {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    Ok(low)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Polygon {
    pub contours: Vec<Vec<[f64; 2]>>,
}

impl Polygon {
    #[must_use]
    pub fn edge_count(&self) -> usize {
        self.contours.iter().map(Vec::len).sum()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.contours.iter().all(|contour| contour.len() < 3)
    }

    pub fn orient_uniformly(&mut self) {
        for contour in &mut self.contours {
            if signed_area(contour) < 0.0 {
                contour.reverse();
            }
        }
    }
}

fn signed_area(contour: &[[f64; 2]]) -> f64 {
    let mut total = 0.0;
    for index in 0..contour.len() {
        let [x0, y0] = contour[index];
        let [x1, y1] = contour[(index + 1) % contour.len()];
        total += x0.mul_add(y1, -(x1 * y0));
    }
    total / 2.0
}

pub fn flatten(path: &Path, matrix: Matrix, limits: RenderLimits) -> Result<Polygon, RenderError> {
    flatten_subpaths(path, matrix, limits).map(|(polygon, _)| polygon)
}

pub fn flatten_subpaths(
    path: &Path,
    matrix: Matrix,
    limits: RenderLimits,
) -> Result<(Polygon, Vec<bool>), RenderError> {
    let mut polygon = Polygon::default();
    let mut closed: Vec<bool> = Vec::new();
    let mut current: Vec<[f64; 2]> = Vec::new();
    let mut cursor = Point { x: 0.0, y: 0.0 };
    let mut start = cursor;
    let mut edges = 0_usize;
    for segment in &path.segments {
        match segment {
            PathSegment::MoveTo { point, .. } => {
                push_contour(&mut polygon, &mut closed, &mut current, false);
                cursor = *point;
                start = cursor;
                current.push(device(matrix, cursor));
            }
            PathSegment::LineTo { point, .. } => {
                if current.is_empty() {
                    current.push(device(matrix, cursor));
                }
                cursor = *point;
                current.push(device(matrix, cursor));
            }
            PathSegment::CubicTo {
                control_1,
                control_2,
                end,
                ..
            } => {
                if current.is_empty() {
                    current.push(device(matrix, cursor));
                }
                flatten_cubic(
                    device(matrix, cursor),
                    device(matrix, *control_1),
                    device(matrix, *control_2),
                    device(matrix, *end),
                    limits.max_flatten_depth,
                    &mut current,
                );
                cursor = *end;
            }
            PathSegment::ClosePath { .. } => {
                push_contour(&mut polygon, &mut closed, &mut current, true);
                cursor = start;
            }
            PathSegment::Rectangle {
                origin,
                width,
                height,
                ..
            } => {
                push_contour(&mut polygon, &mut closed, &mut current, false);
                let corners = [
                    *origin,
                    Point {
                        x: origin.x + width,
                        y: origin.y,
                    },
                    Point {
                        x: origin.x + width,
                        y: origin.y + height,
                    },
                    Point {
                        x: origin.x,
                        y: origin.y + height,
                    },
                ];
                polygon
                    .contours
                    .push(corners.iter().map(|point| device(matrix, *point)).collect());
                closed.push(true);
                cursor = *origin;
                start = cursor;
            }
        }
        edges += 1;
        if edges > limits.max_edges || polygon.edge_count() + current.len() > limits.max_edges {
            return Err(RenderError::PathTooComplex);
        }
    }
    push_contour(&mut polygon, &mut closed, &mut current, false);
    Ok((polygon, closed))
}

fn push_contour(
    polygon: &mut Polygon,
    closed: &mut Vec<bool>,
    current: &mut Vec<[f64; 2]>,
    is_closed: bool,
) {
    if current.len() >= 2 {
        polygon.contours.push(std::mem::take(current));
        closed.push(is_closed);
    } else {
        current.clear();
    }
}

fn device(matrix: Matrix, point: Point) -> [f64; 2] {
    let transformed = matrix.transform(point);
    [transformed.x, transformed.y]
}

fn flatten_cubic(
    start: [f64; 2],
    control_1: [f64; 2],
    control_2: [f64; 2],
    end: [f64; 2],
    depth: u32,
    out: &mut Vec<[f64; 2]>,
) {
    if depth == 0 || is_flat(start, control_1, control_2, end) {
        out.push(end);
        return;
    }
    let ab = midpoint(start, control_1);
    let bc = midpoint(control_1, control_2);
    let cd = midpoint(control_2, end);
    let abc = midpoint(ab, bc);
    let bcd = midpoint(bc, cd);
    let middle = midpoint(abc, bcd);
    flatten_cubic(start, ab, abc, middle, depth - 1, out);
    flatten_cubic(middle, bcd, cd, end, depth - 1, out);
}

fn midpoint(left: [f64; 2], right: [f64; 2]) -> [f64; 2] {
    [
        f64::midpoint(left[0], right[0]),
        f64::midpoint(left[1], right[1]),
    ]
}

const FLATNESS: f64 = 0.1;

fn is_flat(start: [f64; 2], control_1: [f64; 2], control_2: [f64; 2], end: [f64; 2]) -> bool {
    distance_to_chord(control_1, start, end) <= FLATNESS
        && distance_to_chord(control_2, start, end) <= FLATNESS
}

fn distance_to_chord(point: [f64; 2], start: [f64; 2], end: [f64; 2]) -> f64 {
    let dx = end[0] - start[0];
    let dy = end[1] - start[1];
    let length = dx.hypot(dy);
    if length <= f64::EPSILON {
        return (point[0] - start[0]).hypot(point[1] - start[1]);
    }
    let cross = dx.mul_add(point[1] - start[1], -(dy * (point[0] - start[0])));
    cross.abs() / length
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdf_bytes::{ByteStore, SourceId};
    use pdf_content::PageGeometry;

    use super::DeviceTransform;
    use crate::RenderLimits;

    fn geometry(crop_box: [f64; 4], rotate: u16) -> PageGeometry {
        let store = ByteStore::new(SourceId::new(9), Arc::<[u8]>::from(vec![0_u8; 4]));
        let span = store.span(0..1).expect("a span of the store");
        PageGeometry {
            media_box: [0.0, 0.0, 200.0, 100.0],
            media_box_span: span,
            crop_box,
            crop_box_span: Some(span),
            rotate,
            rotate_span: Some(span),
        }
    }

    fn close(left: [f64; 2], right: [f64; 2]) -> bool {
        (left[0] - right[0]).abs() < 1e-9 && (left[1] - right[1]).abs() < 1e-9
    }

    #[test]
    fn the_first_pixel_is_the_upper_left_of_the_crop_box() {
        let device = DeviceTransform::for_page(
            &geometry([10.0, 20.0, 110.0, 90.0], 0),
            1.0,
            RenderLimits::default(),
        )
        .expect("a page with area");
        let corner = device.user_point([0.0, 0.0]).expect("invertible");
        assert!(close(corner, [10.0, 90.0]), "{corner:?}");
        assert!(!close(corner, [0.0, 100.0]));
    }

    #[test]
    fn a_rotated_scaled_page_comes_back_to_the_same_user_point() {
        for rotate in [0, 90, 180, 270] {
            let device = DeviceTransform::for_page(
                &geometry([10.0, 20.0, 110.0, 90.0], rotate),
                2.0,
                RenderLimits::default(),
            )
            .expect("a page with area");
            let pixel = device
                .matrix
                .transform(pdf_paint::Point { x: 30.0, y: 40.0 });
            let back = device.user_point([pixel.x, pixel.y]).expect("invertible");
            assert!(close(back, [30.0, 40.0]), "rotate {rotate}: {back:?}");
        }
    }
}
