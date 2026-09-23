use super::{Block, Inline, List, pieces};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Kind {
    #[default]
    Body,
    Heading(u8),
    Quote,
    Code,
    Math,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Paragraph {
    pub text: String,
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub space_before: f32,
    pub indent: f32,
    pub kind: Kind,
    pub bullet: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub head: Vec<Paragraph>,
    pub rows: Vec<Vec<Paragraph>>,
    pub columns: Vec<f32>,
    pub space_before: f32,
    pub indent: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    Text(Paragraph),
    Rule {
        space_before: f32,
        indent: f32,
    },
    Table(Table),
    Chart {
        spec: String,
        space_before: f32,
        indent: f32,
    },
}

impl Part {
    fn space_before_mut(&mut self) -> &mut f32 {
        match self {
            Self::Text(paragraph) => &mut paragraph.space_before,
            Self::Rule { space_before, .. }
            | Self::Chart { space_before, .. }
            | Self::Table(Table { space_before, .. }) => space_before,
        }
    }
}

pub const HEADINGS: [f32; 6] = [1.8, 1.4, 1.15, 1.05, 1.0, 1.0];

const ABOVE_A_HEADING: f32 = 0.9;

const BETWEEN_PARAGRAPHS: f32 = 0.45;

const BETWEEN_ITEMS: f32 = 0.15;

const AROUND_A_BLOCK: f32 = 0.9;

const BULLET_INDENT: f32 = 1.4;

const CODE_SIZE: f32 = 0.9;

const MATH_SIZE: f32 = 1.15;

const NARROWEST_COLUMN: f32 = 0.08;

const HELD: u32 = 0xF0000;

#[must_use]
pub fn parts(markdown: &str, body: f32) -> Vec<Part> {
    let (held, maths) = hold_the_mathematics(markdown);
    let mut out = Vec::new();
    let mut reading = Reading {
        body,
        maths: &maths,
        out: &mut out,
    };
    reading.blocks(&super::blocks(&held), Place::default());
    if let Some(first) = out.first_mut() {
        *first.space_before_mut() = 0.0;
    }
    out
}

#[must_use]
pub fn paragraphs(markdown: &str, body: f32) -> Vec<Paragraph> {
    parts(markdown, body)
        .into_iter()
        .filter_map(|part| match part {
            Part::Text(paragraph) => Some(paragraph),
            Part::Rule { .. } | Part::Table(_) | Part::Chart { .. } => None,
        })
        .collect()
}

#[must_use]
pub fn mathematics_as_text(markdown: &str) -> String {
    let (held, maths) = hold_the_mathematics(markdown);
    let mut out = String::with_capacity(held.len());
    for letter in held.chars() {
        match held_at(letter).and_then(|at| maths.get(at)) {
            Some(piece) => {
                for letter in crate::composing::math::linear(&piece.latex).chars() {
                    if letter.is_ascii_punctuation() {
                        out.push('\\');
                    }
                    out.push(letter);
                }
            }
            None => out.push(letter),
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Held {
    latex: String,
    display: bool,
}

fn hold_the_mathematics(markdown: &str) -> (String, Vec<Held>) {
    let mut out = String::with_capacity(markdown.len());
    let mut held = Vec::new();
    let mut fenced = false;
    for line in markdown.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            out.push_str(line);
            continue;
        }
        if fenced {
            out.push_str(line);
            continue;
        }
        out.push_str(&hold_in_a_line(line, &mut held));
    }
    let spanning = hold_spanning_displays(&out, &mut held);
    (spanning, held)
}

fn hold_in_a_line(line: &str, held: &mut Vec<Held>) -> String {
    let letters: Vec<char> = line.chars().collect();
    let mut out = String::new();
    let mut at = 0;
    while at < letters.len() {
        let letter = letters[at];
        if letter == '`' {
            let run = letters[at..].iter().take_while(|c| **c == '`').count();
            let close = (at + run..letters.len()).find(|&from| {
                letters[from..].iter().take_while(|c| **c == '`').count() == run
                    && (from == 0 || letters[from - 1] != '`')
            });
            let end = close.map_or(at + run, |from| from + run);
            out.extend(&letters[at..end]);
            at = end;
            continue;
        }
        if letter == '\\' && letters.get(at + 1) == Some(&'$') {
            out.push('$');
            at += 2;
            continue;
        }
        if letter == '$' {
            let display = letters.get(at + 1) == Some(&'$');
            let open = if display { 2 } else { 1 };
            if let Some(close) = closing(&letters, at + open, display) {
                let latex: String = letters[at + open..close].iter().collect();
                out.push(stand_in(held.len()));
                held.push(Held {
                    latex: latex.trim().to_owned(),
                    display,
                });
                at = close + open;
                continue;
            }
        }
        out.push(letter);
        at += 1;
    }
    out
}

fn closing(letters: &[char], from: usize, display: bool) -> Option<usize> {
    if !display && letters.get(from).is_none_or(|c| c.is_whitespace()) {
        return None;
    }
    let mut at = from;
    while at < letters.len() {
        match letters[at] {
            '\\' => at += 2,
            '$' if display => {
                if letters.get(at + 1) == Some(&'$') {
                    return Some(at);
                }
                at += 1;
            }
            '$' => {
                let closes = at > from
                    && !letters[at - 1].is_whitespace()
                    && !letters.get(at + 1).is_some_and(char::is_ascii_digit);
                if closes {
                    return Some(at);
                }
                at += 1;
            }
            '\n' if !display => return None,
            _ => at += 1,
        }
    }
    None
}

fn hold_spanning_displays(text: &str, held: &mut Vec<Held>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut inside: Option<String> = None;
    let mut fenced = false;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        if inside.is_none() && (trimmed.starts_with("```") || trimmed.starts_with("~~~")) {
            fenced = !fenced;
        }
        if fenced {
            out.push_str(line);
            continue;
        }
        match inside.as_mut() {
            None if trimmed == "$$" => inside = Some(String::new()),
            None => out.push_str(line),
            Some(latex) if trimmed == "$$" => {
                out.push(stand_in(held.len()));
                out.push('\n');
                held.push(Held {
                    latex: latex.trim().to_owned(),
                    display: true,
                });
                inside = None;
            }
            Some(latex) => latex.push_str(line),
        }
    }
    if let Some(latex) = inside {
        out.push_str("$$\n");
        out.push_str(&latex);
    }
    out
}

fn stand_in(at: usize) -> char {
    u32::try_from(at)
        .ok()
        .and_then(|at| char::from_u32(HELD + at))
        .unwrap_or('\u{FFFD}')
}

fn held_at(letter: char) -> Option<usize> {
    let code = u32::from(letter);
    (HELD..HELD + 0xFFFD)
        .contains(&code)
        .then(|| usize::try_from(code - HELD).ok())
        .flatten()
}

#[derive(Clone, Copy, Default)]
struct Place {
    depth: u16,
    quoted: bool,
    tight: bool,
}

struct Reading<'a> {
    body: f32,
    maths: &'a [Held],
    out: &'a mut Vec<Part>,
}

impl Reading<'_> {
    fn indent(&self, place: Place) -> f32 {
        self.body * BULLET_INDENT * f32::from(place.depth)
    }

    fn between(&self, place: Place) -> f32 {
        if place.tight {
            self.body * BETWEEN_ITEMS
        } else {
            self.body * BETWEEN_PARAGRAPHS
        }
    }

    fn blocks(&mut self, blocks: &[Block], place: Place) {
        for block in blocks {
            self.block(block, place, None);
        }
    }

    fn words(&self, inlines: &[Inline], mark: Option<&str>) -> (String, bool, bool) {
        let (text, bold, italic) = words(inlines, mark);
        (self.put_back(&text), bold, italic)
    }

    fn put_back(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for letter in text.chars() {
            match held_at(letter).and_then(|at| self.maths.get(at)) {
                Some(held) => out.push_str(&crate::composing::math::linear(&held.latex)),
                None => out.push(letter),
            }
        }
        out
    }

    fn displayed(&self, inlines: &[Inline]) -> Option<String> {
        let (text, _, _) = words(inlines, None);
        let mut letters = text.trim().chars();
        let only = letters.next()?;
        if letters.next().is_some() {
            return None;
        }
        let held = self.maths.get(held_at(only)?)?;
        held.display.then(|| held.latex.clone())
    }

    fn block(&mut self, block: &Block, place: Place, mark: Option<(&str, bool)>) {
        let body = self.body;
        let indent = self.indent(place);
        let (number, bullet) = match mark {
            Some((number, bullet)) => ((!bullet).then_some(number), bullet),
            None => (None, false),
        };
        match block {
            Block::Heading { level, inlines } => {
                let size = HEADINGS
                    .get(usize::from(*level).saturating_sub(1))
                    .map_or(body, |bigger| body * bigger);
                let (text, _, italic) = self.words(inlines, number);
                self.out.push(Part::Text(Paragraph {
                    text,
                    size,
                    bold: true,
                    italic,
                    space_before: size * ABOVE_A_HEADING,
                    indent,
                    kind: Kind::Heading(*level),
                    bullet,
                }));
            }
            Block::Paragraph { inlines } => {
                if let Some(latex) = self.displayed(inlines) {
                    self.out.push(Part::Text(Paragraph {
                        text: latex,
                        size: body * MATH_SIZE,
                        bold: false,
                        italic: false,
                        space_before: self.between(place),
                        indent,
                        kind: Kind::Math,
                        bullet,
                    }));
                    return;
                }
                let (text, bold, italic) = self.words(inlines, number);
                if text.trim().is_empty() {
                    return;
                }
                self.out.push(Part::Text(Paragraph {
                    text,
                    size: body,
                    bold,
                    italic,
                    space_before: self.between(place),
                    indent,
                    kind: if place.quoted {
                        Kind::Quote
                    } else {
                        Kind::Body
                    },
                    bullet,
                }));
            }
            Block::Code { info, text } => {
                let text = text.trim_end_matches('\n');
                if text.trim().is_empty() {
                    return;
                }
                let language = info.split_whitespace().next().unwrap_or_default();
                if language.eq_ignore_ascii_case("chart") {
                    self.out.push(Part::Chart {
                        spec: text.to_owned(),
                        space_before: body * AROUND_A_BLOCK,
                        indent,
                    });
                    return;
                }
                let maths = ["math", "latex", "tex"]
                    .iter()
                    .any(|name| language.eq_ignore_ascii_case(name));
                self.out.push(Part::Text(Paragraph {
                    text: text.to_owned(),
                    size: body * if maths { MATH_SIZE } else { CODE_SIZE },
                    bold: false,
                    italic: false,
                    space_before: body * BETWEEN_PARAGRAPHS,
                    indent,
                    kind: if maths { Kind::Math } else { Kind::Code },
                    bullet,
                }));
            }
            Block::Break => self.out.push(Part::Rule {
                space_before: body * AROUND_A_BLOCK,
                indent,
            }),
            Block::Quote { blocks } => self.blocks(
                blocks,
                Place {
                    quoted: true,
                    ..place
                },
            ),
            Block::List(list) => self.list(list, place),
            Block::Table { head, rows, .. } => self.table(head, rows, place),
        }
    }

    fn list(&mut self, list: &List, place: Place) {
        let inside = Place {
            depth: place.depth.saturating_add(1),
            ..place
        };
        for (at, item) in list.items.iter().enumerate() {
            let number = list
                .first
                .map(|first| format!("{}.  ", first + at as u64))
                .unwrap_or_default();
            for (which, block) in item.iter().enumerate() {
                let place = Place {
                    tight: !list.loose && (at > 0 || which > 0),
                    ..inside
                };
                let mark = (which == 0).then_some((number.as_str(), list.first.is_none()));
                self.block(block, place, mark);
            }
        }
    }

    fn table(&mut self, head: &[Vec<Inline>], rows: &[Vec<Vec<Inline>>], place: Place) {
        let body = self.body;
        let count = std::iter::once(head.len())
            .chain(rows.iter().map(Vec::len))
            .max()
            .unwrap_or(0);
        if count == 0 {
            return;
        }
        let cell = |row: &[Vec<Inline>], column: usize, heading: bool| {
            let (text, bold, italic) = row
                .get(column)
                .map_or_else(Default::default, |cell| self.words(cell, None));
            Paragraph {
                text: text.trim().to_owned(),
                size: body,
                bold: bold || heading,
                italic,
                space_before: 0.0,
                indent: 0.0,
                kind: Kind::Body,
                bullet: false,
            }
        };
        let heads = head
            .iter()
            .any(|cell| !words(cell, None).0.trim().is_empty());
        let head_row: Vec<Paragraph> = if heads {
            (0..count).map(|column| cell(head, column, true)).collect()
        } else {
            Vec::new()
        };
        let body_rows: Vec<Vec<Paragraph>> = rows
            .iter()
            .map(|row| (0..count).map(|column| cell(row, column, false)).collect())
            .collect();
        let columns = shares(
            std::iter::once(&head_row)
                .chain(body_rows.iter())
                .map(|row| row.iter().map(|cell| cell.text.chars().count()).collect()),
        );
        self.out.push(Part::Table(Table {
            head: head_row,
            rows: body_rows,
            columns,
            space_before: body * AROUND_A_BLOCK,
            indent: self.indent(place),
        }));
    }
}

fn words(inlines: &[Inline], mark: Option<&str>) -> (String, bool, bool) {
    let mut text = mark.unwrap_or_default().to_owned();
    let mut bold = true;
    let mut italic = true;
    let mut any = false;
    for (at, line) in inlines
        .split(|inline| matches!(inline, Inline::Hard))
        .enumerate()
    {
        if at > 0 {
            text.push('\n');
        }
        for piece in pieces(line) {
            if piece.text.trim().is_empty() {
                text.push_str(&piece.text);
                continue;
            }
            any = true;
            bold &= piece.bold;
            italic &= piece.italic;
            text.push_str(&piece.text);
        }
    }
    (text, any && bold, any && italic)
}

fn shares(rows: impl Iterator<Item = Vec<usize>>) -> Vec<f32> {
    let mut longest: Vec<usize> = Vec::new();
    for row in rows {
        if longest.len() < row.len() {
            longest.resize(row.len(), 0);
        }
        for (at, letters) in row.into_iter().enumerate() {
            longest[at] = longest[at].max(letters);
        }
    }
    let count = longest.len();
    if count == 0 {
        return Vec::new();
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "letters in a cell, far inside f32"
    )]
    let weights: Vec<f32> = longest
        .iter()
        .map(|letters| (*letters).max(1) as f32)
        .collect();
    let total: f32 = weights.iter().sum();
    #[expect(clippy::cast_precision_loss, reason = "a column count")]
    let least = NARROWEST_COLUMN.min(1.0 / count as f32);
    let raw: Vec<f32> = weights
        .iter()
        .map(|weight| (weight / total).max(least))
        .collect();
    let sum: f32 = raw.iter().sum();
    raw.into_iter().map(|share| share / sum).collect()
}

#[cfg(test)]
mod tests;
