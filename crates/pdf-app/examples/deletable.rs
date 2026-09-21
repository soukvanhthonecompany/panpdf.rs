use std::collections::BTreeSet;
use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};

fn percent(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        return 0.0;
    }
    let (part, whole) = (
        u32::try_from(part).unwrap_or(u32::MAX),
        u32::try_from(whole).unwrap_or(u32::MAX),
    );
    100.0 * f64::from(part) / f64::from(whole)
}

const WIDTHS: [usize; 4] = [1, 2, 3, 5];

fn main() {
    let dir = std::env::args().nth(1).expect("dir");
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .expect("readdir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")))
        .collect();
    paths.sort();

    let mut asked = [0_usize; WIDTHS.len()];
    let mut kept = [0_usize; WIDTHS.len()];
    let mut pages = 0_usize;
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
        let rows: BTreeSet<usize> = overlay
            .blocks
            .iter()
            .flat_map(|block| block.lines.iter().copied())
            .collect();
        for row in rows {
            let width_of_row = overlay
                .clusters
                .iter()
                .filter(|cluster| cluster.line == row)
                .count();
            for (slot, width) in WIDTHS.iter().enumerate() {
                for from in 0..=width_of_row.saturating_sub(*width) {
                    asked[slot] += 1;
                    if pdf_cli::selection_is_actionable(&overlay.clusters, row, from, from + width)
                    {
                        kept[slot] += 1;
                    }
                }
            }
        }
    }
    println!("pages: {pages}");
    for (slot, width) in WIDTHS.iter().enumerate() {
        println!(
            "a selection of {width} cluster(s): {} of {} deletable  ({:.1}%)",
            kept[slot],
            asked[slot],
            percent(kept[slot], asked[slot])
        );
    }
}
