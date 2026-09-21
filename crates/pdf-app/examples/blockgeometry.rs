use std::sync::Arc;

use pdf_app::Editor;
use pdf_bytes::{ByteStore, SourceId};

fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = arguments.next().expect("a PDF path");
    let page: usize = arguments
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let block: usize = arguments
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let bytes = std::fs::read(&path).expect("readable");
    let mut editor =
        Editor::open(ByteStore::new(SourceId::new(7), Arc::<[u8]>::from(bytes))).expect("opens");
    let source = editor.source().expect("a source").clone();
    let view = pdf_session::interpret_page_fully(
        &source,
        page,
        b"",
        editor.grouping(page).as_deref(),
        pdf_cli::font_provider(),
    )
    .expect("page reads");
    editor.adopt_page(page, Arc::new(view));
    println!("frame {:?}", editor.frame_boxes(page).get(block));
    if std::env::var_os("ROWS").is_some() {
        let view = &editor.leaf(page).expect("adopted").view;
        for line in &view.index.blocks[block].lines {
            let first = view.index.lines[*line]
                .clusters
                .first()
                .map(|c| &view.index.clusters[*c]);
            println!(
                "row {line}: baseline {:?}",
                first.map(|c| (
                    (c.baseline.x * 10.0).round() / 10.0,
                    (c.baseline.y * 10.0).round() / 10.0
                ))
            );
            for cluster in &view.index.lines[*line].clusters {
                let c = &view.index.clusters[*cluster];
                println!(
                    "  atom {} glyphs {:?} at ({:.1}, {:.1}) advance {:.2} em {:.1} ink {:?}",
                    c.atom,
                    c.glyphs,
                    c.baseline.x,
                    c.baseline.y,
                    c.advance,
                    c.em,
                    c.bounds.map(|b| b.map(f64::round))
                );
            }
        }
    }
    let leaf = editor.leaf(page).expect("adopted").clone();
    let owner = &leaf.overlay.blocks[block];
    println!("layout {:?}", owner.layout_pixels);
    for line in &owner.lines {
        let clusters: Vec<String> = leaf
            .overlay
            .clusters
            .iter()
            .filter(|cluster| cluster.line == *line)
            .map(|cluster| {
                format!(
                    "{:?}@{:?}",
                    cluster.text.as_deref().unwrap_or("?"),
                    cluster.box_pixels.map(|b| [
                        b[0].round(),
                        b[1].round(),
                        b[2].round(),
                        b[3].round()
                    ])
                )
            })
            .collect();
        println!("line {line}: {}", clusters.join(" "));
    }
    println!(
        "copy {:?}",
        editor.copy_text(page, block, (0, 0), (owner.lines.len() - 1, usize::MAX))
    );
}
