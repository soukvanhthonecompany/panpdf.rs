use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};

fn took(began: std::time::Instant) -> f64 {
    began.elapsed().as_secs_f64() * 1e3
}

fn main() {
    let path = std::env::args().nth(1).expect("path");
    let page: usize = std::env::args()
        .nth(2)
        .map_or(0, |value| value.parse().expect("page"));
    let bytes = std::fs::read(&path).expect("read");
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));

    let began = std::time::Instant::now();
    let session = pdf_session::Session::new(source.clone(), b"");
    let opened = took(began);
    let pages = session.page_count().expect("page count");
    println!("{path}: {pages} pages, opened in {opened:.1} ms");

    let mut runs = Vec::new();
    for _ in 0..5 {
        let began = std::time::Instant::now();
        let geometries = session.page_geometries().expect("geometries");
        runs.push(took(began));
        assert_eq!(geometries.len(), pages);
    }
    runs.sort_by(f64::total_cmp);
    println!(
        "page_geometries over {pages} pages: {:.1} ms (of 5: {:.1} .. {:.1})",
        runs[runs.len() / 2],
        runs[0],
        runs[runs.len() - 1]
    );

    let began = std::time::Instant::now();
    let view = pdf_session::interpret_page(&source, page).expect("interpret");
    println!(
        "interpret one page ({}): {:.1} ms, {} atoms",
        page + 1,
        took(began),
        view.graph.atoms.len()
    );

    let began = std::time::Instant::now();
    let overlay = pdf_cli::page_overlay_view(&view, 1.0).expect("overlay");
    println!(
        "overlay of one page: {:.1} ms, {} blocks",
        took(began),
        overlay.blocks.len()
    );
}
