use std::sync::Arc;

use pdf_app::painter::draw_page;
use pdf_bytes::{ByteStore, SourceId};

fn main() {
    let path = std::env::args().nth(1).expect("path");
    let page: usize = std::env::args()
        .nth(2)
        .expect("page")
        .parse()
        .expect("page");
    let scale: f64 = std::env::args()
        .nth(3)
        .map_or(1.0, |value| value.parse().expect("scale"));
    let bytes = std::fs::read(&path).expect("read");
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
    let Ok(view) = pdf_session::interpret_page(&source, page) else {
        println!("{path} {page} {scale} unreadable");
        return;
    };
    let Ok((canvas, report)) = draw_page(&view, scale) else {
        println!("{path} {page} {scale} undrawable");
        return;
    };
    let mut digest = 0xcbf2_9ce4_8422_2325_u64;
    for byte in canvas.to_rgb8() {
        digest ^= u64::from(byte);
        digest = digest.wrapping_mul(0x0000_0100_0000_01b3);
    }
    println!(
        "{path} {page} {scale} {digest:016x} {}x{} visited={} drawn={} skipped={}",
        canvas.width,
        canvas.height,
        report.visited,
        report.drawn,
        report.skipped.len()
    );
}
