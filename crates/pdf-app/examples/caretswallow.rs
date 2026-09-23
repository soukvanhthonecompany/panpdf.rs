use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};

const STROKE_WIDTH: f64 = 1.5;

const NOTABLE: f64 = 0.05;

struct Tally {
    stops_checked: usize,
    inside_before_box: usize,
    inside_after_box: usize,
    overhang_over_0: usize,
    overhang_sum: f64,
    overhang_max: f64,
    gap_under_stroke: usize,
    gap_negative: usize,
    stroke_touches_before: usize,
    stroke_touches_after: usize,
    overhang_and_stacked: usize,
}

impl Tally {
    const fn new() -> Self {
        Self {
            stops_checked: 0,
            inside_before_box: 0,
            inside_after_box: 0,
            overhang_over_0: 0,
            overhang_sum: 0.0,
            overhang_max: 0.0,
            gap_under_stroke: 0,
            gap_negative: 0,
            stroke_touches_before: 0,
            stroke_touches_after: 0,
            overhang_and_stacked: 0,
        }
    }
}

fn measure_row(
    overlay: &pdf_cli::PageOverlay,
    line: usize,
    path: &std::path::Path,
    tally: &mut Tally,
    printed: &mut usize,
) {
    let mut on_line: Vec<_> = overlay.clusters.iter().filter(|c| c.line == line).collect();
    on_line.sort_by_key(|c| c.index_in_line);
    let stops: Vec<_> = overlay.carets.iter().filter(|s| s.line == line).collect();
    for stop in &stops {
        if stop.up[0].abs() > stop.up[1].abs() * 0.05 {
            continue;
        }
        let Some(before) = on_line.iter().find(|c| c.index_in_line + 1 == stop.offset) else {
            continue;
        };
        let Some(after) = on_line.iter().find(|c| c.index_in_line == stop.offset) else {
            continue;
        };
        let (Some(bb), Some(ab)) = (before.box_pixels, after.box_pixels) else {
            continue;
        };
        tally.stops_checked += 1;
        let x = stop.at[0];
        let overhang = bb[2] - x;
        let lead = ab[0] - x;
        let gap = ab[0] - bb[2];
        if x >= bb[0] && x <= bb[2] {
            tally.inside_before_box += 1;
        }
        if x >= ab[0] && x <= ab[2] {
            tally.inside_after_box += 1;
        }
        if overhang > 0.0 {
            tally.overhang_over_0 += 1;
            tally.overhang_sum += overhang;
            tally.overhang_max = tally.overhang_max.max(overhang);
            if before.stacked {
                tally.overhang_and_stacked += 1;
            }
        }
        if gap < STROKE_WIDTH {
            tally.gap_under_stroke += 1;
        }
        if gap < 0.0 {
            tally.gap_negative += 1;
        }
        let stroke = [x - STROKE_WIDTH / 2.0, x + STROKE_WIDTH / 2.0];
        let touch_before = (stroke[1].min(bb[2]) - stroke[0].max(bb[0])).max(0.0);
        let touch_after = (stroke[1].min(ab[2]) - stroke[0].max(ab[0])).max(0.0);
        if touch_before > 0.0 {
            tally.stroke_touches_before += 1;
        }
        if touch_after > 0.0 {
            tally.stroke_touches_after += 1;
        }
        if *printed < 60 && (overhang > NOTABLE || lead < -NOTABLE || touch_before > 0.0) {
            *printed += 1;
            println!(
                "{} line {line} offset {} x={x:.3}  before {:?} ink=[{:.3},{:.3}] overhang={overhang:.3}  after {:?} ink=[{:.3},{:.3}] lead={lead:.3}  gap={gap:.3}  stroke_on_before={touch_before:.3} stroke_on_after={touch_after:.3}",
                path.file_name().unwrap_or_default().display(),
                stop.offset,
                before.text.as_deref().unwrap_or("?"),
                bb[0],
                bb[2],
                after.text.as_deref().unwrap_or("?"),
                ab[0],
                ab[2],
            );
        }
    }
}

fn main() {
    let dir = std::env::args().nth(1).expect("dir");
    let limit: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(40);
    let scale: f64 = std::env::args()
        .nth(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.0);
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .expect("readdir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")))
        .collect();
    paths.sort();
    let step = (paths.len() / limit.max(1)).max(1);
    let sample: Vec<_> = paths.into_iter().step_by(step).take(limit).collect();

    let mut tally = Tally::new();
    let mut pages = 0_usize;
    let mut printed = 0_usize;

    for path in &sample {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
        let Some(overlay) = [0, 3, 6, 10, 15].iter().find_map(|&page_index| {
            let view = pdf_session::interpret_page(&source, page_index).ok()?;
            let overlay = pdf_cli::page_overlay_view(&view, scale).ok()?;
            (overlay.clusters.len() > 20).then_some(overlay)
        }) else {
            continue;
        };
        pages += 1;

        for block in &overlay.blocks {
            for &line in &block.lines {
                measure_row(&overlay, line, path, &mut tally, &mut printed);
            }
        }
    }

    println!(
        "\npages read: {pages}   stops checked: {}",
        tally.stops_checked
    );
    println!(
        "stops whose x sits inside the BEFORE cluster's own ink box: {} of {}",
        tally.inside_before_box, tally.stops_checked
    );
    println!(
        "stops whose x sits inside the AFTER cluster's own ink box : {} of {}",
        tally.inside_after_box, tally.stops_checked
    );
    println!(
        "stops where the before glyph's ink overhangs past the stop: {} of {}  (mean {:.3}px, max {:.3}px, of those)",
        tally.overhang_over_0,
        tally.stops_checked,
        if tally.overhang_over_0 == 0 {
            0.0
        } else {
            tally.overhang_sum / f64::from(u32::try_from(tally.overhang_over_0).unwrap_or(u32::MAX))
        },
        tally.overhang_max
    );
    println!(
        "   of those, the before cluster carries a stacked mark      : {} of {}",
        tally.overhang_and_stacked, tally.overhang_over_0
    );
    println!(
        "true ink-to-ink gap narrower than the {STROKE_WIDTH}px stroke        : {} of {}",
        tally.gap_under_stroke, tally.stops_checked
    );
    println!(
        "true ink-to-ink gap negative (the two clusters' boxes overlap)     : {} of {}",
        tally.gap_negative, tally.stops_checked
    );
    println!(
        "a stroke centred exactly on the stop would touch the BEFORE glyph  : {} of {}",
        tally.stroke_touches_before, tally.stops_checked
    );
    println!(
        "a stroke centred exactly on the stop would touch the AFTER glyph   : {} of {}",
        tally.stroke_touches_after, tally.stops_checked
    );
}
