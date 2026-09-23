#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Face {
    #[default]
    Body,
    Display,
    Code,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    pub face: Face,
    pub size: f32,
    pub leading: f32,
    pub bold: bool,
    pub italic: bool,
}

impl Style {
    #[must_use]
    pub const fn new(face: Face, size: f32, leading: f32) -> Self {
        Self {
            face,
            size,
            leading,
            bold: false,
            italic: false,
        }
    }

    #[must_use]
    pub const fn bolder(mut self) -> Self {
        self.bold = true;
        self
    }

    #[must_use]
    pub const fn slanted(mut self) -> Self {
        self.italic = true;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edges {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Edges {
    #[must_use]
    pub const fn all(amount: f32) -> Self {
        Self {
            top: amount,
            right: amount,
            bottom: amount,
            left: amount,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    #[must_use]
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    #[must_use]
    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    #[must_use]
    pub fn right(&self) -> f32 {
        self.x + self.width
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colour {
    pub red: f32,
    pub green: f32,
    pub blue: f32,
}

impl Colour {
    #[must_use]
    pub const fn of(red: f32, green: f32, blue: f32) -> Self {
        Self { red, green, blue }
    }

    #[must_use]
    pub const fn grey(light: f32) -> Self {
        Self {
            red: light,
            green: light,
            blue: light,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub page_width: f32,
    pub page_height: f32,
    pub margin: Edges,
    pub body: Style,
    pub headings: [Style; 4],
    pub code: Style,
    pub inline_code: Face,
    pub quote: Style,
    pub caption: Style,
    pub over_heading: [f32; 4],
    pub under_heading: f32,
    pub between_paragraphs: f32,
    pub around_figures: f32,
    pub list_indent: f32,
    pub bullet_gap: f32,
    pub between_items: f32,
    pub quote_indent: f32,
    pub quote_rule: f32,
    pub rule_thickness: f32,
    pub chart_height: f32,
    pub most_picture_height: f32,
    pub cell_padding_across: f32,
    pub cell_padding_down: f32,
    pub table_rule: f32,
    pub axis: Style,
    pub grid: f32,
    pub bar_air: f32,
    pub ink: Colour,
    pub faint: Colour,
    pub palette: Vec<Colour>,
}

impl Theme {
    pub const A4: (f32, f32) = (595.0, 842.0);

    pub const LETTER: (f32, f32) = (612.0, 792.0);

    #[must_use]
    pub fn house() -> Self {
        let (page_width, page_height) = Self::A4;
        Self {
            page_width,
            page_height,
            margin: Edges::all(57.0),
            body: Style::new(Face::Body, 11.0, 15.0),
            headings: [
                Style::new(Face::Display, 20.0, 24.0).bolder(),
                Style::new(Face::Display, 16.0, 20.0).bolder(),
                Style::new(Face::Display, 13.0, 17.0).bolder(),
                Style::new(Face::Display, 11.5, 15.0).bolder(),
            ],
            code: Style::new(Face::Code, 9.5, 13.0),
            inline_code: Face::Code,
            quote: Style::new(Face::Body, 10.5, 15.0),
            caption: Style::new(Face::Body, 9.0, 12.0).slanted(),
            over_heading: [24.0, 20.0, 16.0, 13.0],
            under_heading: 6.0,
            between_paragraphs: 9.0,
            around_figures: 14.0,
            list_indent: 20.0,
            bullet_gap: 8.0,
            between_items: 4.0,
            quote_indent: 18.0,
            quote_rule: 2.0,
            rule_thickness: 0.75,
            chart_height: 200.0,
            most_picture_height: 320.0,
            cell_padding_across: 6.0,
            cell_padding_down: 4.0,
            table_rule: 0.5,
            axis: Style::new(Face::Body, 8.0, 11.0),
            grid: 0.5,
            bar_air: 0.2,
            ink: Colour::grey(0.13),
            faint: Colour::grey(0.82),
            palette: vec![
                Colour::of(0.20, 0.40, 0.68),
                Colour::of(0.86, 0.50, 0.19),
                Colour::of(0.33, 0.60, 0.36),
                Colour::of(0.72, 0.30, 0.31),
                Colour::of(0.48, 0.39, 0.64),
                Colour::of(0.47, 0.35, 0.29),
            ],
        }
    }

    #[must_use]
    pub fn colour(&self, series: usize) -> Colour {
        if self.palette.is_empty() {
            return self.ink;
        }
        self.palette[series % self.palette.len()]
    }

    #[must_use]
    pub fn on_paper(mut self, width: f32, height: f32) -> Self {
        self.page_width = width;
        self.page_height = height;
        self
    }

    #[must_use]
    pub fn column(&self) -> Rect {
        Rect::new(
            self.margin.left,
            self.margin.top,
            (self.page_width - self.margin.left - self.margin.right).max(1.0),
            (self.page_height - self.margin.top - self.margin.bottom).max(1.0),
        )
    }

    #[must_use]
    pub fn heading(&self, level: u8) -> Style {
        let deepest = u8::try_from(self.headings.len()).unwrap_or(4);
        let at = usize::from(level.clamp(1, deepest) - 1);
        self.headings[at]
    }

    #[must_use]
    pub fn space_over_heading(&self, level: u8) -> f32 {
        let deepest = u8::try_from(self.over_heading.len()).unwrap_or(4);
        let at = usize::from(level.clamp(1, deepest) - 1);
        self.over_heading[at]
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::house()
    }
}

#[cfg(test)]
mod tests {
    use super::{Colour, Edges, Face, Rect, Style, Theme};

    #[test]
    fn the_column_is_the_sheet_less_its_margins() {
        let theme = Theme::house();
        let column = theme.column();
        assert!((column.x - 57.0).abs() < 0.01);
        assert!((column.y - 57.0).abs() < 0.01);
        assert!((column.width - (595.0 - 114.0)).abs() < 0.01);
        assert!((column.height - (842.0 - 114.0)).abs() < 0.01);
        let bare = Theme {
            margin: Edges::all(0.0),
            ..Theme::house()
        };
        assert!((bare.column().width - 595.0).abs() < 0.01);
    }

    #[test]
    fn a_column_never_collapses_to_nothing() {
        let squeezed = Theme::house().on_paper(80.0, 80.0);
        assert!(squeezed.column().width >= 1.0);
        assert!(squeezed.column().height >= 1.0);
    }

    #[test]
    fn headings_fall_away_and_stop_at_four() {
        let theme = Theme::house();
        let sizes: Vec<f32> = (1..=4).map(|level| theme.heading(level).size).collect();
        for pair in sizes.windows(2) {
            assert!(pair[0] > pair[1], "level sizes must fall: {sizes:?}");
        }
        assert!(sizes[3] > theme.body.size, "the deepest is still a heading");
        assert_eq!(theme.heading(9), theme.heading(4));
        assert_eq!(theme.heading(0), theme.heading(1));
        assert_ne!(theme.heading(0), theme.heading(9));
    }

    #[test]
    fn air_over_a_heading_falls_away_too() {
        let theme = Theme::house();
        assert!(theme.space_over_heading(1) > theme.space_over_heading(4));
        let deepest = theme.space_over_heading(4);
        assert!((theme.space_over_heading(7) - deepest).abs() < 0.001);
    }

    #[test]
    fn a_series_past_the_end_of_the_palette_starts_it_again() {
        let theme = Theme::house();
        let many = theme.palette.len();
        assert_eq!(theme.colour(0), theme.colour(many));
        assert_ne!(theme.colour(0), theme.colour(1));
        let bare = Theme {
            palette: Vec::new(),
            ..Theme::house()
        };
        assert_eq!(bare.colour(3), bare.ink);
    }

    #[test]
    fn a_grey_is_the_same_in_all_three_parts() {
        assert_eq!(Colour::grey(0.5), Colour::of(0.5, 0.5, 0.5));
    }

    #[test]
    fn a_rectangle_knows_its_far_edges() {
        let rect = Rect::new(10.0, 20.0, 30.0, 40.0);
        assert!((rect.right() - 40.0).abs() < 0.01);
        assert!((rect.bottom() - 60.0).abs() < 0.01);
    }

    #[test]
    fn a_style_can_be_asked_for_another_voice() {
        let plain = Style::new(Face::Body, 11.0, 15.0);
        assert!(!plain.bold && !plain.italic);
        assert!(plain.bolder().bold);
        assert!(plain.slanted().italic);
        assert!(plain.bolder().slanted().bold);
    }

    #[test]
    fn paper_can_be_changed_without_changing_the_style() {
        let letter = Theme::house().on_paper(Theme::LETTER.0, Theme::LETTER.1);
        assert!((letter.page_width - 612.0).abs() < 0.01);
        assert_eq!(letter.body, Theme::house().body);
    }
}
