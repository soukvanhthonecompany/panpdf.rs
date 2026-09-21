use std::sync::Arc;
use std::time::Instant;

use pdf_app::{Applied, Editor};
use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::BlockRange;

fn read(editor: &mut Editor, page: usize) -> bool {
    let Some(source) = editor.source() else {
        return false;
    };
    match pdf_session::interpret_page_fully(
        source,
        page,
        b"",
        editor.grouping(page).as_deref(),
        pdf_cli::font_provider(),
    ) {
        Ok(view) => {
            editor.adopt_page(page, Arc::new(view));
            true
        }
        Err(_) => false,
    }
}

fn timed<T>(work: impl FnOnce() -> T) -> (T, f64) {
    let start = Instant::now();
    let answer = work();
    (answer, start.elapsed().as_secs_f64() * 1000.0)
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = arguments.next().expect("a PDF path");
    let page: usize = arguments
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let letters: usize = arguments
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(10);

    let bytes = std::fs::read(&path).expect("read");
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
    let mut editor = Editor::open(source).expect("open");
    assert!(read(&mut editor, page), "page {page} could not be read");

    let leaf = editor.leaf(page).cloned().expect("the page has an overlay");
    let overlay = &leaf.overlay;
    let (block, rows) = overlay
        .blocks
        .iter()
        .enumerate()
        .max_by_key(|(_, block)| block.lines.len())
        .map(|(index, block)| (index, block.lines.len()))
        .expect("a block");
    let stops = overlay.blocks[block]
        .lines
        .last()
        .and_then(|line| leaf.view.index.lines.get(*line))
        .map_or(0, |line| line.clusters.len());
    let caret = editor
        .copy_text(page, block, (0, 0), (rows - 1, stops))
        .unwrap_or_default()
        .chars()
        .count();
    println!(
        "page {page}: blocks {}  clusters {}  objects {}  widest block {rows} rows, {caret} chars",
        overlay.blocks.len(),
        overlay.clusters.len(),
        overlay.objects.len(),
    );

    let (mut edits, mut rereads, mut typed) = (Vec::new(), Vec::new(), 0_usize);
    for step in 0..letters {
        let Some(leaf) = editor.leaf(page).cloned() else {
            break;
        };
        let Some(owner) = leaf.overlay.blocks.get(block) else {
            break;
        };
        let last = owner.lines.len().saturating_sub(1);
        let at = (
            last,
            owner
                .lines
                .last()
                .and_then(|line| leaf.view.index.lines.get(*line))
                .map_or(0, |line| line.clusters.len()),
        );
        let (applied, edit) =
            timed(|| editor.edit(page, block, BlockRange::Between { from: at, to: at }, "x"));
        if !matches!(applied, Applied::Changed { .. }) {
            println!("letter {step}: {applied:?}");
            break;
        }
        typed += 1;
        let (reread, fresh) = if editor.leaf(page).is_some() {
            (0.0, true)
        } else {
            let (read_again, cost) = timed(|| read(&mut editor, page));
            (cost, read_again)
        };
        assert!(fresh, "the page became unreadable after an edit");
        edits.push(edit);
        rereads.push(reread);
    }

    for _ in 0..typed {
        let _ = editor.undo();
    }
    read(&mut editor, page);

    if edits.is_empty() {
        println!("nothing was typed");
        return;
    }
    let mean = |values: &[f64]| {
        let count = u32::try_from(values.len()).unwrap_or(u32::MAX);
        values.iter().sum::<f64>() / f64::from(count)
    };
    let worst = |values: &[f64]| values.iter().copied().fold(f64::MIN, f64::max);
    println!(
        "letters {typed}\n  edit    mean {:7.1} ms  worst {:7.1} ms\n  reread  mean {:7.1} ms  \
         worst {:7.1} ms\n  total   mean {:7.1} ms  worst {:7.1} ms",
        mean(&edits),
        worst(&edits),
        mean(&rereads),
        worst(&rereads),
        mean(&edits) + mean(&rereads),
        worst(&edits) + worst(&rereads),
    );
}
