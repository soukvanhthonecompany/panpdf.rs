use super::tree::{Align, Block, List};

#[must_use]
pub fn blocks(text: &str) -> Vec<Block> {
    let lines: Vec<String> = text
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .map(|line| line.replace('\t', "    "))
        .collect();
    read(&lines)
}

fn read(lines: &[String]) -> Vec<Block> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < lines.len() {
        let was = at;
        let line = &lines[at];
        if line.trim().is_empty() {
            at += 1;
            continue;
        }
        if let Some(block) = thematic_break(line) {
            out.push(block);
            at += 1;
        } else if let Some(block) = heading(line) {
            out.push(block);
            at += 1;
        } else if let Some((block, used)) = fenced(lines, at) {
            out.push(block);
            at = used;
        } else if let Some((block, used)) = quoted(lines, at) {
            out.push(block);
            at = used;
        } else if let Some((block, used)) = listed(lines, at) {
            out.push(block);
            at = used;
        } else if let Some((block, used)) = indented_code(lines, at) {
            out.push(block);
            at = used;
        } else if let Some((block, used)) = table(lines, at) {
            out.push(block);
            at = used;
        } else {
            let (block, used) = paragraph(lines, at);
            out.push(block);
            at = used;
        }
        at = at.max(was + 1);
    }
    out
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

fn thematic_break(line: &str) -> Option<Block> {
    if indent_of(line) >= 4 {
        return None;
    }
    let bare: String = line.chars().filter(|letter| *letter != ' ').collect();
    let first = bare.chars().next()?;
    if !matches!(first, '*' | '-' | '_') || bare.len() < 3 {
        return None;
    }
    bare.chars()
        .all(|letter| letter == first)
        .then_some(Block::Break)
}

fn heading(line: &str) -> Option<Block> {
    if indent_of(line) >= 4 {
        return None;
    }
    let rest = line.trim_start();
    let hashes = rest.chars().take_while(|letter| *letter == '#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let after = &rest[hashes..];
    if !after.is_empty() && !after.starts_with(' ') {
        return None;
    }
    let text = after.trim().trim_end_matches('#').trim_end();
    Some(Block::Heading {
        level: u8::try_from(hashes).unwrap_or(6),
        inlines: super::inlines::inlines(text),
    })
}

fn fence_at(line: &str) -> Option<(usize, char, usize, String)> {
    let indent = indent_of(line);
    if indent >= 4 {
        return None;
    }
    let rest = line.trim_start();
    let first = rest.chars().next()?;
    if first != '`' && first != '~' {
        return None;
    }
    let count = rest.chars().take_while(|letter| *letter == first).count();
    if count < 3 {
        return None;
    }
    let info = rest[count..].trim().to_owned();
    if first == '`' && info.contains('`') {
        return None;
    }
    Some((indent, first, count, info))
}

fn fenced(lines: &[String], at: usize) -> Option<(Block, usize)> {
    let (indent, letter, count, info) = fence_at(&lines[at])?;
    let mut text = String::new();
    let mut end = at + 1;
    while end < lines.len() {
        let line = &lines[end];
        if let Some((_, closing, closes, after)) = fence_at(line)
            && closing == letter
            && closes >= count
            && after.is_empty()
        {
            end += 1;
            return Some((Block::Code { info, text }, end));
        }
        let stripped = line.strip_prefix(&" ".repeat(indent)).unwrap_or(line);
        text.push_str(stripped);
        text.push('\n');
        end += 1;
    }
    Some((Block::Code { info, text }, end))
}

fn indented_code(lines: &[String], at: usize) -> Option<(Block, usize)> {
    if indent_of(&lines[at]) < 4 || lines[at].trim().is_empty() {
        return None;
    }
    let mut text = String::new();
    let mut end = at;
    let mut blanks = 0;
    while end < lines.len() {
        let line = &lines[end];
        if line.trim().is_empty() {
            blanks += 1;
            end += 1;
            continue;
        }
        if indent_of(line) < 4 {
            break;
        }
        for _ in 0..blanks {
            text.push('\n');
        }
        blanks = 0;
        text.push_str(&line[4..]);
        text.push('\n');
        end += 1;
    }
    Some((
        Block::Code {
            info: String::new(),
            text,
        },
        end - blanks,
    ))
}

fn quoted(lines: &[String], at: usize) -> Option<(Block, usize)> {
    if indent_of(&lines[at]) >= 4 || !lines[at].trim_start().starts_with('>') {
        return None;
    }
    let mut inside: Vec<String> = Vec::new();
    let mut end = at;
    while end < lines.len() {
        let line = &lines[end];
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix('>') {
            inside.push(rest.strip_prefix(' ').unwrap_or(rest).to_owned());
            end += 1;
            continue;
        }
        let carries = !trimmed.is_empty()
            && !inside.last().is_some_and(|last| last.trim().is_empty())
            && thematic_break(line).is_none()
            && heading(line).is_none()
            && fence_at(line).is_none()
            && bullet_at(line).is_none()
            && ordered_at(line).is_none();
        if !carries {
            break;
        }
        inside.push(line.clone());
        end += 1;
    }
    Some((
        Block::Quote {
            blocks: read(&inside),
        },
        end,
    ))
}

fn bullet_at(line: &str) -> Option<(usize, usize, char)> {
    let indent = indent_of(line);
    if indent >= 4 {
        return None;
    }
    let rest = &line[indent..];
    let mark = rest.chars().next()?;
    if !matches!(mark, '-' | '+' | '*') || thematic_break(line).is_some() {
        return None;
    }
    let after = &rest[1..];
    if !after.is_empty() && !after.starts_with(' ') {
        return None;
    }
    let spaces = after.len() - after.trim_start_matches(' ').len();
    let width = if after.trim().is_empty() {
        2
    } else {
        1 + spaces.min(4)
    };
    Some((indent, width, mark))
}

fn ordered_at(line: &str) -> Option<(usize, usize, u64, char)> {
    let indent = indent_of(line);
    if indent >= 4 {
        return None;
    }
    let rest = &line[indent..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || digits.len() > 9 {
        return None;
    }
    let mark = rest[digits.len()..].chars().next()?;
    if mark != '.' && mark != ')' {
        return None;
    }
    let after = &rest[digits.len() + 1..];
    if !after.is_empty() && !after.starts_with(' ') {
        return None;
    }
    let spaces = after.len() - after.trim_start_matches(' ').len();
    let width = if after.trim().is_empty() {
        digits.len() + 2
    } else {
        digits.len() + 1 + spaces.min(4)
    };
    Some((indent, width, digits.parse().unwrap_or(1), mark))
}

fn listed(lines: &[String], at: usize) -> Option<(Block, usize)> {
    let opens = |line: &str| -> Option<(usize, usize, Option<u64>, char)> {
        if let Some((indent, width, mark)) = bullet_at(line) {
            Some((indent, width, None, mark))
        } else {
            ordered_at(line).map(|(indent, width, first, mark)| (indent, width, Some(first), mark))
        }
    };
    let (_, _, first, mark) = opens(&lines[at])?;
    let mut items: Vec<Vec<String>> = Vec::new();
    let mut loose = false;
    let mut end = at;
    let mut blanks = 0;
    while end < lines.len() {
        let line = &lines[end];
        if line.trim().is_empty() {
            blanks += 1;
            end += 1;
            continue;
        }
        if let Some((indent, width, number, letter)) = opens(line) {
            if letter != mark || number.is_some() != first.is_some() {
                break;
            }
            if blanks > 0 && !items.is_empty() {
                loose = true;
            }
            blanks = 0;
            let content = indent + width;
            let mut own = vec![line[content.min(line.len())..].to_owned()];
            end += 1;
            while end < lines.len() {
                let next = &lines[end];
                if next.trim().is_empty() {
                    own.push(String::new());
                    end += 1;
                    continue;
                }
                if indent_of(next) >= content {
                    own.push(next[content..].to_owned());
                    end += 1;
                    continue;
                }
                let carries = opens(next).is_none()
                    && thematic_break(next).is_none()
                    && heading(next).is_none()
                    && fence_at(next).is_none()
                    && !next.trim_start().starts_with('>')
                    && !own.last().is_some_and(|last| last.trim().is_empty());
                if !carries {
                    break;
                }
                own.push(next.clone());
                end += 1;
            }
            while own.len() > 1 && own.last().is_some_and(|last| last.trim().is_empty()) {
                own.pop();
                blanks = 1;
            }
            if own.iter().any(|line| line.trim().is_empty()) {
                loose = true;
            }
            items.push(own);
            continue;
        }
        break;
    }
    if items.is_empty() {
        return None;
    }
    Some((
        Block::List(List {
            first,
            loose,
            items: items.iter().map(|item| read(item)).collect(),
        }),
        end - blanks,
    ))
}

fn paragraph(lines: &[String], at: usize) -> (Block, usize) {
    let mut text = String::new();
    let mut end = at;
    while end < lines.len() {
        let line = &lines[end];
        if line.trim().is_empty() {
            break;
        }
        if end > at {
            if let Some(level) = setext(line) {
                return (
                    Block::Heading {
                        level,
                        inlines: super::inlines::inlines(text.trim()),
                    },
                    end + 1,
                );
            }
            if thematic_break(line).is_some()
                || heading(line).is_some()
                || fence_at(line).is_some()
                || line.trim_start().starts_with('>')
                || bullet_at(line).is_some()
                || ordered_at(line).is_some()
            {
                break;
            }
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(line.trim_start());
        end += 1;
    }
    (
        Block::Paragraph {
            inlines: super::inlines::inlines(text.trim_end()),
        },
        end,
    )
}

fn setext(line: &str) -> Option<u8> {
    if indent_of(line) >= 4 {
        return None;
    }
    let bare = line.trim();
    let first = bare.chars().next()?;
    if first != '=' && first != '-' {
        return None;
    }
    bare.chars()
        .all(|letter| letter == first)
        .then_some(if first == '=' { 1 } else { 2 })
}

fn table(lines: &[String], at: usize) -> Option<(Block, usize)> {
    if at + 1 >= lines.len() || !lines[at].contains('|') {
        return None;
    }
    let head = cells(&lines[at]);
    let align = alignments(&lines[at + 1])?;
    if align.len() != head.len() {
        return None;
    }
    let mut rows = Vec::new();
    let mut end = at + 2;
    while end < lines.len() && lines[end].contains('|') && !lines[end].trim().is_empty() {
        let mut row: Vec<Vec<super::tree::Inline>> = cells(&lines[end])
            .iter()
            .map(|cell| super::inlines::inlines(cell))
            .collect();
        row.resize(head.len(), Vec::new());
        rows.push(row);
        end += 1;
    }
    Some((
        Block::Table {
            head: head
                .iter()
                .map(|cell| super::inlines::inlines(cell))
                .collect(),
            rows,
            align,
        },
        end,
    ))
}

fn cells(line: &str) -> Vec<String> {
    let bare = line.trim();
    let bare = bare.strip_prefix('|').unwrap_or(bare);
    let bare = bare.strip_suffix('|').unwrap_or(bare);
    bare.split('|').map(|cell| cell.trim().to_owned()).collect()
}

fn alignments(line: &str) -> Option<Vec<Align>> {
    if !line.contains('|') && !line.trim().starts_with(':') && !line.trim().starts_with('-') {
        return None;
    }
    let mut out = Vec::new();
    for cell in cells(line) {
        let bare = cell.trim();
        let left = bare.starts_with(':');
        let right = bare.ends_with(':');
        let middle = bare.trim_matches(':');
        if middle.is_empty() || !middle.chars().all(|letter| letter == '-') {
            return None;
        }
        out.push(match (left, right) {
            (true, true) => Align::Middle,
            (false, true) => Align::End,
            _ => Align::Start,
        });
    }
    (!out.is_empty()).then_some(out)
}
