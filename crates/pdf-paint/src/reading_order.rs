#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlacedInk {
    pub origin: (f64, f64),
    pub turn: f64,
    pub em: f64,
    pub ink: Option<[f64; 4]>,
}

const BASELINE_SLACK: f64 = 0.1;

#[derive(Clone, Copy)]
struct Framed {
    index: usize,
    along: f64,
    across: f64,
    em: f64,
    ink: Option<[f64; 4]>,
}

impl Framed {
    fn left(&self) -> f64 {
        self.along + self.ink.map_or(0.0, |ink| ink[0] * self.em)
    }

    fn right(&self) -> f64 {
        self.along + self.ink.map_or(0.0, |ink| ink[2] * self.em)
    }

    fn centre(&self) -> f64 {
        f64::midpoint(self.left(), self.right())
    }
}

#[must_use]
pub fn reading_order(glyphs: &[PlacedInk]) -> Vec<Vec<usize>> {
    let mut turns: Vec<(f64, Vec<usize>)> = Vec::new();
    for (index, glyph) in glyphs.iter().enumerate() {
        match turns
            .iter_mut()
            .find(|(turn, _)| angle_between(*turn, glyph.turn) < 1f64.to_radians())
        {
            Some((_, members)) => members.push(index),
            None => turns.push((glyph.turn, vec![index])),
        }
    }
    let mut lines = Vec::new();
    for (turn, members) in turns {
        let (cos, sin) = (turn.cos(), turn.sin());
        let mut framed: Vec<Framed> = members
            .into_iter()
            .map(|index| {
                let glyph = &glyphs[index];
                let (x, y) = glyph.origin;
                Framed {
                    index,
                    along: x * cos + y * sin,
                    across: -x * sin + y * cos,
                    em: glyph.em,
                    ink: glyph.ink,
                }
            })
            .collect();
        framed.sort_by(|one, other| other.across.total_cmp(&one.across));
        let mut current: Vec<Framed> = Vec::new();
        for glyph in framed {
            let joins = current.first().is_some_and(|first| {
                (first.across - glyph.across).abs() <= first.em.max(glyph.em) / 3.0
            });
            if !joins && !current.is_empty() {
                lines.push(order_line(std::mem::take(&mut current)));
            }
            current.push(glyph);
        }
        if !current.is_empty() {
            lines.push(order_line(current));
        }
    }
    lines
}

fn angle_between(one: f64, other: f64) -> f64 {
    let difference = (one - other).rem_euclid(std::f64::consts::TAU);
    difference.min(std::f64::consts::TAU - difference)
}

fn order_line(mut line: Vec<Framed>) -> Vec<usize> {
    line.sort_by(|one, other| one.along.total_cmp(&other.along));
    let mut tops: Vec<f64> = line
        .iter()
        .filter_map(|glyph| {
            glyph
                .ink
                .filter(|ink| ink[1] <= BASELINE_SLACK)
                .map(|ink| ink[3])
        })
        .collect();
    let Some(body) = median(&mut tops).filter(|top| *top > 0.0) else {
        return line.iter().map(|glyph| glyph.index).collect();
    };
    let is_mark = |glyph: &Framed| {
        glyph.ink.is_some_and(|ink| {
            let narrow = ink[2] - ink[0] < body;
            let above = ink[1] >= body * 0.6;
            let below = ink[3] <= 0.0;
            narrow && (above || below)
        })
    };
    let mut letters: Vec<(Framed, Vec<Framed>)> = Vec::new();
    let mut loose: Vec<Framed> = Vec::new();
    for glyph in line {
        if is_mark(&glyph) {
            loose.push(glyph);
        } else {
            letters.push((glyph, Vec::new()));
        }
    }
    for mark in loose {
        let centre = mark.centre();
        let over = letters
            .iter()
            .position(|(letter, _)| {
                letter.ink.is_some() && letter.left() <= centre && centre <= letter.right()
            })
            .or_else(|| {
                letters
                    .iter()
                    .rposition(|(letter, _)| letter.ink.is_some() && letter.left() <= centre)
            });
        match over {
            Some(at) => letters[at].1.push(mark),
            None => letters.insert(0, (mark, Vec::new())),
        }
    }
    let mut ordered = Vec::new();
    for (letter, mut marks) in letters {
        ordered.push(letter.index);
        marks.sort_by(|one, other| {
            let bottom = |glyph: &Framed| glyph.ink.map_or(0.0, |ink| ink[1]);
            bottom(one).total_cmp(&bottom(other))
        });
        ordered.extend(marks.iter().map(|mark| mark.index));
    }
    ordered
}

fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    Some(values[values.len() / 2])
}

#[cfg(test)]
mod tests {
    use super::{PlacedInk, reading_order};

    fn glyph(x: f64, y: f64, ink: [f64; 4]) -> PlacedInk {
        PlacedInk {
            origin: (x, y),
            turn: 0.0,
            em: 10.0,
            ink: Some(ink),
        }
    }

    const LETTER: [f64; 4] = [0.05, 0.0, 0.5, 0.5];
    const ABOVE: [f64; 4] = [0.15, 0.62, 0.4, 0.75];
    const HIGHER: [f64; 4] = [0.2, 0.8, 0.4, 0.95];
    const BELOW: [f64; 4] = [0.2, -0.3, 0.4, -0.05];

    #[test]
    fn marks_written_after_the_line_follow_the_letter_they_sit_on() {
        let glyphs = [
            glyph(0.0, 100.0, LETTER),
            glyph(6.0, 100.0, LETTER),
            glyph(0.0, 100.0, HIGHER),
            glyph(0.0, 100.0, ABOVE),
        ];
        assert_eq!(reading_order(&glyphs), vec![vec![0, 3, 2, 1]]);
    }

    #[test]
    fn a_mark_below_comes_before_one_above() {
        let glyphs = [
            glyph(0.0, 100.0, HIGHER),
            glyph(0.0, 100.0, BELOW),
            glyph(0.0, 100.0, LETTER),
        ];
        assert_eq!(reading_order(&glyphs), vec![vec![2, 1, 0]]);
    }

    #[test]
    fn lines_run_down_the_page_and_letters_along_them() {
        let glyphs = [
            glyph(6.0, 80.0, LETTER),
            glyph(0.0, 100.0, LETTER),
            glyph(0.0, 80.0, LETTER),
            glyph(6.0, 101.0, LETTER),
        ];
        assert_eq!(reading_order(&glyphs), vec![vec![1, 3], vec![2, 0]]);
    }

    #[test]
    fn a_turned_line_is_read_along_its_own_baseline() {
        let up = |y: f64| PlacedInk {
            origin: (50.0, y),
            turn: std::f64::consts::FRAC_PI_2,
            em: 10.0,
            ink: Some(LETTER),
        };
        let glyphs = [
            up(20.0),
            glyph(40.0, 14.0, LETTER),
            up(8.0),
            glyph(55.0, 14.0, LETTER),
        ];
        assert_eq!(reading_order(&glyphs), vec![vec![2, 0], vec![1, 3]]);
    }

    #[test]
    fn a_space_keeps_its_place() {
        let space = PlacedInk {
            origin: (5.5, 100.0),
            turn: 0.0,
            em: 10.0,
            ink: None,
        };
        let glyphs = [glyph(11.0, 100.0, LETTER), space, glyph(0.0, 100.0, LETTER)];
        assert_eq!(reading_order(&glyphs), vec![vec![2, 1, 0]]);
    }
}
