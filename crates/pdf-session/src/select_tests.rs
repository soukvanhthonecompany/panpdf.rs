use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_paint::Point;
use pdf_semantics::{HitEvidence, ObjectKind};

use crate::{InspectError, SelectionError, Session};

fn overlapping_pictures() -> ByteStore {
    let content = b"q 200 0 0 200 0 0 cm /Back Do Q\n\
                    q 80 0 0 80 60 60 cm /Front Do Q\n\
                    q 20 0 0 20 170 170 cm /Corner Do Q\n";
    let image = |number: u32| {
        format!(
            "{number} 0 obj\n<< /Type /XObject /Subtype /Image /Width 1 /Height 1 \
             /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length 3 >>\nstream\n"
        )
        .into_bytes()
    };
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    let put = |bytes: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &[u8]| {
        offsets.push(bytes.len());
        bytes.extend_from_slice(body);
    };
    put(
        &mut bytes,
        &mut offsets,
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
    );
    put(
        &mut bytes,
        &mut offsets,
        b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 200 200] /Kids [3 0 R] /Count 1 >>\nendobj\n",
    );
    put(
        &mut bytes,
        &mut offsets,
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject \
          << /Back 5 0 R /Front 6 0 R /Corner 7 0 R >> >> >>\nendobj\n",
    );
    let mut stream = format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).into_bytes();
    stream.extend_from_slice(content);
    stream.extend_from_slice(b"\nendstream\nendobj\n");
    put(&mut bytes, &mut offsets, &stream);
    for number in 5..=7 {
        let mut object = image(number);
        object.extend_from_slice(&[10, 20, 30]);
        object.extend_from_slice(b"\nendstream\nendobj\n");
        put(&mut bytes, &mut offsets, &object);
    }
    let table = bytes.len();
    bytes.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
    );
    for offset in &offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{table}\n%%EOF\n",
            offsets.len() + 1
        )
        .as_bytes(),
    );
    ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes))
}

pub(crate) fn session() -> Session {
    Session::new(overlapping_pictures(), b"")
}

#[test]
fn the_fixture_paints_three_pictures() {
    let mut session = session();
    let objects = session.page_objects(0).expect("the page");

    assert_eq!(objects.len(), 3);
    let inspected = session.inspect(&objects).expect("inspection");
    assert!(
        inspected
            .iter()
            .all(|found| found.kind == ObjectKind::Image)
    );
}

#[test]
fn a_point_over_two_pictures_returns_both_with_the_front_one_first() {
    let mut session = session();
    let stack = session
        .hit_candidates(0, Point { x: 100.0, y: 100.0 })
        .expect("the page");

    assert_eq!(stack.all().len(), 2, "the covered picture was dropped");
    let front = stack.current().expect("a choice");
    assert_eq!(front.evidence, HitEvidence::Ink);
    let front_object = front.reference.object;
    assert!(
        stack.all()[1].reference.object < front_object,
        "the stack is not front to back"
    );
}

#[test]
fn select_underneath_cycles_the_stack_it_was_given() {
    let mut session = session();
    let mut stack = session
        .hit_candidates(0, Point { x: 100.0, y: 100.0 })
        .expect("the page");
    let before: Vec<_> = stack.all().to_vec();

    let under = stack.cycle().expect("something underneath").reference;
    assert_eq!(stack.depth(), 1);
    assert_eq!(under, before[1].reference);
    assert_eq!(
        stack.cycle().expect("the front again").reference,
        before[0].reference
    );
    assert_eq!(
        stack.all(),
        before.as_slice(),
        "cycling re-resolved the page"
    );
}

#[test]
fn a_drag_across_another_object_keeps_the_target_it_started_with() {
    let mut session = session();
    let start = Point { x: 20.0, y: 20.0 };
    let across = Point { x: 100.0, y: 100.0 };

    let stack = session.hit_candidates(0, start).expect("the page");
    let target = stack.current().expect("the back picture").reference;
    let mut gesture = session.begin_gesture(&[target]).expect("a gesture");

    gesture.drag_to((across.x - start.x, across.y - start.y));

    assert_eq!(gesture.targets(), [target], "the drag acquired something");
    assert_eq!(gesture.delta(), (80.0, 80.0));
    let now = session.hit_candidates(0, across).expect("the page");
    assert_ne!(
        now.current().expect("a hit").reference,
        target,
        "the fixture does not put another object under the pointer"
    );
}

#[test]
fn a_gesture_holds_the_membership_it_was_given() {
    let mut session = session();
    let objects = session.page_objects(0).expect("the page");
    let gesture = session.begin_gesture(&objects[..1]).expect("a gesture");

    let frozen = gesture.frozen_members(0).expect("frozen membership");
    let live = session.inspect(&objects[..1]).expect("inspection");
    assert_eq!(frozen, live[0].members.as_slice());
    assert!(!frozen.is_empty());
}

#[test]
fn cancelling_a_gesture_asks_for_no_movement() {
    let mut session = session();
    let objects = session.page_objects(0).expect("the page");
    let mut gesture = session.begin_gesture(&objects[..1]).expect("a gesture");

    gesture.drag_to((40.0, 0.0));
    gesture.cancel();

    assert!(gesture.cancelled());
    assert_eq!(gesture.delta(), (0.0, 0.0));
    gesture.drag_to((80.0, 0.0));
    assert_eq!(
        gesture.delta(),
        (0.0, 0.0),
        "a cancelled gesture still moved"
    );
    assert_eq!(
        gesture.targets(),
        &objects[..1],
        "what was abandoned is lost"
    );
}

fn one_text_run() -> ByteStore {
    let content = b"BT /F1 12 Tf 5 5 Td (A) Tj ET";
    let program = crate::tests::hex(crate::tests::TINY_CFF);
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    let put = |bytes: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &[u8]| {
        offsets.push(bytes.len());
        bytes.extend_from_slice(body);
    };
    put(
        &mut bytes,
        &mut offsets,
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
    );
    put(
        &mut bytes,
        &mut offsets,
        b"2 0 obj\n<< /Type /Pages /MediaBox [0 0 100 100] /Kids [3 0 R] /Count 1 >>\nendobj\n",
    );
    put(
        &mut bytes,
        &mut offsets,
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font \
          << /F1 5 0 R >> >> >>\nendobj\n",
    );
    let mut stream = format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).into_bytes();
    stream.extend_from_slice(content);
    stream.extend_from_slice(b"\nendstream\nendobj\n");
    put(&mut bytes, &mut offsets, &stream);
    put(
        &mut bytes,
        &mut offsets,
        b"5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Test /FirstChar 65 /LastChar 65 \
          /Widths [600] /FontDescriptor 6 0 R >>\nendobj\n",
    );
    put(
        &mut bytes,
        &mut offsets,
        b"6 0 obj\n<< /Type /FontDescriptor /FontName /Test /Flags 4 /FontFile3 7 0 R >>\nendobj\n",
    );
    let mut font = format!(
        "7 0 obj\n<< /Subtype /Type1C /Length {} >>\nstream\n",
        program.len()
    )
    .into_bytes();
    font.extend_from_slice(&program);
    font.extend_from_slice(b"\nendstream\nendobj\n");
    put(&mut bytes, &mut offsets, &font);
    let table = bytes.len();
    bytes.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
    );
    for offset in &offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{table}\n%%EOF\n",
            offsets.len() + 1
        )
        .as_bytes(),
    );
    ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes))
}

#[test]
fn a_reference_taken_before_an_edit_is_refused_after_it() {
    use pdf_edit::{Command, Document, TextRunSelection};
    use pdf_syntax::XrefLimits;

    let mut session = Session::new(one_text_run(), b"");
    let before = session.revision();
    let stale = session.page_objects(0).expect("the page")[0];
    assert_eq!(stale.revision, before);

    let plan = Document::open_strict(session.source().clone(), XrefLimits::default())
        .expect("the fixture opens")
        .begin_transaction()
        .plan(
            &Command::MoveTextRun {
                page_index: 0,
                selection: TextRunSelection::Last,
                dx: 4.0,
                dy: 0.0,
            },
            b"",
        )
        .expect("the page has a run to move");
    session.apply(plan).expect("the run moves");

    assert_ne!(
        session.revision(),
        before,
        "an edit left the revision alone"
    );
    match session
        .inspect(&[stale])
        .expect_err("a stale reference was accepted")
    {
        InspectError::Selection(SelectionError::Stale { reference, now }) => {
            assert_eq!(reference, stale);
            assert_eq!(now, session.revision());
        }
        other => panic!("wrong error: {other}"),
    }
    assert!(matches!(
        session.begin_gesture(&[stale]),
        Err(InspectError::Selection(SelectionError::Stale { .. }))
    ));
}

#[test]
fn undo_moves_the_revision_on_rather_than_back() {
    use pdf_edit::{Command, Document, TextRunSelection};
    use pdf_syntax::XrefLimits;

    let mut session = Session::new(one_text_run(), b"");
    let before = session.revision();
    let plan = Document::open_strict(session.source().clone(), XrefLimits::default())
        .expect("the fixture opens")
        .begin_transaction()
        .plan(
            &Command::MoveTextRun {
                page_index: 0,
                selection: TextRunSelection::Last,
                dx: 4.0,
                dy: 0.0,
            },
            b"",
        )
        .expect("a run to move");
    session.apply(plan).expect("the run moves");
    let after_edit = session.revision();
    assert!(session.undo().expect("the inverse commits"));

    assert!(session.revision() > after_edit, "undo reused a revision");
    assert!(session.revision() > before);
}

#[test]
fn a_selection_across_two_pages_is_refused() {
    let mut session = session();
    let objects = session.page_objects(0).expect("the page");
    let mut elsewhere = objects[0];
    elsewhere.page = 1;

    let error = session
        .begin_gesture(&[objects[0], elsewhere])
        .expect_err("a two-page gesture was accepted");
    assert!(matches!(
        error,
        InspectError::Selection(SelectionError::AcrossPages)
    ));
}

#[test]
fn a_gesture_with_nothing_selected_is_refused() {
    let mut session = session();
    assert!(matches!(
        session.begin_gesture(&[]).expect_err("an empty gesture"),
        InspectError::Selection(SelectionError::Empty)
    ));
}

#[test]
fn the_panel_lists_every_object_front_to_back() {
    let mut session = session();
    let rows = session.page_objects(0).expect("the page");

    let order: Vec<usize> = rows.iter().map(|row| row.object).collect();
    assert_eq!(order, vec![2, 1, 0], "the top row is not the front one");
    let corner = session
        .hit_candidates(0, Point { x: 175.0, y: 175.0 })
        .expect("the page");
    assert_eq!(corner.current().expect("a hit").reference, rows[0]);
}

#[test]
fn a_reference_from_another_document_is_refused_at_the_same_revision() {
    let mut one = session();
    let mut two = session();
    assert_eq!(one.revision(), two.revision(), "both start where all do");
    assert_ne!(one.id(), two.id(), "two open documents are two");

    let theirs = one.page_objects(0).expect("the page");
    let mine = two.page_objects(0).expect("the page");
    assert_eq!(
        theirs[0].object, mine[0].object,
        "the fixture puts an object at the same position in both"
    );
    assert_ne!(theirs[0], mine[0], "and the two names are still not equal");

    one.inspect(&theirs[..1]).expect("its own reference");

    let refused = two
        .inspect(&theirs[..1])
        .expect_err("a reference from another document");
    let InspectError::Selection(SelectionError::Foreign { here, .. }) = refused else {
        panic!("refused for the wrong reason: {refused}");
    };
    assert_eq!(here, two.id());

    assert!(matches!(
        two.begin_gesture(&theirs[..1])
            .expect_err("a gesture on another document's object"),
        InspectError::Selection(SelectionError::Foreign { .. })
    ));

    let gesture = one.begin_gesture(&theirs[..1]).expect("its own gesture");
    assert!(gesture.current_in(&one));
    assert!(!gesture.current_in(&two), "a foreign gesture was accepted");
}

#[test]
fn a_cloned_session_does_not_answer_to_the_original_s_references() {
    let mut original = session();
    let taken = original.page_objects(0).expect("the page");
    let mut copy = original.clone();

    assert_ne!(original.id(), copy.id());
    original.inspect(&taken[..1]).expect("its own reference");
    assert!(matches!(
        copy.inspect(&taken[..1])
            .expect_err("a reference from before the copy"),
        InspectError::Selection(SelectionError::Foreign { .. })
    ));
    let its_own = copy.page_objects(0).expect("the page");
    copy.inspect(&its_own[..1]).expect("its own reference");
}
