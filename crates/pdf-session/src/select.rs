use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use pdf_paint::Point;
use pdf_semantics::{
    Candidate, Dependency, DependencyIndex, HitDoubt, HitEvidence, Member, ObjectKind, Quad,
};

use crate::{PageError, Session};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SessionId(u64);

impl SessionId {
    pub(crate) fn next() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(out, "s{}", self.0)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RevisionId(u64);

impl RevisionId {
    pub(crate) const fn first() -> Self {
        Self(0)
    }

    pub(crate) const fn next(self) -> Self {
        Self(self.0 + 1)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for RevisionId {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(out, "r{}", self.0)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ObjectRef {
    pub session: SessionId,
    pub revision: RevisionId,
    pub page: usize,
    pub object: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SelectionError {
    Stale {
        reference: ObjectRef,
        now: RevisionId,
    },
    Foreign {
        reference: ObjectRef,
        here: SessionId,
    },
    Unknown(ObjectRef),
    AcrossPages,
    Empty,
}

impl fmt::Display for SelectionError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stale { reference, now } => write!(
                out,
                "the selection names revision {} and the document is at {now}",
                reference.revision
            ),
            Self::Foreign { reference, here } => write!(
                out,
                "the selection was taken from document {} and this is {here}",
                reference.session
            ),
            Self::Unknown(reference) => write!(
                out,
                "page {} of revision {} has no object {}",
                reference.page + 1,
                reference.revision,
                reference.object
            ),
            Self::AcrossPages => write!(out, "one gesture cannot span two pages"),
            Self::Empty => write!(out, "nothing is selected"),
        }
    }
}

impl std::error::Error for SelectionError {}

#[derive(Clone, Debug, PartialEq)]
pub struct HitCandidate {
    pub reference: ObjectRef,
    pub kind: ObjectKind,
    pub evidence: HitEvidence,
    pub doubts: Vec<HitDoubt>,
    pub quad: Option<Quad>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CandidateStack {
    session: SessionId,
    revision: RevisionId,
    page: usize,
    point: Point,
    candidates: Vec<HitCandidate>,
    at: usize,
}

impl CandidateStack {
    #[must_use]
    pub const fn point(&self) -> Point {
        self.point
    }

    #[must_use]
    pub const fn session(&self) -> SessionId {
        self.session
    }

    #[must_use]
    pub const fn revision(&self) -> RevisionId {
        self.revision
    }

    #[must_use]
    pub const fn page(&self) -> usize {
        self.page
    }

    #[must_use]
    pub fn all(&self) -> &[HitCandidate] {
        &self.candidates
    }

    #[must_use]
    pub fn current(&self) -> Option<&HitCandidate> {
        self.candidates.get(self.at)
    }

    #[must_use]
    pub const fn depth(&self) -> usize {
        self.at
    }

    pub fn cycle(&mut self) -> Option<&HitCandidate> {
        if self.candidates.is_empty() {
            return None;
        }
        self.at = (self.at + 1) % self.candidates.len();
        self.current()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Inspection {
    pub reference: ObjectRef,
    pub kind: ObjectKind,
    pub members: Vec<Member>,
    pub quad: Option<Quad>,
    pub bounds: Option<[f64; 4]>,
    pub shared: Vec<(Dependency, Vec<usize>)>,
    pub dependencies_complete: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Gesture {
    session: SessionId,
    revision: RevisionId,
    page: usize,
    targets: Vec<ObjectRef>,
    members: Vec<Vec<Member>>,
    delta: (f64, f64),
    cancelled: bool,
}

impl Gesture {
    #[must_use]
    pub const fn session(&self) -> SessionId {
        self.session
    }

    #[must_use]
    pub const fn revision(&self) -> RevisionId {
        self.revision
    }

    #[must_use]
    pub const fn page(&self) -> usize {
        self.page
    }

    #[must_use]
    pub fn targets(&self) -> &[ObjectRef] {
        &self.targets
    }

    #[must_use]
    pub fn frozen_members(&self, index: usize) -> Option<&[Member]> {
        self.members.get(index).map(Vec::as_slice)
    }

    pub const fn drag_to(&mut self, delta: (f64, f64)) {
        if !self.cancelled {
            self.delta = delta;
        }
    }

    #[must_use]
    pub const fn delta(&self) -> (f64, f64) {
        self.delta
    }

    pub const fn cancel(&mut self) {
        self.cancelled = true;
        self.delta = (0.0, 0.0);
    }

    #[must_use]
    pub const fn cancelled(&self) -> bool {
        self.cancelled
    }

    #[must_use]
    pub fn current_in(&self, session: &Session) -> bool {
        !self.cancelled && self.session == session.id() && self.revision == session.revision()
    }
}

impl Session {
    #[must_use]
    pub const fn revision(&self) -> RevisionId {
        self.revision
    }

    pub fn hit_candidates(
        &mut self,
        page: usize,
        point: Point,
    ) -> Result<CandidateStack, PageError> {
        let (session, revision) = (self.id, self.revision);
        let view = self.page(page)?;
        let candidates = pdf_semantics::candidates(&view.graph, &view.index, point)
            .into_iter()
            .map(|found: Candidate| HitCandidate {
                reference: ObjectRef {
                    session,
                    revision,
                    page,
                    object: found.object,
                },
                kind: found.kind,
                evidence: found.evidence,
                doubts: found.doubts,
                quad: view
                    .index
                    .objects
                    .get(found.object)
                    .and_then(|object| object.quad),
            })
            .collect();
        Ok(CandidateStack {
            session,
            revision,
            page,
            point,
            candidates,
            at: 0,
        })
    }

    pub fn page_objects(&mut self, page: usize) -> Result<Vec<ObjectRef>, PageError> {
        let (session, revision) = (self.id, self.revision);
        let view = self.page(page)?;
        Ok((0..view.index.objects.len())
            .rev()
            .map(|object| ObjectRef {
                session,
                revision,
                page,
                object,
            })
            .collect())
    }

    pub fn inspect(&mut self, refs: &[ObjectRef]) -> Result<Vec<Inspection>, InspectError> {
        let mut found = Vec::with_capacity(refs.len());
        for reference in refs {
            self.check(*reference)?;
            let view = self.page(reference.page).map_err(InspectError::Page)?;
            let object = view
                .index
                .objects
                .get(reference.object)
                .ok_or(InspectError::Selection(SelectionError::Unknown(*reference)))?;
            let dependencies = DependencyIndex::of(&view.graph, &view.index.objects);
            found.push(Inspection {
                reference: *reference,
                kind: object.kind,
                members: object.members.clone(),
                quad: object.quad,
                bounds: object.bounds,
                shared: dependencies.shared_by(reference.object),
                dependencies_complete: dependencies
                    .of_object(reference.object)
                    .is_some_and(|deps| !deps.on(Dependency::Unfollowed)),
            });
        }
        Ok(found)
    }

    pub fn begin_gesture(&mut self, targets: &[ObjectRef]) -> Result<Gesture, InspectError> {
        let Some(first) = targets.first() else {
            return Err(InspectError::Selection(SelectionError::Empty));
        };
        if targets.iter().any(|target| target.page != first.page) {
            return Err(InspectError::Selection(SelectionError::AcrossPages));
        }
        let mut members = Vec::with_capacity(targets.len());
        for target in targets {
            self.check(*target)?;
            let view = self.page(target.page).map_err(InspectError::Page)?;
            let object = view
                .index
                .objects
                .get(target.object)
                .ok_or(InspectError::Selection(SelectionError::Unknown(*target)))?;
            members.push(object.members.clone());
        }
        Ok(Gesture {
            session: self.id,
            revision: self.revision,
            page: first.page,
            targets: targets.to_vec(),
            members,
            delta: (0.0, 0.0),
            cancelled: false,
        })
    }

    #[must_use]
    pub const fn id(&self) -> SessionId {
        self.id
    }

    fn check(&self, reference: ObjectRef) -> Result<(), InspectError> {
        if reference.session != self.id {
            return Err(InspectError::Selection(SelectionError::Foreign {
                reference,
                here: self.id,
            }));
        }
        if reference.revision == self.revision {
            return Ok(());
        }
        Err(InspectError::Selection(SelectionError::Stale {
            reference,
            now: self.revision,
        }))
    }
}

#[derive(Debug)]
pub enum InspectError {
    Selection(SelectionError),
    Page(PageError),
}

impl fmt::Display for InspectError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Selection(error) => error.fmt(out),
            Self::Page(error) => error.fmt(out),
        }
    }
}

impl std::error::Error for InspectError {}
