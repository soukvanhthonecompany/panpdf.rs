use pdf_bytes::{ByteStore, SourceId};

use crate::incremental::Restrictions;
use crate::plan::Plan;
use crate::spike_move_text::SpikeError;

pub const MOST_HISTORY_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
struct Entry {
    plans: Vec<Plan>,
    inverses: Vec<Plan>,
}

impl Entry {
    fn last(&self) -> &Plan {
        self.plans.last().expect("a step holds at least one plan")
    }

    fn region(&self) -> Option<[f64; 4]> {
        if self.plans.len() == 1 {
            self.last().effect().declared_region
        } else {
            None
        }
    }

    fn bytes(&self) -> usize {
        let plans: usize = self.plans.iter().map(Plan::planned_bytes).sum();
        let inverses: usize = self.inverses.iter().map(Plan::planned_bytes).sum();
        plans + inverses
    }
}

#[derive(Clone, Debug)]
pub struct History {
    original_id: SourceId,
    original_len: usize,
    source: ByteStore,
    credential: Vec<u8>,
    restrictions: Restrictions,
    done: Vec<Entry>,
    undone: Vec<Entry>,
    last_page: Option<usize>,
    last_region: Option<[f64; 4]>,
    budget: usize,
    forgotten: usize,
}

impl History {
    #[must_use]
    pub fn new(source: ByteStore, credential: &[u8]) -> Self {
        Self {
            original_id: source.id(),
            original_len: source.len(),
            source,
            credential: credential.to_vec(),
            restrictions: Restrictions::Respect,
            done: Vec::new(),
            undone: Vec::new(),
            last_page: None,
            last_region: None,
            budget: MOST_HISTORY_BYTES,
            forgotten: 0,
        }
    }

    #[must_use]
    pub fn holding_at_most(mut self, bytes: usize) -> Self {
        self.budget = bytes;
        self.forget_what_does_not_fit();
        self
    }

    #[must_use]
    pub fn held_bytes(&self) -> usize {
        let done: usize = self.done.iter().map(Entry::bytes).sum();
        let undone: usize = self.undone.iter().map(Entry::bytes).sum();
        done + undone
    }

    #[must_use]
    pub const fn forgotten_steps(&self) -> usize {
        self.forgotten
    }

    fn forget_what_does_not_fit(&mut self) {
        let mut held = self.held_bytes();
        while held > self.budget && self.done.len() + self.undone.len() > 1 {
            let gone = if self.undone.is_empty() {
                self.done.remove(0)
            } else {
                self.undone.remove(0)
            };
            held -= gone.bytes();
            self.forgotten += 1;
        }
    }

    pub fn set_aside_restrictions(&mut self) {
        self.restrictions = Restrictions::SetAside;
    }

    #[must_use]
    pub const fn restrictions(&self) -> Restrictions {
        self.restrictions
    }

    #[must_use]
    pub const fn source(&self) -> &ByteStore {
        &self.source
    }

    #[must_use]
    pub fn last_page(&self) -> Option<usize> {
        self.last_page
    }

    #[must_use]
    pub fn last_region(&self) -> Option<[f64; 4]> {
        self.last_region
    }

    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.done.is_empty()
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.undone.is_empty()
    }

    #[must_use]
    pub fn undo_depth(&self) -> usize {
        self.done.len()
    }

    pub fn apply(&mut self, plan: Plan) -> Result<(), SpikeError> {
        self.apply_together(vec![plan])
    }

    pub fn apply_together(&mut self, plans: Vec<Plan>) -> Result<(), SpikeError> {
        let mut plans = plans.into_iter();
        let count = plans.len();
        self.apply_together_with(count, |_, _| {
            plans.next().ok_or(SpikeError::RetypeUnsupported(
                "a step commits at least one plan",
            ))
        })
    }

    pub fn apply_together_with<F>(&mut self, count: usize, mut make: F) -> Result<(), SpikeError>
    where
        F: FnMut(&ByteStore, usize) -> Result<Plan, SpikeError>,
    {
        if count == 0 {
            return Err(SpikeError::RetypeUnsupported(
                "a step commits at least one plan",
            ));
        }
        let mut source = self.source.clone();
        let mut plans = Vec::with_capacity(count);
        let mut inverses = Vec::with_capacity(count);
        for at in 0..count {
            let plan = make(&source, at)?;
            inverses.push(plan.inverse(&source, &self.credential)?);
            source = self.committed(&source, &plan)?;
            plans.push(plan);
        }
        inverses.reverse();
        let entry = Entry { plans, inverses };
        self.source = source;
        self.undone.clear();
        self.last_page = Some(entry.last().effect().page_index);
        self.last_region = entry.region();
        self.done.push(entry);
        self.forget_what_does_not_fit();
        Ok(())
    }

    pub fn undo(&mut self) -> Result<bool, SpikeError> {
        let Some(entry) = self.done.pop() else {
            return Ok(false);
        };
        match self.all_of(&entry.inverses) {
            Ok(committed) => {
                self.source = committed;
                self.last_page = Some(entry.last().effect().page_index);
                self.last_region = entry.region();
                self.undone.push(entry);
                Ok(true)
            }
            Err(error) => {
                self.done.push(entry);
                Err(error)
            }
        }
    }

    pub fn redo(&mut self) -> Result<bool, SpikeError> {
        let Some(entry) = self.undone.pop() else {
            return Ok(false);
        };
        match self.all_of(&entry.plans) {
            Ok(committed) => {
                self.source = committed;
                self.last_page = Some(entry.last().effect().page_index);
                self.last_region = entry.region();
                self.done.push(entry);
                Ok(true)
            }
            Err(error) => {
                self.undone.push(entry);
                Err(error)
            }
        }
    }

    fn all_of(&self, plans: &[Plan]) -> Result<ByteStore, SpikeError> {
        let mut source = self.source.clone();
        for plan in plans {
            source = self.committed(&source, plan)?;
        }
        Ok(source)
    }

    pub fn preview(&self, plan: &Plan) -> Result<ByteStore, SpikeError> {
        self.committed(&self.source, plan)
    }

    fn original_of(&self, source: &ByteStore) -> Result<ByteStore, SpikeError> {
        source
            .prefix(self.original_id, self.original_len)
            .ok_or(SpikeError::RetypeUnsupported(
                "a revision shorter than the file it was opened from",
            ))
    }

    fn committed(&self, source: &ByteStore, plan: &Plan) -> Result<ByteStore, SpikeError> {
        let original = self.original_of(source)?;
        let candidate = plan.commit_bounded(
            source,
            (&self.credential, self.restrictions),
            crate::incremental::session_write_limits(),
        )?;
        if !candidate.as_bytes().starts_with(source.as_bytes()) {
            return Err(SpikeError::RetypeUnsupported(
                "a commit that rewrote the revision it was committed against",
            ));
        }
        Ok(crate::incremental::compact_session(&original, &candidate)?)
    }
}

#[cfg(test)]
mod budget_tests {
    use super::{History, MOST_HISTORY_BYTES};
    use crate::plan::{Command, PenStep, PenStroke};

    fn a_turn() -> Command {
        Command::RotatePages {
            pages: vec![0],
            quarter_turns: 1,
        }
    }

    fn a_line(at: f64) -> Command {
        Command::DrawPath {
            page_index: 0,
            steps: vec![PenStep::Move((10.0, at)), PenStep::Line((60.0, at))],
            closed: false,
            stroke: Some(PenStroke::pen([0.0, 0.0, 0.0], 1.0)),
            fill: None,
        }
    }

    fn took(history: &mut History, steps: usize, what: impl Fn(usize) -> Command) {
        for step in 0..steps {
            let command = what(step);
            history
                .apply_together_with(1, |source, _| {
                    crate::spike_move_text::plan_command_under(
                        source,
                        &command,
                        b"",
                        (None, crate::Restrictions::Respect),
                    )
                })
                .expect("the step is taken");
        }
    }

    fn turned(history: &mut History, steps: usize) {
        took(history, steps, |_| a_turn());
    }

    fn drew(history: &mut History, steps: usize) {
        #[allow(clippy::cast_precision_loss)]
        took(history, steps, |step| a_line(20.0 + step as f64));
    }

    fn fixture() -> pdf_bytes::ByteStore {
        crate::stamp::tests::two_pages()
    }

    fn one_turn() -> usize {
        let mut history = History::new(fixture(), b"");
        turned(&mut history, 3);
        let three = history.held_bytes();
        turned(&mut history, 1);
        let each = history.held_bytes() - three;
        assert!(each > 0, "a step that writes an object holds bytes");
        each
    }

    #[test]
    fn a_history_over_its_budget_forgets_its_oldest_steps() {
        let room = one_turn() * 3;
        let mut history = History::new(fixture(), b"").holding_at_most(room);
        turned(&mut history, 6);
        assert_eq!(history.undo_depth(), 3, "three steps fit and three did not");
        assert_eq!(history.forgotten_steps(), 3);
        assert!(history.held_bytes() <= room);
    }

    #[test]
    fn a_history_inside_its_budget_forgets_nothing() {
        let mut history = History::new(fixture(), b"").holding_at_most(one_turn() * 100);
        turned(&mut history, 6);
        assert_eq!(history.undo_depth(), 6);
        assert_eq!(history.forgotten_steps(), 0);
    }

    #[test]
    fn the_step_just_taken_is_never_forgotten() {
        let mut history = History::new(fixture(), b"").holding_at_most(0);
        turned(&mut history, 4);
        assert_eq!(history.undo_depth(), 1);
        assert!(history.can_undo());
        assert!(history.undo().expect("the last step comes back"));
        assert!(!history.can_undo(), "and there is nothing behind it");
    }

    #[test]
    fn what_is_still_held_still_undoes_and_redoes() {
        let mut history = History::new(fixture(), b"").holding_at_most(one_turn() * 2);
        turned(&mut history, 5);
        assert_eq!(history.undo_depth(), 2);
        assert!(history.undo().expect("one back"));
        assert!(history.undo().expect("two back"));
        assert!(!history.undo().expect("and no further"));
        assert!(history.redo().expect("forward again"));
        assert!(history.redo().expect("and again"));
        assert!(!history.redo().expect("and no further"));
    }

    #[test]
    fn a_step_that_costs_more_forgets_sooner() {
        let room = one_turn() * 8;
        let mut turns = History::new(fixture(), b"").holding_at_most(room);
        turned(&mut turns, 12);
        let mut lines = History::new(fixture(), b"").holding_at_most(room);
        drew(&mut lines, 12);
        assert!(
            lines.undo_depth() < turns.undo_depth(),
            "the same room held {} drawn lines and {} turns",
            lines.undo_depth(),
            turns.undo_depth()
        );
        assert!(lines.undo_depth() >= 1, "and never nothing");
    }

    #[test]
    fn an_ordinary_page_never_reaches_the_shipped_budget() {
        let mut history = History::new(fixture(), b"");
        drew(&mut history, 20);
        let dearest = history.held_bytes() / 20;
        let steps = MOST_HISTORY_BYTES / dearest.max(1);
        assert!(
            steps > 1_000,
            "the shipped budget holds only {steps} steps of an ordinary page"
        );
    }

    #[test]
    fn a_history_says_what_it_is_holding() {
        let mut history = History::new(fixture(), b"");
        assert_eq!(history.held_bytes(), 0);
        turned(&mut history, 2);
        let two = history.held_bytes();
        turned(&mut history, 2);
        assert!(
            history.held_bytes() > two,
            "four steps hold more than two: {two} then {}",
            history.held_bytes()
        );
    }
}
