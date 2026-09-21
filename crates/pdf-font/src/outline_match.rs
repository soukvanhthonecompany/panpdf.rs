use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::glyph::{GlyphPath, GlyphProgram, GlyphSegment};

const PIXELS_PER_EM: f64 = 64.0;
const GRID_LEFT_EM: f64 = -0.5;
const GRID_BOTTOM_EM: f64 = -0.75;
const GRID_SIDE: usize = 128;
const CURVE_STEPS: usize = 12;
const MOST_DISAGREEING: f64 = 0.05;
const LEAST_INK: usize = 6;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Raster {
    rows: Vec<u128>,
    ink: usize,
}

impl Raster {
    #[must_use]
    pub fn of(program: &GlyphProgram, glyph: u16) -> Option<Self> {
        let path = program.path(glyph)?;
        let units = f64::from(program.units_per_em().max(1));
        Self::fill(&path, units)
    }

    pub(crate) fn fill(path: &GlyphPath, units_per_em: f64) -> Option<Self> {
        let edges = flatten(path, units_per_em);
        let mut rows = vec![0_u128; GRID_SIDE];
        let mut ink = 0;
        for (row, bits) in rows.iter_mut().enumerate() {
            let y = centre(row);
            let mut crossings: Vec<(f64, i32)> = edges
                .iter()
                .filter_map(|&((x0, y0), (x1, y1))| {
                    let (low, high, winding) = if y0 < y1 { (y0, y1, 1) } else { (y1, y0, -1) };
                    (y >= low && y < high).then(|| (x0 + (y - y0) * (x1 - x0) / (y1 - y0), winding))
                })
                .collect();
            crossings.sort_by(|one, other| one.0.total_cmp(&other.0));
            let mut winding = 0;
            let mut from = 0.0;
            for (x, turn) in crossings {
                if winding == 0 {
                    from = x;
                }
                winding += turn;
                if winding == 0 {
                    for column in 0..GRID_SIDE {
                        let centre_x = centre(column);
                        if centre_x >= from && centre_x < x {
                            *bits |= 1 << column;
                        }
                    }
                }
            }
            ink += bits.count_ones() as usize;
        }
        (ink >= LEAST_INK).then_some(Self { rows, ink })
    }

    #[must_use]
    pub fn disagreement(&self, other: &Self) -> f64 {
        let differing: u32 = self
            .rows
            .iter()
            .zip(&other.rows)
            .map(|(one, two)| (one ^ two).count_ones())
            .sum();
        let most = self.ink.max(other.ink);
        if most == 0 {
            return 0.0;
        }
        f64::from(differing) / ink_f64(most)
    }

    #[must_use]
    pub fn stacked(&self, upper: &Self, gap: usize) -> Option<Self> {
        let top = self.rows.iter().rposition(|row| *row != 0)?;
        let bottom = upper.rows.iter().position(|row| *row != 0)?;
        let right = |rows: &[u128]| {
            rows.iter()
                .map(|bits| {
                    u32::try_from(GRID_SIDE)
                        .unwrap_or(u32::MAX)
                        .saturating_sub(bits.leading_zeros())
                })
                .max()
                .unwrap_or(0)
        };
        let across = i64::from(right(&self.rows)) - i64::from(right(&upper.rows));
        let shift = (top + 1 + gap).saturating_sub(bottom);
        let mut rows = self.rows.clone();
        for (row, bits) in upper.rows.iter().enumerate() {
            let moved = match across {
                0 => *bits,
                positive if positive > 0 => bits
                    .checked_shl(u32::try_from(positive).unwrap_or(u32::MAX))
                    .unwrap_or(0),
                negative => bits
                    .checked_shr(u32::try_from(-negative).unwrap_or(u32::MAX))
                    .unwrap_or(0),
            };
            if let Some(target) = rows.get_mut(row + shift) {
                *target |= moved;
            }
        }
        let ink = rows.iter().map(|bits| bits.count_ones() as usize).sum();
        (ink >= LEAST_INK).then_some(Self { rows, ink })
    }

    #[must_use]
    pub fn ink_box(&self) -> [f64; 4] {
        let mut columns = 0_u128;
        let (mut bottom, mut top) = (GRID_SIDE, 0);
        for (row, bits) in self.rows.iter().enumerate() {
            if *bits != 0 {
                bottom = bottom.min(row);
                top = top.max(row);
                columns |= bits;
            }
        }
        let left = columns.trailing_zeros() as usize;
        let right = GRID_SIDE - 1 - columns.leading_zeros() as usize;
        let edge = |index: usize| index_f64(index) / PIXELS_PER_EM;
        [
            GRID_LEFT_EM + edge(left),
            GRID_BOTTOM_EM + edge(bottom),
            GRID_LEFT_EM + edge(right + 1),
            GRID_BOTTOM_EM + edge(top + 1),
        ]
    }

    #[must_use]
    pub fn rows(&self) -> &[u128] {
        &self.rows
    }

    #[must_use]
    pub fn coverage(&self, cells: usize) -> Option<Vec<u8>> {
        if cells == 0 || !GRID_SIDE.is_multiple_of(cells) {
            return None;
        }
        let step = GRID_SIDE / cells;
        let whole = step * step;
        let mut out = Vec::with_capacity(cells * cells);
        for cell_row in 0..cells {
            for cell_column in 0..cells {
                let inked: usize = self.rows[cell_row * step..(cell_row + 1) * step]
                    .iter()
                    .map(|bits| {
                        let mask = ((1_u128 << step) - 1) << (cell_column * step);
                        (bits & mask).count_ones() as usize
                    })
                    .sum();
                out.push(u8::try_from(inked * 255 / whole).unwrap_or(u8::MAX));
            }
        }
        Some(out)
    }

    #[must_use]
    pub fn matches(&self, other: &Self) -> bool {
        let differing: u32 = self
            .rows
            .iter()
            .zip(&other.rows)
            .map(|(one, two)| (one ^ two).count_ones())
            .sum();
        let most = self.ink.max(other.ink);
        f64::from(differing) <= MOST_DISAGREEING * ink_f64(most)
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "a count of pixels on a 128 by 128 grid"
)]
const fn ink_f64(ink: usize) -> f64 {
    ink as f64
}

#[expect(clippy::cast_precision_loss, reason = "an index up to 128")]
const fn index_f64(index: usize) -> f64 {
    index as f64
}

#[expect(clippy::cast_precision_loss, reason = "an index below 128")]
fn centre(index: usize) -> f64 {
    (index as f64 + 0.5) / PIXELS_PER_EM
}

fn flatten(path: &GlyphPath, units_per_em: f64) -> Vec<((f64, f64), (f64, f64))> {
    let place = |x: f64, y: f64| {
        (
            x / units_per_em - GRID_LEFT_EM,
            y / units_per_em - GRID_BOTTOM_EM,
        )
    };
    let mut edges = Vec::new();
    let mut start = None;
    let mut pen = (0.0, 0.0);
    let close = |pen: (f64, f64), start: Option<(f64, f64)>, edges: &mut Vec<_>| {
        if let Some(start) = start
            && start != pen
        {
            edges.push((pen, start));
        }
    };
    for segment in &path.segments {
        match *segment {
            GlyphSegment::MoveTo { x, y } => {
                close(pen, start, &mut edges);
                pen = place(x, y);
                start = Some(pen);
            }
            GlyphSegment::LineTo { x, y } => {
                let to = place(x, y);
                edges.push((pen, to));
                pen = to;
            }
            GlyphSegment::CurveTo {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => {
                let (one, two, to) = (place(x1, y1), place(x2, y2), place(x, y));
                let mut last = pen;
                for step in 1..=CURVE_STEPS {
                    let t = step_f64(step) / step_f64(CURVE_STEPS);
                    let u = 1.0 - t;
                    let point = (
                        u * u * u * pen.0
                            + 3.0 * u * u * t * one.0
                            + 3.0 * u * t * t * two.0
                            + t * t * t * to.0,
                        u * u * u * pen.1
                            + 3.0 * u * u * t * one.1
                            + 3.0 * u * t * t * two.1
                            + t * t * t * to.1,
                    );
                    edges.push((last, point));
                    last = point;
                }
                pen = to;
            }
            GlyphSegment::Close => {
                close(pen, start, &mut edges);
                if let Some(start) = start {
                    pen = start;
                }
            }
        }
    }
    close(pen, start, &mut edges);
    edges
        .into_iter()
        .filter(|((_, y0), (_, y1))| (y0 - y1).abs() > f64::EPSILON)
        .collect()
}

#[expect(clippy::cast_precision_loss, reason = "a step count below 16")]
const fn step_f64(step: usize) -> f64 {
    step as f64
}

#[derive(Debug)]
pub struct Reference {
    glyphs: Vec<(u16, Raster, char)>,
}

impl Reference {
    #[must_use]
    pub fn of(program: &GlyphProgram) -> Self {
        let sfnt = match program {
            GlyphProgram::TrueType(font) => Some(font),
            GlyphProgram::Cff { sfnt, .. } => sfnt.as_deref(),
            GlyphProgram::Type1 { .. } => None,
        };
        let mut by_glyph: std::collections::BTreeMap<u16, Option<char>> =
            std::collections::BTreeMap::new();
        for (glyph, character) in sfnt
            .map(crate::truetype::TrueTypeFont::characters)
            .unwrap_or_default()
        {
            by_glyph
                .entry(glyph)
                .and_modify(|held| {
                    if *held != Some(character) {
                        *held = None;
                    }
                })
                .or_insert(Some(character));
        }
        let glyphs = by_glyph
            .into_iter()
            .filter_map(|(glyph, character)| {
                let character = character?;
                Some((glyph, Raster::of(program, glyph)?, character))
            })
            .collect();
        Self { glyphs }
    }

    #[must_use]
    pub fn character_of(&self, raster: &Raster, hint: Option<u16>) -> Option<char> {
        if let Some(hint) = hint
            && let Some((_, candidate, character)) =
                self.glyphs.iter().find(|(glyph, _, _)| *glyph == hint)
            && candidate.matches(raster)
        {
            return Some(*character);
        }
        let mut found: Option<char> = None;
        for (_, candidate, character) in &self.glyphs {
            if candidate.matches(raster) {
                match found {
                    None => found = Some(*character),
                    Some(held) if held == *character => {}
                    Some(_) => return None,
                }
            }
        }
        found
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.glyphs.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }
}

type FaceKey = (String, u32, String);

#[must_use]
pub fn reference_of(face: &crate::substitute::SubstitutedFace) -> Arc<Reference> {
    static BUILT: OnceLock<Mutex<HashMap<FaceKey, Arc<Reference>>>> = OnceLock::new();
    let key = (
        face.identity.sha256.clone(),
        face.identity.face_index,
        face.identity.origin.clone(),
    );
    let built = BUILT.get_or_init(Mutex::default);
    if let Some(found) = built.lock().ok().and_then(|held| held.get(&key).cloned()) {
        return found;
    }
    let reference = Arc::new(Reference::of(&face.program));
    if let Ok(mut held) = built.lock() {
        held.insert(key, Arc::clone(&reference));
    }
    reference
}

#[must_use]
pub fn is_same_family(face: &crate::substitute::SubstitutedFace, family: &str) -> bool {
    let wanted = crate::substitute::normalize_name(font_family(family));
    !wanted.is_empty() && crate::substitute::normalize_name(&face.identity.family) == wanted
}

#[must_use]
pub fn font_family(family: &str) -> &str {
    ["-Identity-H", "-Identity-V"]
        .iter()
        .find_map(|cmap| family.strip_suffix(cmap))
        .unwrap_or(family)
}

#[must_use]
pub fn index_in_name(name: &[u8]) -> Option<u16> {
    let digits = name.strip_prefix(b"g")?;
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(digits).ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(path: &mut GlyphPath, left: f64, bottom: f64, side: f64) {
        path.move_to(left, bottom);
        path.line_to(left + side, bottom);
        path.line_to(left + side, bottom + side);
        path.line_to(left, bottom + side);
        path.segments.push(GlyphSegment::Close);
    }

    fn raster(left: f64, bottom: f64, side: f64, units: f64) -> Raster {
        let mut path = GlyphPath::default();
        square(&mut path, left, bottom, side);
        Raster::fill(&path, units).expect("a square has ink")
    }

    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "pixel edges on a 64-to-the-em grid are exact binary fractions"
    )]
    fn coverage_says_which_cells_a_shape_inks() {
        let square = raster(0.0, 0.0, 500.0, 1000.0);
        let cells = square.coverage(8).expect("8 divides 128");
        let inked: Vec<usize> = (0..64).filter(|at| cells[*at] > 0).collect();
        assert_eq!(inked, vec![26, 27, 34, 35]);
        assert!(inked.iter().all(|at| cells[*at] == 255));
        assert_eq!(square.coverage(7), None);
        let mark = raster(100.0, 100.0, 125.0, 1000.0);
        let stacked = square.stacked(&mark, 1).expect("ink");
        assert_eq!(stacked.ink_box(), [0.0, 0.0, 0.5, 0.640_625]);
        let square_right = 128 - square.rows()[60].leading_zeros();
        assert_eq!(128 - stacked.rows()[85].leading_zeros(), square_right);
        assert!(mark.rows()[58] != 0);
        assert_ne!(128 - mark.rows()[58].leading_zeros(), square_right);
        assert_eq!(square.ink_box(), [0.0, 0.0, 0.5, 0.5]);
        let high = raster(250.0, 600.0, 125.0, 1000.0);
        assert_eq!(high.ink_box(), [0.25, 0.59375, 0.375, 0.71875]);
    }

    #[test]
    fn the_same_shape_in_two_unit_systems_matches() {
        let thousand = raster(100.0, 0.0, 500.0, 1000.0);
        let rounded = raster(205.0, 1.0, 1024.0, 2048.0);
        assert!(thousand.matches(&rounded));
    }

    #[test]
    fn disagreement_is_the_number_matches_tests_and_does_not_separate_letters() {
        let thousand = raster(100.0, 0.0, 500.0, 1000.0);
        let rounded = raster(205.0, 1.0, 1024.0, 2048.0);
        assert!(
            thousand.disagreement(&rounded) <= MOST_DISAGREEING,
            "the same shape: {}",
            thousand.disagreement(&rounded)
        );
        let moved = raster(100.0, 700.0, 500.0, 1000.0);
        assert!(
            (thousand.disagreement(&moved) - 2.0).abs() < 1e-9,
            "no shared ink at all: {}",
            thousand.disagreement(&moved)
        );
        let empty = Raster {
            rows: vec![0; GRID_SIDE],
            ink: 0,
        };
        assert!((empty.disagreement(&empty) - 0.0).abs() < 1e-9);
    }

    #[test]
    fn a_moved_or_resized_shape_does_not_match() {
        let base = raster(100.0, 0.0, 500.0, 1000.0);
        assert!(
            !base.matches(&raster(100.0, 700.0, 500.0, 1000.0)),
            "a mark above the line"
        );
        assert!(
            !base.matches(&raster(100.0, 0.0, 440.0, 1000.0)),
            "a smaller shape"
        );
        assert!(
            !base.matches(&raster(160.0, 0.0, 500.0, 1000.0)),
            "shifted along"
        );
    }

    #[test]
    fn a_counter_is_left_unfilled_by_winding() {
        let mut ring = GlyphPath::default();
        square(&mut ring, 100.0, 0.0, 500.0);
        ring.move_to(200.0, 100.0);
        ring.line_to(200.0, 400.0);
        ring.line_to(500.0, 400.0);
        ring.line_to(500.0, 100.0);
        ring.segments.push(GlyphSegment::Close);
        let ring = Raster::fill(&ring, 1000.0).expect("ink");
        assert!(!ring.matches(&raster(100.0, 0.0, 500.0, 1000.0)));
        assert!(ring.ink < raster(100.0, 0.0, 500.0, 1000.0).ink);
    }

    #[test]
    fn a_shape_two_characters_share_answers_nothing_unless_hinted() {
        let square = raster(100.0, 0.0, 500.0, 1000.0);
        let other = raster(100.0, 0.0, 300.0, 1000.0);
        let reference = Reference {
            glyphs: vec![
                (1, square.clone(), 'a'),
                (2, other.clone(), 'b'),
                (3, square.clone(), 'c'),
            ],
        };
        assert_eq!(reference.character_of(&other, None), Some('b'));
        assert_eq!(reference.character_of(&square, None), None, "a or c");
        assert_eq!(reference.character_of(&square, Some(3)), Some('c'));
        assert_eq!(
            reference.character_of(&square, Some(2)),
            None,
            "a hint whose outline differs is not taken on its word"
        );
        let same = Reference {
            glyphs: vec![(1, square.clone(), 'a'), (3, square.clone(), 'a')],
        };
        assert_eq!(same.character_of(&square, None), Some('a'));
    }

    #[test]
    fn a_subsetters_name_says_an_index_and_nothing_else_does() {
        assert_eq!(index_in_name(b"g302"), Some(302));
        assert_eq!(index_in_name(b"g"), None);
        assert_eq!(index_in_name(b"gcedilla"), None);
        assert_eq!(index_in_name(b"a302"), None);
    }
}
