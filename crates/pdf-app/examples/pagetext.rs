use std::fmt::Write as _;
use std::sync::Arc;

use pdf_app::Editor;
use pdf_bytes::{ByteStore, SourceId};

fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = arguments.next().expect("a PDF path");
    let page: usize = arguments.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let bytes = std::fs::read(&path).expect("readable");
    let mut editor =
        Editor::open(ByteStore::new(SourceId::new(7), Arc::<[u8]>::from(bytes))).expect("opens");
    let source = editor.source().expect("a source").clone();
    let view =
        pdf_session::interpret_page_fully(&source, page, b"", None, pdf_cli::font_provider())
            .expect("page reads");
    editor.adopt_page(page, Arc::new(view));
    let overlay = &editor.leaf(page).expect("adopted").overlay;
    for (number, block) in overlay.blocks.iter().enumerate() {
        let rows: Vec<String> = block
            .lines
            .iter()
            .map(|line| {
                overlay
                    .clusters
                    .iter()
                    .filter(|cluster| cluster.line == *line)
                    .map(|cluster| {
                        cluster
                            .text
                            .clone()
                            .unwrap_or_default()
                            .replace('\u{fffd}', "")
                    })
                    .collect()
            })
            .collect();
        let mut text = String::new();
        for character in rows.join("\n").chars() {
            match character {
                '"' => text.push_str("\\\""),
                '\\' => text.push_str("\\\\"),
                '\n' => text.push_str("\\n"),
                other if other.is_control() => {
                    let _ = write!(text, "\\u{:04x}", u32::from(other));
                }
                other => text.push(other),
            }
        }
        println!(
            "{{\"block\":{number},\"turn\":{},\"text\":\"{text}\"}}",
            block.turn
        );
    }
}
