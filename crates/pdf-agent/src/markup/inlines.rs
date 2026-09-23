use super::tree::Inline;

#[must_use]
pub fn inlines(text: &str) -> Vec<Inline> {
    let letters: Vec<char> = text.chars().collect();
    let mut pieces = scan(&letters);
    emphasis(&mut pieces);
    gather(pieces)
}

#[derive(Clone, Debug)]
enum Piece {
    Text(String),
    Done(Inline),
    Run {
        mark: char,
        count: usize,
        opens: bool,
        closes: bool,
    },
}

fn marks(letter: char) -> bool {
    letter.is_ascii_punctuation()
        || matches!(
            letter,
            '\u{2018}'..='\u{201f}' | '\u{2013}' | '\u{2014}' | '\u{2026}'
        )
}

#[expect(
    clippy::too_many_lines,
    reason = "one arm per kind of mark, read as a table"
)]
fn scan(letters: &[char]) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    let mut text = String::new();
    let mut at = 0;
    while at < letters.len() {
        let letter = letters[at];
        match letter {
            '\\' if at + 1 < letters.len() => {
                let next = letters[at + 1];
                if next == '\n' {
                    push(&mut out, &mut text);
                    out.push(Piece::Done(Inline::Hard));
                } else if marks(next) {
                    text.push(next);
                } else {
                    text.push('\\');
                    text.push(next);
                }
                at += 2;
                continue;
            }
            '`' => {
                let count = letters[at..]
                    .iter()
                    .take_while(|letter| **letter == '`')
                    .count();
                if let Some(close) = closing_ticks(letters, at + count, count) {
                    push(&mut out, &mut text);
                    let inside: String = letters[at + count..close].iter().collect();
                    out.push(Piece::Done(Inline::Code(tidy_code(&inside))));
                    at = close + count;
                    continue;
                }
            }
            '<' => {
                if let Some(close) = letters[at..].iter().position(|letter| *letter == '>') {
                    let inside: String = letters[at + 1..at + close].iter().collect();
                    if looks_like_a_link(&inside) {
                        push(&mut out, &mut text);
                        out.push(Piece::Done(Inline::Link {
                            to: inside.clone(),
                            text: vec![Inline::Text(inside)],
                        }));
                        at += close + 1;
                        continue;
                    }
                }
            }
            '!' if letters.get(at + 1) == Some(&'[') => {
                if let Some((node, used)) = link(letters, at + 1, true) {
                    push(&mut out, &mut text);
                    out.push(Piece::Done(node));
                    at = used;
                    continue;
                }
            }
            '[' => {
                if let Some((node, used)) = link(letters, at, false) {
                    push(&mut out, &mut text);
                    out.push(Piece::Done(node));
                    at = used;
                    continue;
                }
            }
            '\n' => {
                push(&mut out, &mut text);
                let hard = text.ends_with("  ");
                if let Some(Piece::Text(said)) = out.last_mut() {
                    let trimmed = said.trim_end_matches(' ').to_owned();
                    let spaces = said.len() - trimmed.len();
                    *said = trimmed;
                    if spaces >= 2 {
                        out.push(Piece::Done(Inline::Hard));
                        at += 1;
                        continue;
                    }
                }
                out.push(Piece::Done(if hard { Inline::Hard } else { Inline::Soft }));
                at += 1;
                continue;
            }
            '*' | '_' | '~' => {
                let count = letters[at..]
                    .iter()
                    .take_while(|next| **next == letter)
                    .count();
                if letter != '~' || count == 2 {
                    let before = at.checked_sub(1).map(|back| letters[back]);
                    let after = letters.get(at + count).copied();
                    let (opens, closes) = flanking(letter, before, after);
                    if opens || closes {
                        push(&mut out, &mut text);
                        out.push(Piece::Run {
                            mark: letter,
                            count,
                            opens,
                            closes,
                        });
                        at += count;
                        continue;
                    }
                }
            }
            _ => {}
        }
        text.push(letter);
        at += 1;
    }
    push(&mut out, &mut text);
    out
}

fn flanking(mark: char, before: Option<char>, after: Option<char>) -> (bool, bool) {
    let white = |letter: Option<char>| letter.is_none_or(char::is_whitespace);
    let punct = |letter: Option<char>| letter.is_some_and(marks);
    let left = !white(after) && (!punct(after) || white(before) || punct(before));
    let right = !white(before) && (!punct(before) || white(after) || punct(after));
    if mark == '_' {
        (
            left && (!right || punct(before)),
            right && (!left || punct(after)),
        )
    } else {
        (left, right)
    }
}

fn closing_ticks(letters: &[char], from: usize, count: usize) -> Option<usize> {
    let mut at = from;
    while at < letters.len() {
        if letters[at] == '`' {
            let run = letters[at..]
                .iter()
                .take_while(|letter| **letter == '`')
                .count();
            if run == count {
                return Some(at);
            }
            at += run;
        } else {
            at += 1;
        }
    }
    None
}

fn tidy_code(inside: &str) -> String {
    let flat = inside.replace('\n', " ");
    if flat.starts_with(' ') && flat.ends_with(' ') && flat.trim() != "" {
        flat[1..flat.len() - 1].to_owned()
    } else {
        flat
    }
}

fn looks_like_a_link(inside: &str) -> bool {
    !inside.contains(' ')
        && (inside.contains("://") || inside.starts_with("mailto:") || inside.contains('@'))
}

fn link(letters: &[char], at: usize, picture: bool) -> Option<(Inline, usize)> {
    let close = balanced(letters, at, '[', ']')?;
    if letters.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = balanced(letters, close + 1, '(', ')')?;
    let text: String = letters[at + 1..close].iter().collect();
    let target: String = letters[close + 2..end].iter().collect();
    let to = target
        .split_once(char::is_whitespace)
        .map_or(target.as_str(), |(address, _)| address)
        .trim()
        .to_owned();
    let inside = inlines(&text);
    Some((
        if picture {
            Inline::Image {
                at: to,
                text: inside,
            }
        } else {
            Inline::Link { to, text: inside }
        },
        end + 1,
    ))
}

fn balanced(letters: &[char], from: usize, open: char, shut: char) -> Option<usize> {
    if letters.get(from) != Some(&open) {
        return None;
    }
    let mut depth = 0;
    let mut at = from;
    while at < letters.len() {
        match letters[at] {
            '\\' => at += 1,
            letter if letter == open => depth += 1,
            letter if letter == shut => {
                depth -= 1;
                if depth == 0 {
                    return Some(at);
                }
            }
            _ => {}
        }
        at += 1;
    }
    None
}

fn emphasis(pieces: &mut Vec<Piece>) {
    let mut at = 0;
    while at < pieces.len() {
        let Piece::Run {
            mark,
            count,
            closes,
            opens: closer_opens,
            ..
        } = pieces[at].clone()
        else {
            at += 1;
            continue;
        };
        if !closes {
            at += 1;
            continue;
        }
        let Some(open) = look_back(pieces, at, mark, count, closer_opens) else {
            at += 1;
            continue;
        };
        let Piece::Run { count: opener, .. } = pieces[open].clone() else {
            at += 1;
            continue;
        };
        let used = if mark == '~' || (opener >= 2 && count >= 2) {
            2
        } else {
            1
        };
        let inside: Vec<Piece> = pieces.drain(open + 1..at).collect();
        let node = wrap(mark, used, gather(inside));
        shorten(&mut pieces[open], used);
        let at_now = open + 1;
        shorten(&mut pieces[at_now], used);
        let mut put = at_now;
        if matches!(&pieces[open], Piece::Run { count: 0, .. }) {
            pieces.remove(open);
            put -= 1;
        }
        pieces.insert(put, Piece::Done(node));
        if matches!(pieces.get(put + 1), Some(Piece::Run { count: 0, .. })) {
            pieces.remove(put + 1);
        }
        at = put;
    }
}

fn look_back(
    pieces: &[Piece],
    to: usize,
    mark: char,
    closer: usize,
    closer_opens: bool,
) -> Option<usize> {
    let mut back = to;
    while back > 0 {
        back -= 1;
        let Piece::Run {
            mark: theirs,
            count,
            opens,
            closes,
            ..
        } = &pieces[back]
        else {
            continue;
        };
        if *theirs != mark || !*opens || *count == 0 {
            continue;
        }
        let either_way = closer_opens || *closes;
        if either_way
            && (count + closer).is_multiple_of(3)
            && !(count.is_multiple_of(3) && closer.is_multiple_of(3))
        {
            continue;
        }
        return Some(back);
    }
    None
}

fn shorten(piece: &mut Piece, used: usize) {
    if let Piece::Run { count, .. } = piece {
        *count = count.saturating_sub(used);
    }
}

fn wrap(mark: char, used: usize, inside: Vec<Inline>) -> Inline {
    match (mark, used) {
        ('~', _) => Inline::Strike(inside),
        (_, 2) => Inline::Strong(inside),
        _ => Inline::Emphasis(inside),
    }
}

fn gather(pieces: Vec<Piece>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    for piece in pieces {
        match piece {
            Piece::Done(node) => out.push(node),
            Piece::Text(text) => add(&mut out, &text),
            Piece::Run { mark, count, .. } => {
                if count > 0 {
                    add(&mut out, &mark.to_string().repeat(count));
                }
            }
        }
    }
    out
}

fn add(out: &mut Vec<Inline>, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(Inline::Text(last)) = out.last_mut() {
        last.push_str(text);
    } else {
        out.push(Inline::Text(text.to_owned()));
    }
}

fn push(out: &mut Vec<Piece>, text: &mut String) {
    if !text.is_empty() {
        out.push(Piece::Text(std::mem::take(text)));
    }
}
