use pdf_content::PageGeometry;

#[cfg(not(target_arch = "wasm32"))]
pub const TILE: u32 = 512;
#[cfg(target_arch = "wasm32")]
pub const TILE: u32 = 256;

pub const GAP: f64 = 12.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Strip {
    sizes: Vec<(f64, f64)>,
    tops: Vec<f64>,
    width: f64,
}

impl Strip {
    #[must_use]
    pub fn of(geometries: &[PageGeometry]) -> Self {
        let sizes: Vec<(f64, f64)> = geometries
            .iter()
            .map(PageGeometry::rotated_size)
            .map(|(width, height)| (width.max(1.0), height.max(1.0)))
            .collect();
        let mut tops = Vec::with_capacity(sizes.len() + 1);
        let mut y = 0.0;
        for (_, height) in &sizes {
            tops.push(y);
            y += height + GAP;
        }
        tops.push((y - GAP).max(0.0));
        let width = sizes
            .iter()
            .map(|(width, _)| *width)
            .fold(0.0_f64, f64::max)
            .max(1.0);
        Self { sizes, tops, width }
    }

    #[must_use]
    pub fn pages(&self) -> usize {
        self.sizes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sizes.is_empty()
    }

    #[must_use]
    pub fn size(&self) -> (f64, f64) {
        (self.width, self.tops.last().copied().unwrap_or(0.0))
    }

    #[must_use]
    pub fn page_size(&self, page: usize) -> Option<(f64, f64)> {
        self.sizes.get(page).copied()
    }

    #[must_use]
    pub fn origin(&self, page: usize) -> Option<(f64, f64)> {
        let (width, _) = self.sizes.get(page)?;
        Some((0.5 * (self.width - width), self.tops[page]))
    }

    #[must_use]
    pub fn between(&self, top: f64, bottom: f64) -> std::ops::Range<usize> {
        if self.sizes.is_empty() || bottom.partial_cmp(&top) != Some(std::cmp::Ordering::Greater) {
            return 0..0;
        }
        let first = self.first_reaching(top);
        let mut last = first;
        while last < self.sizes.len() && self.tops[last] < bottom {
            last += 1;
        }
        first..last
    }

    fn first_reaching(&self, y: f64) -> usize {
        let mut low = 0;
        let mut high = self.sizes.len();
        while low < high {
            let middle = low + (high - low) / 2;
            if self.tops[middle] + self.sizes[middle].1 <= y {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        low
    }

    #[must_use]
    pub fn locate(&self, at: (f64, f64)) -> Option<(usize, (f64, f64))> {
        let page = self.first_reaching(at.1);
        let (width, height) = *self.sizes.get(page)?;
        let (x0, y0) = self.origin(page)?;
        let (x, y) = (at.0 - x0, at.1 - y0);
        (x >= 0.0 && x < width && y >= 0.0 && y < height).then_some((page, (x, y)))
    }
}

#[must_use]
pub const fn boxes_overlap(one: [u32; 4], other: [u32; 4]) -> bool {
    one[0] < other[2] && other[0] < one[2] && one[1] < other[3] && other[1] < one[3]
}

#[must_use]
pub fn tiles_across(page: (u32, u32)) -> (u32, u32) {
    (page.0.div_ceil(TILE), page.1.div_ceil(TILE))
}

#[must_use]
pub fn tile_box(page: (u32, u32), col: u32, row: u32) -> Option<[u32; 4]> {
    let x0 = col.checked_mul(TILE)?;
    let y0 = row.checked_mul(TILE)?;
    if x0 >= page.0 || y0 >= page.1 {
        return None;
    }
    Some([x0, y0, (x0 + TILE).min(page.0), (y0 + TILE).min(page.1)])
}

#[must_use]
pub fn tiles_over(page: (u32, u32), window: [f64; 4]) -> Vec<(u32, u32)> {
    let (across, down) = tiles_across(page);
    let [x0, y0, x1, y1] = window;
    if !(x1 > x0 && y1 > y0) {
        return Vec::new();
    }
    let first_col = cell(x0);
    let first_row = cell(y0);
    let last_col = cell((x1 - 1.0).max(0.0)).min(across.saturating_sub(1));
    let last_row = cell((y1 - 1.0).max(0.0)).min(down.saturating_sub(1));
    let mut found = Vec::new();
    for row in first_row..=last_row {
        for col in first_col..=last_col {
            if row < down && col < across {
                found.push((col, row));
            }
        }
    }
    found
}

fn cell(value: f64) -> u32 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    let mut low = 0_u32;
    let mut high = u32::MAX / TILE;
    while low < high {
        let middle = low + (high - low).div_ceil(2);
        if f64::from(middle) * f64::from(TILE) <= value {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    low
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry(width: f64, height: f64, rotate: u16) -> PageGeometry {
        let span = pdf_bytes::SourceSpan::new(pdf_bytes::SourceId::new(1), 0, 0)
            .expect("an empty span at the start of a source");
        PageGeometry {
            media_box: [0.0, 0.0, width, height],
            media_box_span: span,
            crop_box: [0.0, 0.0, width, height],
            crop_box_span: None,
            rotate,
            rotate_span: None,
        }
    }

    fn strip_of(pages: &[(f64, f64)]) -> Strip {
        let geometries: Vec<_> = pages
            .iter()
            .map(|(width, height)| geometry(*width, *height, 0))
            .collect();
        Strip::of(&geometries)
    }

    #[test]
    fn a_page_starts_where_everything_above_it_ends() {
        let strip = strip_of(&[(600.0, 800.0), (600.0, 400.0), (600.0, 800.0)]);

        assert_eq!(strip.origin(0), Some((0.0, 0.0)));
        assert_eq!(strip.origin(1), Some((0.0, 800.0 + GAP)));
        assert_eq!(strip.origin(2), Some((0.0, 800.0 + GAP + 400.0 + GAP)));
        assert_eq!(strip.size(), (600.0, 800.0 + GAP + 400.0 + GAP + 800.0));
    }

    #[test]
    fn the_strip_ends_at_the_last_page_and_not_a_gap_later() {
        let strip = strip_of(&[(600.0, 800.0)]);

        assert_eq!(strip.size(), (600.0, 800.0));
    }

    #[test]
    fn a_narrow_page_is_centred_on_the_widest_one() {
        let strip = strip_of(&[(600.0, 800.0), (400.0, 800.0)]);

        assert_eq!(strip.size(), (600.0, 800.0 + GAP + 800.0));
        assert_eq!(strip.origin(1), Some((100.0, 800.0 + GAP)));
    }

    #[test]
    fn a_rotated_page_is_laid_out_at_the_size_it_is_shown() {
        let strip = Strip::of(&[geometry(600.0, 800.0, 90)]);

        assert_eq!(strip.page_size(0), Some((800.0, 600.0)));
    }

    #[test]
    fn only_the_pages_a_band_touches_are_between_its_edges() {
        let strip = strip_of(&[(600.0, 800.0), (600.0, 800.0), (600.0, 800.0)]);
        let second = 800.0 + GAP;

        assert_eq!(strip.between(0.0, 10.0), 0..1);
        assert_eq!(strip.between(799.0, 801.0), 0..1, "the gap is not a page");
        assert_eq!(strip.between(799.0, second + 1.0), 0..2);
        assert_eq!(strip.between(second, second + 10.0), 1..2);
        assert_eq!(strip.between(0.0, strip.size().1), 0..3);
        assert_eq!(
            strip.between(10.0, 10.0),
            0..0,
            "an empty band holds nothing"
        );
    }

    #[test]
    fn a_band_starting_in_a_gap_finds_the_page_below_it() {
        let strip = strip_of(&[(600.0, 800.0), (600.0, 800.0)]);

        assert_eq!(strip.between(805.0, 900.0), 1..2);
    }

    #[test]
    fn a_point_lands_on_a_page_or_on_nothing() {
        let strip = strip_of(&[(600.0, 800.0), (400.0, 800.0)]);

        assert_eq!(strip.locate((10.0, 10.0)), Some((0, (10.0, 10.0))));
        assert_eq!(strip.locate((10.0, 805.0)), None, "in the gap");
        let second = 800.0 + GAP;
        assert_eq!(strip.locate((110.0, second + 5.0)), Some((1, (10.0, 5.0))));
        assert_eq!(
            strip.locate((10.0, second + 5.0)),
            None,
            "beside the narrow page, which is paper"
        );
    }

    #[test]
    fn a_page_is_cut_into_tiles_that_cover_it_exactly_once() {
        let page = (TILE + 3, 2 * TILE);
        assert_eq!(tiles_across(page), (2, 2));

        assert_eq!(tile_box(page, 0, 0), Some([0, 0, TILE, TILE]));
        assert_eq!(
            tile_box(page, 1, 1),
            Some([TILE, TILE, TILE + 3, 2 * TILE]),
            "the last tile is clipped to the page"
        );
        assert_eq!(tile_box(page, 2, 0), None, "off the page");
    }

    #[test]
    fn a_window_asks_for_the_tiles_it_touches_and_no_others() {
        let page = (3 * TILE, 3 * TILE);
        let one = f64::from(TILE);

        assert_eq!(tiles_over(page, [0.0, 0.0, 1.0, 1.0]), vec![(0, 0)]);
        assert_eq!(
            tiles_over(page, [one - 1.0, one - 1.0, one + 1.0, one + 1.0]),
            vec![(0, 0), (1, 0), (0, 1), (1, 1)],
            "a window over a tile corner needs all four"
        );
        assert_eq!(
            tiles_over(page, [0.0, 0.0, one, one]),
            vec![(0, 0)],
            "a window ending exactly on a boundary does not need the next tile"
        );
        assert_eq!(tiles_over(page, [0.0, 0.0, 0.0, 0.0]), Vec::new());
    }

    #[test]
    fn tiles_come_back_in_reading_order() {
        let page = (2 * TILE, 2 * TILE);

        assert_eq!(
            tiles_over(
                page,
                [0.0, 0.0, 2.0 * f64::from(TILE), 2.0 * f64::from(TILE)]
            ),
            vec![(0, 0), (1, 0), (0, 1), (1, 1)]
        );
    }

    #[test]
    fn a_window_past_the_page_is_clamped_to_it() {
        let page = (TILE, TILE);

        assert_eq!(
            tiles_over(
                page,
                [0.0, 0.0, 10.0 * f64::from(TILE), 10.0 * f64::from(TILE)]
            ),
            vec![(0, 0)]
        );
    }

    #[test]
    fn boxes_share_a_pixel_or_they_do_not() {
        assert!(boxes_overlap([0, 0, 10, 10], [9, 9, 20, 20]));
        assert!(
            !boxes_overlap([0, 0, 10, 10], [10, 0, 20, 10]),
            "half-open: a box ending at 10 does not contain pixel 10"
        );
        assert!(!boxes_overlap([0, 0, 10, 10], [0, 10, 10, 20]));
        assert!(
            !boxes_overlap([0, 0, 0, 0], [0, 0, 10, 10]),
            "an empty box touches nothing"
        );
        assert!(boxes_overlap([5, 5, 6, 6], [0, 0, 100, 100]), "contained");
    }

    #[test]
    fn an_empty_document_lays_out_to_nothing() {
        let strip = Strip::of(&[]);

        assert!(strip.is_empty());
        assert_eq!(strip.size(), (1.0, 0.0));
        assert_eq!(strip.between(0.0, 100.0), 0..0);
        assert_eq!(strip.locate((0.0, 0.0)), None);
    }
}
