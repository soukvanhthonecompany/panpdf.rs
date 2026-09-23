use crate::chart::Chart;
use crate::measure::{Measure, Pen};
use crate::page::{Grid, Ink, Marker, Text};
use crate::theme::{Style, Theme};
use crate::wrap::{Line, pen_for, wrap};
use crate::{Align, Block, Doc, Emphasis, Item, Span, Table};

#[derive(Clone, Debug, PartialEq)]
pub struct Flowed {
    pub x: f32,
    pub width: f32,
    pub height: f32,
    pub space_before: f32,
    pub ink: Ink,
    pub keep_with_next: bool,
    pub quote_at: Option<f32>,
}

impl Flowed {
    #[must_use]
    pub fn carrying(&self, ink: Ink, height: f32, space_before: f32) -> Self {
        Self {
            x: self.x,
            width: self.width,
            height,
            space_before,
            ink,
            keep_with_next: self.keep_with_next,
            quote_at: self.quote_at,
        }
    }
}

#[must_use]
pub fn flow(doc: &Doc, theme: &Theme, measure: &dyn Measure) -> Vec<Flowed> {
    let column = theme.column();
    let mut writer = Writer {
        theme,
        measure,
        out: Vec::new(),
        below: 0.0,
    };
    if let Some(title) = &doc.title {
        let spans = [Span::plain(title.clone())];
        writer.words(&spans, theme.heading(1), column.x, column.width, None, true);
    }
    writer.blocks(&doc.blocks, column.x, column.width, None);
    writer.out
}

struct Writer<'a> {
    theme: &'a Theme,
    measure: &'a dyn Measure,
    out: Vec<Flowed>,
    below: f32,
}

impl Writer<'_> {
    fn blocks(&mut self, blocks: &[Block], x: f32, width: f32, quote_at: Option<f32>) {
        for block in blocks {
            self.block(block, x, width, quote_at);
        }
    }

    fn block(&mut self, block: &Block, x: f32, width: f32, quote_at: Option<f32>) {
        let theme = self.theme;
        match block {
            Block::Heading { level, spans } => {
                self.owe(theme.space_over_heading(*level));
                self.words(spans, theme.heading(*level), x, width, quote_at, true);
                self.below = theme.under_heading;
            }
            Block::Paragraph(spans) => {
                self.owe(theme.between_paragraphs);
                self.words(spans, theme.body, x, width, quote_at, false);
                self.below = theme.between_paragraphs;
            }
            Block::List { ordered, items } => {
                self.owe(theme.between_paragraphs);
                self.list(items, *ordered, x, width, quote_at, 0);
                self.below = theme.between_paragraphs;
            }
            Block::Quote(inner) => self.quote(inner, x, width),
            Block::Code { lines, .. } => self.code(lines, x, width, quote_at),
            Block::Table(table) => self.table(table, x, width, quote_at),
            Block::Rule => {
                self.owe(theme.around_figures);
                self.put(Ink::Bar, x, width, theme.rule_thickness, quote_at, false);
                self.below = theme.around_figures;
            }
            Block::Chart(chart) => self.chart(chart, x, width, quote_at),
            Block::Picture { path, alt } => self.picture(path, alt, x, width, quote_at),
            Block::Unreadable { text, why } => {
                self.owe(theme.around_figures);
                let said = format!("Could not be read ({why}): {text}");
                let spans = [Span::plain(said).in_voice(Emphasis::PLAIN.slanted())];
                self.words(&spans, theme.caption, x, width, quote_at, false);
                self.below = theme.around_figures;
            }
        }
    }

    fn owe(&mut self, above: f32) {
        self.below = self.below.max(above);
    }

    fn put(
        &mut self,
        ink: Ink,
        x: f32,
        width: f32,
        height: f32,
        quote_at: Option<f32>,
        keep_with_next: bool,
    ) {
        let space_before = if self.out.is_empty() { 0.0 } else { self.below };
        self.below = 0.0;
        self.out.push(Flowed {
            x,
            width,
            height,
            space_before,
            ink,
            keep_with_next,
            quote_at,
        });
    }

    fn words(
        &mut self,
        spans: &[Span],
        style: Style,
        x: f32,
        width: f32,
        quote_at: Option<f32>,
        keep_with_next: bool,
    ) {
        let lines = wrap(spans, style, self.theme.inline_code, width, self.measure);
        let text = Text {
            lines,
            style,
            align: Align::Left,
            marker: None,
        };
        let height = text.height();
        self.put(Ink::Text(text), x, width, height, quote_at, keep_with_next);
    }

    fn list(
        &mut self,
        items: &[Item],
        ordered: bool,
        x: f32,
        width: f32,
        quote_at: Option<f32>,
        depth: u8,
    ) {
        let theme = self.theme;
        let indent = theme.list_indent;
        for (at, item) in items.iter().enumerate() {
            let mark = if ordered {
                format!("{}.", at + 1)
            } else {
                bullet(depth).to_owned()
            };
            let style = theme.body;
            let pen = Pen::from(style);
            let marker = Marker {
                x: -(theme.bullet_gap + self.measure.width(&mark, pen)),
                text: mark,
                pen,
            };
            let lines = wrap(
                &item.spans,
                style,
                theme.inline_code,
                width - indent,
                self.measure,
            );
            let text = Text {
                lines,
                style,
                align: Align::Left,
                marker: Some(marker),
            };
            let height = text.height();
            if at > 0 {
                self.owe(theme.between_items);
            }
            let keep = !item.children.is_empty();
            self.put(
                Ink::Text(text),
                x + indent,
                width - indent,
                height,
                quote_at,
                keep,
            );
            if !item.children.is_empty() {
                self.owe(theme.between_items);
                self.list(
                    &item.children,
                    ordered,
                    x + indent,
                    width - indent,
                    quote_at,
                    depth.saturating_add(1),
                );
            }
        }
    }

    fn quote(&mut self, inner: &[Block], x: f32, width: f32) {
        let theme = self.theme;
        self.owe(theme.around_figures);
        let indent = theme.quote_indent;
        self.blocks(inner, x + indent, width - indent, Some(x));
        self.below = theme.around_figures;
    }

    fn code(&mut self, lines: &[String], x: f32, width: f32, quote_at: Option<f32>) {
        let theme = self.theme;
        self.owe(theme.around_figures);
        let style = theme.code;
        let mut broken: Vec<Line> = Vec::new();
        for line in lines {
            let spans = [Span::plain(line.clone())];
            broken.extend(wrap(&spans, style, style.face, width, self.measure));
        }
        let text = Text {
            lines: broken,
            style,
            align: Align::Left,
            marker: None,
        };
        let height = text.height();
        self.put(Ink::Text(text), x, width, height, quote_at, false);
        self.below = theme.around_figures;
    }

    fn chart(&mut self, chart: &Chart, x: f32, width: f32, quote_at: Option<f32>) {
        let theme = self.theme;
        self.owe(theme.around_figures);
        if let Some(title) = &chart.title {
            let spans = [Span::plain(title.clone()).in_voice(Emphasis::PLAIN.bolder())];
            self.words(&spans, theme.body, x, width, quote_at, true);
            self.below = theme.under_heading;
        }
        self.put(
            Ink::Chart(chart.clone()),
            x,
            width,
            theme.chart_height,
            quote_at,
            chart.note.is_some(),
        );
        if let Some(note) = &chart.note {
            self.below = theme.under_heading;
            let spans = [Span::plain(note.clone())];
            self.words(&spans, theme.caption, x, width, quote_at, false);
        }
        self.below = theme.around_figures;
    }

    fn picture(&mut self, path: &str, alt: &str, x: f32, width: f32, quote_at: Option<f32>) {
        let theme = self.theme;
        self.owe(theme.around_figures);
        self.put(
            Ink::Picture {
                path: path.to_owned(),
                alt: alt.to_owned(),
            },
            x,
            width,
            theme.most_picture_height,
            quote_at,
            !alt.is_empty(),
        );
        if !alt.is_empty() {
            self.below = theme.under_heading;
            let spans = [Span::plain(alt.to_owned())];
            self.words(&spans, theme.caption, x, width, quote_at, false);
        }
        self.below = theme.around_figures;
    }

    fn table(&mut self, table: &Table, x: f32, width: f32, quote_at: Option<f32>) {
        let theme = self.theme;
        self.owe(theme.around_figures);
        let grid = self.grid(table, width);
        let height = grid.height();
        self.put(Ink::Table(grid), x, width, height, quote_at, false);
        self.below = theme.around_figures;
    }

    fn grid(&self, table: &Table, width: f32) -> Grid {
        let theme = self.theme;
        let body = theme.body;
        let head_style = body.bolder();
        let columns = wide_enough(table);
        let widths = columns_of(table, columns, width, theme, self.measure);
        let head = self.cells(&table.head, head_style, &widths);
        let rows: Vec<Vec<Vec<Line>>> = table
            .rows
            .iter()
            .map(|row| self.cells(row, body, &widths))
            .collect();
        let head_height = tallest(&head, head_style.leading, theme.cell_padding_down);
        let row_heights = rows
            .iter()
            .map(|row| tallest(row, body.leading, theme.cell_padding_down))
            .collect();
        let mut align = table.align.clone();
        align.resize(columns, Align::Left);
        Grid {
            head,
            rows,
            widths,
            align,
            head_style,
            body_style: body,
            head_height,
            row_heights,
            rule: theme.table_rule,
            padding_across: theme.cell_padding_across,
            padding_down: theme.cell_padding_down,
        }
    }

    fn cells(&self, row: &[Vec<Span>], style: Style, widths: &[f32]) -> Vec<Vec<Line>> {
        widths
            .iter()
            .enumerate()
            .map(|(at, column)| {
                let empty = Vec::new();
                let spans = row.get(at).unwrap_or(&empty);
                let room = column - self.theme.cell_padding_across * 2.0;
                wrap(spans, style, self.theme.inline_code, room, self.measure)
            })
            .collect()
    }
}

fn bullet(depth: u8) -> &'static str {
    ["\u{2022}", "\u{25e6}", "\u{2013}"][usize::from(depth % 3)]
}

fn wide_enough(table: &Table) -> usize {
    let widest = table.rows.iter().map(Vec::len).max().unwrap_or(0);
    table.head.len().max(widest).max(table.align.len()).max(1)
}

fn columns_of(
    table: &Table,
    columns: usize,
    width: f32,
    theme: &Theme,
    measure: &dyn Measure,
) -> Vec<f32> {
    let padding = theme.cell_padding_across * 2.0;
    let least = padding + theme.body.size;
    let mut wanted = vec![least; columns];
    for (at, cell) in table.head.iter().enumerate().take(columns) {
        let want = said_width(cell, theme.body.bolder(), theme, measure) + padding;
        wanted[at] = wanted[at].max(want);
    }
    for row in &table.rows {
        for (at, cell) in row.iter().enumerate().take(columns) {
            let want = said_width(cell, theme.body, theme, measure) + padding;
            wanted[at] = wanted[at].max(want);
        }
    }
    let total: f32 = wanted.iter().sum();
    if total <= 0.0 {
        #[allow(clippy::cast_precision_loss)]
        let share = width / columns as f32;
        return vec![share; columns];
    }
    let scale = width / total;
    wanted.iter().map(|want| want * scale).collect()
}

fn said_width(spans: &[Span], style: Style, theme: &Theme, measure: &dyn Measure) -> f32 {
    spans
        .iter()
        .map(|span| {
            let pen = pen_for(style, theme.inline_code, span.emphasis);
            measure.width(&span.text, pen)
        })
        .sum()
}

fn tallest(row: &[Vec<Line>], leading: f32, padding: f32) -> f32 {
    #[allow(clippy::cast_precision_loss)]
    let deepest = row.iter().map(Vec::len).max().unwrap_or(1).max(1) as f32;
    deepest.mul_add(leading, padding * 2.0)
}

#[must_use]
pub fn compose(doc: &Doc, theme: &Theme, measure: &dyn Measure) -> Vec<crate::page::Sheet> {
    crate::page::paginate(flow(doc, theme, measure), theme)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::flow;
    use crate::measure::Even;
    use crate::page::Ink;
    use crate::parse;
    use crate::theme::{Edges, Face, Style, Theme};

    pub(crate) fn plain() -> Theme {
        let body = Style::new(Face::Body, 10.0, 20.0);
        Theme {
            page_width: 200.0,
            page_height: 100.0,
            margin: Edges::all(10.0),
            body,
            headings: [Style::new(Face::Display, 10.0, 20.0); 4],
            code: body,
            quote: body,
            caption: body,
            over_heading: [0.0; 4],
            under_heading: 0.0,
            between_paragraphs: 0.0,
            around_figures: 0.0,
            between_items: 0.0,
            chart_height: 40.0,
            most_picture_height: 40.0,
            ..Theme::house()
        }
    }

    #[test]
    fn a_paragraph_is_as_tall_as_the_lines_it_broke_into() {
        let doc = parse::read(&"ab ".repeat(20));
        let laid = flow(&doc, &plain(), &Even::half());
        assert_eq!(laid.len(), 1);
        let Ink::Text(text) = &laid[0].ink else {
            panic!("a paragraph is text");
        };
        assert_eq!(text.lines.len(), 2);
        assert!((laid[0].height - 40.0).abs() < 0.01);
    }

    #[test]
    fn air_between_two_blocks_is_the_larger_ask_and_not_the_sum() {
        let mut theme = plain();
        theme.under_heading = 6.0;
        theme.between_paragraphs = 9.0;
        let laid = flow(&parse::read("# Title\n\nWords.\n"), &theme, &Even::half());
        assert_eq!(laid.len(), 2);
        assert!(
            (laid[1].space_before - 9.0).abs() < 0.01,
            "got {}",
            laid[1].space_before
        );
        theme.under_heading = 20.0;
        let again = flow(&parse::read("# Title\n\nWords.\n"), &theme, &Even::half());
        assert!((again[1].space_before - 20.0).abs() < 0.01);
    }

    #[test]
    fn nothing_asks_for_air_above_the_first_block() {
        let mut theme = plain();
        theme.over_heading = [24.0; 4];
        let laid = flow(&parse::read("# Title\n"), &theme, &Even::half());
        assert!((laid[0].space_before - 0.0).abs() < 0.01);
    }

    #[test]
    fn a_heading_is_kept_with_what_follows_it_and_a_paragraph_is_not() {
        let laid = flow(&parse::read("# Title\n\nWords.\n"), &plain(), &Even::half());
        assert!(laid[0].keep_with_next);
        assert!(!laid[1].keep_with_next);
    }

    #[test]
    fn a_list_item_carries_its_bullet_outside_its_frame() {
        let theme = plain();
        let laid = flow(&parse::read("- one\n- two\n"), &theme, &Even::half());
        assert_eq!(laid.len(), 2);
        let Ink::Text(first) = &laid[0].ink else {
            panic!("an item is text");
        };
        let marker = first.marker.as_ref().expect("an item has a bullet");
        assert!(marker.x < 0.0, "the bullet stands to the left of the words");
        assert!((laid[0].x - (theme.column().x + theme.list_indent)).abs() < 0.01);
        let plain_words = flow(&parse::read("words\n"), &theme, &Even::half());
        let Ink::Text(text) = &plain_words[0].ink else {
            panic!()
        };
        assert!(text.marker.is_none());
    }

    #[test]
    fn a_numbered_list_counts_and_a_bulleted_one_does_not() {
        let laid = flow(&parse::read("1. one\n2. two\n"), &plain(), &Even::half());
        let marks: Vec<String> = laid
            .iter()
            .filter_map(|item| match &item.ink {
                Ink::Text(text) => text.marker.as_ref().map(|mark| mark.text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(marks, ["1.", "2."]);
    }

    #[test]
    fn everything_inside_a_quotation_knows_where_its_rule_stands() {
        let theme = plain();
        let laid = flow(&parse::read("> quoted\n"), &theme, &Even::half());
        let at = laid[0]
            .quote_at
            .expect("a quoted block stands beside a rule");
        assert!((at - theme.column().x).abs() < 0.01);
        assert!(laid[0].x > at, "the words are set in from the rule");
        let ordinary = flow(&parse::read("quoted\n"), &theme, &Even::half());
        assert!(ordinary[0].quote_at.is_none());
    }

    #[test]
    fn a_table_is_measured_to_the_whole_measure() {
        let theme = plain();
        let table = "| a | bbbbbbbb |\n| --- | --- |\n| c | d |\n";
        let laid = flow(&parse::read(table), &theme, &Even::half());
        let Ink::Table(grid) = &laid[0].ink else {
            panic!("a table is a table")
        };
        assert_eq!(grid.widths.len(), 2);
        let total: f32 = grid.widths.iter().sum();
        assert!((total - theme.column().width).abs() < 0.01, "got {total}");
        assert!(grid.widths[1] > grid.widths[0]);
        assert!((laid[0].height - grid.height()).abs() < 0.01);
    }

    #[test]
    fn a_chart_keeps_its_title_and_its_note_with_it() {
        let markdown = "```chart\n{\"kind\":\"bar\",\"title\":\"T\",\"note\":\"N\",\
            \"labels\":[\"a\"],\"series\":[{\"name\":\"s\",\"values\":[1]}]}\n```\n";
        let laid = flow(&parse::read(markdown), &plain(), &Even::half());
        assert_eq!(laid.len(), 3, "title, plot and note");
        assert!(laid[0].keep_with_next && laid[1].keep_with_next);
        assert!(matches!(laid[1].ink, Ink::Chart(_)));
    }

    #[test]
    fn a_block_this_crate_could_not_read_still_takes_room_on_the_page() {
        let markdown = "```chart\n{not json}\n```\n";
        let doc = parse::read(markdown);
        let laid = flow(&doc, &plain(), &Even::half());
        let Ink::Text(text) = &laid[0].ink else {
            panic!("a note is text")
        };
        assert!(text.text().contains("Could not be read"));
        assert!(laid[0].height > 0.0);
    }
}
