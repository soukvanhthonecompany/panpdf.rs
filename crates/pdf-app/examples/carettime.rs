use pdf_app::view::{Step, caret_at_in, caret_step_in};
use pdf_bytes::{ByteStore, SourceId};
use std::sync::Arc;
use std::time::Instant;

const ROUNDS: u32 = 1000;

fn main() {
    let path = std::env::args().nth(1).expect("file");
    let bytes = std::fs::read(&path).expect("read");
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
    let view = pdf_session::interpret_page(&source, 0).expect("page 0");
    let overlay = pdf_cli::page_overlay_view(&view, 1.0).expect("overlay");
    let block = overlay
        .blocks
        .iter()
        .max_by_key(|b| b.lines.len())
        .expect("a block");
    let rows = block.lines.clone();
    println!(
        "stops {}  clusters {}  widest block {} rows",
        overlay.carets.len(),
        overlay.clusters.len(),
        rows.len()
    );
    let start = Instant::now();
    for _ in 0..ROUNDS {
        std::hint::black_box(caret_at_in(&overlay.carets, &rows, (300.0, 200.0)));
    }
    println!(
        "caret_at_in            {:.1} us",
        start.elapsed().as_secs_f64() * 1e6 / f64::from(ROUNDS)
    );
    let start = Instant::now();
    for _ in 0..ROUNDS {
        std::hint::black_box(caret_step_in(&overlay.carets, &rows, 5, Step::Down));
    }
    println!(
        "caret_step_in          {:.1} us",
        start.elapsed().as_secs_f64() * 1e6 / f64::from(ROUNDS)
    );
    let line = *rows.first().expect("a row");
    let start = Instant::now();
    for _ in 0..ROUNDS {
        std::hint::black_box(pdf_cli::selection_is_actionable(
            &overlay.clusters,
            line,
            0,
            5,
        ));
    }
    println!(
        "selection_is_actionable {:.1} us",
        start.elapsed().as_secs_f64() * 1e6 / f64::from(ROUNDS)
    );
    println!("(a frame at 60fps is 16700 us)");
}
