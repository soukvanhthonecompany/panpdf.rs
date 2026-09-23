use crate::{Align, Block, Doc, Emphasis, Item, Span, Table};

#[must_use]
pub fn read(text: &str) -> Doc {
    let (title, body) = front_matter(text);
    let lines: Vec<&str> = body.lines().collect();
    let mut blocks = Vec::new();
    let mut at = 0;
    while at < lines.len() {
        if lines[at].trim().is_empty() {
            at += 1;
            continue;
        }
        let (block, next) = one_block(&lines, at);
        if let Some(block) = block {
            blocks.push(block);
        }
        at = next.max(at + 1);
    }
    Doc { title, blocks }
}

fn front_matter(text: &str) -> (Option<String>, &str) {
    let body = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some(rest) = body.strip_prefix("---\n") else {
        return (None, body);
    };
    let Some(end) = rest.find("\n---") else {
        return (None, body);
    };
    let after = rest[end..]
        .strip_prefix("\n---")
        .map_or("", |tail| tail.trim_start_matches(['\r', '\n']));
    let title = rest[..end]
        .lines()
        .find_map(|line| line.trim().strip_prefix("title:").map(str::trim))
        .map(|name| name.trim_matches(['"', '\'']).to_owned())
        .filter(|name| !name.is_empty());
    (title, after)
}

fn one_block(lines: &[&str], at: usize) -> (Option<Block>, usize) {
    let line = lines[at];
    let bare = line.trim_start();
    if let Some(fence) = fence_of(bare) {
        return fenced(lines, at, fence);
    }
    if is_rule(bare) {
        return (Some(Block::Rule), at + 1);
    }
    if let Some(block) = heading(bare) {
        return (Some(block), at + 1);
    }
    if bare.starts_with('>') {
        return quote(lines, at);
    }
    if bullet_of(line).is_some() || number_of(line).is_some() {
        return list(lines, at);
    }
    if let Some(block) = table(lines, at) {
        return block;
    }
    if let Some(block) = lone_picture(bare) {
        return (Some(block), at + 1);
    }
    paragraph(lines, at)
}

fn fence_of(bare: &str) -> Option<(char, usize, String)> {
    let mark = bare.chars().next().filter(|it| matches!(it, '`' | '~'))?;
    let run = bare.chars().take_while(|it| *it == mark).count();
    if run < 3 {
        return None;
    }
    Some((mark, run, bare[run..].trim().to_owned()))
}

fn fenced(lines: &[&str], at: usize, fence: (char, usize, String)) -> (Option<Block>, usize) {
    let (mark, run, word) = fence;
    let mut body = Vec::new();
    let mut end = lines.len();
    for (which, line) in lines.iter().enumerate().skip(at + 1) {
        let closing = line.trim();
        if closing.chars().take_while(|it| *it == mark).count() >= run
            && closing.chars().all(|it| it == mark)
        {
            end = which;
            break;
        }
        body.push((*line).to_owned());
    }
    let after = (end + 1).min(lines.len().max(end));
    if word.eq_ignore_ascii_case("chart") {
        return (Some(chart_block(&body)), after);
    }
    let language = Some(word).filter(|it| !it.is_empty());
    (
        Some(Block::Code {
            language,
            lines: body,
        }),
        after,
    )
}

fn chart_block(body: &[String]) -> Block {
    let text = body.join("\n");
    match crate::chart::read(&text) {
        Ok(chart) => Block::Chart(chart),
        Err(why) => Block::Unreadable { text, why },
    }
}

fn is_rule(bare: &str) -> bool {
    let plain: String = bare.chars().filter(|it| !it.is_whitespace()).collect();
    plain.len() >= 3
        && (plain.chars().all(|it| it == '-')
            || plain.chars().all(|it| it == '*')
            || plain.chars().all(|it| it == '_'))
}

fn heading(bare: &str) -> Option<Block> {
    let hashes = bare.chars().take_while(|it| *it == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = bare[hashes..].strip_prefix(' ')?;
    let level = u8::try_from(hashes).unwrap_or(6).min(4);
    Some(Block::Heading {
        level,
        spans: spans_of(rest.trim_end_matches([' ', '#'])),
    })
}

fn quote(lines: &[&str], at: usize) -> (Option<Block>, usize) {
    let mut inner = Vec::new();
    let mut end = at;
    while end < lines.len() {
        let bare = lines[end].trim_start();
        if let Some(rest) = bare.strip_prefix('>') {
            inner.push(rest.strip_prefix(' ').unwrap_or(rest).to_owned());
            end += 1;
        } else if bare.is_empty() || inner.is_empty() {
            break;
        } else {
            inner.push(bare.to_owned());
            end += 1;
        }
    }
    let within = read(&inner.join("\n"));
    (Some(Block::Quote(within.blocks)), end)
}

fn lone_picture(bare: &str) -> Option<Block> {
    let rest = bare.strip_prefix("![")?;
    let close = rest.find("](")?;
    let end = rest.rfind(')')?;
    if end < close + 2 || !rest[end + 1..].trim().is_empty() {
        return None;
    }
    Some(Block::Picture {
        alt: rest[..close].to_owned(),
        path: rest[close + 2..end].trim().to_owned(),
    })
}

fn paragraph(lines: &[&str], at: usize) -> (Option<Block>, usize) {
    let mut gathered = Vec::new();
    let mut end = at;
    while end < lines.len() {
        let line = lines[end];
        let bare = line.trim();
        if bare.is_empty() {
            break;
        }
        if end > at && starts_something(lines, end) {
            break;
        }
        gathered.push(bare);
        end += 1;
    }
    if gathered.is_empty() {
        return (None, end + 1);
    }
    (Some(Block::Paragraph(spans_of(&gathered.join(" ")))), end)
}

fn starts_something(lines: &[&str], at: usize) -> bool {
    let bare = lines[at].trim_start();
    fence_of(bare).is_some()
        || is_rule(bare)
        || heading(bare).is_some()
        || bare.starts_with('>')
        || bullet_of(lines[at]).is_some()
        || number_of(lines[at]).is_some()
}

fn bullet_of(line: &str) -> Option<(usize, &str)> {
    let indent = line.len() - line.trim_start().len();
    let bare = line.trim_start();
    let mark = bare.chars().next()?;
    if !matches!(mark, '-' | '*' | '+') {
        return None;
    }
    let rest = bare[mark.len_utf8()..].strip_prefix(' ')?;
    if is_rule(bare) {
        return None;
    }
    Some((indent, rest))
}

fn number_of(line: &str) -> Option<(usize, &str)> {
    let indent = line.len() - line.trim_start().len();
    let bare = line.trim_start();
    let digits = bare.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 || digits > 9 {
        return None;
    }
    let after = &bare[digits..];
    let rest = after
        .strip_prefix(". ")
        .or_else(|| after.strip_prefix(") "))?;
    Some((indent, rest))
}

struct Entry {
    indent: usize,
    text: String,
}

fn list(lines: &[&str], at: usize) -> (Option<Block>, usize) {
    let ordered = number_of(lines[at]).is_some();
    let mut entries: Vec<Entry> = Vec::new();
    let mut end = at;
    while end < lines.len() {
        let line = lines[end];
        if let Some((indent, rest)) = bullet_of(line).or_else(|| number_of(line)) {
            entries.push(Entry {
                indent,
                text: rest.trim().to_owned(),
            });
            end += 1;
        } else if line.trim().is_empty() {
            break;
        } else if let Some(last) = entries.last_mut() {
            last.text.push(' ');
            last.text.push_str(line.trim());
            end += 1;
        } else {
            break;
        }
    }
    if entries.is_empty() {
        return (None, at + 1);
    }
    let items = nest(&entries);
    (Some(Block::List { ordered, items }), end)
}

fn nest(entries: &[Entry]) -> Vec<Item> {
    let mut roots: Vec<Item> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    for entry in entries {
        while open.last().is_some_and(|indent| *indent >= entry.indent) {
            open.pop();
        }
        let item = Item {
            spans: spans_of(&entry.text),
            children: Vec::new(),
        };
        let depth = open.len();
        put(&mut roots, depth, item);
        open.push(entry.indent);
    }
    roots
}

fn put(items: &mut Vec<Item>, depth: usize, item: Item) {
    if depth == 0 || items.is_empty() {
        items.push(item);
        return;
    }
    let last = items.len() - 1;
    put(&mut items[last].children, depth - 1, item);
}

fn table(lines: &[&str], at: usize) -> Option<(Option<Block>, usize)> {
    let head = lines[at];
    if !head.contains('|') {
        return None;
    }
    let align = alignments(lines.get(at + 1)?)?;
    let mut end = at + 2;
    let mut rows = Vec::new();
    while end < lines.len() && lines[end].contains('|') && !lines[end].trim().is_empty() {
        rows.push(cells_of(lines[end]));
        end += 1;
    }
    let table = Table {
        head: cells_of(head),
        rows,
        align,
    };
    Some((Some(Block::Table(table)), end))
}

fn alignments(line: &str) -> Option<Vec<Align>> {
    let bare = line.trim();
    if !bare.contains('-') || !bare.contains('|') {
        return None;
    }
    let mut out = Vec::new();
    for cell in split_cells(bare) {
        let mark = cell.trim();
        if mark.is_empty() || !mark.chars().all(|it| matches!(it, '-' | ':' | ' ')) {
            return None;
        }
        out.push(match (mark.starts_with(':'), mark.ends_with(':')) {
            (true, true) => Align::Centre,
            (false, true) => Align::Right,
            _ => Align::Left,
        });
    }
    (!out.is_empty()).then_some(out)
}

fn cells_of(line: &str) -> Vec<Vec<Span>> {
    split_cells(line.trim())
        .into_iter()
        .map(|cell| spans_of(cell.trim()))
        .collect()
}

fn split_cells(bare: &str) -> Vec<&str> {
    let inner = bare
        .strip_prefix('|')
        .unwrap_or(bare)
        .strip_suffix('|')
        .unwrap_or_else(|| bare.strip_prefix('|').unwrap_or(bare));
    inner.split('|').collect()
}

#[must_use]
pub fn spans_of(text: &str) -> Vec<Span> {
    let letters: Vec<char> = text.chars().collect();
    let mut ink = Ink::default();
    let mut at = 0;
    while at < letters.len() {
        at = ink.step(&letters, at);
    }
    ink.finish()
}

#[derive(Default)]
struct Ink {
    out: Vec<Span>,
    buffer: String,
    emphasis: Emphasis,
}

impl Ink {
    fn step(&mut self, letters: &[char], at: usize) -> usize {
        match letters[at] {
            '\\' if at + 1 < letters.len() => {
                self.buffer.push(letters[at + 1]);
                at + 2
            }
            '`' => self.code(letters, at),
            '*' | '_' if run(letters, at) >= 2 && marks(letters, at) => {
                self.turn(|voice| voice.bold = !voice.bold);
                at + 2
            }
            '*' | '_' if marks(letters, at) => {
                self.turn(|voice| voice.italic = !voice.italic);
                at + 1
            }
            '[' => self.link(letters, at),
            '!' if letters.get(at + 1) == Some(&'[') => self.link(letters, at + 1),
            letter => {
                self.buffer.push(letter);
                at + 1
            }
        }
    }

    fn code(&mut self, letters: &[char], at: usize) -> usize {
        let Some(end) = letters[at + 1..].iter().position(|it| *it == '`') else {
            self.buffer.push('`');
            return at + 1;
        };
        let inner: String = letters[at + 1..=at + end].iter().collect();
        self.flush();
        let mut voice = self.emphasis;
        voice.code = true;
        self.out.push(Span {
            text: inner,
            emphasis: voice,
            link: None,
        });
        at + end + 2
    }

    fn link(&mut self, letters: &[char], at: usize) -> usize {
        let Some(shut) = letters[at..].iter().position(|it| *it == ']') else {
            self.buffer.push(letters[at]);
            return at + 1;
        };
        if letters.get(at + shut + 1) != Some(&'(') {
            self.buffer.push(letters[at]);
            return at + 1;
        }
        let Some(close) = letters[at + shut..].iter().position(|it| *it == ')') else {
            self.buffer.push(letters[at]);
            return at + 1;
        };
        let words: String = letters[at + 1..at + shut].iter().collect();
        let where_to: String = letters[at + shut + 2..at + shut + close].iter().collect();
        self.flush();
        for mut span in spans_of(&words) {
            span.emphasis.bold |= self.emphasis.bold;
            span.emphasis.italic |= self.emphasis.italic;
            span.link = Some(where_to.trim().to_owned());
            self.out.push(span);
        }
        at + shut + close + 1
    }

    fn turn(&mut self, change: impl FnOnce(&mut Emphasis)) {
        self.flush();
        change(&mut self.emphasis);
    }

    fn flush(&mut self) {
        if self.buffer.is_empty() {
            return;
        }
        self.out.push(Span {
            text: std::mem::take(&mut self.buffer),
            emphasis: self.emphasis,
            link: None,
        });
    }

    fn finish(mut self) -> Vec<Span> {
        self.flush();
        self.out
    }
}

fn run(letters: &[char], at: usize) -> usize {
    letters[at..]
        .iter()
        .take_while(|it| **it == letters[at])
        .count()
}

fn marks(letters: &[char], at: usize) -> bool {
    if letters[at] == '*' {
        return true;
    }
    let before = at.checked_sub(1).and_then(|back| letters.get(back));
    let after = letters.get(at + run(letters, at));
    let inside = before.is_some_and(|it| it.is_alphanumeric())
        && after.is_some_and(|it| it.is_alphanumeric());
    !inside
}

#[cfg(test)]
mod tests {
    use super::{read, spans_of};
    use crate::{Align, Block, ChartKind, Emphasis};

    fn voices(text: &str) -> Vec<(String, bool, bool, bool)> {
        spans_of(text)
            .into_iter()
            .map(|span| {
                (
                    span.text,
                    span.emphasis.bold,
                    span.emphasis.italic,
                    span.emphasis.code,
                )
            })
            .collect()
    }

    #[test]
    fn emphasis_is_flattened_into_runs() {
        assert_eq!(
            voices("a **b *c* d** e"),
            vec![
                ("a ".to_owned(), false, false, false),
                ("b ".to_owned(), true, false, false),
                ("c".to_owned(), true, true, false),
                (" d".to_owned(), true, false, false),
                (" e".to_owned(), false, false, false),
            ]
        );
    }

    #[test]
    fn an_underscore_inside_a_word_is_not_emphasis() {
        assert_eq!(
            voices("file_name_here"),
            vec![("file_name_here".to_owned(), false, false, false)]
        );
        assert_eq!(
            voices("_yes_"),
            vec![("yes".to_owned(), false, true, false)]
        );
        assert_eq!(
            voices("a*b*c"),
            vec![
                ("a".to_owned(), false, false, false),
                ("b".to_owned(), false, true, false),
                ("c".to_owned(), false, false, false),
            ]
        );
    }

    #[test]
    fn escapes_and_code_are_kept_as_written() {
        assert_eq!(
            voices(r"\*not italic\*"),
            vec![("*not italic*".to_owned(), false, false, false)]
        );
        assert_eq!(
            voices("say `a **b**` now"),
            vec![
                ("say ".to_owned(), false, false, false),
                ("a **b**".to_owned(), false, false, true),
                (" now".to_owned(), false, false, false),
            ]
        );
    }

    #[test]
    fn a_link_carries_where_it_points() {
        let spans = spans_of("see [the **notes**](https://x/y) now");
        assert_eq!(spans.len(), 4);
        assert_eq!(spans[1].text, "the ");
        assert_eq!(spans[1].link.as_deref(), Some("https://x/y"));
        assert_eq!(spans[2].text, "notes");
        assert!(spans[2].emphasis.bold);
        assert_eq!(spans[2].link.as_deref(), Some("https://x/y"));
        assert_eq!(spans[3].link, None);
    }

    #[test]
    fn front_matter_names_the_document() {
        let doc = read("---\ntitle: The Report\nwhen: today\n---\n# One\n");
        assert_eq!(doc.title.as_deref(), Some("The Report"));
        assert_eq!(doc.blocks.len(), 1);
        let doc = read("# One\n\n---\n\n# Two\n");
        assert_eq!(doc.title, None);
        assert!(matches!(doc.blocks[1], Block::Rule));
    }

    #[test]
    fn a_paragraph_is_its_lines_joined() {
        let doc = read("one\ntwo\n\nthree\n");
        assert_eq!(doc.blocks.len(), 2);
        match &doc.blocks[0] {
            Block::Paragraph(spans) => assert_eq!(spans[0].text, "one two"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_list_nests_by_indent() {
        let doc = read("- one\n  - under\n    - deeper\n- two\n");
        match &doc.blocks[0] {
            Block::List { ordered, items } => {
                assert!(!ordered);
                assert_eq!(items.len(), 2);
                assert_eq!(items[0].children.len(), 1);
                assert_eq!(items[0].children[0].children.len(), 1);
                assert_eq!(items[0].children[0].children[0].spans[0].text, "deeper");
                assert!(items[1].children.is_empty());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_numbered_list_is_ordered() {
        match &read("1. one\n2. two\n").blocks[0] {
            Block::List { ordered, items } => {
                assert!(ordered);
                assert_eq!(items.len(), 2);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_table_reads_its_columns_and_how_they_stand() {
        let doc = read("| a | b | c |\n|---|:-:|--:|\n| 1 | 2 | 3 |\n");
        match &doc.blocks[0] {
            Block::Table(table) => {
                assert_eq!(table.align, vec![Align::Left, Align::Centre, Align::Right]);
                assert_eq!(table.head.len(), 3);
                assert_eq!(table.rows.len(), 1);
                assert_eq!(table.rows[0][2][0].text, "3");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            read("| a | b | c |\nplain words\n").blocks[0],
            Block::Paragraph(_)
        ));
    }

    #[test]
    fn a_chart_is_read_or_says_why_not() {
        let good = read(
            "```chart\n{\"kind\":\"bar\",\"labels\":[\"a\"],\"series\":[{\"name\":\"s\",\"values\":[1]}]}\n```\n",
        );
        match &good.blocks[0] {
            Block::Chart(chart) => {
                assert_eq!(chart.kind, ChartKind::Bar);
                assert_eq!(chart.series[0].values, vec![1.0]);
            }
            other => panic!("{other:?}"),
        }
        let bad = read("```chart\n{\"kind\":\"spiral\",\"labels\":[],\"series\":[]}\n```\n");
        match &bad.blocks[0] {
            Block::Unreadable { text, why } => {
                assert!(text.contains("spiral"), "{text}");
                assert!(why.contains("not a kind of chart"), "{why}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_unclosed_fence_is_still_code() {
        let doc = read("```rust\nfn main() {}\n# not a heading\n");
        assert_eq!(doc.blocks.len(), 1);
        match &doc.blocks[0] {
            Block::Code { language, lines } => {
                assert_eq!(language.as_deref(), Some("rust"));
                assert_eq!(lines.len(), 2);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn headings_stop_at_four_levels() {
        let doc = read("# a\n\n## b\n\n#### d\n\n###### f\n");
        let levels: Vec<u8> = doc
            .blocks
            .iter()
            .filter_map(|block| match block {
                Block::Heading { level, .. } => Some(*level),
                _ => None,
            })
            .collect();
        assert_eq!(levels, vec![1, 2, 4, 4]);
        assert!(matches!(read("#no\n").blocks[0], Block::Paragraph(_)));
    }

    #[test]
    fn a_picture_stands_alone_or_is_words() {
        match &read("![a cat](cat.png)\n").blocks[0] {
            Block::Picture { path, alt } => {
                assert_eq!(path, "cat.png");
                assert_eq!(alt, "a cat");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            read("see ![a cat](cat.png) here\n").blocks[0],
            Block::Paragraph(_)
        ));
    }

    #[test]
    fn a_quote_holds_blocks() {
        match &read("> **said**\n> - one\n> - two\n").blocks[0] {
            Block::Quote(inner) => {
                assert_eq!(inner.len(), 2);
                assert!(matches!(inner[0], Block::Paragraph(_)));
                assert!(matches!(inner[1], Block::List { .. }));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn nothing_written_is_dropped() {
        let doc = read("# Title\n\nA **word** here.\n\n- one\n- two\n");
        let words = doc.words();
        for wanted in ["Title", "A word here.", "one", "two"] {
            assert!(words.contains(wanted), "{wanted} missing from {words:?}");
        }
        assert_eq!(Emphasis::PLAIN, Emphasis::default());
    }
}
