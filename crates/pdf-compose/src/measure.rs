use crate::theme::{Face, Style};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pen {
    pub face: Face,
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
}

impl Pen {
    #[must_use]
    pub const fn new(face: Face, size: f32) -> Self {
        Self {
            face,
            size,
            bold: false,
            italic: false,
        }
    }
}

impl From<Style> for Pen {
    fn from(style: Style) -> Self {
        Self {
            face: style.face,
            size: style.size,
            bold: style.bold,
            italic: style.italic,
        }
    }
}

pub trait Measure {
    fn width(&self, text: &str, pen: Pen) -> f32;

    fn ascent(&self, pen: Pen) -> f32 {
        pen.size * 0.8
    }

    fn descent(&self, pen: Pen) -> f32 {
        pen.size * 0.2
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Even {
    pub per_em: f32,
}

impl Even {
    #[must_use]
    pub const fn half() -> Self {
        Self { per_em: 0.5 }
    }
}

impl Default for Even {
    fn default() -> Self {
        Self::half()
    }
}

impl Measure for Even {
    fn width(&self, text: &str, pen: Pen) -> f32 {
        let glyphs = text.chars().count();
        #[allow(clippy::cast_precision_loss)]
        let glyphs = glyphs as f32;
        glyphs * pen.size * self.per_em
    }
}

#[cfg(test)]
mod tests {
    use super::{Even, Measure, Pen};
    use crate::theme::{Face, Style};

    #[test]
    fn the_instrument_answers_what_can_be_worked_out_by_hand() {
        let ruler = Even::half();
        let pen = Pen::new(Face::Body, 10.0);
        assert!((ruler.width(&"x".repeat(40), pen) - 200.0).abs() < 0.001);
        assert!((ruler.width("", pen) - 0.0).abs() < 0.001);
        assert!((ruler.width("xxxx", Pen::new(Face::Body, 20.0)) - 40.0).abs() < 0.001);
    }

    #[test]
    fn the_instrument_counts_letters_and_not_bytes() {
        let ruler = Even::half();
        let pen = Pen::new(Face::Body, 10.0);
        let thai = "กขคงจ";
        assert_eq!(thai.len(), 15);
        assert!((ruler.width(thai, pen) - ruler.width("abcde", pen)).abs() < 0.001);
    }

    #[test]
    fn a_pen_comes_from_a_style_without_its_leading() {
        let style = Style::new(Face::Display, 20.0, 24.0).bolder();
        let pen = Pen::from(style);
        assert_eq!(pen.face, Face::Display);
        assert!((pen.size - 20.0).abs() < 0.001);
        assert!(pen.bold);
        assert!(!pen.italic);
    }

    #[test]
    fn letters_stand_above_the_baseline_and_hang_below_it() {
        let ruler = Even::half();
        let pen = Pen::new(Face::Body, 10.0);
        assert!((ruler.ascent(pen) - 8.0).abs() < 0.001);
        assert!((ruler.descent(pen) - 2.0).abs() < 0.001);
        assert!(ruler.ascent(pen) > ruler.descent(pen));
    }
}
