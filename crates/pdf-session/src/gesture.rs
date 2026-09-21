use std::fmt;

use pdf_edit::{Command, FixedPoint, ObjectSelection, SourceAnchor};
use pdf_paint::Matrix;
use pdf_semantics::ObjectKind;

use crate::select::{Gesture, ObjectRef, SelectionError};
use crate::{PageError, PlanError, Session};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Intent {
    Move,
    Remove,
}

impl fmt::Display for Intent {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(match self {
            Self::Move => "move",
            Self::Remove => "remove",
        })
    }
}

#[derive(Debug)]
pub enum GestureError {
    Selection(SelectionError),
    Page(PageError),
    Plan(PlanError),
    Cancelled,
    NoMovement,
    Unsupported {
        reference: ObjectRef,
        kind: ObjectKind,
        reason: &'static str,
    },
    TooManyTargets {
        targets: usize,
    },
}

impl fmt::Display for GestureError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Selection(error) => error.fmt(out),
            Self::Page(error) => error.fmt(out),
            Self::Plan(error) => error.fmt(out),
            Self::Cancelled => out.write_str("the gesture was cancelled"),
            Self::NoMovement => out.write_str("the gesture moved nothing"),
            Self::Unsupported {
                reference, reason, ..
            } => write!(
                out,
                "object {} of page {} cannot be edited this way: {reason}",
                reference.object, reference.page
            ),
            Self::TooManyTargets { targets } => write!(
                out,
                "{targets} objects were selected and this engine acts on one at a time"
            ),
        }
    }
}

impl std::error::Error for GestureError {}

impl Session {
    pub fn plan_gesture(
        &mut self,
        gesture: &Gesture,
        intent: Intent,
    ) -> Result<pdf_edit::Plan, GestureError> {
        if gesture.cancelled() {
            return Err(GestureError::Cancelled);
        }
        if !gesture.current_in(self) {
            let first = gesture.targets().first().copied();
            return Err(GestureError::Selection(match first {
                Some(reference) if reference.session != self.id() => SelectionError::Foreign {
                    reference,
                    here: self.id(),
                },
                Some(reference) => SelectionError::Stale {
                    reference,
                    now: self.revision(),
                },
                None => SelectionError::Empty,
            }));
        }
        let targets = gesture.targets();
        let [target] = targets else {
            return Err(GestureError::TooManyTargets {
                targets: targets.len(),
            });
        };
        let selection = self.selection_of(*target)?;
        let command = match intent {
            Intent::Move => {
                let (dx, dy) = gesture.delta();
                if dx == 0.0 && dy == 0.0 {
                    return Err(GestureError::NoMovement);
                }
                Command::PlaceObject {
                    page_index: target.page,
                    target: selection,
                    transform: Matrix {
                        a: 1.0,
                        b: 0.0,
                        c: 0.0,
                        d: 1.0,
                        e: dx,
                        f: dy,
                    },
                    about: FixedPoint::Origin,
                }
            }
            Intent::Remove => match selection {
                ObjectSelection::Painted(anchor) => Command::RemoveObject {
                    page_index: target.page,
                    target: anchor,
                },
                ObjectSelection::Text(_) => {
                    return Err(GestureError::Unsupported {
                        reference: *target,
                        kind: self.kind_of(*target)?,
                        reason: "removing a text block is a delete of its glyphs, \
                                 which this seam does not carry yet",
                    });
                }
            },
        };
        self.plan(&command).map_err(GestureError::Plan)
    }

    fn selection_of(&mut self, reference: ObjectRef) -> Result<ObjectSelection, GestureError> {
        let kind = self.kind_of(reference)?;
        let anchors = self.anchors_of(reference)?;
        if anchors.is_empty() {
            return Err(GestureError::Unsupported {
                reference,
                kind,
                reason: "it paints nothing this engine can name in the source",
            });
        }
        if matches!(kind, ObjectKind::Text(_)) {
            return Ok(ObjectSelection::Text(anchors));
        }
        let [anchor] = anchors.as_slice() else {
            return Err(GestureError::Unsupported {
                reference,
                kind,
                reason: "it is painted by more than one invocation, \
                         which placement names one at a time",
            });
        };
        Ok(ObjectSelection::Painted(anchor.clone()))
    }

    fn kind_of(&mut self, reference: ObjectRef) -> Result<ObjectKind, GestureError> {
        let view = self.page(reference.page).map_err(GestureError::Page)?;
        view.index
            .objects
            .get(reference.object)
            .map(|object| object.kind)
            .ok_or(GestureError::Selection(SelectionError::Unknown(reference)))
    }

    fn anchors_of(&mut self, reference: ObjectRef) -> Result<Vec<SourceAnchor>, GestureError> {
        let view = self.page(reference.page).map_err(GestureError::Page)?;
        let object = view
            .index
            .objects
            .get(reference.object)
            .ok_or(GestureError::Selection(SelectionError::Unknown(reference)))?;
        Ok(object
            .atoms()
            .filter_map(|atom| view.graph.atoms.get(atom))
            .map(|atom| SourceAnchor::of(&atom.id))
            .collect())
    }
}
