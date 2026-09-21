use std::collections::BTreeMap;

use pdf_paint::Point;
use pdf_semantics::Quad;

use crate::select_tests::session;
use crate::{GestureError, Intent, SelectionError, Session};

fn quads(session: &mut Session) -> BTreeMap<usize, Option<Quad>> {
    let refs = session.page_objects(0).expect("the page");
    session
        .inspect(&refs)
        .expect("inspection")
        .into_iter()
        .map(|found| (found.reference.object, found.quad))
        .collect()
}

fn quad_of(quads: &BTreeMap<usize, Option<Quad>>, object: usize) -> Option<&Quad> {
    quads
        .get(&object)
        .unwrap_or_else(|| panic!("no object {object} on the page"))
        .as_ref()
}

fn displacement(before: Option<&Quad>, after: Option<&Quad>) -> Option<(f64, f64)> {
    let (before, after) = (before?, after?);
    let (before, after) = (before.corners, after.corners);
    let first = (after[0].x - before[0].x, after[0].y - before[0].y);
    for (b, a) in before.iter().zip(&after) {
        if ((a.x - b.x) - first.0).abs() > 1e-6 || ((a.y - b.y) - first.1).abs() > 1e-6 {
            return None;
        }
    }
    Some(first)
}

#[track_caller]
fn assert_moved(actual: Option<(f64, f64)>, expected: (f64, f64), what: &str) {
    let Some((dx, dy)) = actual else {
        panic!("{what}: the object did not move rigidly, or has no extent");
    };
    assert!(
        (dx - expected.0).abs() < 1e-9 && (dy - expected.1).abs() < 1e-9,
        "{what}: moved by ({dx}, {dy}), asked for ({}, {})",
        expected.0,
        expected.1
    );
}

fn front_gesture(session: &mut Session, dx: f64, dy: f64) -> crate::Gesture {
    let stack = session
        .hit_candidates(0, Point { x: 100.0, y: 100.0 })
        .expect("the page");
    let front = stack
        .current()
        .expect("something under the point")
        .reference;
    let mut gesture = session.begin_gesture(&[front]).expect("a gesture");
    gesture.drag_to((dx, dy));
    gesture
}

#[test]
fn the_measure_of_movement_reads_zero_on_a_page_nobody_edited() {
    let mut session = session();
    let before = quads(&mut session);
    let after = quads(&mut session);

    assert_eq!(before.len(), 3);
    for object in before.keys() {
        assert_moved(
            displacement(quad_of(&before, *object), quad_of(&after, *object)),
            (0.0, 0.0),
            "a page nobody edited",
        );
    }
}

#[test]
fn a_drag_moves_the_object_it_froze_and_only_that_one() {
    let mut session = session();
    let before = quads(&mut session);
    let gesture = front_gesture(&mut session, 12.0, -7.0);
    let front = gesture.targets()[0].object;

    let plan = session
        .plan_gesture(&gesture, Intent::Move)
        .expect("a plan for a picture");
    session.apply(plan).expect("the commit");

    let after = quads(&mut session);
    assert_eq!(after.len(), before.len(), "an object appeared or vanished");
    for object in before.keys() {
        let moved = displacement(quad_of(&before, *object), quad_of(&after, *object));
        if *object == front {
            assert_moved(moved, (12.0, -7.0), "the target");
        } else {
            assert_moved(
                moved,
                (0.0, 0.0),
                &format!("object {object}, not asked to move"),
            );
        }
    }
}

#[test]
fn naming_the_covered_picture_moves_the_covered_picture() {
    let mut session = session();
    let before = quads(&mut session);
    let mut stack = session
        .hit_candidates(0, Point { x: 100.0, y: 100.0 })
        .expect("the page");
    let front = stack.current().expect("a front").reference;
    let under = stack.cycle().expect("something underneath").reference;
    assert_ne!(front.object, under.object, "the control names one object");

    let mut gesture = session.begin_gesture(&[under]).expect("a gesture");
    gesture.drag_to((12.0, -7.0));
    let plan = session
        .plan_gesture(&gesture, Intent::Move)
        .expect("a plan");
    session.apply(plan).expect("the commit");

    let after = quads(&mut session);
    assert_moved(
        displacement(
            quad_of(&before, under.object),
            quad_of(&after, under.object),
        ),
        (12.0, -7.0),
        "the covered picture the control named",
    );
    assert_moved(
        displacement(
            quad_of(&before, front.object),
            quad_of(&after, front.object),
        ),
        (0.0, 0.0),
        "the front picture, which the control did not name",
    );
}

#[test]
fn naming_the_corner_picture_moves_the_corner_picture() {
    let mut session = session();
    let before = quads(&mut session);
    let stack = session
        .hit_candidates(0, Point { x: 175.0, y: 175.0 })
        .expect("the page");
    let corner = stack.current().expect("the corner picture").reference;
    assert_ne!(corner.object, 0, "the control must not name object 0");

    let mut gesture = session.begin_gesture(&[corner]).expect("a gesture");
    gesture.drag_to((-4.0, 11.0));
    let plan = session
        .plan_gesture(&gesture, Intent::Move)
        .expect("a plan");
    session.apply(plan).expect("the commit");

    let after = quads(&mut session);
    for object in before.keys() {
        let moved = displacement(quad_of(&before, *object), quad_of(&after, *object));
        let expected = if *object == corner.object {
            (-4.0, 11.0)
        } else {
            (0.0, 0.0)
        };
        assert_moved(moved, expected, &format!("object {object}"));
    }
}

#[test]
fn a_gesture_frozen_before_a_commit_is_refused_after_it() {
    let mut session = session();
    let stale = front_gesture(&mut session, 5.0, 0.0);
    let plan = session.plan_gesture(&stale, Intent::Move).expect("a plan");
    session.apply(plan).expect("the commit");

    let refused = session.plan_gesture(&stale, Intent::Move);
    assert!(
        matches!(
            refused,
            Err(GestureError::Selection(SelectionError::Stale { .. }))
        ),
        "a stale gesture was not refused: {refused:?}"
    );
}

#[test]
fn a_gesture_from_another_document_is_refused() {
    let mut one = session();
    let mut other = session();
    let gesture = front_gesture(&mut one, 5.0, 5.0);

    let refused = other.plan_gesture(&gesture, Intent::Move);
    assert!(
        matches!(
            refused,
            Err(GestureError::Selection(SelectionError::Foreign { .. }))
        ),
        "a foreign gesture was not refused: {refused:?}"
    );
}

#[test]
fn a_drag_of_no_distance_is_refused_and_changes_no_bytes() {
    let mut session = session();
    let before = session.source().as_bytes().to_vec();
    let revision = session.revision();
    let gesture = front_gesture(&mut session, 0.0, 0.0);

    let refused = session.plan_gesture(&gesture, Intent::Move);
    assert!(
        matches!(refused, Err(GestureError::NoMovement)),
        "a drag of nothing was planned: {refused:?}"
    );
    assert_eq!(session.revision(), revision, "a refusal moved the revision");
    assert_eq!(session.source().as_bytes(), before.as_slice());
}

#[test]
fn a_cancelled_gesture_plans_nothing() {
    let mut session = session();
    let mut gesture = front_gesture(&mut session, 30.0, 30.0);
    gesture.cancel();

    let refused = session.plan_gesture(&gesture, Intent::Move);
    assert!(
        matches!(refused, Err(GestureError::Cancelled)),
        "a cancelled gesture was planned: {refused:?}"
    );
}

#[test]
fn removing_a_picture_removes_that_picture() {
    let mut session = session();
    let before = quads(&mut session);
    let gesture = front_gesture(&mut session, 0.0, 0.0);
    let front = gesture.targets()[0].object;

    let plan = session
        .plan_gesture(&gesture, Intent::Remove)
        .expect("a plan for a removal");
    session.apply(plan).expect("the commit");

    let after = quads(&mut session);
    assert_eq!(
        after.len(),
        before.len() - 1,
        "removing one picture did not leave one fewer object"
    );
    let kept: Vec<_> = before
        .iter()
        .filter(|(object, _)| **object != front)
        .map(|(_, quad)| *quad)
        .collect();
    let now: Vec<_> = after.values().copied().collect();
    assert_eq!(kept.len(), now.len());
    for (b, a) in kept.iter().zip(&now) {
        assert_moved(
            displacement(b.as_ref(), a.as_ref()),
            (0.0, 0.0),
            "a kept picture",
        );
    }
}

#[test]
fn undo_and_redo_walk_a_gesture_back_and_forward() {
    let mut session = session();
    let start = quads(&mut session);
    let gesture = front_gesture(&mut session, 9.0, 3.0);
    let front = gesture.targets()[0].object;
    let plan = session
        .plan_gesture(&gesture, Intent::Move)
        .expect("a plan");
    session.apply(plan).expect("the commit");
    let moved = quads(&mut session);

    assert!(session.undo().expect("undo"));
    let back = quads(&mut session);
    for object in start.keys() {
        assert_moved(
            displacement(quad_of(&start, *object), quad_of(&back, *object)),
            (0.0, 0.0),
            &format!("object {object} after undo"),
        );
    }

    assert!(session.redo().expect("redo"));
    let again = quads(&mut session);
    for object in moved.keys() {
        assert_moved(
            displacement(quad_of(&moved, *object), quad_of(&again, *object)),
            (0.0, 0.0),
            &format!("object {object} after redo"),
        );
    }
    assert_moved(
        displacement(quad_of(&start, front), quad_of(&again, front)),
        (9.0, 3.0),
        "the target after redo, measured from where it started",
    );
}

#[test]
fn a_gesture_on_two_objects_is_refused_and_says_how_many() {
    let mut session = session();
    let refs = session.page_objects(0).expect("the page");
    let mut gesture = session
        .begin_gesture(&refs[..2])
        .expect("a two-object gesture");
    gesture.drag_to((4.0, 4.0));

    let refused = session.plan_gesture(&gesture, Intent::Move);
    assert!(
        matches!(refused, Err(GestureError::TooManyTargets { targets: 2 })),
        "a two-object move was not refused: {refused:?}"
    );
}
