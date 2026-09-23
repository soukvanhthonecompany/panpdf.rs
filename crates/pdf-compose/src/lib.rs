pub mod chart;
pub mod draw;
pub mod flow;
pub mod measure;
pub mod page;
pub mod parse;
pub mod theme;
pub mod value;
pub mod wrap;

pub use chart::{Chart, ChartKind, Series};
pub use draw::{Shape, Spot};
pub use flow::compose;
pub use measure::{Measure, Pen};
pub use page::{Ink, Piece, Sheet};
pub use theme::{Face, Rect, Style, Theme};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Doc {
    pub title: Option<String>,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Heading {
        level: u8,
        spans: Vec<Span>,
    },
    Paragraph(Vec<Span>),
    List {
        ordered: bool,
        items: Vec<Item>,
    },
    Quote(Vec<Block>),
    Code {
        language: Option<String>,
        lines: Vec<String>,
    },
    Table(Table),
    Rule,
    Chart(Chart),
    Picture {
        path: String,
        alt: String,
    },
    Unreadable {
        text: String,
        why: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Item {
    pub spans: Vec<Span>,
    pub children: Vec<Item>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Table {
    pub head: Vec<Vec<Span>>,
    pub rows: Vec<Vec<Vec<Span>>>,
    pub align: Vec<Align>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Align {
    #[default]
    Left,
    Centre,
    Right,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Span {
    pub text: String,
    pub emphasis: Emphasis,
    pub link: Option<String>,
}

impl Span {
    #[must_use]
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            emphasis: Emphasis::default(),
            link: None,
        }
    }

    #[must_use]
    pub fn in_voice(mut self, emphasis: Emphasis) -> Self {
        self.emphasis = emphasis;
        self
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Emphasis {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
}

impl Emphasis {
    pub const PLAIN: Self = Self {
        bold: false,
        italic: false,
        code: false,
    };

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

impl Doc {
    #[must_use]
    pub fn words(&self) -> String {
        let mut out = String::new();
        for block in &self.blocks {
            block.words_into(&mut out);
        }
        out
    }
}

impl Block {
    fn words_into(&self, out: &mut String) {
        match self {
            Self::Heading { spans, .. } | Self::Paragraph(spans) => say(spans, out),
            Self::List { items, .. } => {
                for item in items {
                    item.words_into(out);
                }
            }
            Self::Quote(blocks) => {
                for block in blocks {
                    block.words_into(out);
                }
            }
            Self::Code { lines, .. } => {
                for line in lines {
                    out.push_str(line);
                    out.push('\n');
                }
            }
            Self::Table(table) => table.words_into(out),
            Self::Picture { alt, .. } => {
                out.push_str(alt);
                out.push('\n');
            }
            Self::Chart(chart) => chart.words_into(out),
            Self::Rule | Self::Unreadable { .. } => {}
        }
    }
}

impl Item {
    fn words_into(&self, out: &mut String) {
        say(&self.spans, out);
        for child in &self.children {
            child.words_into(out);
        }
    }
}

impl Table {
    fn words_into(&self, out: &mut String) {
        for cell in &self.head {
            say(cell, out);
        }
        for row in &self.rows {
            for cell in row {
                say(cell, out);
            }
        }
    }
}

fn say(spans: &[Span], out: &mut String) {
    for span in spans {
        out.push_str(&span.text);
    }
    out.push('\n');
}
