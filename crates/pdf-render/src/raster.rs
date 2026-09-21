use pdf_paint::FillRule;

use crate::geometry::Polygon;

const BANDS: usize = 4;

#[derive(Clone, Debug, PartialEq)]
pub struct Mask {
    x0: u32,
    y0: u32,
    width: u32,
    height: u32,
    coverage: Vec<f32>,
}

impl Mask {
    #[must_use]
    pub fn opaque(width: u32, height: u32) -> Self {
        Self {
            x0: 0,
            y0: 0,
            width,
            height,
            coverage: vec![1.0; pixel_count(width, height)],
        }
    }

    #[must_use]
    pub fn from_coverage(x0: u32, y0: u32, width: u32, height: u32, coverage: Vec<f32>) -> Self {
        assert_eq!(coverage.len(), pixel_count(width, height));
        Self {
            x0,
            y0,
            width,
            height,
            coverage,
        }
    }

    #[must_use]
    pub fn empty(_width: u32, _height: u32) -> Self {
        Self::over(0, 0, 0, 0)
    }

    fn over(x0: u32, y0: u32, width: u32, height: u32) -> Self {
        Self {
            x0,
            y0,
            width,
            height,
            coverage: vec![0.0; pixel_count(width, height)],
        }
    }

    #[must_use]
    pub fn bounds(&self) -> (u32, u32, u32, u32) {
        (
            self.x0,
            self.y0,
            self.x0 + self.width,
            self.y0 + self.height,
        )
    }

    #[must_use]
    pub fn at(&self, x: u32, y: u32) -> f32 {
        let Some(index) = self.index(x, y) else {
            return 0.0;
        };
        self.coverage[index]
    }

    fn index(&self, x: u32, y: u32) -> Option<usize> {
        let column = x.checked_sub(self.x0)?;
        let row = y.checked_sub(self.y0)?;
        if column >= self.width || row >= self.height {
            return None;
        }
        Some(row as usize * self.width as usize + column as usize)
    }

    pub fn intersect(&mut self, other: &Self) {
        let x0 = self.x0.max(other.x0);
        let y0 = self.y0.max(other.y0);
        let x1 = (self.x0 + self.width).min(other.x0 + other.width);
        let y1 = (self.y0 + self.height).min(other.y0 + other.height);
        if x1 <= x0 || y1 <= y0 {
            *self = Self::over(0, 0, 0, 0);
            return;
        }
        let mut narrowed = Self::over(x0, y0, x1 - x0, y1 - y0);
        for y in y0..y1 {
            for x in x0..x1 {
                let index =
                    y0.abs_diff(y) as usize * narrowed.width as usize + x0.abs_diff(x) as usize;
                narrowed.coverage[index] = self.at(x, y) * other.at(x, y);
            }
        }
        *self = narrowed;
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.coverage.iter().all(|value| *value <= 0.0)
    }
}

fn pixel_count(width: u32, height: u32) -> usize {
    width as usize * height as usize
}

struct Edge {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    winding: i32,
}

#[derive(Clone, Copy, Debug)]
struct Span {
    lowest: f64,
    highest: f64,
    leftmost: f64,
    rightmost: f64,
}

fn edges_of(polygon: &Polygon) -> (Vec<Edge>, Span) {
    let mut edges = Vec::new();
    let mut span = Span {
        lowest: f64::INFINITY,
        highest: f64::NEG_INFINITY,
        leftmost: f64::INFINITY,
        rightmost: f64::NEG_INFINITY,
    };
    for contour in &polygon.contours {
        if contour.len() < 3 {
            continue;
        }
        for index in 0..contour.len() {
            let start = contour[index];
            let end = contour[(index + 1) % contour.len()];
            if !start[0].is_finite()
                || !start[1].is_finite()
                || !end[0].is_finite()
                || !end[1].is_finite()
            {
                continue;
            }
            if start[1].total_cmp(&end[1]).is_eq() {
                continue;
            }
            span.lowest = span.lowest.min(start[1]).min(end[1]);
            span.highest = span.highest.max(start[1]).max(end[1]);
            span.leftmost = span.leftmost.min(start[0]).min(end[0]);
            span.rightmost = span.rightmost.max(start[0]).max(end[0]);
            edges.push(if start[1] < end[1] {
                Edge {
                    x0: start[0],
                    y0: start[1],
                    x1: end[0],
                    y1: end[1],
                    winding: 1,
                }
            } else {
                Edge {
                    x0: end[0],
                    y0: end[1],
                    x1: start[0],
                    y1: start[1],
                    winding: -1,
                }
            });
        }
    }
    (edges, span)
}

#[must_use]
pub fn rasterise_fill_within(
    polygon: &Polygon,
    rule: FillRule,
    bounds: (u32, u32, u32, u32),
) -> Mask {
    match rectangle_of(polygon) {
        Some(rect) => whole_pixels(rect, bounds, snapped_fill),
        None => rasterise_within(polygon, rule, bounds),
    }
}

#[must_use]
pub fn rasterise_clip_within(
    polygon: &Polygon,
    rule: FillRule,
    bounds: (u32, u32, u32, u32),
) -> Mask {
    match rectangle_of(polygon) {
        Some(rect) => whole_pixels(rect, bounds, snapped_clip),
        None => rasterise_within(polygon, rule, bounds),
    }
}

#[allow(clippy::float_cmp)]
fn rectangle_of(polygon: &Polygon) -> Option<[f64; 4]> {
    let [contour] = polygon.contours.as_slice() else {
        return None;
    };
    let corners = match contour.len() {
        4 => &contour[..],
        5 if contour[4] == contour[0] => &contour[..4],
        _ => return None,
    };
    let (first, second, third, fourth) = (corners[0], corners[1], corners[2], corners[3]);
    let flat = first[1] == second[1]
        && second[0] == third[0]
        && third[1] == fourth[1]
        && fourth[0] == first[0];
    let upright = first[0] == second[0]
        && second[1] == third[1]
        && third[0] == fourth[0]
        && fourth[1] == first[1];
    if !(flat || upright) {
        return None;
    }
    let (low_x, high_x) = (first[0].min(third[0]), first[0].max(third[0]));
    let (low_y, high_y) = (first[1].min(third[1]), first[1].max(third[1]));
    if high_x > low_x && high_y > low_y {
        Some([low_x, low_y, high_x, high_y])
    } else {
        None
    }
}

#[allow(clippy::cast_possible_truncation)]
fn snapped_fill(low: f64, high: f64) -> (f64, f64) {
    let near = f64::from(low as f32);
    let far = f64::from(high as f32);
    let extent = f64::from((far - near) as f32).ceil().max(1.0);
    let start = (near + far - extent).mul_add(0.5, -0.5).ceil();
    (start, start + extent)
}

fn snapped_clip(low: f64, high: f64) -> (f64, f64) {
    (low.floor(), high.ceil())
}

fn whole_pixels(
    rect: [f64; 4],
    bounds: (u32, u32, u32, u32),
    edges: fn(f64, f64) -> (f64, f64),
) -> Mask {
    let (bx0, by0, bx1, by1) = bounds;
    let (left, right) = edges(rect[0], rect[2]);
    let (top, bottom) = edges(rect[1], rect[3]);
    let x0 = pixel_edge(left, bx0, bx1);
    let x1 = pixel_edge(right, bx0, bx1);
    let y0 = pixel_edge(top, by0, by1);
    let y1 = pixel_edge(bottom, by0, by1);
    if x1 <= x0 || y1 <= y0 {
        return Mask::empty(0, 0);
    }
    let (width, height) = (x1 - x0, y1 - y0);
    Mask::from_coverage(x0, y0, width, height, vec![1.0; pixel_count(width, height)])
}

fn pixel_edge(value: f64, low: u32, high: u32) -> u32 {
    if !value.is_finite() || value <= f64::from(low) {
        return low;
    }
    bounded_index(value, high).max(low)
}

#[must_use]
pub fn rasterise_within(polygon: &Polygon, rule: FillRule, bounds: (u32, u32, u32, u32)) -> Mask {
    let (bx0, by0, bx1, by1) = bounds;
    if bx1 <= bx0 || by1 <= by0 {
        return Mask::empty(0, 0);
    }
    let (width, height) = (bx1, by1);
    let (edges, span) = edges_of(polygon);
    if edges.is_empty() {
        return Mask::empty(width, height);
    }
    let first_row = row_containing(span.lowest, height).max(by0);
    let last_row = row_containing(span.highest, height).min(by1 - 1);
    let first_column = column_at_or_before(span.leftmost, width).max(bx0);
    let last_column = column_at_or_before(span.rightmost, width).min(bx1 - 1);
    if last_row < first_row || last_column < first_column {
        return Mask::empty(0, 0);
    }
    let box_width = last_column - first_column + 1;
    let box_height = last_row - first_row + 1;
    let mut mask = Mask::over(first_column, first_row, box_width, box_height);

    let columns = box_width as usize;
    let stride = columns + 2;
    let rows = box_height as usize * BANDS;
    let mut cells = vec![0.0_f64; stride * rows];

    let origin_x = f64::from(first_column);
    #[allow(clippy::cast_precision_loss)]
    let scale = BANDS as f64;
    let origin_y = f64::from(first_row) * scale;
    for edge in &edges {
        let banded = Edge {
            x0: edge.x0,
            y0: edge.y0 * scale,
            x1: edge.x1,
            y1: edge.y1 * scale,
            winding: edge.winding,
        };
        accumulate(
            &mut cells, stride, columns, rows, origin_x, origin_y, &banded,
        );
    }

    for row in 0..box_height as usize {
        let mask_row = &mut mask.coverage[row * columns..(row + 1) * columns];
        for band in 0..BANDS {
            let cells_row = &row_slice(&cells, stride, row * BANDS + band)[..columns];
            let mut accumulated = 0.0_f64;
            for (column, cell) in cells_row.iter().enumerate() {
                accumulated += *cell;
                #[allow(clippy::cast_possible_truncation)]
                {
                    mask_row[column] += (apply_fill_rule(accumulated, rule) / scale) as f32;
                }
            }
        }
    }
    mask
}

fn row_slice(cells: &[f64], stride: usize, row: usize) -> &[f64] {
    &cells[row * stride..(row + 1) * stride]
}

fn apply_fill_rule(accumulated: f64, rule: FillRule) -> f64 {
    let magnitude = accumulated.abs();
    match rule {
        FillRule::Nonzero => magnitude.min(1.0),
        FillRule::EvenOdd => {
            let folded = magnitude % 2.0;
            if folded > 1.0 { 2.0 - folded } else { folded }
        }
    }
}

fn accumulate(
    cells: &mut [f64],
    stride: usize,
    columns: usize,
    rows: usize,
    origin_x: f64,
    origin_y: f64,
    edge: &Edge,
) {
    let direction = f64::from(edge.winding);
    let top = edge.y0 - origin_y;
    let bottom = edge.y1 - origin_y;
    let height = bottom - top;
    if !height.is_finite() || height <= 0.0 {
        return;
    }
    let left = edge.x0 - origin_x;
    let right = edge.x1 - origin_x;
    if !left.is_finite() || !right.is_finite() {
        return;
    }
    let slope = (right - left) / height;

    let entry = top.max(0.0);
    #[allow(clippy::cast_precision_loss)]
    let exit = bottom.min(rows as f64);
    if exit <= entry {
        return;
    }
    let mut x = slope.mul_add(entry - top, left);
    let mut row = floor_index(entry, rows.saturating_sub(1));
    loop {
        #[allow(clippy::cast_precision_loss)]
        let row_top = row as f64;
        let band_top = row_top.max(entry);
        let band_bottom = (row_top + 1.0).min(exit);
        if band_bottom <= band_top {
            break;
        }
        let next = slope.mul_add(band_bottom - band_top, x);
        let weight = (band_bottom - band_top) * direction;
        let cells_row = &mut cells[row * stride..(row + 1) * stride];
        clipped_band(cells_row, columns, x, next, weight);
        x = next;
        row += 1;
        if row >= rows {
            break;
        }
    }
}

fn clipped_band(cells_row: &mut [f64], columns: usize, from: f64, to: f64, weight: f64) {
    #[allow(clippy::cast_precision_loss)]
    let limit = columns as f64;
    if from.total_cmp(&to).is_eq() {
        band(
            cells_row,
            columns,
            from.clamp(0.0, limit),
            to.clamp(0.0, limit),
            weight,
        );
        return;
    }
    let mut cuts = [0.0_f64, 1.0, 1.0, 1.0];
    let mut held = 1;
    for boundary in [0.0, limit] {
        let at = (boundary - from) / (to - from);
        if at > 0.0 && at < 1.0 {
            cuts[held] = at;
            held += 1;
        }
    }
    cuts[..held].sort_by(f64::total_cmp);
    cuts[held] = 1.0;
    for part in 0..held {
        let (start, end) = (cuts[part], cuts[part + 1]);
        if end <= start {
            continue;
        }
        let x0 = (to - from).mul_add(start, from).clamp(0.0, limit);
        let x1 = (to - from).mul_add(end, from).clamp(0.0, limit);
        band(cells_row, columns, x0, x1, weight * (end - start));
    }
}

fn band(cells_row: &mut [f64], columns: usize, from: f64, to: f64, weight: f64) {
    if weight.total_cmp(&0.0).is_eq() {
        return;
    }
    let (low, high) = if from <= to { (from, to) } else { (to, from) };
    let first = floor_index(low, columns);
    let last = floor_index(high, columns);
    if first == last {
        #[allow(clippy::cast_precision_loss)]
        let inset = 0.5f64.mul_add(from + to, -(first as f64));
        cells_row[first] += weight * (1.0 - inset);
        cells_row[first + 1] += weight * inset;
        return;
    }
    let scale = (high - low).recip();
    #[allow(clippy::cast_precision_loss)]
    let low_fraction = low - first as f64;
    #[allow(clippy::cast_precision_loss)]
    let high_fraction = high - last as f64;
    let entering = 0.5 * scale * (1.0 - low_fraction) * (1.0 - low_fraction);
    let leaving = 0.5 * scale * high_fraction * high_fraction;
    cells_row[first] += weight * entering;
    if last == first + 1 {
        cells_row[last] += weight * (1.0 - entering - leaving);
    } else {
        let after_first = scale * (1.5 - low_fraction);
        cells_row[first + 1] += weight * (after_first - entering);
        for cell in &mut cells_row[first + 2..last] {
            *cell += weight * scale;
        }
        #[allow(clippy::cast_precision_loss)]
        let before_last = (last - first - 2) as f64 * scale + after_first;
        cells_row[last] += weight * (1.0 - before_last - leaving);
    }
    cells_row[last + 1] += weight * leaving;
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn floor_index(value: f64, limit: usize) -> usize {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    #[allow(clippy::cast_precision_loss)]
    let ceiling = limit as f64;
    if value >= ceiling {
        return limit;
    }
    value as usize
}

fn row_containing(value: f64, height: u32) -> u32 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    bounded_index(value.floor(), height.saturating_sub(1))
}

fn column_at_or_before(value: f64, width: u32) -> u32 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    bounded_index(value.floor(), width.saturating_sub(1))
}

fn bounded_index(value: f64, maximum: u32) -> u32 {
    if value >= f64::from(maximum) {
        return maximum;
    }
    let mut low = 0_u32;
    let mut high = maximum;
    while low < high {
        let middle = low + (high - low).div_ceil(2);
        if f64::from(middle) <= value {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    low
}

#[cfg(test)]
mod tests {
    use super::{BANDS, Mask, rasterise_clip_within, rasterise_fill_within, rasterise_within};
    use crate::geometry::Polygon;
    use pdf_paint::FillRule;

    fn rasterise_over(polygon: &Polygon, rule: FillRule, width: u32, height: u32) -> Mask {
        rasterise_within(polygon, rule, (0, 0, width, height))
    }

    fn square(x0: f64, y0: f64, x1: f64, y1: f64) -> Polygon {
        Polygon {
            contours: vec![vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]],
        }
    }

    fn total(mask: &Mask, width: u32, height: u32) -> f32 {
        let mut sum = 0.0;
        for y in 0..height {
            for x in 0..width {
                sum += mask.at(x, y);
            }
        }
        sum
    }

    fn strip(y0: f64, y1: f64, clockwise: bool) -> Vec<[f64; 2]> {
        let mut contour = vec![[0.0, y0], [1.0, y0], [1.0, y1], [0.0, y1]];
        if clockwise {
            contour.reverse();
        }
        contour
    }

    #[test]
    fn two_opposing_contours_in_one_pixel_do_not_cancel_each_other_away() {
        let only_a = Polygon {
            contours: vec![strip(0.0, 0.25, false)],
        };
        let both = Polygon {
            contours: vec![strip(0.0, 0.25, false), strip(0.75, 1.0, true)],
        };
        let at = |polygon: &Polygon, rule| rasterise_within(polygon, rule, (0, 0, 1, 1)).at(0, 0);

        assert!(
            (at(&only_a, FillRule::Nonzero) - 0.25).abs() < 1e-5,
            "control"
        );
        assert!((at(&both, FillRule::Nonzero) - 0.5).abs() < 1e-5, "nonzero");
        assert!(
            (at(&both, FillRule::EvenOdd) - 0.5).abs() < 1e-5,
            "even-odd"
        );
    }

    #[test]
    fn two_strips_wound_the_same_way_cover_what_they_cover() {
        let both = Polygon {
            contours: vec![strip(0.0, 0.25, false), strip(0.75, 1.0, false)],
        };
        let at = |rule| rasterise_within(&both, rule, (0, 0, 1, 1)).at(0, 0);

        assert!((at(FillRule::Nonzero) - 0.5).abs() < 1e-5);
        assert!((at(FillRule::EvenOdd) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn opposing_windings_inside_one_band_still_cancel_by_up_to_one_band() {
        let both = Polygon {
            contours: vec![strip(0.0, 0.125, false), strip(0.125, 0.25, true)],
        };
        let covered = rasterise_within(&both, FillRule::Nonzero, (0, 0, 1, 1)).at(0, 0);

        assert!(covered <= 0.25 + 1e-5);
        #[allow(clippy::cast_precision_loss)]
        let band = 1.0 / BANDS as f32;
        assert!(
            (0.25 - covered) <= band + 1e-5,
            "cancellation reached beyond one band: {covered}"
        );
    }

    #[test]
    fn a_small_shape_on_a_large_page_covers_its_own_area_and_holds_only_its_own_box() {
        let mask = rasterise_over(
            &square(10.0, 20.0, 14.0, 24.0),
            FillRule::Nonzero,
            1000,
            1000,
        );

        assert!((total(&mask, 1000, 1000) - 16.0).abs() < 1e-5);
        assert_eq!(mask.bounds(), (10, 20, 15, 25));
    }

    #[test]
    fn coverage_outside_the_box_reads_as_zero_rather_than_panicking() {
        let mask = rasterise_over(
            &square(10.0, 20.0, 14.0, 24.0),
            FillRule::Nonzero,
            1000,
            1000,
        );

        assert!(mask.at(9, 20) <= 0.0);
        assert!(mask.at(10, 19) <= 0.0);
        assert!(mask.at(14, 24) <= 0.0);
        assert!(mask.at(999, 999) <= 0.0);
        assert!((mask.at(10, 20) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn a_shape_crossing_the_grid_edge_is_clamped_to_the_page() {
        let mask = rasterise_over(&square(-5.0, -5.0, 3.0, 3.0), FillRule::Nonzero, 10, 10);

        assert_eq!(mask.bounds(), (0, 0, 4, 4));
        assert!((total(&mask, 10, 10) - 9.0).abs() < 1e-5);
    }

    #[test]
    fn intersecting_narrows_the_box_to_the_overlap() {
        let mut left = rasterise_over(&square(0.0, 0.0, 10.0, 10.0), FillRule::Nonzero, 100, 100);
        let right = rasterise_over(&square(6.0, 6.0, 20.0, 20.0), FillRule::Nonzero, 100, 100);
        left.intersect(&right);

        assert_eq!(left.bounds(), (6, 6, 11, 11));
        assert!((total(&left, 100, 100) - 16.0).abs() < 1e-5);
    }

    #[test]
    fn masks_that_do_not_overlap_intersect_to_nothing() {
        let mut left = rasterise_over(&square(0.0, 0.0, 4.0, 4.0), FillRule::Nonzero, 100, 100);
        let right = rasterise_over(&square(50.0, 50.0, 60.0, 60.0), FillRule::Nonzero, 100, 100);
        left.intersect(&right);

        assert!(left.is_empty());
        assert!(total(&left, 100, 100) <= 0.0);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn a_rectangle_is_filled_by_whole_pixels_and_never_half_covers_one() {
        let mask = rasterise_fill_within(
            &square(2.3, 2.0, 11.7, 14.0),
            FillRule::Nonzero,
            (0, 0, 16, 16),
        );
        for x in 0..16 {
            let coverage = mask.at(x, 8);
            assert!(
                coverage == 0.0 || coverage == 1.0,
                "column {x} is {coverage}, and a filled rectangle has no partial column",
            );
        }
        assert_eq!(mask.at(1, 8), 0.0);
        assert_eq!(mask.at(2, 8), 1.0);
        assert_eq!(mask.at(11, 8), 1.0);
        assert_eq!(mask.at(12, 8), 0.0);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn a_rectangular_clip_rounds_outwards_where_a_fill_centres() {
        let rectangle = square(2.8, 2.0, 11.2, 14.0);
        let fill = rasterise_fill_within(&rectangle, FillRule::Nonzero, (0, 0, 16, 16));
        let clip = rasterise_clip_within(&rectangle, FillRule::Nonzero, (0, 0, 16, 16));
        let inked = |mask: &Mask| (0..16).filter(|x| mask.at(*x, 8) > 0.0).count();
        assert_eq!(inked(&fill), 9);
        assert_eq!(inked(&clip), 10);
        assert_eq!(clip.at(2, 8), 1.0);
        assert_eq!(clip.at(11, 8), 1.0);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn a_rectangle_thinner_than_a_pixel_still_covers_one() {
        let mask = rasterise_fill_within(
            &square(3.4, 2.0, 3.6, 14.0),
            FillRule::Nonzero,
            (0, 0, 16, 16),
        );
        assert_eq!(mask.at(3, 8), 1.0);
        assert_eq!(mask.at(2, 8), 0.0);
        assert_eq!(mask.at(4, 8), 0.0);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn a_shape_that_is_not_a_rectangle_is_still_rasterised_exactly() {
        let mut polygon = square(2.3, 2.0, 11.7, 14.0);
        polygon.contours[0][2][0] = 11.9;
        let mask = rasterise_fill_within(&polygon, FillRule::Nonzero, (0, 0, 16, 16));
        let partial = (0..16)
            .flat_map(|x| (0..16).map(move |y| (x, y)))
            .filter(|(x, y)| {
                let coverage = mask.at(*x, *y);
                coverage > 0.0 && coverage < 1.0
            })
            .count();
        assert!(
            partial > 0,
            "a shape that is not a rectangle keeps its antialiased edges"
        );
    }

    #[test]
    fn an_opaque_mask_covers_the_whole_grid() {
        let mask = Mask::opaque(8, 4);

        assert_eq!(mask.bounds(), (0, 0, 8, 4));
        assert!((total(&mask, 8, 4) - 32.0).abs() < 1e-5);
    }
}
