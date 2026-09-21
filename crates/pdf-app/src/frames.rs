use std::collections::BTreeMap;

type Rect = [f64; 4];

const TURN_SLACK: f64 = 1e-9;

pub type Edges = (usize, usize);

pub type Breaks = Option<pdf_edit::RowEnds>;

#[derive(Clone, Debug)]
struct Change {
    page: usize,
    before: Vec<Rect>,
    after: Vec<Rect>,
    declared_before: Vec<bool>,
    declared_after: Vec<bool>,
    edges_before: Vec<Edges>,
    edges_after: Vec<Edges>,
    breaks_before: Vec<Breaks>,
    breaks_after: Vec<Breaks>,
    turns_before: Vec<f64>,
    turns_after: Vec<f64>,
    source: bool,
    pages: Option<pdf_edit::PageChange>,
    parked: bool,
    forgets: bool,
    taken: Vec<(usize, Option<Kept>)>,
}

#[derive(Clone, Debug)]
struct Kept {
    rects: Vec<Rect>,
    declared: Vec<bool>,
    edges: Vec<Edges>,
    breaks: Vec<Breaks>,
    turns: Vec<f64>,
}

#[derive(Default, Debug)]
pub struct Frames {
    pages: BTreeMap<usize, Vec<Rect>>,
    declared: BTreeMap<usize, Vec<bool>>,
    edges: BTreeMap<usize, Vec<Edges>>,
    breaks: BTreeMap<usize, Vec<Breaks>>,
    turns: BTreeMap<usize, Vec<f64>>,
    done: Vec<Change>,
    undone: Vec<Change>,
}

impl Frames {
    pub fn establish(&mut self, page: usize, boxes: Vec<Rect>) {
        let turns = vec![0.0; boxes.len()];
        self.establish_turned(page, boxes, turns);
    }

    pub fn establish_turned(&mut self, page: usize, boxes: Vec<Rect>, turns: Vec<f64>) {
        if self.pages.contains_key(&page) {
            return;
        }
        self.turns.insert(page, turns);
        self.declared.insert(page, vec![false; boxes.len()]);
        self.edges.insert(page, vec![(0, 0); boxes.len()]);
        self.breaks.insert(page, vec![None; boxes.len()]);
        self.pages.insert(page, boxes);
    }

    pub fn fit(&mut self, page: usize, seeds: &[Rect], turns: &[f64]) {
        let Some(rects) = self.pages.get_mut(&page) else {
            return;
        };
        let count = seeds.len();
        if rects.len() == count {
            return;
        }
        let held = rects.len();
        rects.truncate(count);
        rects.extend(seeds.iter().skip(held).copied());
        let declared = self.declared.entry(page).or_default();
        declared.resize(count, false);
        let edges = self.edges.entry(page).or_default();
        edges.resize(count, (0, 0));
        let breaks = self.breaks.entry(page).or_default();
        breaks.resize(count, None);
        let kept = self.turns.entry(page).or_default();
        kept.truncate(count);
        kept.extend(turns.iter().skip(kept.len()).copied());
        kept.resize(count, 0.0);
    }

    #[must_use]
    pub fn is_declared(&self, page: usize, block: usize) -> bool {
        self.declared
            .get(&page)
            .and_then(|flags| flags.get(block))
            .copied()
            .unwrap_or(false)
    }

    #[must_use]
    pub fn edges(&self, page: usize, block: usize) -> Edges {
        self.edges
            .get(&page)
            .and_then(|edges| edges.get(block))
            .copied()
            .unwrap_or((0, 0))
    }

    #[must_use]
    pub fn breaks(&self, page: usize, block: usize) -> Breaks {
        self.breaks
            .get(&page)
            .and_then(|all| all.get(block))
            .cloned()
            .unwrap_or_default()
    }

    #[must_use]
    pub fn turn(&self, page: usize, block: usize) -> f64 {
        self.turns
            .get(&page)
            .and_then(|turns| turns.get(block))
            .copied()
            .unwrap_or(0.0)
    }

    fn turns_page(&self, page: usize) -> Vec<f64> {
        let mut turns = self.turns.get(&page).cloned().unwrap_or_default();
        turns.resize(self.page(page).len(), 0.0);
        turns
    }

    pub fn retune(&mut self, page: usize, turns: &[f64], seeds: &[Rect]) {
        let mut held = self.turns_page(page);
        let mut rects = self.page(page).to_vec();
        let mut changed = false;
        for (index, (turn, seed)) in turns.iter().zip(seeds).enumerate() {
            if index >= rects.len() {
                break;
            }
            if (held[index] - turn).abs() > TURN_SLACK {
                rects[index] = *seed;
                held[index] = *turn;
                changed = true;
            }
        }
        if !changed {
            return;
        }
        self.pages.insert(page, rects.clone());
        self.turns.insert(page, held.clone());
        if let Some(last) = self.done.last_mut()
            && last.source
            && last.page == page
        {
            last.after = rects;
            last.turns_after = held;
        }
    }

    fn breaks_page(&self, page: usize) -> Vec<Breaks> {
        self.breaks.get(&page).cloned().unwrap_or_default()
    }

    fn declared_page(&self, page: usize) -> Vec<bool> {
        self.declared.get(&page).cloned().unwrap_or_default()
    }

    fn edges_page(&self, page: usize) -> Vec<Edges> {
        self.edges.get(&page).cloned().unwrap_or_default()
    }

    #[must_use]
    pub fn page(&self, page: usize) -> &[Rect] {
        self.pages.get(&page).map_or(&[], Vec::as_slice)
    }

    pub fn preview(&mut self, page: usize, block: usize, bounds: Rect) {
        if bounds.iter().all(|value| value.is_finite())
            && bounds[0] < bounds[2]
            && bounds[1] < bounds[3]
            && let Some(held) = self
                .pages
                .get_mut(&page)
                .and_then(|boxes| boxes.get_mut(block))
        {
            *held = bounds;
        }
    }

    pub fn declare(&mut self, page: usize, block: usize) {
        if let Some(flag) = self
            .declared
            .get_mut(&page)
            .and_then(|flags| flags.get_mut(block))
        {
            *flag = true;
        }
    }

    pub fn finish_resize(&mut self, page: usize, block: usize, started: Rect) {
        let after = self.page(page).to_vec();
        let mut before = after.clone();
        let Some(bounds) = before.get_mut(block) else {
            return;
        };
        *bounds = started;
        if before != after {
            let declared_before = self.declared_page(page);
            if let Some(flag) = self
                .declared
                .get_mut(&page)
                .and_then(|flags| flags.get_mut(block))
            {
                *flag = true;
            }
            let declared_after = self.declared_page(page);
            let edges = self.edges_page(page);
            let breaks = self.breaks_page(page);
            let turns = self.turns_page(page);
            self.record(Change {
                turns_before: turns.clone(),
                turns_after: turns,
                page,
                before,
                after,
                declared_before,
                declared_after,
                edges_before: edges.clone(),
                edges_after: edges,
                breaks_before: breaks.clone(),
                breaks_after: breaks,
                source: false,
                pages: None,
                parked: false,
                forgets: false,
                taken: Vec::new(),
            });
        }
    }

    pub fn pages_forgotten(&mut self, pages: &[usize]) {
        let taken: Vec<(usize, Option<Kept>)> = pages
            .iter()
            .map(|page| {
                (
                    *page,
                    self.pages.contains_key(page).then(|| self.kept(*page)),
                )
            })
            .collect();
        for page in pages {
            self.forget(*page);
        }
        self.record(Change {
            page: pages.first().copied().unwrap_or(0),
            before: Vec::new(),
            after: Vec::new(),
            declared_before: Vec::new(),
            declared_after: Vec::new(),
            edges_before: Vec::new(),
            edges_after: Vec::new(),
            breaks_before: Vec::new(),
            breaks_after: Vec::new(),
            turns_before: Vec::new(),
            turns_after: Vec::new(),
            source: true,
            pages: None,
            parked: false,
            forgets: true,
            taken,
        });
    }

    fn forget(&mut self, page: usize) {
        self.pages.remove(&page);
        self.declared.remove(&page);
        self.edges.remove(&page);
        self.breaks.remove(&page);
        self.turns.remove(&page);
    }

    pub fn pages_changed(&mut self, change: &pdf_edit::PageChange) {
        let taken = match change {
            pdf_edit::PageChange::Removed(gone) => gone
                .iter()
                .map(|at| (*at, self.pages.contains_key(at).then(|| self.kept(*at))))
                .collect(),
            _ => Vec::new(),
        };
        self.renumber(change, false);
        self.record(Change {
            page: change.first(),
            before: Vec::new(),
            after: Vec::new(),
            declared_before: Vec::new(),
            declared_after: Vec::new(),
            edges_before: Vec::new(),
            edges_after: Vec::new(),
            breaks_before: Vec::new(),
            breaks_after: Vec::new(),
            turns_before: Vec::new(),
            turns_after: Vec::new(),
            source: true,
            pages: Some(change.clone()),
            parked: false,
            forgets: false,
            taken,
        });
    }

    fn kept(&self, page: usize) -> Kept {
        Kept {
            rects: self.page(page).to_vec(),
            declared: self.declared_page(page),
            edges: self.edges_page(page),
            breaks: self.breaks_page(page),
            turns: self.turns.get(&page).cloned().unwrap_or_default(),
        }
    }

    fn keep(&mut self, page: usize, kept: &Kept) {
        self.pages.insert(page, kept.rects.clone());
        self.declared.insert(page, kept.declared.clone());
        self.edges.insert(page, kept.edges.clone());
        self.breaks.insert(page, kept.breaks.clone());
        self.turns.insert(page, kept.turns.clone());
    }
}

fn shift<T>(map: &mut BTreeMap<usize, T>, moved: impl Fn(usize) -> Option<usize>) {
    *map = std::mem::take(map)
        .into_iter()
        .filter_map(|(page, value)| moved(page).map(|page| (page, value)))
        .collect();
}

impl Frames {
    fn renumber(&mut self, change: &pdf_edit::PageChange, walking_back: bool) {
        let moved = |page: usize| change.renumbered(page);
        shift(&mut self.pages, moved);
        shift(&mut self.declared, moved);
        shift(&mut self.edges, moved);
        shift(&mut self.breaks, moved);
        shift(&mut self.turns, moved);
        let mut unparking = walking_back;
        for step in self.done.iter_mut().rev() {
            if step.pages.is_some() {
                if matches!(step.pages, Some(pdf_edit::PageChange::Removed(_))) {
                    unparking = false;
                }
                continue;
            }
            if step.parked {
                if unparking && matches!(change, pdf_edit::PageChange::Added(_)) {
                    step.parked = false;
                }
                continue;
            }
            match moved(step.page) {
                Some(page) => step.page = page,
                None => step.parked = true,
            }
        }
    }

    pub fn source_changed(&mut self, page: usize, movement: Option<(usize, f64, f64)>) {
        let before = self.page(page).to_vec();
        let edges_before = self.edges_page(page);
        let breaks_before = self.breaks_page(page);
        if let Some((block, dx, dy)) = movement
            && let Some(bounds) = before.get(block)
        {
            self.preview(page, block, crate::view::box_shifted(*bounds, dx, dy));
        }
        self.record_source(page, before, edges_before, breaks_before);
    }

    pub fn source_resized(
        &mut self,
        page: usize,
        block: usize,
        bounds: Rect,
        edges: Edges,
        breaks: Breaks,
    ) {
        let before = self.page(page).to_vec();
        let edges_before = self.edges_page(page);
        let breaks_before = self.breaks_page(page);
        self.preview(page, block, bounds);
        if let Some(held) = self.edges.get_mut(&page).and_then(|all| all.get_mut(block)) {
            *held = edges;
        }
        if let Some(held) = self
            .breaks
            .get_mut(&page)
            .and_then(|all| all.get_mut(block))
        {
            *held = breaks;
        }
        self.record_source(page, before, edges_before, breaks_before);
    }

    pub fn source_relaid(&mut self, page: usize, grown: &[(usize, Rect)]) {
        let before = self.page(page).to_vec();
        let edges_before = self.edges_page(page);
        let breaks_before = self.breaks_page(page);
        for (block, bounds) in grown {
            self.preview(page, *block, *bounds);
        }
        self.record_source(page, before, edges_before, breaks_before);
    }

    fn record_source(
        &mut self,
        page: usize,
        before: Vec<Rect>,
        edges_before: Vec<Edges>,
        breaks_before: Vec<Breaks>,
    ) {
        let after = self.page(page).to_vec();
        let declared = self.declared_page(page);
        let edges_after = self.edges_page(page);
        let breaks_after = self.breaks_page(page);
        let turns = self.turns_page(page);
        self.record(Change {
            turns_before: turns.clone(),
            turns_after: turns,
            page,
            before,
            after,
            declared_before: declared.clone(),
            declared_after: declared,
            edges_before,
            edges_after,
            breaks_before,
            breaks_after,
            source: true,
            pages: None,
            parked: false,
            forgets: false,
            taken: Vec::new(),
        });
    }

    fn record(&mut self, change: Change) {
        self.done.push(change);
        self.undone.clear();
    }

    #[must_use]
    pub fn source_step(&self, backwards: bool) -> Option<bool> {
        if backwards {
            self.done.last()
        } else {
            self.undone.last()
        }
        .map(|step| step.source)
    }

    pub fn walk(&mut self, backwards: bool) {
        let entry = if backwards {
            self.done.pop()
        } else {
            self.undone.pop()
        };
        let Some(entry) = entry else { return };
        if entry.forgets {
            for (page, _) in &entry.taken {
                self.forget(*page);
            }
            if backwards {
                for (page, kept) in &entry.taken {
                    if let Some(kept) = kept {
                        self.keep(*page, kept);
                    }
                }
                self.undone.push(entry);
            } else {
                self.done.push(entry);
            }
            return;
        }
        if let Some(change) = entry.pages.clone() {
            if backwards {
                let removed = matches!(change, pdf_edit::PageChange::Removed(_));
                self.renumber(&change.undone(), removed);
                for (at, kept) in &entry.taken {
                    if let Some(kept) = kept {
                        self.keep(*at, kept);
                    }
                }
                self.undone.push(entry);
            } else {
                self.renumber(&change, false);
                self.done.push(entry);
            }
            return;
        }
        let (rects, declared, edges, breaks, turns) = if backwards {
            (
                &entry.before,
                &entry.declared_before,
                &entry.edges_before,
                &entry.breaks_before,
                &entry.turns_before,
            )
        } else {
            (
                &entry.after,
                &entry.declared_after,
                &entry.edges_after,
                &entry.breaks_after,
                &entry.turns_after,
            )
        };
        self.turns.insert(entry.page, turns.clone());
        self.pages.insert(entry.page, rects.clone());
        self.declared.insert(entry.page, declared.clone());
        self.edges.insert(entry.page, edges.clone());
        self.breaks.insert(entry.page, breaks.clone());
        if backwards {
            self.undone.push(entry);
        } else {
            self.done.push(entry);
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::float_cmp,
    reason = "fixed snapshots restore exactly; arithmetic controls use integer coordinates"
)]
mod tests {
    use super::Frames;

    #[test]
    fn a_page_put_in_moves_the_frames_of_every_page_after_it() {
        let mut frames = Frames::default();
        frames.establish(0, vec![[0.0, 0.0, 10.0, 10.0]]);
        frames.establish(1, vec![[0.0, 0.0, 20.0, 20.0]]);
        frames.preview(1, 0, [0.0, 0.0, 30.0, 20.0]);
        frames.finish_resize(1, 0, [0.0, 0.0, 20.0, 20.0]);

        frames.pages_changed(&pdf_edit::PageChange::Added(vec![1]));
        assert_eq!(frames.page(0), [[0.0, 0.0, 10.0, 10.0]]);
        assert!(frames.page(1).is_empty(), "the new page has no frames");
        assert_eq!(frames.page(2), [[0.0, 0.0, 30.0, 20.0]]);
        assert!(frames.is_declared(2, 0));

        frames.walk(true);
        assert_eq!(frames.page(1), [[0.0, 0.0, 30.0, 20.0]]);
        assert!(frames.page(2).is_empty());
        frames.walk(true);
        assert_eq!(
            frames.page(1),
            [[0.0, 0.0, 20.0, 20.0]],
            "the resize undone"
        );

        frames.walk(false);
        frames.walk(false);
        assert_eq!(frames.page(2), [[0.0, 0.0, 30.0, 20.0]]);
        assert!(frames.page(1).is_empty());
    }

    #[test]
    fn several_pages_are_one_step_for_their_frames() {
        use pdf_edit::PageChange;
        let mut frames = Frames::default();
        for page in 0..3 {
            frames.establish(
                page,
                vec![[
                    0.0,
                    0.0,
                    f64::from(10 * u8::try_from(page + 1).expect("small")),
                    10.0,
                ]],
            );
        }
        frames.pages_changed(&PageChange::Removed(vec![0, 2]));
        assert_eq!(frames.page(0), [[0.0, 0.0, 20.0, 10.0]]);
        frames.walk(true);
        assert_eq!(frames.page(0), [[0.0, 0.0, 10.0, 10.0]]);
        assert_eq!(frames.page(2), [[0.0, 0.0, 30.0, 10.0]]);
        frames.pages_forgotten(&[0, 1]);
        assert!(frames.page(0).is_empty() && frames.page(1).is_empty());
        frames.walk(true);
        assert_eq!(
            frames.page(1),
            [[0.0, 0.0, 20.0, 10.0]],
            "one walk gives both back"
        );
        assert_eq!(frames.page(0), [[0.0, 0.0, 10.0, 10.0]]);
    }

    #[test]
    fn a_page_turned_is_measured_again_and_undone_gets_its_frames_back() {
        let mut frames = Frames::default();
        frames.establish(0, vec![[0.0, 0.0, 20.0, 20.0]]);
        frames.preview(0, 0, [0.0, 0.0, 30.0, 20.0]);
        frames.finish_resize(0, 0, [0.0, 0.0, 20.0, 20.0]);
        frames.pages_forgotten(&[0]);
        assert!(frames.page(0).is_empty());
        frames.establish(0, vec![[0.0, 0.0, 20.0, 30.0]]);
        assert_eq!(frames.page(0), [[0.0, 0.0, 20.0, 30.0]], "measured again");
        frames.walk(true);
        assert_eq!(frames.page(0), [[0.0, 0.0, 30.0, 20.0]]);
        assert!(frames.is_declared(0, 0));
        frames.walk(false);
        assert!(
            frames.page(0).is_empty(),
            "redone, and measured again next read"
        );
    }

    #[test]
    fn frames_go_and_come_back_with_their_page() {
        use pdf_edit::PageChange;
        let mut frames = Frames::default();
        frames.establish(0, vec![[0.0, 0.0, 10.0, 10.0]]);
        frames.establish(1, vec![[0.0, 0.0, 20.0, 20.0]]);
        frames.establish(2, vec![[0.0, 0.0, 40.0, 40.0]]);
        frames.preview(1, 0, [0.0, 0.0, 30.0, 20.0]);
        frames.finish_resize(1, 0, [0.0, 0.0, 20.0, 20.0]);

        frames.pages_changed(&PageChange::Reordered(vec![1, 0, 2]));
        assert_eq!(frames.page(0), [[0.0, 0.0, 30.0, 20.0]]);
        assert_eq!(frames.page(1), [[0.0, 0.0, 10.0, 10.0]]);
        frames.pages_changed(&PageChange::Removed(vec![0]));
        assert_eq!(frames.page(0), [[0.0, 0.0, 10.0, 10.0]]);
        assert_eq!(frames.page(1), [[0.0, 0.0, 40.0, 40.0]]);
        assert!(frames.page(2).is_empty());

        frames.walk(true);
        assert_eq!(frames.page(0), [[0.0, 0.0, 30.0, 20.0]], "the page is back");
        assert!(
            frames.is_declared(0, 0),
            "and its frame is still the person's"
        );
        assert_eq!(frames.page(2), [[0.0, 0.0, 40.0, 40.0]]);
        frames.walk(true);
        assert_eq!(frames.page(1), [[0.0, 0.0, 30.0, 20.0]]);
        frames.walk(true);
        assert_eq!(
            frames.page(1),
            [[0.0, 0.0, 20.0, 20.0]],
            "the resize undone"
        );
        assert_eq!(frames.page(0), [[0.0, 0.0, 10.0, 10.0]]);

        for _ in 0..3 {
            frames.walk(false);
        }
        assert_eq!(frames.page(0), [[0.0, 0.0, 10.0, 10.0]]);
        assert_eq!(frames.page(1), [[0.0, 0.0, 40.0, 40.0]]);
    }

    #[test]
    fn editing_and_overlapping_never_reinfer_a_frame() {
        let mut frames = Frames::default();
        frames.establish(
            0,
            vec![[10.0, 20.0, 210.0, 120.0], [220.0, 20.0, 420.0, 120.0]],
        );
        frames.source_changed(0, None);
        frames.establish(0, vec![[10.0, 20.0, 30.0, 40.0]]);
        assert_eq!(frames.page(0)[0], [10.0, 20.0, 210.0, 120.0]);
        frames.source_changed(0, Some((0, 210.0, 0.0)));
        assert_eq!(frames.page(0)[0], [220.0, 20.0, 420.0, 120.0]);
        assert_eq!(frames.page(0).len(), 2);
        frames.walk(true);
        assert_eq!(frames.page(0)[0], [10.0, 20.0, 210.0, 120.0]);
        frames.walk(false);
        assert_eq!(frames.page(0)[0], [220.0, 20.0, 420.0, 120.0]);
    }

    #[test]
    fn a_frame_drawn_for_new_text_is_declared_without_a_drag() {
        let mut frames = Frames::default();
        frames.establish(0, vec![[0.0, 0.0, 150.0, 20.0]]);
        assert!(!frames.is_declared(0, 0));
        frames.finish_resize(0, 0, [0.0, 0.0, 150.0, 20.0]);
        assert!(!frames.is_declared(0, 0), "the old way: no change, no mark");
        frames.declare(0, 0);
        assert!(frames.is_declared(0, 0));
        frames.declare(0, 9);
        assert!(!frames.is_declared(0, 9), "a block the page does not have");
        assert!(frames.source_step(true).is_none(), "not a step of its own");
    }

    #[test]
    fn only_a_drag_declares_a_frame_and_undo_takes_the_declaration_with_it() {
        let mut frames = Frames::default();
        let first = [10.0, 20.0, 210.0, 120.0];
        frames.establish(0, vec![first, [220.0, 20.0, 420.0, 120.0]]);
        assert!(!frames.is_declared(0, 0));
        assert!(!frames.is_declared(0, 1));
        assert!(!frames.is_declared(9, 0));
        assert!(!frames.is_declared(0, 7));

        frames.source_changed(0, Some((0, 5.0, 0.0)));
        assert!(!frames.is_declared(0, 0));

        let moved = frames.page(0)[0];
        let wider = [moved[0], moved[1], moved[2] + 40.0, moved[3]];
        frames.preview(0, 0, wider);
        frames.finish_resize(0, 0, moved);
        assert!(frames.is_declared(0, 0));
        assert!(!frames.is_declared(0, 1));

        frames.walk(true);
        assert_eq!(frames.page(0)[0], moved);
        assert!(!frames.is_declared(0, 0));
        frames.walk(false);
        assert_eq!(frames.page(0)[0], wider);
        assert!(frames.is_declared(0, 0));
    }

    #[test]
    fn a_reflow_changes_the_height_as_one_source_step_and_undo_gives_it_back() {
        let mut frames = Frames::default();
        let first = [10.0, 20.0, 210.0, 120.0];
        frames.establish(0, vec![first, [10.0, 200.0, 210.0, 240.0]]);
        let enter = Some(pdf_edit::RowEnds::paragraphs(vec![0]));
        frames.source_resized(0, 0, [10.0, 20.0, 210.0, 132.0], (0, 1), enter.clone());
        assert_eq!(frames.page(0)[0], [10.0, 20.0, 210.0, 132.0]);
        assert_eq!(frames.edges(0, 0), (0, 1), "the empty line Enter left");
        assert_eq!(
            frames.breaks(0, 0),
            enter,
            "and the paragraph break it made"
        );
        assert_eq!(
            frames.page(0)[1],
            [10.0, 200.0, 210.0, 240.0],
            "the neighbour is not pushed"
        );
        assert_eq!(frames.edges(0, 1), (0, 0));
        assert_eq!(frames.source_step(true), Some(true));
        assert!(
            !frames.is_declared(0, 0),
            "content-driven height declares nothing"
        );
        frames.walk(true);
        assert_eq!(frames.page(0)[0], first);
        assert_eq!(frames.edges(0, 0), (0, 0), "undo takes the empty line back");
        assert_eq!(frames.breaks(0, 0), None, "and the knowledge of it");
        frames.walk(false);
        assert_eq!(frames.page(0)[0], [10.0, 20.0, 210.0, 132.0]);
        assert_eq!(frames.edges(0, 0), (0, 1));
        frames.source_changed(0, Some((1, 0.0, 5.0)));
        assert_eq!(frames.edges(0, 0), (0, 1));
    }

    #[test]
    fn resize_and_source_undo_share_one_order_and_branch() {
        let mut frames = Frames::default();
        let first = [10.0, 20.0, 210.0, 120.0];
        frames.establish(0, vec![first]);
        frames.source_changed(0, Some((0, 15.0, -5.0)));
        let moved = [25.0, 15.0, 225.0, 115.0];
        assert_eq!(frames.page(0)[0], moved);
        frames.preview(0, 0, [25.0, 15.0, 300.0, 115.0]);
        frames.finish_resize(0, 0, moved);
        assert_eq!(frames.source_step(true), Some(false));
        frames.walk(true);
        assert_eq!(frames.page(0)[0], moved);
        assert_eq!(frames.source_step(true), Some(true));
        frames.walk(true);
        assert_eq!(frames.page(0)[0], first);
        frames.source_changed(0, None);
        assert_eq!(frames.source_step(false), None);
    }
}
