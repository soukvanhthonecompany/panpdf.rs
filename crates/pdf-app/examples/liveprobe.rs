use std::sync::Arc;

use pdf_app::Editor;
use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::BlockRange;

fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = arguments.next().expect("a PDF path");
    let page: usize = arguments.next().and_then(|v| v.parse().ok()).expect("page");
    let block: usize = arguments
        .next()
        .and_then(|v| v.parse().ok())
        .expect("block");
    let text = arguments.next().expect("text");
    let bytes = std::fs::read(&path).expect("read");
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
    let mut editor = Editor::open(source).expect("open");
    editor.set_aside_restrictions();
    let view = pdf_session::interpret_page_fully(
        editor.source().unwrap(),
        page,
        editor.credential(),
        editor.grouping(page).as_deref(),
        pdf_cli::font_provider(),
    )
    .expect("page");
    editor.adopt_page(page, Arc::new(view));
    let reading = editor.block_reading(page, block).expect("a reading");
    let last = reading.lines.len() - 1;
    let at = (last, reading.lines[last].clusters.len());
    println!(
        "block {block}: {} lines, typing at {at:?}",
        reading.lines.len()
    );
    let mut typed = String::new();
    for character in text.chars() {
        typed.push(character);
        let began = std::time::Instant::now();
        match editor.live_block(
            page,
            block,
            BlockRange::Between { from: at, to: at },
            &typed,
        ) {
            Ok(live) => println!(
                "  {typed:?}: {} lines, {:.2} ms",
                live.lines.len(),
                began.elapsed().as_secs_f64() * 1e3
            ),
            Err(why) => println!("  {typed:?}: REFUSED {why}"),
        }
    }
}
