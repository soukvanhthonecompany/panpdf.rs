use std::process::ExitCode;
use std::sync::Arc;

use pdf_app::painter::{draw_page, draw_region};
use pdf_app::strip::{tile_box, tiles_across};
use pdf_bytes::{ByteStore, SourceId};

fn main() -> ExitCode {
    let path = std::env::args().nth(1).expect("path");
    let page: usize = std::env::args()
        .nth(2)
        .map_or(0, |value| value.parse().expect("page"));
    let scales: Vec<f64> = match std::env::args().nth(3) {
        Some(scale) => vec![scale.parse().expect("scale")],
        None => vec![0.5, 1.0, 2.0, 4.0],
    };
    let bytes = std::fs::read(&path).expect("read");
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
    let Ok(view) = pdf_session::interpret_page(&source, page) else {
        println!("{path} page {}: will not read", page + 1);
        return ExitCode::SUCCESS;
    };

    let mut wrong = 0_usize;
    for scale in scales {
        let Ok((whole, _)) = draw_page(&view, scale) else {
            continue;
        };
        let pixels = (whole.width, whole.height);
        let (across, down) = tiles_across(pixels);
        for row in 0..down {
            for col in 0..across {
                let Some(window) = tile_box(pixels, col, row) else {
                    continue;
                };
                let Ok((tile, _)) = draw_region(&view, scale, window) else {
                    continue;
                };
                let (mut shown, mut differ) = (0_usize, 0_usize);
                let mut worst = 0.0_f32;
                let (one_bytes, other_bytes) = (tile.to_rgb8(), whole.to_rgb8());
                for y in 0..tile.height {
                    for x in 0..tile.width {
                        let (Some(here), Some(there)) = (
                            tile.index(window[0] + x, window[1] + y),
                            whole.index(window[0] + x, window[1] + y),
                        ) else {
                            continue;
                        };
                        let (one, other) = (tile.pixels[here], whole.pixels[there]);
                        let apart = one
                            .iter()
                            .zip(other)
                            .map(|(one, other)| (one - other).abs())
                            .fold(0.0_f32, f32::max);
                        if apart > 0.0 {
                            differ += 1;
                            worst = worst.max(apart);
                        }
                        if one_bytes[here * 3..here * 3 + 3]
                            != other_bytes[there * 3..there * 3 + 3]
                        {
                            shown += 1;
                        }
                    }
                }
                if shown > 0 || differ > 0 {
                    if shown > 0 {
                        wrong += 1;
                    }
                    println!(
                        "{path} page {} at {scale}x tile {col},{row} {window:?}: \
                         {shown} pixels shown differently, \
                         {differ} numbers differ, worst {worst:e}",
                        page + 1
                    );
                }
            }
        }
    }
    if wrong > 0 {
        println!("{path} page {}: {wrong} squares are not the page", page + 1);
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
