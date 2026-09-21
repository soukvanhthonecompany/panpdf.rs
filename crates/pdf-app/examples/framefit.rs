use std::sync::Arc;

use pdf_app::Editor;
use pdf_bytes::{ByteStore, SourceId};

fn number(argument: Option<String>, fallback: usize) -> usize {
    argument
        .and_then(|value| value.parse().ok())
        .unwrap_or(fallback)
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = arguments
        .next()
        .expect("usage: framefit file.pdf [line from to text]");
    let line = number(arguments.next(), 0);
    let from = number(arguments.next(), 0);
    let to = number(arguments.next(), 5);
    let typed = arguments.next().unwrap_or_else(|| "HELLO".to_owned());

    let bytes = std::fs::read(&path).expect("read");
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
    let mut editor = Editor::open(source).expect("open");
    let view = pdf_session::interpret_page_grouped(editor.source().unwrap(), 0, b"", None)
        .expect("interpret");
    editor.adopt_page(0, Arc::new(view));

    let frames = editor.frame_boxes(0).to_vec();
    let block = editor
        .leaf(0)
        .unwrap()
        .view
        .index
        .blocks
        .iter()
        .position(|block| block.lines.contains(&line))
        .expect("no block owns that row");
    println!("row {line} belongs to block {block}");
    println!("  declared frame {:?}", frames[block]);

    let bytes = std::fs::read(&path).expect("read");
    let source = ByteStore::new(SourceId::new(2), Arc::<[u8]>::from(bytes));
    let mut session = pdf_session::Session::with_fonts(source, b"", pdf_cli::font_provider());
    let page = session.page(0).expect("page");
    let runs = match pdf_cli::page_replacement_view(&page, line, from, to, &typed) {
        Ok(runs) => runs,
        Err(reason) => {
            println!("  the replacement was refused before any frame check: {reason}");
            return;
        }
    };
    let plan = match session.plan(&pdf_edit::Command::RewriteText {
        page_index: 0,
        runs,
    }) {
        Ok(plan) => plan,
        Err(error) => {
            println!("  the plan was refused: {error}");
            return;
        }
    };
    let candidate = match session.preview(&plan) {
        Ok(candidate) => candidate,
        Err(reason) => {
            println!("  the candidate could not be interpreted: {reason}");
            return;
        }
    };
    let overlay = pdf_cli::page_overlay_view(&candidate, 1.0).expect("overlay");
    let frame = frames[block];
    for (name, bounds) in [
        ("ink", overlay.blocks[block].box_pixels),
        ("layout", overlay.blocks[block].layout_pixels),
    ] {
        let over = pdf_app::document::frame_fit(frame, bounds, 0.0).over;
        println!(
            "  candidate {name} {bounds:?}: overflow left {:.6} top {:.6} right {:.6} bottom {:.6}",
            over[0], over[1], over[2], over[3]
        );
    }
    let declared = editor.frame_is_declared(0, block);
    let fit = pdf_app::document::frame_fit(
        frame,
        overlay.blocks[block].layout_pixels,
        pdf_app::document::FRAME_SLACK,
    );
    let (side, worst) = fit.worst();
    println!(
        "  the frame is {}; the check measures the layout: worst {worst:.6} pt on the {side:?} side",
        if declared { "declared" } else { "inferred" },
    );
    println!(
        "  the editor answered: {:?}",
        editor.type_text(0, line, from, to, &typed)
    );
    println!("  status: {}", editor.status());
}
