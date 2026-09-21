use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};

fn covers(box_of: [f64; 4], point: (f64, f64)) -> bool {
    point.0 >= box_of[0] && point.0 < box_of[2] && point.1 >= box_of[1] && point.1 < box_of[3]
}

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

fn main() {
    let dir = std::env::args().nth(1).expect("dir");
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .expect("readdir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")))
        .collect();
    paths.sort();

    let (mut runs, mut centre_free, mut wholly_free, mut pages) = (0_usize, 0_usize, 0_usize, 0);
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
        let boxes: Vec<[f64; 4]> = overlay
            .blocks
            .iter()
            .map(|block| block.box_pixels)
            .collect();
        for run in &overlay.runs {
            runs += 1;
            let b = run.box_pixels;
            let centre = (f64::midpoint(b[0], b[2]), f64::midpoint(b[1], b[3]));
            if !boxes.iter().any(|box_of| covers(*box_of, centre)) {
                centre_free += 1;
            }
            let corners = [
                (b[0], b[1]),
                (b[2], b[1]),
                (b[0], b[3]),
                (b[2], b[3]),
                centre,
            ];
            if !corners
                .iter()
                .any(|point| boxes.iter().any(|box_of| covers(*box_of, *point)))
            {
                wholly_free += 1;
            }
        }
    }
    println!("pages: {pages}   text runs: {runs}");
    println!(
        "runs whose centre no block covers : {centre_free}  ({:.3}%)",
        percent(centre_free, runs)
    );
    println!(
        "runs no block covers at all       : {wholly_free}  ({:.3}%)",
        percent(wholly_free, runs)
    );
}
