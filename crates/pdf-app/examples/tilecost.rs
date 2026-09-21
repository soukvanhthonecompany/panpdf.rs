use std::sync::Arc;

use pdf_app::painter::{draw_page, draw_region};
use pdf_app::strip::{tile_box, tiles_across};
use pdf_bytes::{ByteStore, SourceId};

fn main() {
    let path = std::env::args().nth(1).expect("path");
    let page: usize = std::env::args()
        .nth(2)
        .map_or(0, |value| value.parse().expect("page"));
    let scale: f64 = std::env::args()
        .nth(3)
        .map_or(2.0, |value| value.parse().expect("scale"));
    let bytes = std::fs::read(&path).expect("read");
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
    let view = pdf_session::interpret_page(&source, page).expect("interpret");

    let began = std::time::Instant::now();
    let (canvas, report) = draw_page(&view, scale).expect("draw page");
    let whole = began.elapsed();
    let pixels = (canvas.width, canvas.height);
    println!(
        "{path} page {} at {scale}x: {}x{} pixels, {} atoms visited, {} drawn, {} skipped as out of the way",
        page + 1,
        pixels.0,
        pixels.1,
        report.visited,
        report.drawn,
        report.culled
    );
    println!("whole page: {:.1} ms", whole.as_secs_f64() * 1e3);

    let began = std::time::Instant::now();
    let (_, report) = draw_region(&view, scale, [0, 0, 8, 8]).expect("draw region");
    println!(
        "8x8 corner: {:.1} ms, {} visited, {} drawn, {} out of the way",
        began.elapsed().as_secs_f64() * 1e3,
        report.visited,
        report.drawn,
        report.culled
    );

    let (across, down) = tiles_across(pixels);
    let mut total = std::time::Duration::ZERO;
    let mut tiles = 0_u32;
    for row in 0..down {
        for col in 0..across {
            let Some(window) = tile_box(pixels, col, row) else {
                continue;
            };
            let began = std::time::Instant::now();
            let (_, report) = draw_region(&view, scale, window).expect("draw region");
            let took = began.elapsed();
            total += took;
            tiles += 1;
            println!(
                "  tile {col},{row} {:?}: {:.1} ms, {} visited, {} drawn, {} out of the way",
                window,
                took.as_secs_f64() * 1e3,
                report.visited,
                report.drawn,
                report.culled
            );
        }
    }
    println!(
        "{tiles} tiles: {:.1} ms together, {:.1}x the whole page",
        total.as_secs_f64() * 1e3,
        total.as_secs_f64() / whole.as_secs_f64()
    );
}
