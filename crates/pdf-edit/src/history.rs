use pdf_bytes::ByteStore;

use crate::incremental::Restrictions;
use crate::plan::Plan;
use crate::spike_move_text::SpikeError;

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
}

#[derive(Clone, Debug)]
pub struct History {
    original: ByteStore,
    source: ByteStore,
    credential: Vec<u8>,
    restrictions: Restrictions,
    done: Vec<Entry>,
    undone: Vec<Entry>,
    last_page: Option<usize>,
    last_region: Option<[f64; 4]>,
}

impl History {
    #[must_use]
    pub fn new(source: ByteStore, credential: &[u8]) -> Self {
        Self {
            original: source.clone(),
            source,
            credential: credential.to_vec(),
            restrictions: Restrictions::Respect,
            done: Vec::new(),
            undone: Vec::new(),
            last_page: None,
            last_region: None,
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

    fn committed(&self, source: &ByteStore, plan: &Plan) -> Result<ByteStore, SpikeError> {
        let candidate = plan.commit_bounded(
            source,
            (&self.credential, self.restrictions),
            crate::incremental::session_write_limits(),
        )?;
        Ok(crate::incremental::compact_session(
            &self.original,
            &candidate,
        )?)
    }
}
