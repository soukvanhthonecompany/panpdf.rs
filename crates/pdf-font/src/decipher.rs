use std::collections::HashMap;

use crate::outline_match::Raster;
use crate::tounicode::Code;

const CELLS: usize = 16;
const GRID: usize = 128;
const CANDIDATES: usize = 12;
const TEMPERATURE: f64 = 0.03;
const SHAPE_WEIGHT: f64 = 50.0;
const SHARED_CHARACTER: f64 = 2.0;
const PASSES: usize = 8;
const SMOOTHING: f64 = 0.1;

#[derive(Clone, Debug, PartialEq)]
pub struct Features {
    values: Vec<f32>,
    bands: usize,
}

impl Features {
    #[must_use]
    pub fn of(raster: &Raster) -> Self {
        let pixel = |row: usize, column: usize| (raster.rows()[row] >> column) & 1 == 1;
        let step = GRID / CELLS;
        let mut anchored = vec![0.0_f32; CELLS * CELLS];
        for (cell, value) in anchored.iter_mut().enumerate() {
            let (cell_row, cell_column) = (cell / CELLS, cell % CELLS);
            let mut inked = 0_u32;
            for row in cell_row * step..(cell_row + 1) * step {
                for column in cell_column * step..(cell_column + 1) * step {
                    inked += u32::from(pixel(row, column));
                }
            }
            *value = count_f32(inked) / count_f32(u32::try_from(step * step).unwrap_or(1));
        }
        let (mut bottom, mut top, mut left, mut right) = (GRID, 0, GRID, 0);
        for (row, bits) in raster.rows().iter().enumerate() {
            if *bits != 0 {
                bottom = bottom.min(row);
                top = top.max(row);
                left = left.min(bits.trailing_zeros() as usize);
                right = right.max(GRID - 1 - bits.leading_zeros() as usize);
            }
        }
        let boxed = |keep_aspect: bool| -> Vec<f32> {
            let mut cells = vec![0.0_f32; CELLS * CELLS];
            if bottom > top {
                return cells;
            }
            let (height, width) = (top - bottom + 1, right - left + 1);
            let (side_rows, side_columns) = if keep_aspect {
                (height.max(width), height.max(width))
            } else {
                (height, width)
            };
            let (row_from, column_from) = (
                i64::try_from(bottom).unwrap_or(0)
                    - i64::try_from((side_rows - height) / 2).unwrap_or(0),
                i64::try_from(left).unwrap_or(0)
                    - i64::try_from((side_columns - width) / 2).unwrap_or(0),
            );
            let edges = |length: usize| -> Vec<usize> {
                (0..=CELLS).map(|index| index * length / CELLS).collect()
            };
            let (rows, columns) = (edges(side_rows), edges(side_columns));
            for (cell, value) in cells.iter_mut().enumerate() {
                let (cell_row, cell_column) = (cell / CELLS, cell % CELLS);
                let row_span = rows[cell_row]..rows[cell_row + 1].max(rows[cell_row] + 1);
                let column_span =
                    columns[cell_column]..columns[cell_column + 1].max(columns[cell_column] + 1);
                let mut inked = 0_u32;
                let mut all = 0_u32;
                for row in row_span {
                    for column in column_span.clone() {
                        all += 1;
                        let (y, x) = (
                            row_from + i64::try_from(row).unwrap_or(0),
                            column_from + i64::try_from(column).unwrap_or(0),
                        );
                        if let (Ok(y), Ok(x)) = (usize::try_from(y), usize::try_from(x))
                            && y < GRID
                            && x < GRID
                        {
                            inked += u32::from(pixel(y, x));
                        }
                    }
                }
                *value = count_f32(inked) / count_f32(all.max(1));
            }
            cells
        };
        let bands = raster
            .rows()
            .iter()
            .zip(raster.rows().iter().skip(1))
            .filter(|(below, above)| **below == 0 && **above != 0)
            .count()
            + usize::from(raster.rows().first().is_some_and(|row| *row != 0));
        let mut values = blurred_unit(&anchored);
        values.extend(blurred_unit(&boxed(false)));
        values.extend(blurred_unit(&boxed(true)));
        Self { values, bands }
    }

    #[must_use]
    pub fn likeness(&self, other: &Self) -> f64 {
        let third = self.values.len() / 3;
        let dot = |range: std::ops::Range<usize>| {
            f64::from(
                self.values[range.clone()]
                    .iter()
                    .zip(&other.values[range])
                    .map(|(one, two)| one * two)
                    .sum::<f32>(),
            )
        };
        let weight = if self.bands >= 2 {
            STACKED_SHAPE_WEIGHT
        } else {
            1.0
        };
        let shape = f64::midpoint(dot(third..2 * third), dot(2 * third..3 * third));
        (dot(0..third) + weight * weight * shape) / (1.0 + weight * weight)
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "a count of pixels on a 128 by 128 grid"
)]
const fn count_f32(count: u32) -> f32 {
    count as f32
}

fn blurred_unit(grid: &[f32]) -> Vec<f32> {
    let side = CELLS;
    let mut out = vec![0.0_f32; side * side];
    for row in 0..side {
        for column in 0..side {
            let mut sum = 0.0;
            for dy in [-1_isize, 0, 1] {
                for dx in [-1_isize, 0, 1] {
                    let (y, x) = (row.checked_add_signed(dy), column.checked_add_signed(dx));
                    if let (Some(y), Some(x)) = (y, x)
                        && y < side
                        && x < side
                    {
                        sum += grid[y * side + x];
                    }
                }
            }
            out[row * side + column] = sum / 9.0;
        }
    }
    let length = out.iter().map(|value| value * value).sum::<f32>().sqrt();
    if length > 0.0 {
        for value in &mut out {
            *value /= length;
        }
    }
    out
}

#[derive(Clone, Debug, Default)]
pub struct Shapes {
    drawings: Vec<(String, Features)>,
}

impl Shapes {
    pub fn add_face(
        &mut self,
        program: &crate::glyph::GlyphProgram,
        characters: impl IntoIterator<Item = char>,
    ) {
        for character in characters {
            if let Some(raster) = raster_of(program, character) {
                self.drawings
                    .push((character.to_string(), Features::of(&raster)));
            }
        }
    }

    pub fn add_stacks(
        &mut self,
        program: &crate::glyph::GlyphProgram,
        lower: &[char],
        upper: &[char],
    ) {
        for below in lower {
            let Some(below_raster) = raster_of(program, *below) else {
                continue;
            };
            for above in upper {
                if let Some(stacked) = raster_of(program, *above)
                    .and_then(|above_raster| below_raster.stacked(&above_raster, STACK_GAP))
                {
                    self.drawings
                        .push((format!("{below}{above}"), Features::of(&stacked)));
                }
            }
        }
    }

    #[must_use]
    pub fn ranked(
        &self,
        features: &Features,
        allowed: impl Fn(&str) -> bool,
    ) -> Vec<(String, f64)> {
        let mut best: HashMap<&str, f64> = HashMap::new();
        for (text, drawing) in &self.drawings {
            if !allowed(text) {
                continue;
            }
            let likeness = features.likeness(drawing);
            let held = best.entry(text).or_insert(f64::NEG_INFINITY);
            if likeness > *held {
                *held = likeness;
            }
        }
        let mut ranked: Vec<(String, f64)> = best
            .into_iter()
            .map(|(text, likeness)| (text.to_owned(), likeness))
            .collect();
        ranked.sort_by(|one, other| other.1.total_cmp(&one.1).then(one.0.cmp(&other.0)));
        ranked
    }

    fn likeness_to(&self, features: &Features, text: &str) -> Option<f64> {
        self.drawings
            .iter()
            .filter(|(drawn, _)| drawn == text)
            .map(|(_, drawing)| features.likeness(drawing))
            .max_by(f64::total_cmp)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.drawings.is_empty()
    }
}

fn raster_of(program: &crate::glyph::GlyphProgram, character: char) -> Option<Raster> {
    program
        .glyph_for_char(character)
        .and_then(|glyph| Raster::of(program, glyph))
}

const STACKED_SHAPE_WEIGHT: f64 = 2.0;

const STACK_GAP: usize = 4;

pub const LAO_VOWELS_ABOVE: [char; 7] = ['ັ', 'ິ', 'ີ', 'ຶ', 'ື', 'ົ', 'ໍ'];
pub const LAO_TONES: [char; 4] = ['່', '້', '໊', '໋'];

#[derive(Clone, Debug)]
pub struct CharModel {
    pairs: HashMap<(Option<char>, Option<char>), f64>,
    singles: HashMap<Option<char>, f64>,
    after: HashMap<Option<char>, f64>,
    total: f64,
    vocabulary: usize,
}

impl CharModel {
    pub fn parse(table: &str) -> Result<Self, String> {
        let side = |field: &str| -> Result<Option<char>, String> {
            if field == "-" {
                return Ok(None);
            }
            u32::from_str_radix(field, 16)
                .ok()
                .and_then(char::from_u32)
                .map(Some)
                .ok_or_else(|| format!("{field} is not a character"))
        };
        let mut pairs = HashMap::new();
        let mut singles: HashMap<Option<char>, f64> = HashMap::new();
        let mut after: HashMap<Option<char>, f64> = HashMap::new();
        for (number, line) in table.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let fields: Vec<&str> = line.split_whitespace().collect();
            let [first, second, count] = fields.as_slice() else {
                return Err(format!(
                    "line {}: not two characters and a count",
                    number + 1
                ));
            };
            let (first, second) = (side(first)?, side(second)?);
            let count: f64 = count
                .parse::<u32>()
                .map(f64::from)
                .map_err(|_| format!("line {}: {count} is not a count", number + 1))?;
            *pairs.entry((first, second)).or_insert(0.0) += count;
            *singles.entry(second).or_insert(0.0) += count;
            *after.entry(first).or_insert(0.0) += count;
        }
        let total = singles.values().sum();
        let vocabulary = singles.len();
        Ok(Self {
            pairs,
            singles,
            after,
            total,
            vocabulary,
        })
    }

    #[must_use]
    pub fn knows(&self, character: char) -> bool {
        self.singles.contains_key(&Some(character))
    }

    #[must_use]
    pub fn knows_text(&self, text: &str) -> bool {
        !text.is_empty() && text.chars().all(|character| self.knows(character))
    }

    #[must_use]
    pub fn text_log_p(&self, before: Option<char>, text: &str) -> f64 {
        let mut previous = before;
        let mut score = 0.0;
        for character in text.chars() {
            score += self.log_p(previous, Some(character));
            previous = Some(character);
        }
        score
    }

    #[must_use]
    pub fn log_p(&self, first: Option<char>, second: Option<char>) -> f64 {
        let pair = self.pairs.get(&(first, second)).copied().unwrap_or(0.0);
        let single = self.singles.get(&second).copied().unwrap_or(0.0);
        let context = self.after.get(&first).copied().unwrap_or(0.0);
        #[expect(clippy::cast_precision_loss, reason = "a count of characters")]
        let unseen = SMOOTHING * (single + 1.0) / (self.total + self.vocabulary as f64);
        ((pair + unseen) / (context + SMOOTHING)).ln()
    }
}

#[must_use]
pub fn lao_model() -> Option<&'static CharModel> {
    static MODEL: std::sync::OnceLock<Option<CharModel>> = std::sync::OnceLock::new();
    MODEL
        .get_or_init(|| CharModel::parse(include_str!("../data/lao.pairs")).ok())
        .as_ref()
}

pub const LAO_LETTER: char = '\u{0E81}';

impl CharModel {
    #[must_use]
    pub fn characters(&self) -> Vec<char> {
        let mut characters: Vec<char> = self
            .singles
            .keys()
            .filter_map(|character| *character)
            .collect();
        characters.sort_unstable();
        characters
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cipher {
    pub codes: Vec<Code>,
    pub drawings: Vec<Option<Features>>,
    pub claims: Vec<Option<char>>,
    pub lines: Vec<Vec<usize>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    pub characters: Vec<String>,
    pub gain: f64,
    pub evidence: f64,
    pub changed: f64,
    pub fluency: f64,
    pub undecided: Vec<bool>,
}

pub const LEAST_GAIN: f64 = 0.10;
pub const LEAST_CHANGED: f64 = 0.95;
pub const LEAST_EVIDENCE: f64 = 15.0;
pub const LEAST_FLUENCY: f64 = -5.5;

impl Reading {
    #[must_use]
    pub fn outweighs_the_file(&self) -> bool {
        self.gain >= LEAST_GAIN
            && self.changed >= LEAST_CHANGED
            && self.evidence >= LEAST_EVIDENCE
            && self.fluency >= LEAST_FLUENCY
    }
}

#[must_use]
pub fn decipher(cipher: &Cipher, shapes: &Shapes, model: &CharModel) -> Reading {
    let candidates: Vec<Vec<(String, f64)>> = cipher
        .drawings
        .iter()
        .map(|drawing| candidates_of(drawing.as_ref(), shapes, model))
        .collect();
    let neighbours = Neighbours::of(cipher);
    let characters = solve(cipher, &candidates, &neighbours, model);
    verdict(cipher, shapes, model, &neighbours, characters)
}

fn candidates_of(
    drawing: Option<&Features>,
    shapes: &Shapes,
    model: &CharModel,
) -> Vec<(String, f64)> {
    let space = || vec![(" ".to_owned(), 0.0)];
    let Some(features) = drawing else {
        return space();
    };
    let mut ranked = shapes.ranked(features, |text| {
        model.knows_text(text) && (features.bands >= 2 || text.chars().nth(1).is_none())
    });
    ranked.truncate(CANDIDATES);
    let Some(top) = ranked.first().map(|(_, likeness)| *likeness) else {
        return space();
    };
    let normaliser = ranked
        .iter()
        .map(|(_, likeness)| ((likeness - top) / TEMPERATURE).exp())
        .sum::<f64>()
        .ln();
    ranked
        .into_iter()
        .map(|(text, likeness)| (text, (likeness - top) / TEMPERATURE - normaliser))
        .collect()
}

fn first(text: &str) -> Option<char> {
    text.chars().next()
}

fn last(text: &str) -> Option<char> {
    text.chars().next_back()
}

struct Neighbours {
    following: Vec<HashMap<Option<usize>, f64>>,
    preceding: Vec<HashMap<Option<usize>, f64>>,
    shown: Vec<f64>,
}

impl Neighbours {
    fn of(cipher: &Cipher) -> Self {
        let count = cipher.codes.len();
        let mut neighbours = Self {
            following: vec![HashMap::new(); count],
            preceding: vec![HashMap::new(); count],
            shown: vec![0.0; count],
        };
        for line in &cipher.lines {
            let mut previous: Option<usize> = None;
            for &code in line {
                neighbours.shown[code] += 1.0;
                *neighbours.preceding[code].entry(previous).or_insert(0.0) += 1.0;
                if let Some(previous) = previous {
                    *neighbours.following[previous]
                        .entry(Some(code))
                        .or_insert(0.0) += 1.0;
                }
                previous = Some(code);
            }
            if let Some(last) = previous {
                *neighbours.following[last].entry(None).or_insert(0.0) += 1.0;
            }
        }
        neighbours
    }

    fn busy(&self, code: usize) -> f64 {
        self.following[code].values().sum::<f64>() + self.preceding[code].values().sum::<f64>()
    }

    fn fit(&self, code: usize, text: &str, reading: &[String], model: &CharModel) -> f64 {
        let mut score = 0.0;
        for (next, times) in &self.following[code] {
            let next = next.and_then(|next| {
                if next == code {
                    first(text)
                } else {
                    first(&reading[next])
                }
            });
            score += times * model.log_p(last(text), next);
        }
        for (previous, times) in &self.preceding[code] {
            if *previous == Some(code) {
                score += times
                    * model.text_log_p(first(text), &text[first(text).map_or(0, char::len_utf8)..]);
                continue;
            }
            let before = previous.and_then(|previous| last(&reading[previous]));
            score += times * model.text_log_p(before, text);
        }
        score
    }
}

fn solve(
    cipher: &Cipher,
    candidates: &[Vec<(String, f64)>],
    neighbours: &Neighbours,
    model: &CharModel,
) -> Vec<String> {
    let count = cipher.codes.len();
    let mut reading: Vec<String> = candidates.iter().map(|each| each[0].0.clone()).collect();
    let mut order: Vec<usize> = (0..count).collect();
    order.sort_by(|one, other| {
        neighbours
            .busy(*other)
            .total_cmp(&neighbours.busy(*one))
            .then(one.cmp(other))
    });
    for _ in 0..PASSES {
        let mut changed = false;
        for &code in &order {
            let outings = neighbours.following[code].values().sum::<f64>() + 1.0;
            let mut best: Option<(usize, f64)> = None;
            for (at, (text, shape)) in candidates[code].iter().enumerate() {
                #[expect(clippy::cast_precision_loss, reason = "a count of codes")]
                let shared = (0..count)
                    .filter(|other| {
                        *other != code
                            && cipher.drawings[*other].is_some()
                            && reading[*other] == *text
                    })
                    .count() as f64;
                let score = neighbours.fit(code, text, &reading, model) + SHAPE_WEIGHT * shape
                    - SHARED_CHARACTER * shared * outings;
                if best.is_none_or(|(_, held)| score > held) {
                    best = Some((at, score));
                }
            }
            if let Some((at, _)) = best
                && candidates[code][at].0 != reading[code]
            {
                reading[code].clone_from(&candidates[code][at].0);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    reading
}

fn verdict(
    cipher: &Cipher,
    shapes: &Shapes,
    model: &CharModel,
    neighbours: &Neighbours,
    reading: Vec<String>,
) -> Reading {
    let undecided = undecided_codes(cipher, shapes, neighbours, &reading);
    let (mut gained, mut evidence, mut changed, mut drawn) = (0.0, 0.0, 0.0, 0.0);
    for (code, shown) in neighbours.shown.iter().enumerate() {
        let Some(features) = &cipher.drawings[code] else {
            continue;
        };
        drawn += shown;
        let agrees = cipher.claims[code].is_some_and(|claim| {
            let mut characters = reading[code].chars();
            characters.next() == Some(claim) && characters.next().is_none()
        });
        changed += shown * f64::from(u8::from(!agrees));
        let Some(claimed) =
            cipher.claims[code].and_then(|claim| shapes.likeness_to(features, &claim.to_string()))
        else {
            continue;
        };
        let read = shapes.likeness_to(features, &reading[code]).unwrap_or(0.0);
        gained += shown * (read - claimed);
        evidence += shown;
    }
    let (mut likely, mut steps) = (0.0, 0.0);
    for line in &cipher.lines {
        let mut previous = None;
        for &code in line {
            likely += model.text_log_p(previous, &reading[code]);
            #[expect(clippy::cast_precision_loss, reason = "a character count")]
            let characters = reading[code].chars().count() as f64;
            steps += characters;
            previous = last(&reading[code]).or(previous);
        }
        likely += model.log_p(previous, None);
        steps += 1.0;
    }
    Reading {
        characters: reading,
        gain: gained / f64::max(evidence, 1.0),
        evidence,
        changed: changed / f64::max(drawn, 1.0),
        fluency: likely / f64::max(steps, 1.0),
        undecided,
    }
}

fn undecided_codes(
    cipher: &Cipher,
    shapes: &Shapes,
    neighbours: &Neighbours,
    reading: &[String],
) -> Vec<bool> {
    let likeness = |code: usize| -> f64 {
        cipher.drawings[code]
            .as_ref()
            .and_then(|features| shapes.likeness_to(features, &reading[code]))
            .unwrap_or(f64::NEG_INFINITY)
    };
    let better = |code: usize, than: usize| -> bool {
        let (shown, held) = (neighbours.shown[code], neighbours.shown[than]);
        match shown.total_cmp(&held) {
            std::cmp::Ordering::Greater => true,
            std::cmp::Ordering::Equal => likeness(code) > likeness(than),
            std::cmp::Ordering::Less => false,
        }
    };
    let mut best: HashMap<&str, usize> = HashMap::new();
    for (code, text) in reading.iter().enumerate() {
        if cipher.drawings[code].is_none() {
            continue;
        }
        let held = best.entry(text.as_str()).or_insert(code);
        if better(code, *held) {
            *held = code;
        }
    }
    (0..cipher.codes.len())
        .map(|code| {
            cipher.drawings[code].is_some()
                && best.get(reading[code].as_str()).copied() != Some(code)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{CharModel, Cipher, Features, Shapes, decipher};
    use crate::glyph::{GlyphPath, GlyphSegment};
    use crate::outline_match::Raster;
    use crate::tounicode::Code;

    fn drawing(boxes: &[[f64; 4]]) -> Features {
        let mut path = GlyphPath::default();
        for [left, bottom, right, top] in boxes {
            path.move_to(*left, *bottom);
            path.line_to(*right, *bottom);
            path.line_to(*right, *top);
            path.line_to(*left, *top);
            path.segments.push(GlyphSegment::Close);
        }
        Features::of(&Raster::fill(&path, 1000.0).expect("ink"))
    }

    const TALL: [f64; 4] = [100.0, 0.0, 300.0, 700.0];
    const WIDE: [f64; 4] = [0.0, 0.0, 600.0, 200.0];
    const HIGH: [f64; 4] = [100.0, 800.0, 250.0, 950.0];

    fn shapes(entries: &[(char, &[[f64; 4]])]) -> Shapes {
        Shapes {
            drawings: entries
                .iter()
                .map(|(character, boxes)| (character.to_string(), drawing(boxes)))
                .collect(),
        }
    }

    fn code(value: u32) -> Code {
        Code { value, byte_len: 1 }
    }

    fn raster(boxes: &[[f64; 4]]) -> Raster {
        let mut path = GlyphPath::default();
        for [left, bottom, right, top] in boxes {
            path.move_to(*left, *bottom);
            path.line_to(*right, *bottom);
            path.line_to(*right, *top);
            path.line_to(*left, *top);
            path.segments.push(GlyphSegment::Close);
        }
        Raster::fill(&path, 1000.0).expect("ink")
    }

    #[test]
    fn a_vowel_and_tone_drawn_as_one_glyph_read_as_both() {
        const VOWEL: [f64; 4] = [50.0, 560.0, 450.0, 650.0];
        const TONE: [f64; 4] = [300.0, 700.0, 380.0, 900.0];
        let (vowel, tone) = (raster(&[VOWEL]), raster(&[TONE]));
        let stacked = vowel.stacked(&tone, super::STACK_GAP).expect("ink");
        let mut shapes = shapes(&[
            ('ຊ', &[TALL]),
            ('ນ', &[WIDE]),
            ('ັ', &[VOWEL]),
            ('້', &[TONE]),
        ]);
        shapes
            .drawings
            .push(("ັ້".to_owned(), Features::of(&stacked)));
        let model = CharModel::parse(
            "- 0E8A 9\n0E8A 0EB1 9\n0EB1 0EC9 9\n0EC9 0E99 9\n0EB1 0E99 9\n0E99 - 9\n",
        )
        .unwrap();
        let cipher = Cipher {
            codes: vec![code(1), code(2), code(3), code(4)],
            drawings: vec![
                Some(drawing(&[TALL])),
                Some(Features::of(&stacked)),
                Some(drawing(&[WIDE])),
                Some(drawing(&[VOWEL])),
            ],
            claims: vec![None; 4],
            lines: vec![vec![0, 1, 2], vec![0, 3, 2]],
        };
        assert_eq!(Features::of(&stacked).bands, 2);
        assert_eq!(drawing(&[VOWEL]).bands, 1);
        let reading = decipher(&cipher, &shapes, &model);
        assert_eq!(reading.characters, ["ຊ", "ັ້", "ນ", "ັ"]);
        assert_eq!(reading.undecided, [false, false, false, false]);
        let offered = super::candidates_of(cipher.drawings[3].as_ref(), &shapes, &model);
        assert!(
            offered.iter().all(|(text, _)| text.chars().count() == 1),
            "{offered:?}"
        );
        let offered = super::candidates_of(cipher.drawings[1].as_ref(), &shapes, &model);
        assert!(offered.iter().any(|(text, _)| text == "ັ້"));
    }

    #[test]
    fn the_worse_of_two_codes_read_alike_is_left_to_the_file() {
        const NEAR: [f64; 4] = [110.0, 0.0, 305.0, 690.0];
        let shapes = shapes(&[('ກ', &[TALL]), ('ຂ', &[WIDE])]);
        let model = CharModel::parse("- 0E81 6\n0E81 0E82 6\n0E82 0E81 6\n0E81 - 6\n").unwrap();
        let cipher = Cipher {
            codes: vec![code(1), code(2), code(3)],
            drawings: vec![
                Some(drawing(&[NEAR])),
                Some(drawing(&[WIDE])),
                Some(drawing(&[TALL])),
            ],
            claims: vec![Some('x'), Some('y'), Some('z')],
            lines: vec![vec![0, 1], vec![2, 1]],
        };
        let reading = decipher(&cipher, &shapes, &model);
        assert_eq!(reading.characters, ["ກ", "ຂ", "ກ"]);
        assert_eq!(
            reading.undecided,
            [true, false, false],
            "shown as often as each other, the exact drawing keeps ກ"
        );

        let often = Cipher {
            drawings: vec![
                Some(drawing(&[TALL])),
                Some(drawing(&[WIDE])),
                Some(drawing(&[NEAR])),
            ],
            lines: std::iter::repeat_n(vec![2, 1], 5)
                .chain(std::iter::once(vec![0, 1]))
                .collect(),
            ..cipher
        };
        let reading = decipher(&often, &shapes, &model);
        assert_eq!(reading.characters, ["ກ", "ຂ", "ກ"]);
        assert_eq!(reading.undecided, [true, false, false]);
    }

    #[test]
    fn each_gate_alone_keeps_the_file() {
        use super::{LEAST_CHANGED, LEAST_EVIDENCE, LEAST_FLUENCY, LEAST_GAIN, Reading};
        let passing = Reading {
            characters: Vec::new(),
            undecided: Vec::new(),
            gain: LEAST_GAIN,
            evidence: LEAST_EVIDENCE,
            changed: LEAST_CHANGED,
            fluency: LEAST_FLUENCY,
        };
        assert!(passing.outweighs_the_file());
        let short = 1e-6;
        for failing in [
            Reading {
                gain: LEAST_GAIN - short,
                ..passing.clone()
            },
            Reading {
                evidence: LEAST_EVIDENCE - short,
                ..passing.clone()
            },
            Reading {
                changed: LEAST_CHANGED - short,
                ..passing.clone()
            },
            Reading {
                fluency: LEAST_FLUENCY - short,
                ..passing.clone()
            },
        ] {
            assert!(!failing.outweighs_the_file(), "{failing:?}");
        }
    }

    #[test]
    fn the_carried_lao_model_reads_and_knows_the_script() {
        let model = super::lao_model().expect("data/lao.pairs reads");
        let characters = model.characters();
        assert!(characters.contains(&'ກ') && characters.contains(&'່') && characters.contains(&' '));
        assert!(model.log_p(Some('ກ'), Some('າ')) > model.log_p(Some('ກ'), None));
    }

    #[test]
    fn a_pair_table_reads_and_counts_the_pairs_it_names() {
        let model = CharModel::parse("# pairs\n- 0E81 3\n0E81 0E82 3\n0E82 - 3\n").unwrap();
        assert!(model.knows('ກ') && model.knows('ຂ') && !model.knows('A'));
        assert!(model.log_p(Some('ກ'), Some('ຂ')) > model.log_p(Some('ກ'), Some('ກ')));
        assert!(CharModel::parse("0E81 3").is_err());
        assert!(CharModel::parse("ZZ 0E81 3").is_err());
    }

    #[test]
    fn a_font_is_read_as_its_glyphs_draw_it_and_not_as_the_file_says() {
        let shapes = shapes(&[
            ('ກ', &[TALL]),
            ('ຂ', &[WIDE]),
            ('ຄ', &[HIGH]),
            ('x', &[[0.0, 0.0, 500.0, 500.0]]),
            ('y', &[[0.0, 300.0, 500.0, 500.0]]),
            ('z', &[[400.0, 0.0, 500.0, 900.0]]),
        ]);
        let model = CharModel::parse("- 0E81 5\n0E81 0E82 5\n0E82 0E84 5\n0E84 - 5\n").unwrap();
        let cipher = Cipher {
            codes: vec![code(0x78), code(0x79), code(0x7A)],
            drawings: vec![
                Some(drawing(&[TALL])),
                Some(drawing(&[WIDE])),
                Some(drawing(&[HIGH])),
            ],
            claims: vec![Some('x'), Some('y'), Some('z')],
            lines: vec![vec![0, 1, 2], vec![0, 1, 2]],
        };
        let reading = decipher(&cipher, &shapes, &model);
        assert_eq!(reading.characters, ["ກ", "ຂ", "ຄ"]);
        assert!((reading.changed - 1.0).abs() < 1e-9);
        assert!(reading.gain > 0.2, "{}", reading.gain);
    }

    #[test]
    fn letters_that_look_alike_are_told_apart_by_the_sequence() {
        let shapes = shapes(&[('ກ', &[TALL]), ('ຂ', &[TALL]), ('ຄ', &[WIDE])]);
        let model =
            CharModel::parse("- 0E81 40\n0E81 0E84 40\n0E84 0E82 40\n0E82 - 40\n0E81 0E82 0\n")
                .unwrap();
        let cipher = Cipher {
            codes: vec![code(1), code(2), code(3)],
            drawings: vec![
                Some(drawing(&[TALL])),
                Some(drawing(&[WIDE])),
                Some(drawing(&[TALL])),
            ],
            claims: vec![None, None, None],
            lines: vec![vec![0, 1, 2]; 6],
        };
        let reading = decipher(&cipher, &shapes, &model);
        assert_eq!(reading.characters, ["ກ", "ຄ", "ຂ"]);
    }

    #[test]
    fn an_honest_font_gains_nothing() {
        let shapes = shapes(&[('ກ', &[TALL]), ('ຂ', &[WIDE])]);
        let model = CharModel::parse("- 0E81 5\n0E81 0E82 5\n0E82 - 5\n").unwrap();
        let cipher = Cipher {
            codes: vec![code(1), code(2)],
            drawings: vec![Some(drawing(&[TALL])), Some(drawing(&[WIDE]))],
            claims: vec![Some('ກ'), Some('ຂ')],
            lines: vec![vec![0, 1]; 3],
        };
        let reading = decipher(&cipher, &shapes, &model);
        assert_eq!(reading.characters, ["ກ", "ຂ"]);
        assert!(reading.gain.abs() < 1e-9 && reading.changed == 0.0);
    }
}
