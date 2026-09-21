use std::sync::Arc;

use pdf_app::{Applied, Editor};
use pdf_bytes::{ByteStore, SourceId};

fn read(editor: &mut Editor) {
    let view = pdf_session::interpret_page_grouped(
        editor.source().expect("idle"),
        0,
        b"",
        editor.grouping(0).as_deref(),
    )
    .expect("page reads");
    editor.adopt_page(0, Arc::new(view));
}

fn blocks(editor: &Editor) -> Vec<String> {
    let overlay = &editor.leaf(0).expect("read").overlay;
    overlay
        .blocks
        .iter()
        .map(|block| {
            block
                .lines
                .iter()
                .map(|line| {
                    overlay
                        .clusters
                        .iter()
                        .filter(|cluster| cluster.line == *line)
                        .map(|cluster| cluster.text.clone().unwrap_or_default())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("|")
        })
        .collect()
}

fn control_pens(editor: &Editor) -> Vec<(f64, f64)> {
    let view = &editor.leaf(0).expect("read").view;
    view.index
        .clusters
        .iter()
        .filter(|cluster| {
            let atom = &view.graph.atoms[cluster.atom];
            matches!(&atom.kind, pdf_paint::PaintAtomKind::Text(text)
                if text.glyphs.len() == 7)
        })
        .map(|cluster| (cluster.baseline.x, cluster.baseline.y))
        .collect()
}

fn main() {
    let path = std::env::args().nth(1).expect("a PDF path");
    let bytes = std::fs::read(&path).expect("readable");
    let mut editor =
        Editor::open(ByteStore::new(SourceId::new(7), Arc::<[u8]>::from(bytes))).expect("opens");
    read(&mut editor);
    println!("opened: {:?}", blocks(&editor));
    let before_control = control_pens(&editor);

    let line = editor.leaf(0).expect("read").overlay.blocks[0].lines[0];
    for step in 0..3 {
        let (row, stop) = if step == 0 {
            (0, 6)
        } else {
            editor.landed_caret().expect("caret")
        };
        let range = pdf_edit::BlockRange::Between {
            from: (row, stop),
            to: (row, stop),
        };
        let applied = if step == 0 {
            editor.type_text(0, line, 6, 6, "\n")
        } else {
            editor.edit(0, 0, range, "\n")
        };
        assert!(matches!(applied, Applied::Changed { .. }), "{applied:?}");
        read(&mut editor);
    }
    let (row, stop) = editor.landed_caret().expect("caret");
    let applied = editor.edit(
        0,
        0,
        pdf_edit::BlockRange::Between {
            from: (row, stop),
            to: (row, stop),
        },
        "abc",
    );
    assert!(matches!(applied, Applied::Changed { .. }), "{applied:?}");
    read(&mut editor);
    println!("edited, same session: {:?}", blocks(&editor));

    let saved = editor.export().expect("exports").bytes;
    let mut reopened =
        Editor::open(ByteStore::new(SourceId::new(8), Arc::<[u8]>::from(saved))).expect("reopens");
    read(&mut reopened);
    let reopened_blocks = blocks(&reopened);
    println!("reopened: {reopened_blocks:?}");
    let with_control = reopened_blocks
        .iter()
        .position(|text| text.contains("CONTROL"))
        .expect("CONTROL is on the page");
    println!(
        "CONTROL shares its block with other text after reopening: {}",
        reopened_blocks[with_control] != "CONTROL"
    );

    let edited = reopened_blocks
        .iter()
        .position(|text| text.contains("abc"))
        .expect("the typed text is on the page");
    let anchors = reopened.leaf(0).expect("read").overlay.blocks[edited]
        .anchors
        .clone();
    let control = control_pens(&reopened);
    let moved = reopened.move_block(0, &anchors, 0.0, 100.0);
    println!("moved the block holding abc (block {edited}): {moved:?}");
    read(&mut reopened);
    let after = control_pens(&reopened);
    let carried = control
        .iter()
        .zip(&after)
        .any(|(one, other)| (one.1 - other.1).abs() > 1e-6 || (one.0 - other.0).abs() > 1e-6);
    println!(
        "CONTROL pens before edit {:?}\nCONTROL pens before move {:?}\nCONTROL pens after move  {:?}\nCONTROL carried by the move: {carried}",
        before_control.first(),
        control.first(),
        after.first()
    );
}
