#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub text: String,
    pub pixels: [u32; 4],
    pub confidence: f32,
    pub space_after: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub pixels: [u32; 4],
    pub words: Vec<Word>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReadingError {
    NotATable,
    BadRow(usize),
}

impl std::fmt::Display for ReadingError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotATable => formatter.write_str("the recogniser's table has no header"),
            Self::BadRow(row) => write!(
                formatter,
                "row {row} of the recogniser's table is not numbers"
            ),
        }
    }
}

impl std::error::Error for ReadingError {}

const HEADER: &str =
    "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext";

pub fn read(tsv: &str, text: &str) -> Result<Vec<Line>, ReadingError> {
    let mut rows = tsv.lines();
    if rows.next().map(str::trim_end) != Some(HEADER) {
        return Err(ReadingError::NotATable);
    }
    let mut lines: Vec<Line> = Vec::new();
    for (index, row) in rows.enumerate() {
        let cells: Vec<&str> = row.split('\t').collect();
        if cells.len() < 11 {
            continue;
        }
        let number = |at: usize| {
            cells[at]
                .trim()
                .parse::<i64>()
                .map_err(|_| ReadingError::BadRow(index + 1))
        };
        let level = number(0)?;
        let [left, top, width, height] = [number(6)?, number(7)?, number(8)?, number(9)?]
            .map(|value| u32::try_from(value.max(0)).unwrap_or(u32::MAX));
        let pixels = [
            left,
            top,
            left.saturating_add(width),
            top.saturating_add(height),
        ];
        match level {
            4 => lines.push(Line {
                pixels,
                words: Vec::new(),
            }),
            5 => {
                let said = cells.get(11).copied().unwrap_or("").trim();
                let confidence = cells[10].trim().parse::<f32>().unwrap_or(-1.0);
                if said.is_empty() || confidence < 0.0 {
                    continue;
                }
                if let Some(line) = lines.last_mut() {
                    line.words.push(Word {
                        text: said.to_owned(),
                        pixels,
                        confidence,
                        space_after: false,
                    });
                }
            }
            _ => {}
        }
    }
    lines.retain(|line| !line.words.is_empty());
    let said: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if said.len() == lines.len() {
        for (line, said) in lines.iter_mut().zip(said) {
            spaced(line, said);
        }
    }
    Ok(lines)
}

fn spaced(line: &mut Line, said: &str) {
    let mut rest = said;
    let mut after = Vec::with_capacity(line.words.len());
    for word in &line.words {
        let Some(tail) = rest.strip_prefix(word.text.as_str()) else {
            return;
        };
        let trimmed = tail.trim_start_matches(' ');
        after.push(trimmed.len() != tail.len());
        rest = trimmed;
    }
    if !rest.is_empty() {
        return;
    }
    for (word, space) in line.words.iter_mut().zip(after) {
        word.space_after = space;
    }
    if let Some(last) = line.words.last_mut() {
        last.space_after = false;
    }
}

#[cfg(test)]
mod tests {
    use super::{HEADER, ReadingError, read};

    fn row(level: u8, line: u8, word: u8, left: u32, width: u32, text: &str) -> String {
        format!(
            "{level}\t1\t1\t1\t{line}\t{word}\t{left}\t100\t{width}\t20\t{}\t{text}",
            if level == 5 { "91.5" } else { "-1" }
        )
    }

    fn table(rows: &[String]) -> String {
        let mut out = String::from(HEADER);
        for one in rows {
            out.push('\n');
            out.push_str(one);
        }
        out
    }

    #[test]
    fn words_take_the_spaces_the_text_gives_them() {
        let tsv = table(&[
            row(4, 1, 0, 10, 200, ""),
            row(5, 1, 1, 10, 50, "Hello"),
            row(5, 1, 2, 70, 60, "world"),
        ]);
        let lines = read(&tsv, "Hello world\n").expect("reads");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].pixels, [10, 100, 210, 120]);
        let words: Vec<(&str, bool)> = lines[0]
            .words
            .iter()
            .map(|word| (word.text.as_str(), word.space_after))
            .collect();
        assert_eq!(words, vec![("Hello", true), ("world", false)]);
        assert!((lines[0].words[0].confidence - 91.5).abs() < f32::EPSILON);
    }

    #[test]
    fn a_script_without_spaces_gets_only_the_spaces_the_text_has() {
        let letters = ["ก", "ข", "ค", "ง"];
        let mut rows = vec![row(4, 1, 0, 0, 100, "")];
        for (index, letter) in letters.iter().enumerate() {
            let at = u32::try_from(index).expect("small") * 20;
            rows.push(row(
                5,
                1,
                u8::try_from(index + 1).expect("small"),
                at,
                15,
                letter,
            ));
        }
        let lines = read(&table(&rows), "กข คง\n").expect("reads");
        let after: Vec<bool> = lines[0].words.iter().map(|word| word.space_after).collect();
        assert_eq!(after, vec![false, true, false, false]);
    }

    #[test]
    fn lines_are_matched_in_order_across_a_paragraph_break() {
        let tsv = table(&[
            row(4, 1, 0, 0, 100, ""),
            row(5, 1, 1, 0, 40, "one"),
            row(5, 1, 2, 50, 40, "two"),
            row(4, 2, 0, 0, 100, ""),
            row(5, 2, 1, 0, 40, "three"),
            row(5, 2, 2, 50, 40, "four"),
        ]);
        let lines = read(&tsv, "one two\n\nthree four\n\n").expect("reads");
        assert!(lines.iter().all(|line| line.words[0].space_after));
    }

    #[test]
    fn a_text_that_disagrees_invents_no_space() {
        let tsv = table(&[
            row(4, 1, 0, 0, 100, ""),
            row(5, 1, 1, 0, 40, "one"),
            row(5, 1, 2, 50, 40, "two"),
        ]);
        for text in ["one tw0\n", "one two three\n", "one\ntwo\n"] {
            let lines = read(&tsv, text).expect("reads");
            assert!(
                lines[0].words.iter().all(|word| !word.space_after),
                "{text:?}"
            );
        }
    }

    #[test]
    fn only_words_are_words() {
        let tsv = table(&[
            "1\t1\t0\t0\t0\t0\t0\t0\t500\t800\t-1\t".to_owned(),
            row(4, 1, 0, 0, 100, ""),
            row(5, 1, 1, 0, 40, " "),
            row(5, 1, 2, 50, 40, "word"),
        ]);
        let lines = read(&tsv, "word\n").expect("reads");
        assert_eq!(lines[0].words.len(), 1);
        assert_eq!(read("no header", ""), Err(ReadingError::NotATable));
        assert_eq!(
            read(
                &table(&["5\t1\t1\t1\t1\t1\tx\t0\t1\t1\t90\ta".to_owned()]),
                ""
            ),
            Err(ReadingError::BadRow(1))
        );
    }
}
