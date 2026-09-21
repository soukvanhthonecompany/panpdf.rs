use pdf_edit::{BlockRange, BlockReading, LineEnd};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    pub from: (usize, usize),
    pub to: (usize, usize),
    pub text: String,
}

impl Pending {
    #[must_use]
    pub const fn at(caret: (usize, usize)) -> Self {
        Self {
            from: caret,
            to: caret,
            text: String::new(),
        }
    }

    #[must_use]
    pub fn over(one: (usize, usize), other: (usize, usize)) -> Self {
        Self {
            from: one.min(other),
            to: one.max(other),
            text: String::new(),
        }
    }

    #[must_use]
    pub const fn range(&self) -> BlockRange {
        BlockRange::Between {
            from: self.from,
            to: self.to,
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.from == self.to && self.text.is_empty()
    }

    pub fn insert(&mut self, text: &str) {
        self.text.push_str(text);
    }

    pub fn back(&mut self, reading: &BlockReading) -> bool {
        if self.text.pop().is_some() {
            return true;
        }
        let Some((before, rest)) = unit_before(reading, self.from) else {
            return false;
        };
        self.from = before;
        self.text = rest;
        true
    }

    pub fn forward(&mut self, reading: &BlockReading) -> bool {
        let Some(after) = unit_after(reading, self.to) else {
            return false;
        };
        self.to = after;
        true
    }
}

fn unit_before(
    reading: &BlockReading,
    (row, stop): (usize, usize),
) -> Option<((usize, usize), String)> {
    let line = reading.lines.get(row)?;
    if stop > 0 {
        let cluster = line.clusters.get(stop - 1)?;
        return Some(((row, stop - 1), all_but_the_last_character(cluster)));
    }
    let above = reading.lines.get(row.checked_sub(1)?)?;
    let end = above.clusters.len();
    match above.end {
        LineEnd::Wrap if end > 0 => Some((
            (row - 1, end - 1),
            all_but_the_last_character(&above.clusters[end - 1]),
        )),
        _ => Some(((row - 1, end), String::new())),
    }
}

fn unit_after(reading: &BlockReading, (row, stop): (usize, usize)) -> Option<(usize, usize)> {
    let line = reading.lines.get(row)?;
    if stop < line.clusters.len() {
        return Some((row, stop + 1));
    }
    let below = reading.lines.get(row + 1)?;
    match line.end {
        LineEnd::Wrap if !below.clusters.is_empty() => Some((row + 1, 1)),
        _ => Some((row + 1, 0)),
    }
}

fn all_but_the_last_character(cluster: &str) -> String {
    let mut rest = cluster.to_owned();
    rest.pop();
    rest
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdf_edit::ReadLine;

    fn line(clusters: &[&str], end: LineEnd) -> ReadLine {
        ReadLine {
            row: Some(0),
            origin: (0.0, 0.0),
            em: 10.0,
            clusters: clusters
                .iter()
                .map(|cluster| (*cluster).to_owned())
                .collect(),
            end,
        }
    }

    fn reading(lines: Vec<ReadLine>) -> BlockReading {
        BlockReading {
            pitch: 12.0,
            alignment: pdf_edit::Alignment::Start,
            turn: 0.0,
            lines,
        }
    }

    #[test]
    fn backspace_takes_what_was_typed_then_what_was_there() {
        let block = reading(vec![line(&["ก", "สี"], LineEnd::Last)]);
        let mut pending = Pending::at((0, 2));
        pending.insert("ab");
        assert!(pending.back(&block));
        assert_eq!(
            pending,
            Pending {
                from: (0, 2),
                to: (0, 2),
                text: "a".into()
            }
        );
        assert!(pending.back(&block));
        assert!(pending.back(&block), "past what was typed");
        assert_eq!(
            pending,
            Pending {
                from: (0, 1),
                to: (0, 2),
                text: "ส".into()
            },
            "`สี` leaves `ส`, with the caret after it"
        );
        assert!(pending.back(&block));
        assert!(pending.back(&block));
        assert_eq!(
            pending,
            Pending {
                from: (0, 0),
                to: (0, 2),
                text: String::new()
            }
        );
        assert!(!pending.back(&block), "nothing before the block's start");
    }

    #[test]
    fn backspace_at_a_row_start_takes_what_the_engine_takes() {
        for (end, from, text) in [
            (LineEnd::Wrap, (0, 1), "a"),
            (LineEnd::WrapWithSpace, (0, 2), ""),
            (LineEnd::Paragraph, (0, 2), ""),
            (LineEnd::Line, (0, 2), ""),
        ] {
            let block = reading(vec![line(&["x", "ab"], end), line(&["y"], LineEnd::Last)]);
            let mut pending = Pending::at((1, 0));
            assert!(pending.back(&block));
            assert_eq!(
                (pending.from, pending.text.as_str()),
                (from, text),
                "{end:?}"
            );
        }
    }

    #[test]
    fn delete_takes_the_next_cluster_and_crosses_a_row_end() {
        let block = reading(vec![
            line(&["a", "b"], LineEnd::Wrap),
            line(&["c"], LineEnd::Paragraph),
            line(&["d"], LineEnd::Last),
        ]);
        let mut pending = Pending::at((0, 1));
        pending.insert("z");
        assert!(pending.forward(&block));
        assert_eq!(pending.to, (0, 2));
        assert!(
            pending.forward(&block),
            "a wrapped row: the first letter below"
        );
        assert_eq!(pending.to, (1, 1));
        assert!(pending.forward(&block), "a paragraph end: the break");
        assert_eq!(pending.to, (2, 0));
        assert!(pending.forward(&block));
        assert!(!pending.forward(&block), "nothing after the block's end");
        assert_eq!(pending.text, "z", "Delete leaves what was typed alone");
    }

    #[test]
    fn a_selection_is_a_range_whichever_way_it_was_drawn() {
        assert_eq!(
            Pending::over((1, 3), (0, 2)).range(),
            BlockRange::Between {
                from: (0, 2),
                to: (1, 3),
            }
        );
        assert!(Pending::at((0, 0)).is_empty());
        assert!(!Pending::over((0, 0), (0, 1)).is_empty());
    }
}
