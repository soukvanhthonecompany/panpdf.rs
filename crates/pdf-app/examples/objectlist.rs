use std::sync::Arc;

use pdf_app::Editor;
use pdf_bytes::{ByteStore, SourceId};

fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = arguments.next().expect("a PDF path");
    let page = arguments
        .next()
        .and_then(|value| value.parse::<usize>().ok())
        .map_or(0, |value| value.saturating_sub(1));
    let bytes = std::fs::read(&path).expect("the file reads");
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
    let mut editor = Editor::open(source).expect("the file opens");
    let Some(source) = editor.source().cloned() else {
        return;
    };
    let view =
        pdf_session::interpret_page_fully(&source, page, b"", None, pdf_cli::font_provider())
            .expect("the page reads");
    editor.adopt_page(page, Arc::new(view));
    let Some(leaf) = editor.leaf(page) else {
        return;
    };
    for (index, object) in leaf.overlay.objects.iter().enumerate() {
        let [x0, y0, x1, y1] = object.box_pixels;
        println!(
            "{index:3} {:?} box [{x0:.0} {y0:.0} {x1:.0} {y1:.0}] quad {:?} {}",
            object.kind, object.quad, object.anchor
        );
    }
}
