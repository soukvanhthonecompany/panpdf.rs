use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};

fn corner_distance(one: [f64; 4], other: [f64; 4]) -> f64 {
    (one[0] - other[0])
        .abs()
        .max((one[1] - other[1]).abs())
        .max((one[2] - other[2]).abs())
        .max((one[3] - other[3]).abs())
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

    for scale in [0.25_f64, 1.0, 4.0] {
        let mut blocks = 0_usize;
        let mut pages = 0_usize;
        let mut closest = f64::INFINITY;
        let mut closest_where = String::new();
        let mut within: Vec<usize> = vec![0; 5];
        let bands = [0.001_f64, 0.01, 0.1, 1.0, 4.0];
        for path in &paths {
            let Ok(bytes) = std::fs::read(path) else {
                continue;
            };
            let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
            let Ok(view) = pdf_session::interpret_page(&source, 0) else {
                continue;
            };
            let Ok(overlay) = pdf_cli::page_overlay_view(&view, scale) else {
                continue;
            };
            pages += 1;
            let boxes: Vec<[f64; 4]> = overlay
                .blocks
                .iter()
                .map(|block| block.box_pixels)
                .collect();
            blocks += boxes.len();
            for (index, one) in boxes.iter().enumerate() {
                let mut nearest = f64::INFINITY;
                for (other_index, other) in boxes.iter().enumerate() {
                    if index != other_index {
                        nearest = nearest.min(corner_distance(*one, *other));
                    }
                }
                if nearest < closest {
                    closest = nearest;
                    closest_where = path.file_name().unwrap().to_string_lossy().to_string();
                }
                for (slot, band) in bands.iter().enumerate() {
                    if nearest <= *band {
                        within[slot] += 1;
                    }
                }
            }
        }
        println!("scale {scale}: {pages} pages, {blocks} blocks");
        for (slot, band) in bands.iter().enumerate() {
            println!(
                "   a different block within {band:>6}: {:>5}  ({:.2}%)",
                within[slot],
                percent(within[slot], blocks)
            );
        }
        println!("   closest pair: {closest:.4} px, in {closest_where}");
    }
}
