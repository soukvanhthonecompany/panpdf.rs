use std::collections::BTreeSet;
use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_session::PageView;

use crate::Editor;

fn a_document() -> ByteStore {
    ByteStore::new(
        SourceId::new(0),
        &include_bytes!("../../pdf-edit/tests/data/modifiable-r3.pdf")[..],
    )
}

fn holding(pages: usize) -> (Editor, usize) {
    let source = a_document();
    let view =
        Arc::new(pdf_session::interpret_page_with(&source, 0, b"view").expect("the fixture reads"));
    let each = view.footprint();
    assert!(each > 0, "a reading holds bytes");
    let mut editor = Editor::open_with(source, b"view").expect("the fixture opens");
    for page in 0..pages {
        editor.adopt_page(page, Arc::clone(&view));
    }
    assert_eq!(editor.held_pages(), pages);
    (editor, each)
}

fn wanting(page: usize) -> BTreeSet<usize> {
    let mut wanted = BTreeSet::new();
    wanted.insert(page);
    wanted
}

#[test]
fn the_byte_bound_decides_when_it_is_the_one_that_binds() {
    let (mut editor, each) = holding(6);
    editor.keep_pages(&wanting(0), 12, each * 2);
    assert_eq!(editor.held_pages(), 2, "held {} bytes", editor.held_bytes());
    assert!(editor.held_bytes() <= each * 2);
    assert!(
        editor.leaf(0).is_some(),
        "the page being read is still here"
    );
}

#[test]
fn the_count_decides_when_it_is_the_one_that_binds() {
    let (mut editor, each) = holding(6);
    editor.keep_pages(&wanting(0), 3, each * 100);
    assert_eq!(editor.held_pages(), 3);
}

#[test]
fn a_cache_inside_both_bounds_lets_go_of_nothing() {
    let (mut editor, each) = holding(6);
    editor.keep_pages(&wanting(0), 12, each * 100);
    assert_eq!(editor.held_pages(), 6);
}

#[test]
fn the_page_in_front_of_the_person_is_never_let_go_of() {
    let (mut editor, _) = holding(4);
    editor.keep_pages(&wanting(2), 12, 0);
    assert_eq!(editor.held_pages(), 1);
    assert!(editor.leaf(2).is_some());
    assert!(editor.leaf(0).is_none());
}

#[test]
fn everything_wanted_survives_however_many_there_are() {
    let (mut editor, _) = holding(6);
    let wanted: BTreeSet<usize> = (0..5).collect();
    editor.keep_pages(&wanted, 2, 0);
    assert_eq!(editor.held_pages(), 5, "five were asked for");
    for page in 0..5 {
        assert!(editor.leaf(page).is_some());
    }
}

#[test]
fn the_oldest_is_the_first_to_go() {
    let (mut editor, each) = holding(4);
    editor.keep_pages(&wanting(3), 12, each * 100);
    editor.keep_pages(&wanting(1), 12, each * 100);
    editor.keep_pages(&wanting(1), 12, each * 3);
    assert_eq!(editor.held_pages(), 3);
    assert!(editor.leaf(1).is_some(), "wanted now");
    assert!(editor.leaf(3).is_some(), "wanted most recently before that");
}

#[test]
fn an_editor_says_what_it_is_holding() {
    let (editor, each) = holding(3);
    assert_eq!(editor.held_bytes(), each * 3);
    let (empty, _) = holding(0);
    assert_eq!(empty.held_bytes(), 0);
}

#[test]
fn a_reading_of_a_real_page_costs_something() {
    let source = a_document();
    let view: PageView =
        pdf_session::interpret_page_with(&source, 0, b"view").expect("the fixture reads");
    assert!(view.footprint() > std::mem::size_of::<PageView>());
}
