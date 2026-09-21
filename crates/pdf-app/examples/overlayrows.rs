use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};

fn main() {
    let dir = std::env::args().nth(1).expect("dir");
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .expect("readdir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")))
        .collect();
    paths.sort();

    let (mut pages, mut rows, mut clusters, mut wrong, mut off_by) = (0, 0_usize, 0_usize, 0, 0);
    let (mut undrawable, mut rowless, mut holed) = (0_usize, 0_usize, 0_usize);
    let mut first = String::new();
    for path in &paths {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
        let Ok(view) = pdf_session::interpret_page(&source, 0) else {
            continue;
        };
        let Ok(overlay) = pdf_cli::page_overlay_view(&view, 1.0) else {
            continue;
        };
        pages += 1;
        let mut in_a_row = vec![false; view.index.clusters.len()];
        for line in &view.index.lines {
            for cluster in &line.clusters {
                in_a_row[*cluster] = true;
            }
        }
        for (ordinal, cluster) in view.index.clusters.iter().enumerate() {
            if !in_a_row[ordinal] {
                rowless += 1;
            }
            let drawn = match &view.graph.atoms[cluster.atom].kind {
                pdf_paint::PaintAtomKind::Text(text) => {
                    text.outline_bounds_in(cluster.glyphs.clone()).is_some()
                }
                _ => false,
            };
            if !drawn {
                undrawable += 1;
            }
        }
        for (row, line) in view.index.lines.iter().enumerate() {
            let drawn_here = line
                .clusters
                .iter()
                .filter(|cluster| {
                    match &view.graph.atoms[view.index.clusters[**cluster].atom].kind {
                        pdf_paint::PaintAtomKind::Text(text) => text
                            .outline_bounds_in(view.index.clusters[**cluster].glyphs.clone())
                            .is_some(),
                        _ => false,
                    }
                })
                .count();
            if drawn_here != line.clusters.len() {
                holed += 1;
            }
            rows += 1;
            clusters += line.clusters.len();
            let held = overlay
                .clusters
                .iter()
                .filter(|cluster| cluster.line == row)
                .count();
            if held != line.clusters.len() {
                wrong += 1;
                off_by += held.abs_diff(line.clusters.len());
                if first.is_empty() {
                    first = format!(
                        "{}: row {row} has {} clusters, the overlay holds {held}",
                        path.file_name().unwrap().to_string_lossy(),
                        line.clusters.len()
                    );
                }
            }
        }
    }
    println!("pages {pages}   rows {rows}   clusters {clusters}");
    println!(
        "under the old rules this list would have dropped {undrawable} clusters with no \
         outline (on {holed} rows) and misplaced {rowless} belonging to no row"
    );
    println!("rows the overlay miscounts: {wrong}   clusters out by: {off_by}");
    if first.is_empty() {
        println!("every row holds exactly the clusters its numbering counts");
    } else {
        println!("for instance -- {first}");
        std::process::exit(1);
    }
}
