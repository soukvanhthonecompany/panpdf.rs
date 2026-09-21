use std::sync::Arc;
use std::time::Instant;

use pdf_bytes::{ByteStore, SourceId};

const VIEW: (u32, u32) = (1400, 900);

fn window_at(scale_width: u32, scale_height: u32) -> [u32; 4] {
    let w = VIEW.0.min(scale_width);
    let h = VIEW.1.min(scale_height);
    [0, 0, w, h]
}

fn main() {
    let path = std::env::args().nth(1).expect("path");
    let page: usize = std::env::args()
        .nth(2)
        .map_or(0, |a| a.parse().expect("page"));
    let bytes = std::fs::read(&path).expect("read");
    println!("{path}  page {page}  ({} bytes)", bytes.len());
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));

    let start = Instant::now();
    let view = pdf_session::interpret_page(&source, page).expect("interpret");
    let interpret = start.elapsed();
    println!(
        "interpret (once, cached afterwards): {:.3} s   atoms {}  clusters {}  lines {}",
        interpret.as_secs_f64(),
        view.graph.atoms.len(),
        view.index.clusters.len(),
        view.index.lines.len()
    );

    println!(
        "\n{:>6}  {:>11}  {:>11}  {:>11}  {:>11}  {:>9}",
        "zoom", "page px", "whole page", "one window", "overlay", "zoom = w+o"
    );
    for scale in [0.25_f64, 0.5, 1.0, 2.0, 4.0] {
        let device = pdf_render::DeviceTransform::for_page(
            &view.program.geometry,
            scale,
            pdf_render::RenderLimits::default(),
        )
        .expect("device");
        let window = window_at(device.width, device.height);

        let start = Instant::now();
        let _ = pdf_cli::render_page_view(&view, scale).expect("whole");
        let whole = start.elapsed();

        let start = Instant::now();
        let _ = pdf_cli::render_region_pixels_view(&view, scale, window).expect("window");
        let one = start.elapsed();

        let start = Instant::now();
        let _ = pdf_cli::page_overlay_view(&view, scale).expect("overlay");
        let overlay = start.elapsed();

        println!(
            "{scale:>6.2}  {:>5}x{:<5}  {:>9.3} s  {:>9.3} s  {:>9.3} s  {:>7.3} s",
            device.width,
            device.height,
            whole.as_secs_f64(),
            one.as_secs_f64(),
            overlay.as_secs_f64(),
            one.as_secs_f64() + overlay.as_secs_f64()
        );
    }

    let scale = 4.0;
    let device = pdf_render::DeviceTransform::for_page(
        &view.program.geometry,
        scale,
        pdf_render::RenderLimits::default(),
    )
    .expect("device");
    println!(
        "\nhow one window's cost follows its size, at zoom {scale:.2} on a {}x{} page:",
        device.width, device.height
    );
    for side in [16_u32, 100, 400, 900, 1400, 2000] {
        let (w, h) = (side.min(device.width), side.min(device.height));
        let start = Instant::now();
        let _ = pdf_cli::render_region_pixels_view(&view, scale, [0, 0, w, h]).expect("window");
        println!(
            "  {w:>5}x{h:<5} ({:>9} px) : {:.3} s",
            u64::from(w) * u64::from(h),
            start.elapsed().as_secs_f64()
        );
    }
}
