#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Block {
    Heading {
        level: u8,
        inlines: Vec<Inline>,
    },
    Paragraph {
        inlines: Vec<Inline>,
    },
    Code {
        info: String,
        text: String,
    },
    Break,
    Quote {
        blocks: Vec<Block>,
    },
    List(List),
    Table {
        head: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
        align: Vec<Align>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct List {
    pub first: Option<u64>,
    pub loose: bool,
    pub items: Vec<Vec<Block>>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Align {
    #[default]
    Start,
    Middle,
    End,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Inline {
    Text(String),
    Soft,
    Hard,
    Emphasis(Vec<Inline>),
    Strong(Vec<Inline>),
    Strike(Vec<Inline>),
    Code(String),
    Link { to: String, text: Vec<Inline> },
    Image { at: String, text: Vec<Inline> },
}

impl Inline {
    #[must_use]
    pub fn plain(&self) -> String {
        match self {
            Self::Text(text) | Self::Code(text) => text.clone(),
            Self::Soft => " ".to_owned(),
            Self::Hard => "\n".to_owned(),
            Self::Emphasis(inside)
            | Self::Strong(inside)
            | Self::Strike(inside)
            | Self::Link { text: inside, .. }
            | Self::Image { text: inside, .. } => plain(inside),
        }
    }
}

#[must_use]
pub fn plain(inlines: &[Inline]) -> String {
    inlines.iter().map(Inline::plain).collect()
}
