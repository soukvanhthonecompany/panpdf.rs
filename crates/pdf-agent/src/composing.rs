pub mod chart;
pub mod expr;
pub mod math;
pub mod shapes;
pub mod theme;

use std::sync::Arc;

pub use pdf_edit::{PenStep, Room};

use crate::markup::laying_out::{Kind, Paragraph, Part, Table};
use theme::Theme;

pub type Colour = [f64; 3];

#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    pub family: String,
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    pub colour: Option<Colour>,
}

pub trait Measure {
    fn room(&self, text: &str, style: &Style, width: f64) -> Result<Room, String>;
}

pub struct Faces(pub Arc<dyn pdf_content::FontProvider>);

impl Measure for Faces {
    fn room(&self, text: &str, style: &Style, width: f64) -> Result<Room, String> {
        pdf_edit::room_for_new_text(
            &self.0,
            text,
            (&style.family, style.size, style.bold, style.italic),
            width,
        )
        .map_err(|why| why.to_string())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Mark {
    NewPage {
        after: usize,
    },
    Text {
        page: usize,
        area: [f64; 4],
        text: String,
        style: Style,
    },
    Shape {
        page: usize,
        steps: Vec<PenStep>,
        stroke: Option<(Colour, f64)>,
        fill: Option<Colour>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sheet {
    pub wide: f64,
    pub high: f64,
    pub margin: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct Setting<'a> {
    pub sheet: Sheet,
    pub from_page: usize,
    pub start: Option<f64>,
    pub family: &'a str,
    pub theme: &'a Theme,
    pub body: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Composed {
    pub marks: Vec<Mark>,
    pub pieces: usize,
    pub pages: usize,
    pub left_out: String,
}

pub const MONO: &str = "Liberation Mono";

pub const WIDER: [&str; 3] = ["DejaVu Sans", "Noto Sans Math", "Noto Sans Symbols"];

pub fn compose(
    parts: &[Part],
    setting: &Setting<'_>,
    measure: &dyn Measure,
) -> Result<Composed, String> {
    let mut composer = Composer {
        setting,
        measure,
        marks: Vec::new(),
        page: setting.from_page,
        top: setting.start.unwrap_or(setting.sheet.margin),
        pages: 1,
        pieces: 0,
        titled: false,
        keep: 0.0,
        left_out: std::cell::RefCell::new(std::collections::BTreeSet::new()),
    };
    if setting.start.is_none() {
        composer.paper();
    }
    let mut at = 0;
    while at < parts.len() {
        match &parts[at] {
            Part::Text(paragraph) if paragraph.kind == Kind::Quote => {
                let run: Vec<&Paragraph> = parts[at..]
                    .iter()
                    .map_while(|part| match part {
                        Part::Text(paragraph) if paragraph.kind == Kind::Quote => Some(paragraph),
                        _ => None,
                    })
                    .collect();
                at += run.len();
                composer.callout(&run)?;
                continue;
            }
            Part::Text(paragraph) => {
                composer.keep = match (paragraph.kind, parts.get(at + 1)) {
                    (Kind::Heading(_), Some(next)) => composer.start_of(next),
                    _ => 0.0,
                };
                composer.paragraph(paragraph)?;
                composer.keep = 0.0;
            }
            Part::Rule {
                space_before,
                indent,
            } => composer.rule(f64::from(*space_before), f64::from(*indent)),
            Part::Table(table) => composer.table(table)?,
            Part::Chart {
                spec,
                space_before,
                indent,
            } => composer.chart(spec, f64::from(*space_before), f64::from(*indent))?,
        }
        at += 1;
    }
    let left_out = composer.left_out.borrow().iter().collect();
    Ok(Composed {
        marks: composer.marks,
        pieces: composer.pieces,
        pages: composer.pages,
        left_out,
    })
}

#[must_use]
pub fn family_for<'a>(mono: bool, text: &str, family: &'a str) -> &'a str {
    if mono && text.is_ascii() {
        MONO
    } else {
        family
    }
}

fn pictograph(letter: char) -> bool {
    let code = u32::from(letter);
    (0x1_F000..=0x1_FAFF).contains(&code)
        || (0x2600..=0x27BF).contains(&code)
        || (0x2B00..=0x2BFF).contains(&code)
        || (0xFE00..=0xFE0F).contains(&code)
        || code == 0x200D
        || code == 0x20E3
}

pub(crate) struct Composer<'a> {
    pub(crate) setting: &'a Setting<'a>,
    measure: &'a dyn Measure,
    pub(crate) marks: Vec<Mark>,
    pub(crate) page: usize,
    pub(crate) top: f64,
    pages: usize,
    pieces: usize,
    titled: bool,
    keep: f64,
    left_out: std::cell::RefCell<std::collections::BTreeSet<char>>,
}

impl Composer<'_> {
    pub(crate) fn theme(&self) -> &Theme {
        self.setting.theme
    }

    pub(crate) fn left(&self) -> f64 {
        self.setting.sheet.margin
    }

    pub(crate) fn right(&self) -> f64 {
        self.setting.sheet.wide - self.setting.sheet.margin
    }

    fn bottom(&self) -> f64 {
        self.setting.sheet.high - self.setting.sheet.margin
    }

    fn fresh(&self) -> bool {
        self.top <= self.setting.sheet.margin + 1e-6
    }

    fn paper(&mut self) {
        if let Some(paper) = self.theme().paper {
            let sheet = self.setting.sheet;
            self.shape(
                shapes::rect(0.0, 0.0, sheet.wide, sheet.high),
                None,
                Some(paper),
            );
        }
    }

    fn new_page(&mut self) {
        self.marks.push(Mark::NewPage { after: self.page });
        self.page += 1;
        self.pages += 1;
        self.top = self.setting.sheet.margin;
        self.paper();
    }

    fn start_of(&self, part: &Part) -> f64 {
        let body = self.setting.body;
        match part {
            Part::Chart { spec, .. } => crate::json::Json::parse(spec)
                .ok()
                .and_then(|json| match json.get("height") {
                    Some(crate::json::Json::Number(height)) => Some(height.clamp(100.0, 640.0)),
                    _ => None,
                })
                .unwrap_or_else(|| ((self.right() - self.left()) * 0.58).min(300.0))
                .max(120.0),
            Part::Table(_) => body * 1.2 * 2.0 + 20.0,
            Part::Text(paragraph) if paragraph.kind == Kind::Math => {
                f64::from(paragraph.size) * 3.0
            }
            _ => body * 1.2 * 2.0,
        }
    }

    pub(crate) fn place(&mut self, space_before: f64, height: f64) -> f64 {
        let top = if self.fresh() {
            self.top
        } else {
            self.top + space_before
        };
        if top + height + self.keep > self.bottom() && !self.fresh() {
            self.new_page();
            return self.top;
        }
        top
    }

    pub(crate) fn shape(
        &mut self,
        steps: Vec<PenStep>,
        stroke: Option<(Colour, f64)>,
        fill: Option<Colour>,
    ) {
        if steps.is_empty() || (stroke.is_none() && fill.is_none()) {
            return;
        }
        self.marks.push(Mark::Shape {
            page: self.page,
            steps,
            stroke,
            fill,
        });
    }

    pub(crate) fn text(&mut self, (x, top): (f64, f64), width: f64, fitted: Fitted) {
        let Fitted { text, style, room } = fitted;
        self.pieces += 1;
        self.marks.push(Mark::Text {
            page: self.page,
            area: [x, top, x + width + 0.5, top + room.height],
            text,
            style,
        });
    }

    pub(crate) fn fit(&self, text: &str, style: &Style, width: f64) -> Result<Fitted, String> {
        let width = width.max(1.0);
        let first = match self.measure.room(text, style, width) {
            Ok(room) => {
                return Ok(Fitted {
                    text: text.to_owned(),
                    style: style.clone(),
                    room,
                });
            }
            Err(why) => why,
        };
        let plain: String = text.chars().filter(|letter| !pictograph(*letter)).collect();
        let tries = [text, plain.as_str()];
        for candidate in tries
            .iter()
            .filter(|candidate| !candidate.trim().is_empty())
        {
            let families = std::iter::once(style.family.as_str()).chain(WIDER);
            for family in families {
                let style = Style {
                    family: family.to_owned(),
                    ..style.clone()
                };
                if let Ok(room) = self.measure.room(candidate, &style, width) {
                    if candidate.len() < text.len() {
                        self.left_out
                            .borrow_mut()
                            .extend(text.chars().filter(|letter| pictograph(*letter)));
                    }
                    return Ok(Fitted {
                        text: (*candidate).to_owned(),
                        style,
                        room,
                    });
                }
            }
        }
        let shown: String = text.chars().take(40).collect();
        Err(format!("{first}, in \"{shown}\""))
    }

    fn ink(&self) -> Colour {
        self.theme().ink
    }

    fn style(&self, paragraph: &Paragraph, colour: Colour) -> Style {
        Style {
            family: family_for(
                paragraph.kind == Kind::Code,
                &paragraph.text,
                self.setting.family,
            )
            .to_owned(),
            size: f64::from(paragraph.size),
            bold: paragraph.bold,
            italic: paragraph.italic,
            colour: Some(colour),
        }
    }

    fn paragraph(&mut self, paragraph: &Paragraph) -> Result<(), String> {
        match paragraph.kind {
            Kind::Heading(level) => self.heading(paragraph, level),
            Kind::Code => self.code(paragraph),
            Kind::Math => self.equation(paragraph),
            Kind::Body | Kind::Quote => self.body(paragraph),
        }
    }

    fn body(&mut self, paragraph: &Paragraph) -> Result<(), String> {
        let x = self.left() + f64::from(paragraph.indent);
        let width = self.right() - x;
        let fitted = self.fit(&paragraph.text, &self.style(paragraph, self.ink()), width)?;
        let top = self.place(f64::from(paragraph.space_before), fitted.room.height);
        let size = fitted.style.size;
        let bottom = top + fitted.room.height;
        self.text((x, top), width, fitted);
        if paragraph.bullet {
            let colour = if self.theme().dressed {
                self.theme().accent
            } else {
                self.ink()
            };
            self.shape(
                shapes::circle(x - size * 0.75, top + size * 0.62, size * 0.17),
                None,
                Some(colour),
            );
        }
        self.top = bottom;
        Ok(())
    }

    fn heading(&mut self, paragraph: &Paragraph, level: u8) -> Result<(), String> {
        let theme = *self.theme();
        let x = self.left() + f64::from(paragraph.indent);
        let size = f64::from(paragraph.size);
        if !theme.dressed {
            let fitted = self.fit(
                &paragraph.text,
                &self.style(paragraph, theme.ink),
                self.right() - x,
            )?;
            let top = self.place(f64::from(paragraph.space_before), fitted.room.height);
            self.top = top + fitted.room.height;
            self.text((x, top), self.right() - x, fitted);
            return Ok(());
        }
        if level == 1 && !self.titled {
            self.titled = true;
            let (across, down) = (size * 0.6, size * 0.45);
            let inner = self.right() - x - 2.0 * across;
            let fitted = self.fit(
                &paragraph.text,
                &self.style(paragraph, theme.on_accent),
                inner,
            )?;
            let height = fitted.room.height + 2.0 * down;
            let lift = 3.0;
            let top = self.place(f64::from(paragraph.space_before), height + lift);
            let right = self.right();
            self.shape(
                shapes::rounded(x + lift, top + lift, right + lift, top + height + lift, 7.0),
                None,
                Some(theme.shadow),
            );
            self.shape(
                shapes::rounded(x, top, right, top + height, 7.0),
                None,
                Some(theme.accent),
            );
            self.text((x + across, top + down), inner, fitted);
            self.top = top + height + lift;
            return Ok(());
        }
        if level <= 2 {
            let gap = 10.0;
            let width = self.right() - x - gap;
            let fitted = self.fit(&paragraph.text, &self.style(paragraph, theme.accent), width)?;
            let under = if level == 1 { 5.0 } else { 0.0 };
            let top = self.place(
                f64::from(paragraph.space_before),
                fitted.room.height + under,
            );
            let bottom = top + fitted.room.height;
            self.shape(
                shapes::rounded(x, top + size * 0.12, x + 4.0, bottom - size * 0.08, 2.0),
                None,
                Some(theme.accent),
            );
            if level == 1 {
                let right = self.right();
                self.shape(
                    shapes::rect(x, bottom + 2.5, right, bottom + 3.7),
                    None,
                    Some(theme::paler(theme.accent, 0.55)),
                );
            }
            self.text((x + gap, top), width, fitted);
            self.top = bottom + under;
            return Ok(());
        }
        let width = self.right() - x;
        let fitted = self.fit(&paragraph.text, &self.style(paragraph, theme.accent), width)?;
        let top = self.place(f64::from(paragraph.space_before), fitted.room.height);
        self.top = top + fitted.room.height;
        self.text((x, top), width, fitted);
        Ok(())
    }

    fn callout(&mut self, run: &[&Paragraph]) -> Result<(), String> {
        let Some(first) = run.first() else {
            return Ok(());
        };
        let theme = *self.theme();
        let x = self.left() + f64::from(first.indent);
        let size = f64::from(first.size);
        let (bar, pad) = (4.0, size * 0.8);
        let inner = self.right() - x - bar - 2.0 * pad;
        let mut fitted = Vec::new();
        let mut height = 0.0;
        for (at, paragraph) in run.iter().enumerate() {
            let piece = self.fit(&paragraph.text, &self.style(paragraph, theme.ink), inner)?;
            let above = if at == 0 {
                0.0
            } else {
                f64::from(paragraph.space_before)
            };
            height += above + piece.room.height;
            fitted.push((above, piece));
        }
        let boxed = height + 2.0 * pad;
        let top = self.place(f64::from(first.space_before) + size * 0.2, boxed);
        let right = self.right();
        if theme.dressed {
            self.shape(
                shapes::rounded(x, top, right, top + boxed, 5.0),
                None,
                Some(theme.soft),
            );
            self.shape(
                shapes::rounded(x, top, x + bar, top + boxed, 2.0),
                None,
                Some(theme.accent),
            );
        } else {
            self.shape(
                shapes::rect(x, top, x + 2.0, top + boxed),
                None,
                Some(theme.line),
            );
        }
        let mut y = top + pad;
        for (above, piece) in fitted {
            y += above;
            let height = piece.room.height;
            self.text((x + bar + pad, y), inner, piece);
            y += height;
        }
        self.top = top + boxed + size * 0.2;
        Ok(())
    }

    fn code(&mut self, paragraph: &Paragraph) -> Result<(), String> {
        let theme = *self.theme();
        let x = self.left() + f64::from(paragraph.indent);
        let pad = f64::from(paragraph.size) * 0.8;
        let inner = self.right() - x - 2.0 * pad;
        let fitted = self.fit(&paragraph.text, &self.style(paragraph, theme.ink), inner)?;
        let boxed = fitted.room.height + 2.0 * pad;
        let top = self.place(f64::from(paragraph.space_before), boxed);
        let right = self.right();
        let fill = theme.dressed.then_some(theme.code);
        self.shape(
            shapes::rounded(x, top, right, top + boxed, 4.0),
            Some((theme.line, 0.6)),
            fill,
        );
        self.text((x + pad, top + pad), inner, fitted);
        self.top = top + boxed;
        Ok(())
    }

    fn equation(&mut self, paragraph: &Paragraph) -> Result<(), String> {
        let x = self.left() + f64::from(paragraph.indent);
        let style = Style {
            family: WIDER[0].to_owned(),
            size: f64::from(paragraph.size),
            bold: false,
            italic: false,
            colour: Some(self.ink()),
        };
        math::set_out(
            self,
            &paragraph.text,
            &style,
            (x, self.right()),
            f64::from(paragraph.space_before),
        )
    }

    fn rule(&mut self, space_before: f64, indent: f64) {
        let theme = *self.theme();
        let x = self.left() + indent;
        let top = self.place(space_before, 2.0);
        let right = self.right();
        let colour = if theme.dressed {
            theme::paler(theme.accent, 0.35)
        } else {
            theme.line
        };
        self.shape(
            shapes::line(x, top + 1.0, right, top + 1.0),
            Some((colour, 0.9)),
            None,
        );
        self.top = top + 2.0 + space_before / 2.0;
    }

    fn chart(&mut self, spec: &str, space_before: f64, indent: f64) -> Result<(), String> {
        chart::set_out(self, spec, space_before, indent)
    }

    fn table(&mut self, table: &Table) -> Result<(), String> {
        let theme = *self.theme();
        let x0 = self.left() + f64::from(table.indent);
        let wide = self.right() - x0;
        let mut edges = vec![x0];
        let mut at = x0;
        for share in &table.columns {
            at += wide * f64::from(*share);
            edges.push(at);
        }
        let pad = 5.0;
        let head = if table.head.is_empty() {
            None
        } else {
            let colour = if theme.dressed {
                theme.on_accent
            } else {
                theme.ink
            };
            Some(self.fit_row(&table.head, &edges, pad, colour)?)
        };
        let mut rows = Vec::new();
        for row in &table.rows {
            rows.push(self.fit_row(row, &edges, pad, theme.ink)?);
        }
        let first = rows.first().map_or(0.0, |row| row.height);
        let head_height = head.as_ref().map_or(0.0, |head| head.height);
        let top = self.place(f64::from(table.space_before), head_height + first);
        self.top = top;
        let mut segment = Segment::open(self);
        if let Some(head) = &head {
            self.row(
                head,
                &edges,
                &mut segment,
                theme.dressed.then_some(theme.accent),
            );
        }
        for (at, row) in rows.into_iter().enumerate() {
            if self.top + row.height > self.bottom() && self.top > segment.top + 1e-6 {
                segment.close(self, &edges);
                self.new_page();
                segment = Segment::open(self);
                if let Some(head) = &head {
                    self.row(
                        head,
                        &edges,
                        &mut segment,
                        theme.dressed.then_some(theme.accent),
                    );
                }
            }
            let stripe = (theme.dressed && at % 2 == 1).then_some(theme.stripe);
            self.row(&row, &edges, &mut segment, stripe);
        }
        segment.close(self, &edges);
        Ok(())
    }

    fn fit_row(
        &self,
        cells: &[Paragraph],
        edges: &[f64],
        pad: f64,
        colour: Colour,
    ) -> Result<FittedRow, String> {
        let mut fitted = Vec::new();
        let mut tallest: f64 = 0.0;
        for (at, cell) in cells.iter().enumerate() {
            let width = (edges[at + 1] - edges[at] - 2.0 * pad).max(1.0);
            if cell.text.trim().is_empty() {
                fitted.push(None);
                continue;
            }
            let piece = self.fit(&cell.text, &self.style(cell, colour), width)?;
            tallest = tallest.max(piece.room.height);
            fitted.push(Some(piece));
        }
        let least = cells
            .first()
            .map_or(12.0, |cell| f64::from(cell.size) * 1.2);
        Ok(FittedRow {
            cells: fitted,
            height: tallest.max(least) + 2.0 * pad,
            pad,
        })
    }

    fn row(&mut self, row: &FittedRow, edges: &[f64], segment: &mut Segment, fill: Option<Colour>) {
        let (top, bottom) = (self.top, self.top + row.height);
        let (left, right) = (edges[0], edges[edges.len() - 1]);
        if let Some(fill) = fill {
            segment
                .grounds
                .push((fill, shapes::rect(left, top, right, bottom)));
        }
        for (at, cell) in row.cells.iter().enumerate() {
            if let Some(cell) = cell {
                let width = (edges[at + 1] - edges[at] - 2.0 * row.pad).max(1.0);
                self.text((edges[at] + row.pad, top + row.pad), width, cell.clone());
            }
        }
        segment
            .lines
            .extend(shapes::line(left, bottom, right, bottom));
        self.top = bottom;
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Fitted {
    pub(crate) text: String,
    pub(crate) style: Style,
    pub(crate) room: Room,
}

struct FittedRow {
    cells: Vec<Option<Fitted>>,
    height: f64,
    pad: f64,
}

struct Segment {
    top: f64,
    first_mark: usize,
    grounds: Vec<(Colour, Vec<PenStep>)>,
    lines: Vec<PenStep>,
}

impl Segment {
    fn open(composer: &Composer<'_>) -> Self {
        Self {
            top: composer.top,
            first_mark: composer.marks.len(),
            grounds: Vec::new(),
            lines: Vec::new(),
        }
    }

    fn close(self, composer: &mut Composer<'_>, edges: &[f64]) {
        let theme = *composer.theme();
        let (top, bottom) = (self.top, composer.top);
        if bottom <= top {
            return;
        }
        let mut grouped: Vec<(Colour, Vec<PenStep>)> = Vec::new();
        for (colour, steps) in self.grounds {
            let same = |seen: &Colour| seen.map(f64::to_bits) == colour.map(f64::to_bits);
            match grouped.iter_mut().find(|(seen, _)| same(seen)) {
                Some((_, all)) => all.extend(steps),
                None => grouped.push((colour, steps)),
            }
        }
        let page = composer.page;
        for (offset, (colour, steps)) in grouped.into_iter().enumerate() {
            composer.marks.insert(
                self.first_mark + offset,
                Mark::Shape {
                    page,
                    steps,
                    stroke: None,
                    fill: Some(colour),
                },
            );
        }
        let mut inner = self.lines;
        for &x in &edges[1..edges.len() - 1] {
            inner.extend(shapes::line(x, top, x, bottom));
        }
        composer.shape(inner, Some((theme.line, 0.5)), None);
        let (left, right) = (edges[0], edges[edges.len() - 1]);
        let border = if theme.dressed {
            theme.accent
        } else {
            theme.line
        };
        composer.shape(
            shapes::rect(left, top, right, bottom),
            Some((border, 0.9)),
            None,
        );
    }
}

#[cfg(test)]
mod tests;
