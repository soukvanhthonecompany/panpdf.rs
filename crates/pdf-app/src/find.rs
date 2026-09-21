use pdf_cli::TextClusterBox;

#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub page: usize,
    pub clusters: std::ops::Range<usize>,
    pub from: (usize, usize),
    pub to: (usize, usize),
    pub boxes: Vec<[f64; 4]>,
}

impl Hit {
    #[must_use]
    pub fn bounds(&self) -> Option<[f64; 4]> {
        self.boxes.iter().copied().reduce(|one, other| {
            [
                one[0].min(other[0]),
                one[1].min(other[1]),
                one[2].max(other[2]),
                one[3].max(other[3]),
            ]
        })
    }
}

#[must_use]
pub fn hits_in(page: usize, clusters: &[TextClusterBox], needle: &str) -> Vec<Hit> {
    let wanted: Vec<char> = needle.to_lowercase().chars().collect();
    if wanted.is_empty() || clusters.is_empty() {
        return Vec::new();
    }
    let mut folded: Vec<char> = Vec::new();
    let mut owner: Vec<usize> = Vec::new();
    let mut last_line = clusters.first().map(|cluster| cluster.line);
    for (at, cluster) in clusters.iter().enumerate() {
        if last_line != Some(cluster.line) {
            folded.push(' ');
            owner.push(at);
            last_line = Some(cluster.line);
        }
        let Some(text) = cluster.text.as_deref() else {
            folded.push('\u{fffc}');
            owner.push(at);
            continue;
        };
        for letter in text.to_lowercase().chars() {
            folded.push(letter);
            owner.push(at);
        }
    }
    let mut hits = Vec::new();
    let mut at = 0;
    while at + wanted.len() <= folded.len() {
        if folded[at..at + wanted.len()] != wanted[..] {
            at += 1;
            continue;
        }
        let first = owner[at];
        let last = owner[at + wanted.len() - 1];
        hits.push(Hit {
            page,
            clusters: first..last + 1,
            from: (clusters[first].line, clusters[first].index_in_line),
            to: (clusters[last].line, clusters[last].index_in_line + 1),
            boxes: boxes_of(&clusters[first..=last]),
        });
        at += wanted.len();
    }
    hits
}

fn boxes_of(clusters: &[TextClusterBox]) -> Vec<[f64; 4]> {
    let mut boxes: Vec<(usize, [f64; 4])> = Vec::new();
    for cluster in clusters {
        let Some(box_of) = cluster.box_pixels else {
            continue;
        };
        match boxes.last_mut() {
            Some((line, grown)) if *line == cluster.line => {
                *grown = [
                    grown[0].min(box_of[0]),
                    grown[1].min(box_of[1]),
                    grown[2].max(box_of[2]),
                    grown[3].max(box_of[3]),
                ];
            }
            _ => boxes.push((cluster.line, box_of)),
        }
    }
    boxes.into_iter().map(|(_, box_of)| box_of).collect()
}

#[derive(Clone, Debug, Default)]
pub struct Found {
    pages: std::collections::BTreeMap<usize, Vec<Hit>>,
    answered: std::collections::BTreeSet<usize>,
    at: Option<(usize, usize)>,
}

impl Found {
    pub fn take(&mut self, page: usize, hits: Vec<Hit>) {
        self.answered.insert(page);
        if hits.is_empty() {
            self.pages.remove(&page);
            return;
        }
        self.pages.insert(page, hits);
    }

    #[must_use]
    pub fn has_answered(&self, page: usize) -> bool {
        self.answered.contains(&page)
    }

    #[must_use]
    pub fn pages_answered(&self) -> usize {
        self.answered.len()
    }

    #[must_use]
    pub fn count(&self) -> usize {
        self.pages.values().map(Vec::len).sum()
    }

    #[must_use]
    pub fn pages(&self) -> usize {
        self.pages.len()
    }

    #[must_use]
    pub fn on(&self, page: usize) -> &[Hit] {
        self.pages.get(&page).map_or(&[], Vec::as_slice)
    }

    #[must_use]
    pub fn current(&self) -> Option<&Hit> {
        let (page, at) = self.at?;
        self.pages.get(&page)?.get(at)
    }

    #[must_use]
    pub fn ordinal(&self) -> Option<usize> {
        let (page, at) = self.at?;
        let before: usize = self.pages.range(..page).map(|(_, hits)| hits.len()).sum();
        Some(before + at + 1)
    }

    #[must_use]
    pub fn is_current(&self, page: usize, at: usize) -> bool {
        self.at == Some((page, at))
    }

    pub fn walk(&mut self, forwards: bool, from: usize) -> Option<&Hit> {
        let places: Vec<(usize, usize)> = self
            .pages
            .iter()
            .flat_map(|(page, hits)| (0..hits.len()).map(move |at| (*page, at)))
            .collect();
        if places.is_empty() {
            self.at = None;
            return None;
        }
        let next = match self
            .at
            .and_then(|at| places.iter().position(|place| *place == at))
        {
            Some(at) if forwards => (at + 1) % places.len(),
            Some(at) => (at + places.len() - 1) % places.len(),
            None if forwards => places
                .iter()
                .position(|(page, _)| *page >= from)
                .unwrap_or(0),
            None => places
                .iter()
                .rposition(|(page, _)| *page <= from)
                .unwrap_or(places.len() - 1),
        };
        self.at = places.get(next).copied();
        self.current()
    }

    pub fn let_go(&mut self) {
        self.at = None;
    }

    pub fn forget(&mut self, page: usize) {
        self.pages.remove(&page);
        self.answered.remove(&page);
        if self.at.is_some_and(|(held, _)| held == page) {
            self.at = None;
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    reason = "these boxes are whole pixels, which arrive exactly"
)]
mod tests {
    use pdf_cli::TextClusterBox;

    use super::{Hit, hits_in};

    fn cluster(text: &str, line: usize, index: usize) -> TextClusterBox {
        #[allow(clippy::cast_precision_loss)]
        let x = index as f64 * 10.0;
        #[allow(clippy::cast_precision_loss)]
        let y = line as f64 * 20.0;
        TextClusterBox {
            anchor: format!("{line}:{index}"),
            glyphs: 0..1,
            box_pixels: Some([x, y, x + 10.0, y + 14.0]),
            stacked: false,
            line,
            index_in_line: index,
            text: Some(text.to_owned()),
        }
    }

    fn page(rows: &[&str]) -> Vec<TextClusterBox> {
        let mut clusters = Vec::new();
        for (line, row) in rows.iter().enumerate() {
            for (index, letter) in row.chars().enumerate() {
                clusters.push(cluster(&letter.to_string(), line, index));
            }
        }
        clusters
    }

    fn found(rows: &[&str], needle: &str) -> Vec<Hit> {
        hits_in(3, &page(rows), needle)
    }

    #[test]
    fn a_word_is_found_where_it_is_and_nowhere_else() {
        let hits = found(&["the cat sat"], "cat");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].page, 3);
        assert_eq!(hits[0].clusters, 4..7);
        assert_eq!(hits[0].boxes, [[40.0, 0.0, 70.0, 14.0]]);
    }

    #[test]
    fn a_hit_names_the_rows_and_places_a_replacement_is_written_between() {
        let hits = found(&["the cat sat"], "cat");
        assert_eq!((hits[0].from, hits[0].to), ((0, 4), (0, 7)));
        let across = found(&["the quick", "brown fox"], "quick brown");
        assert_eq!((across[0].from, across[0].to), ((0, 4), (1, 5)));
    }

    #[test]
    fn case_does_not_matter_either_way() {
        assert_eq!(found(&["The CAT sat"], "cat").len(), 1);
        assert_eq!(found(&["the cat sat"], "CAT").len(), 1);
        assert_eq!(found(&["the cat sat"], "CaT").len(), 1);
    }

    #[test]
    fn thai_is_found_by_its_characters() {
        let hits = found(&["ผมกินข้าว"], "ข้าว");
        assert_eq!(hits.len(), 1, "one hit in ผมกินข้าว");
        assert_eq!(hits[0].clusters.len(), 4);
    }

    #[test]
    fn a_phrase_across_a_row_end_is_found_and_boxed_row_by_row() {
        let hits = found(&["the quick", "brown fox"], "quick brown");
        assert_eq!(hits.len(), 1, "the row end reads as a space");
        assert_eq!(hits[0].boxes.len(), 2, "one box a row: {:?}", hits[0].boxes);
        assert_eq!(hits[0].boxes[0], [40.0, 0.0, 90.0, 14.0]);
        assert_eq!(hits[0].boxes[1], [0.0, 20.0, 50.0, 34.0]);
        assert_eq!(
            hits[0].bounds(),
            Some([0.0, 0.0, 90.0, 34.0]),
            "and one box round the lot, for scrolling to it"
        );
    }

    #[test]
    fn every_hit_is_found_and_they_do_not_overlap() {
        assert_eq!(found(&["banana"], "an").len(), 2);
        assert_eq!(found(&["aaa"], "aa").len(), 1, "the second would overlap");
        assert_eq!(found(&["one", "two", "one"], "one").len(), 2);
    }

    #[test]
    fn what_is_not_there_is_not_found() {
        assert!(found(&["the cat"], "dog").is_empty());
        assert!(
            found(&["the cat"], "").is_empty(),
            "everything is no answer"
        );
        assert!(found(&[], "cat").is_empty());
        assert!(found(&["cat", "alog"], "catalog").is_empty());
        assert_eq!(found(&["cat", "alog"], "cat alog").len(), 1);
    }

    #[test]
    fn a_cluster_that_says_nothing_matches_nothing_and_blocks_nothing() {
        let mut clusters = page(&["cat"]);
        clusters[1].text = None;
        assert!(hits_in(0, &clusters, "cat").is_empty());
        assert_eq!(hits_in(0, &clusters, "c").len(), 1);
        assert_eq!(hits_in(0, &clusters, "t").len(), 1);
    }

    fn hit(page: usize) -> Hit {
        Hit {
            page,
            clusters: 0..1,
            from: (0, 0),
            to: (0, 1),
            boxes: vec![[0.0, 0.0, 1.0, 1.0]],
        }
    }

    #[test]
    fn what_is_found_is_counted_and_walked_in_page_order_however_it_arrives() {
        let mut found = super::Found::default();
        found.take(5, vec![hit(5)]);
        found.take(1, vec![hit(1), hit(1)]);
        found.take(3, Vec::new());
        assert_eq!(found.count(), 3);
        assert_eq!(found.pages_answered(), 3);
        assert!(found.has_answered(3) && !found.has_answered(2));
        assert_eq!(found.walk(true, 0).map(|hit| hit.page), Some(1));
        assert_eq!(found.ordinal(), Some(1));
        assert_eq!(found.walk(true, 0).map(|hit| hit.page), Some(1));
        assert_eq!(found.ordinal(), Some(2));
        assert_eq!(found.walk(true, 0).map(|hit| hit.page), Some(5));
        assert_eq!(found.ordinal(), Some(3));
        assert_eq!(found.walk(true, 0).map(|hit| hit.page), Some(1));
        assert_eq!(found.ordinal(), Some(1));
    }

    #[test]
    fn a_search_with_nothing_in_hand_starts_at_the_page_on_screen() {
        let mut found = super::Found::default();
        found.take(1, vec![hit(1)]);
        found.take(4, vec![hit(4)]);
        found.take(9, vec![hit(9)]);
        assert_eq!(found.walk(true, 4).map(|hit| hit.page), Some(4));
        found.let_go();
        assert_eq!(found.walk(true, 5).map(|hit| hit.page), Some(9));
        found.let_go();
        assert_eq!(found.walk(false, 5).map(|hit| hit.page), Some(4));
        found.let_go();
        assert_eq!(found.walk(false, 0).map(|hit| hit.page), Some(9));
    }

    #[test]
    fn a_page_that_has_been_edited_is_forgotten_and_nothing_else_is() {
        let mut found = super::Found::default();
        found.take(1, vec![hit(1)]);
        found.take(2, vec![hit(2)]);
        assert_eq!(found.walk(true, 2).map(|hit| hit.page), Some(2));
        found.forget(2);
        assert_eq!(found.count(), 1, "page 1 still has its hit");
        assert!(!found.has_answered(2), "and page 2 will be searched again");
        assert!(found.current().is_none(), "what was in hand was on page 2");
        assert!(
            found.ordinal().is_none(),
            "and nothing is the third of anything"
        );
        assert!(found.has_answered(1));
    }

    #[test]
    fn a_page_answering_again_replaces_what_it_said_before() {
        let mut found = super::Found::default();
        found.take(2, vec![hit(2), hit(2)]);
        assert_eq!(found.count(), 2);
        found.take(2, vec![hit(2)]);
        assert_eq!(found.count(), 1);
        found.take(2, Vec::new());
        assert_eq!(found.count(), 0);
        assert!(found.walk(true, 0).is_none(), "and nothing to walk to");
    }
}
