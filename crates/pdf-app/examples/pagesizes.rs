use std::sync::Arc;
use std::time::Instant;

use pdf_bytes::{ByteStore, SourceId};

fn per(total: f64, count: usize) -> f64 {
    let count = u32::try_from(count).unwrap_or(u32::MAX);
    if count == 0 {
        return 0.0;
    }
    total / f64::from(count)
}

fn main() {
    let dir = std::env::args().nth(1).expect("dir");
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .expect("readdir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")))
        .collect();
    paths.sort();

    println!(
        "{:>6}  {:>10}  {:>12}  {:>10}   file",
        "pages", "layout", "per page", "one page"
    );
    let (mut worst, mut worst_name) = (0.0_f64, String::new());
    for path in &paths {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
        let session = pdf_session::Session::new(source.clone(), b"");
        let start = Instant::now();
        let Ok(sizes) = session.page_geometries() else {
            continue;
        };
        let layout = start.elapsed().as_secs_f64();
        let start = Instant::now();
        let read = pdf_session::interpret_page(&source, 0)
            .ok()
            .map_or(f64::NAN, |_| start.elapsed().as_secs_f64());
        if layout > worst {
            worst = layout;
            worst_name = path.file_name().unwrap().to_string_lossy().to_string();
        }
        if sizes.len() >= 50 {
            println!(
                "{:>6}  {layout:>8.3} s  {:>10.4} ms  {read:>8.3} s   {}",
                sizes.len(),
                per(1000.0 * layout, sizes.len()),
                path.file_name().unwrap().to_string_lossy()
            );
        }
    }
    println!("\nslowest layout: {worst:.3} s  ({worst_name})");
}
