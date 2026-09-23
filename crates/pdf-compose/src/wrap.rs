use crate::measure::{Measure, Pen};
use crate::theme::{Face, Style};
use crate::{Emphasis, Span};

#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub text: String,
    pub pen: Pen,
    pub emphasis: Emphasis,
    pub link: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Line {
    pub runs: Vec<Run>,
    pub width: f32,
    pub split_word: bool,
}

impl Line {
    #[must_use]
    pub fn text(&self) -> String {
        self.runs.iter().map(|run| run.text.as_str()).collect()
    }

    #[must_use]
    pub fn tallest(&self) -> f32 {
        self.runs
            .iter()
            .map(|run| run.pen.size)
            .fold(0.0_f32, f32::max)
    }
}

#[must_use]
pub fn pen_for(style: Style, code: Face, emphasis: Emphasis) -> Pen {
    Pen {
        face: if emphasis.code { code } else { style.face },
        size: style.size,
        bold: style.bold || emphasis.bold,
        italic: style.italic || emphasis.italic,
    }
}

#[must_use]
pub fn wrap(
    spans: &[Span],
    style: Style,
    code: Face,
    width: f32,
    measure: &dyn Measure,
) -> Vec<Line> {
    let width = width.max(1.0);
    let mut lines = Vec::new();
    let mut line = Line::default();
    let mut owed_space: Option<(String, Pen, &Span)> = None;

    for span in spans {
        let pen = pen_for(style, code, span.emphasis);
        for word in pieces(&span.text) {
            if word.chars().all(char::is_whitespace) {
                if !line.runs.is_empty() {
                    owed_space = Some((word.to_owned(), pen, span));
                }
                continue;
            }
            let mut rest = word;
            loop {
                let gap = owed_space
                    .as_ref()
                    .map_or(0.0, |(text, pen, _)| measure.width(text, *pen));
                if measure.width(rest, pen) <= width - line.width - gap {
                    if let Some((text, gap_pen, gap_span)) = owed_space.take() {
                        put(&mut line, &text, gap_pen, gap_span, measure);
                    }
                    put(&mut line, rest, pen, span, measure);
                    break;
                }
                if !line.runs.is_empty() {
                    lines.push(std::mem::take(&mut line));
                    owed_space = None;
                    continue;
                }
                let (taken, _) = fits(rest, pen, width, measure);
                let taken = if taken.is_empty() {
                    &rest[..rest.chars().next().map_or(rest.len(), char::len_utf8)]
                } else {
                    taken
                };
                put(&mut line, taken, pen, span, measure);
                line.split_word = true;
                lines.push(std::mem::take(&mut line));
                rest = &rest[taken.len()..];
                if rest.is_empty() {
                    break;
                }
            }
        }
    }
    if !line.runs.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

fn put(line: &mut Line, text: &str, pen: Pen, span: &Span, measure: &dyn Measure) {
    line.width += measure.width(text, pen);
    if let Some(last) = line.runs.last_mut()
        && last.pen == pen
        && last.link == span.link
    {
        last.text.push_str(text);
        return;
    }
    line.runs.push(Run {
        text: text.to_owned(),
        pen,
        emphasis: span.emphasis,
        link: span.link.clone(),
    });
}

fn fits<'a>(
    word: &'a str,
    pen: Pen,
    room: f32,
    measure: &dyn Measure,
) -> (&'a str, Option<&'a str>) {
    if measure.width(word, pen) <= room {
        return (word, None);
    }
    let mut end = 0;
    for (at, letter) in word.char_indices() {
        let next = at + letter.len_utf8();
        if measure.width(&word[..next], pen) > room {
            break;
        }
        end = next;
    }
    (&word[..end], Some(&word[end..]))
}

fn pieces(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut space = None;
    for (at, letter) in text.char_indices() {
        let blank = letter.is_whitespace();
        if space != Some(blank) {
            if at > start {
                out.push(&text[start..at]);
            }
            start = at;
            space = Some(blank);
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Line, pen_for, wrap};
    use crate::measure::{Even, Pen};
    use crate::theme::{Face, Style};
    use crate::{Emphasis, Span};

    fn said(lines: &[Line]) -> Vec<String> {
        lines.iter().map(Line::text).collect()
    }

    fn body() -> Style {
        Style::new(Face::Body, 10.0, 15.0)
    }

    #[test]
    fn words_break_at_a_space_and_the_space_is_dropped() {
        let ruler = Even::half();
        let spans = [Span::plain("hello world again")];
        let lines = wrap(&spans, body(), Face::Code, 60.0, &ruler);
        assert_eq!(said(&lines), ["hello world", "again"]);
        let wide = wrap(&spans, body(), Face::Code, 200.0, &ruler);
        assert_eq!(said(&wide), ["hello world again"]);
    }

    #[test]
    fn a_line_says_whether_it_broke_a_word() {
        let ruler = Even::half();
        let broken = wrap(&[Span::plain("aaaaaaaa")], body(), Face::Code, 20.0, &ruler);
        assert!(broken[0].split_word, "the word carries on");
        let spaced = wrap(
            &[Span::plain("aaaa aaaa")],
            body(),
            Face::Code,
            20.0,
            &ruler,
        );
        assert!(!spaced[0].split_word);
    }

    #[test]
    fn a_word_wider_than_the_measure_is_split() {
        let ruler = Even::half();
        let spans = [Span::plain("aaaaaaaaaa")];
        let lines = wrap(&spans, body(), Face::Code, 20.0, &ruler);
        assert_eq!(said(&lines), ["aaaa", "aaaa", "aa"]);
        assert_eq!(
            said(&lines).concat().chars().count(),
            10,
            "no letter is lost"
        );
    }

    #[test]
    fn a_measure_narrower_than_one_letter_still_ends() {
        let ruler = Even::half();
        let spans = [Span::plain("abc")];
        let lines = wrap(&spans, body(), Face::Code, 0.5, &ruler);
        assert_eq!(said(&lines), ["a", "b", "c"]);
    }

    #[test]
    fn no_line_is_wider_than_the_measure_unless_one_letter_is() {
        let ruler = Even::half();
        let spans = [Span::plain(
            "the quick brown fox jumps over the lazy dog again and again",
        )];
        let lines = wrap(&spans, body(), Face::Code, 100.0, &ruler);
        assert!(lines.len() > 1);
        for line in &lines {
            assert!(line.width <= 100.01, "{:?} is {}", line.text(), line.width);
        }
    }

    #[test]
    fn nothing_is_lost_or_gained_but_the_spaces_broken_at() {
        let ruler = Even::half();
        let text = "one two three four five six seven eight nine ten";
        let lines = wrap(&[Span::plain(text)], body(), Face::Code, 70.0, &ruler);
        let back = said(&lines).join(" ");
        assert_eq!(back, text);
    }

    #[test]
    fn an_empty_paragraph_is_one_empty_line_and_not_none() {
        let ruler = Even::half();
        assert_eq!(wrap(&[], body(), Face::Code, 100.0, &ruler).len(), 1);
    }

    #[test]
    fn runs_in_one_voice_are_joined_and_two_voices_are_not() {
        let ruler = Even::half();
        let spans = [
            Span::plain("plain "),
            Span::plain("more"),
            Span::plain(" bold").in_voice(Emphasis::PLAIN.bolder()),
        ];
        let lines = wrap(&spans, body(), Face::Code, 400.0, &ruler);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].runs.len(), 2, "{:?}", lines[0].runs);
        assert_eq!(lines[0].runs[0].text, "plain more");
        assert!(lines[0].runs[1].pen.bold);
    }

    #[test]
    fn code_keeps_the_size_of_the_words_around_it_and_changes_face() {
        let style = body();
        let pen = pen_for(
            style,
            Face::Code,
            Emphasis {
                code: true,
                ..Emphasis::PLAIN
            },
        );
        assert_eq!(pen.face, Face::Code);
        assert!((pen.size - style.size).abs() < 0.001);
        assert_eq!(pen_for(style, Face::Code, Emphasis::PLAIN).face, Face::Body);
    }

    #[test]
    fn emphasis_adds_to_the_style_and_does_not_replace_it() {
        let heading = Style::new(Face::Display, 20.0, 24.0).bolder();
        let pen = pen_for(heading, Face::Code, Emphasis::PLAIN.slanted());
        assert_eq!(
            pen,
            Pen {
                face: Face::Display,
                size: 20.0,
                bold: true,
                italic: true,
            }
        );
    }

    #[test]
    fn a_line_is_as_wide_as_its_words_measure() {
        let ruler = Even::half();
        let lines = wrap(&[Span::plain("abcd")], body(), Face::Code, 100.0, &ruler);
        assert!((lines[0].width - 20.0).abs() < 0.001);
    }
}
