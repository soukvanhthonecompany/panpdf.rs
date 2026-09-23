use crate::Align;
use crate::chart::Chart;
use crate::flow::Flowed;
use crate::measure::Pen;
use crate::theme::{Rect, Style, Theme};
use crate::wrap::Line;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sheet {
    pub width: f32,
    pub height: f32,
    pub pieces: Vec<Piece>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub frame: Rect,
    pub ink: Ink,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Ink {
    Text(Text),
    Bar,
    Chart(Chart),
    Picture { path: String, alt: String },
    Table(Grid),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Text {
    pub lines: Vec<Line>,
    pub style: Style,
    pub align: Align,
    pub marker: Option<Marker>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Marker {
    pub text: String,
    pub pen: Pen,
    pub x: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    pub head: Vec<Vec<Line>>,
    pub rows: Vec<Vec<Vec<Line>>>,
    pub widths: Vec<f32>,
    pub align: Vec<Align>,
    pub head_style: Style,
    pub body_style: Style,
    pub head_height: f32,
    pub row_heights: Vec<f32>,
    pub rule: f32,
    pub padding_across: f32,
    pub padding_down: f32,
}

impl Text {
    #[must_use]
    pub fn text(&self) -> String {
        let mut out = String::new();
        for (at, line) in self.lines.iter().enumerate() {
            if at > 0 && !self.lines[at - 1].split_word {
                out.push(' ');
            }
            out.push_str(&line.text());
        }
        out
    }

    #[must_use]
    pub fn height(&self) -> f32 {
        #[allow(clippy::cast_precision_loss)]
        let count = self.lines.len() as f32;
        count * self.style.leading
    }
}

impl Grid {
    #[must_use]
    pub fn height(&self) -> f32 {
        #[allow(clippy::cast_precision_loss)]
        let rules = (self.rows.len() + 1) as f32 * self.rule;
        self.head_height + self.row_heights.iter().sum::<f32>() + rules
    }
}

#[must_use]
pub fn paginate(flowed: Vec<Flowed>, theme: &Theme) -> Vec<Sheet> {
    let column = theme.column();
    let mut pages = Pages::new(theme, column);
    let mut waiting: Vec<Flowed> = flowed.into_iter().rev().collect();
    while let Some(item) = waiting.pop() {
        let gap = if pages.at_top() {
            0.0
        } else {
            item.space_before
        };
        let room = column.bottom() - pages.y - gap;
        if item.height <= room && !pages.would_strand(&item, room, waiting.last()) {
            pages.place(item, gap);
            continue;
        }
        match split(&item, room) {
            Some((first, rest)) if !pages.at_top() || first.height <= column.height => {
                pages.place(first, gap);
                waiting.push(rest);
                pages.turn();
            }
            _ => {
                if pages.at_top() {
                    pages.place(item, 0.0);
                    pages.turn();
                } else {
                    pages.turn();
                    waiting.push(item);
                }
            }
        }
    }
    pages.finish()
}

struct Pages<'a> {
    theme: &'a Theme,
    column: Rect,
    done: Vec<Sheet>,
    pieces: Vec<Piece>,
    bars: Vec<(f32, f32, f32)>,
    y: f32,
}

impl<'a> Pages<'a> {
    fn new(theme: &'a Theme, column: Rect) -> Self {
        Self {
            theme,
            column,
            done: Vec::new(),
            pieces: Vec::new(),
            bars: Vec::new(),
            y: column.y,
        }
    }

    fn at_top(&self) -> bool {
        self.pieces.is_empty()
    }

    fn place(&mut self, item: Flowed, gap: f32) {
        let top = self.y + gap;
        if let Some(x) = item.quote_at {
            match self.bars.last_mut() {
                Some(bar) if (bar.0 - x).abs() < 0.01 && (bar.2 - self.y).abs() < gap + 0.01 => {
                    bar.2 = top + item.height;
                }
                _ => self.bars.push((x, top, top + item.height)),
            }
        }
        self.pieces.push(Piece {
            frame: Rect::new(item.x, top, item.width, item.height),
            ink: item.ink,
        });
        self.y = top + item.height;
    }

    fn would_strand(&self, item: &Flowed, room: f32, next: Option<&Flowed>) -> bool {
        if !item.keep_with_next || self.at_top() {
            return false;
        }
        let Some(next) = next else { return false };
        let left = room - item.height - next.space_before;
        let needed = least_of(next, self.theme);
        needed > left
    }

    fn turn(&mut self) {
        if self.pieces.is_empty() && self.bars.is_empty() {
            return;
        }
        for (x, top, bottom) in std::mem::take(&mut self.bars) {
            self.pieces.push(Piece {
                frame: Rect::new(x, top, self.theme.quote_rule, bottom - top),
                ink: Ink::Bar,
            });
        }
        self.done.push(Sheet {
            width: self.theme.page_width,
            height: self.theme.page_height,
            pieces: std::mem::take(&mut self.pieces),
        });
        self.y = self.column.y;
    }

    fn finish(mut self) -> Vec<Sheet> {
        self.turn();
        self.done
    }
}

fn least_of(item: &Flowed, theme: &Theme) -> f32 {
    match &item.ink {
        Ink::Text(text) if text.lines.len() >= 4 => text.style.leading * 2.0,
        Ink::Table(grid) if grid.rows.len() >= 2 => {
            grid.head_height + grid.row_heights[0] + theme.table_rule * 2.0
        }
        _ => item.height,
    }
}

fn split(item: &Flowed, room: f32) -> Option<(Flowed, Flowed)> {
    if room <= 0.0 {
        return None;
    }
    match &item.ink {
        Ink::Text(text) => split_text(item, text, room),
        Ink::Table(grid) => split_table(item, grid, room),
        Ink::Bar | Ink::Chart(_) | Ink::Picture { .. } => None,
    }
}

fn split_text(item: &Flowed, text: &Text, room: f32) -> Option<(Flowed, Flowed)> {
    let leading = text.style.leading;
    if leading <= 0.0 || text.lines.len() < 4 {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let room_holds = (room / leading).floor().max(0.0) as usize;
    let keep = room_holds.min(text.lines.len() - 2);
    if keep < 2 {
        return None;
    }
    let mut first = text.clone();
    let rest_lines = first.lines.split_off(keep);
    let rest = Text {
        lines: rest_lines,
        style: text.style,
        align: text.align,
        marker: None,
    };
    let tall = first.height();
    let over = rest.height();
    Some((
        item.carrying(Ink::Text(first), tall, item.space_before),
        item.carrying(Ink::Text(rest), over, 0.0),
    ))
}

fn split_table(item: &Flowed, grid: &Grid, room: f32) -> Option<(Flowed, Flowed)> {
    if grid.rows.len() < 2 {
        return None;
    }
    let mut used = grid.head_height + grid.rule;
    let mut keep = 0;
    for height in &grid.row_heights {
        if used + height + grid.rule > room {
            break;
        }
        used += height + grid.rule;
        keep += 1;
    }
    let keep = keep.min(grid.rows.len() - 1);
    if keep == 0 {
        return None;
    }
    let mut first = grid.clone();
    let rest_rows = first.rows.split_off(keep);
    let rest_heights = first.row_heights.split_off(keep);
    let rest = Grid {
        rows: rest_rows,
        row_heights: rest_heights,
        ..grid.clone()
    };
    let tall = first.height();
    let over = rest.height();
    Some((
        item.carrying(Ink::Table(first), tall, item.space_before),
        item.carrying(Ink::Table(rest), over, 0.0),
    ))
}

#[cfg(test)]
mod tests {
    use super::{Ink, Sheet};
    use crate::flow::tests::plain;
    use crate::flow::{compose, flow};
    use crate::measure::Even;
    use crate::parse;
    use crate::theme::Theme;

    fn lines_on(sheet: &Sheet) -> Vec<String> {
        sheet
            .pieces
            .iter()
            .filter_map(|piece| match &piece.ink {
                Ink::Text(text) => Some(text.lines.iter().map(crate::wrap::Line::text)),
                _ => None,
            })
            .flatten()
            .collect()
    }

    fn counts(sheets: &[Sheet]) -> Vec<usize> {
        sheets.iter().map(|sheet| lines_on(sheet).len()).collect()
    }

    #[test]
    fn a_sheet_holds_what_fits_and_the_rest_goes_over() {
        let theme = plain();
        let doc = parse::read(&"ab ".repeat(150));
        let sheets = compose(&doc, &theme, &Even::half());
        assert!(sheets.len() > 2, "got {} sheets", sheets.len());
        for count in counts(&sheets) {
            assert!(count <= 4, "a sheet held {count} lines");
        }
        let said: String = sheets
            .iter()
            .flat_map(lines_on)
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(said.matches("ab").count(), 150);
    }

    #[test]
    fn a_break_never_leaves_one_line_by_itself() {
        let theme = plain();
        let doc = parse::read(&"ab ".repeat(58));
        let laid = flow(&doc, &theme, &Even::half());
        let Ink::Text(whole) = &laid[0].ink else {
            panic!()
        };
        assert_eq!(whole.lines.len(), 5, "the test needs exactly five lines");
        let sheets = compose(&doc, &theme, &Even::half());
        assert_eq!(counts(&sheets), [3, 2]);
    }

    #[test]
    fn a_heading_does_not_stand_alone_at_the_foot_of_a_sheet() {
        let theme = plain();
        let doc = parse::read(&format!("{}\n\n# Heading\n\nAfter.\n", "ab ".repeat(34)));
        let sheets = compose(&doc, &theme, &Even::half());
        assert_eq!(sheets.len(), 2);
        assert!(!lines_on(&sheets[0]).iter().any(|line| line == "Heading"));
        let second = lines_on(&sheets[1]);
        assert_eq!(second[0], "Heading");
        assert_eq!(second[1], "After.");
        let roomy = compose(&parse::read("# Heading\n\nAfter.\n"), &theme, &Even::half());
        assert_eq!(roomy.len(), 1);
    }

    #[test]
    fn a_table_writes_its_head_again_over_the_page() {
        let theme = plain();
        let mut markdown = String::from("| a | b |\n| --- | --- |\n");
        for row in 0..8 {
            use std::fmt::Write;
            let _ = writeln!(markdown, "| r{row} | v{row} |");
        }
        let sheets = compose(&parse::read(&markdown), &theme, &Even::half());
        assert!(sheets.len() > 1, "eight rows do not fit one short sheet");
        let mut rows = 0;
        for sheet in &sheets {
            let grid = sheet
                .pieces
                .iter()
                .find_map(|piece| match &piece.ink {
                    Ink::Table(grid) => Some(grid),
                    _ => None,
                })
                .expect("every sheet of a split table holds part of it");
            assert_eq!(grid.head[0][0].text(), "a", "the head is written again");
            assert_eq!(grid.rows.len(), grid.row_heights.len());
            rows += grid.rows.len();
        }
        assert_eq!(rows, 8, "no row was lost or written twice");
    }

    #[test]
    fn a_quotation_gets_one_rule_beside_it() {
        let theme = plain();
        let sheets = compose(&parse::read("> one\n>\n> two\n"), &theme, &Even::half());
        let bars: Vec<_> = sheets[0]
            .pieces
            .iter()
            .filter(|piece| piece.ink == Ink::Bar)
            .collect();
        assert_eq!(bars.len(), 1, "one rule for the whole quotation");
        assert!((bars[0].frame.width - theme.quote_rule).abs() < 0.01);
        assert!(bars[0].frame.height >= 40.0, "it reaches both paragraphs");
        let dry = compose(&parse::read("one\n\ntwo\n"), &theme, &Even::half());
        assert!(!dry[0].pieces.iter().any(|piece| piece.ink == Ink::Bar));
    }

    #[test]
    fn everything_stands_inside_the_margins() {
        let theme = Theme::house();
        let markdown = "# Report\n\nSome words about the matter at hand, at \
            length, so that the paragraph runs over more than one line and \
            over more than one sheet of paper as well.\n\n\
            - first\n- second\n  - deeper\n\n> quoted words\n\n---\n\n\
            | a | b |\n| --- | --- |\n| 1 | 2 |\n";
        let doc = parse::read(&markdown.repeat(6));
        let sheets = compose(&doc, &theme, &Even::half());
        let column = theme.column();
        assert!(sheets.len() > 1);
        for sheet in &sheets {
            for piece in &sheet.pieces {
                assert!(piece.frame.x >= column.x - 0.01, "{:?}", piece.frame);
                assert!(
                    piece.frame.right() <= column.right() + 0.01,
                    "{:?}",
                    piece.frame
                );
                assert!(piece.frame.y >= column.y - 0.01, "{:?}", piece.frame);
                assert!(
                    piece.frame.bottom() <= column.bottom() + 0.01,
                    "{:?} runs off the foot",
                    piece.frame
                );
            }
        }
    }

    #[test]
    fn a_block_puts_its_words_back_together_as_they_were_written() {
        let theme = plain();
        let said = "one two three four five six seven eight nine ten eleven";
        let sheets = compose(&parse::read(said), &theme, &Even::half());
        let back: Vec<String> = sheets
            .iter()
            .flat_map(|sheet| sheet.pieces.iter())
            .filter_map(|piece| match &piece.ink {
                Ink::Text(text) => Some(text.text()),
                _ => None,
            })
            .collect();
        assert_eq!(back.join(" "), said);
    }

    #[test]
    fn a_word_broken_across_lines_is_not_given_a_space_it_never_had() {
        let theme = plain();
        let long = "a".repeat(80);
        let sheets = compose(&parse::read(&long), &theme, &Even::half());
        let back: String = sheets
            .iter()
            .flat_map(|sheet| sheet.pieces.iter())
            .filter_map(|piece| match &piece.ink {
                Ink::Text(text) => Some(text.text()),
                _ => None,
            })
            .collect();
        assert_eq!(back, long);
    }

    #[test]
    fn a_document_with_nothing_in_it_makes_no_paper() {
        assert!(compose(&parse::read(""), &plain(), &Even::half()).is_empty());
    }

    #[test]
    fn a_sheet_says_how_big_the_paper_is() {
        let sheets = compose(&parse::read("words\n"), &Theme::house(), &Even::half());
        assert!((sheets[0].width - 595.0).abs() < 0.01);
        assert!((sheets[0].height - 842.0).abs() < 0.01);
    }
}
