use std::sync::Arc;
use std::time::Instant;

use pdf_app::{Applied, Editor};
use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::BlockRange;

fn read(editor: &mut Editor, page: usize) -> bool {
    let Some(source) = editor.source() else {
        return false;
    };
    match pdf_session::interpret_page_fully(
        source,
        page,
        editor.credential(),
        editor.grouping(page).as_deref(),
        pdf_cli::font_provider(),
    ) {
        Ok(view) => {
            editor.adopt_page(page, Arc::new(view));
            true
        }
        Err(_) => false,
    }
}

fn millis(began: Instant) -> f64 {
    began.elapsed().as_secs_f64() * 1e3
}

fn device_region(device: &pdf_render::DeviceTransform, bounds: [f64; 4]) -> Option<[u32; 4]> {
    let corners = [
        (bounds[0], bounds[1]),
        (bounds[2], bounds[1]),
        (bounds[0], bounds[3]),
        (bounds[2], bounds[3]),
    ]
    .map(|(x, y)| device.matrix.transform(pdf_paint::Point { x, y }));
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for corner in corners {
        x0 = x0.min(corner.x);
        y0 = y0.min(corner.y);
        x1 = x1.max(corner.x);
        y1 = y1.max(corner.y);
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let clamp = |value: f64, most: u32| value.clamp(0.0, f64::from(most)) as u32;
    let region = [
        clamp(x0.floor() - 1.0, device.width),
        clamp(y0.floor() - 1.0, device.height),
        clamp(x1.ceil() + 1.0, device.width),
        clamp(y1.ceil() + 1.0, device.height),
    ];
    (region[2] > region[0] && region[3] > region[1]).then_some(region)
}

#[expect(
    clippy::too_many_lines,
    reason = "an instrument read top to bottom: set up, one block at a time, report"
)]
fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = arguments.next().expect("a PDF path");
    let page: usize = arguments
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let typing = arguments.next();
    let bytes = std::fs::read(&path).expect("read");
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
    let mut editor = Editor::open(source).expect("open");
    editor.set_aside_restrictions();
    assert!(read(&mut editor, page), "page {page} could not be read");
    let blocks = editor
        .leaf(page)
        .map_or(0, |leaf| leaf.overlay.blocks.len());
    let options = pdf_render::RenderOptions {
        scale: 2.0,
        ..pdf_render::RenderOptions::default()
    };

    let (mut same, mut differ, mut declined, mut refused) = (0, 0, 0, 0);
    let (mut live_ms, mut draw_ms, mut commit_ms) = (Vec::new(), Vec::new(), Vec::new());
    let mut first_ms = Vec::new();
    let mut skipped: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    let mut reasons: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for block in 0..blocks {
        let Some(leaf) = editor.leaf(page).cloned() else {
            break;
        };
        let Some(owner) = leaf.overlay.blocks.get(block) else {
            *skipped.entry("no such block").or_default() += 1;
            continue;
        };
        let Some((row, index_line)) = owner.lines.iter().enumerate().next_back() else {
            *skipped.entry("no lines").or_default() += 1;
            continue;
        };
        let Some(clusters) = leaf
            .view
            .index
            .lines
            .get(*index_line)
            .map(|found| &found.clusters)
        else {
            *skipped.entry("line not in the index").or_default() += 1;
            continue;
        };
        let stops = clusters.len();
        let Some(text) = typing.clone().or_else(|| {
            (1..=stops).rev().find_map(|stop| {
                editor
                    .copy_text(page, block, (row, stop - 1), (row, stop))
                    .filter(|text| !text.trim().is_empty() && !text.contains('\u{FFFD}'))
            })
        }) else {
            *skipped.entry("last cluster copies as nothing").or_default() += 1;
            continue;
        };
        let range = BlockRange::Between {
            from: (row, stops),
            to: (row, stops),
        };
        let began = Instant::now();
        let laid_live = match editor.live_block(page, block, range, &text) {
            Ok(laid_live) => laid_live,
            Err(why) => {
                declined += 1;
                *reasons.entry(why.chars().take(90).collect()).or_default() += 1;
                continue;
            }
        };
        first_ms.push(millis(began));
        let began = Instant::now();
        for _ in 0..5 {
            let _ = editor.live_block(page, block, range, &text);
        }
        live_ms.push(millis(began) / 5.0);
        let view = &leaf.view;
        let device = pdf_render::DeviceTransform::for_page(
            &view.program.geometry,
            options.scale,
            options.limits,
        )
        .unwrap();
        let before = pdf_paint::decipher_fonts::region_of(
            &view.graph,
            &laid_live.hidden.iter().copied().collect::<Vec<_>>(),
        );
        let bounds = match (laid_live.extent, before) {
            (Some(one), Some(other)) => [
                one[0].min(other[0]),
                one[1].min(other[1]),
                one[2].max(other[2]),
                one[3].max(other[3]),
            ],
            (Some(one), None) | (None, Some(one)) => one,
            (None, None) => continue,
        };
        let Some(region) = device_region(&device, bounds) else {
            continue;
        };
        let typed = pdf_paint::PaintGraph {
            atoms: laid_live.atoms.clone(),
            ..pdf_paint::PaintGraph::default()
        };
        let began = Instant::now();
        let above: Vec<&pdf_paint::PaintGraph> = view
            .annotations
            .iter()
            .map(|annotation| &annotation.graph)
            .collect();
        let Ok((shown, _)) = pdf_render::render_region_replacing(
            (&view.graph, &laid_live.hidden, &typed),
            &above,
            &view.program.geometry,
            options,
            region,
        ) else {
            continue;
        };
        draw_ms.push(millis(began));
        let began = Instant::now();
        let applied = editor.edit(page, block, range, &text);
        commit_ms.push(millis(began));
        if !matches!(applied, Applied::Changed { .. }) {
            refused += 1;
            *reasons
                .entry(
                    format!("live, but the commit said {applied:?}")
                        .chars()
                        .take(90)
                        .collect(),
                )
                .or_default() += 1;
            continue;
        }
        if editor.leaf(page).is_none() {
            read(&mut editor, page);
        }
        let after = editor.leaf(page).expect("read again").view.clone();
        let layers = after.layers();
        let (committed, _) =
            pdf_render::render_region_layers(&layers, &after.program.geometry, options, region)
                .unwrap();
        let worst = shown
            .pixels
            .iter()
            .zip(&committed.pixels)
            .flat_map(|(one, other)| (0..3).map(move |c| (one[c] - other[c]).abs()))
            .fold(0.0_f32, f32::max);
        let (mut count, mut bbox) = (0_usize, [u32::MAX, u32::MAX, 0, 0]);
        for (index, (one, other)) in shown.pixels.iter().zip(&committed.pixels).enumerate() {
            if (0..3).any(|c| (one[c] - other[c]).abs() > 1.0 / 255.0) {
                count += 1;
                #[allow(clippy::cast_possible_truncation)]
                let (x, y) = (index as u32 % shown.width, index as u32 / shown.width);
                bbox = [
                    bbox[0].min(x),
                    bbox[1].min(y),
                    bbox[2].max(x),
                    bbox[3].max(y),
                ];
            }
        }
        if let Ok(dir) = std::env::var("LIVECHECK_DUMP")
            && count > 0
        {
            for (name, canvas) in [("live", &shown), ("committed", &committed)] {
                let mut ppm = format!("P6 {} {} 255\n", canvas.width, canvas.height).into_bytes();
                for pixel in &canvas.pixels {
                    for channel in pixel {
                        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                        ppm.push((channel.clamp(0.0, 1.0) * 255.0).round() as u8);
                    }
                }
                let _ = std::fs::write(format!("{dir}/block{block}-{name}.ppm"), ppm);
            }
        }
        if count == 0 {
            same += 1;
        } else {
            differ += 1;
            println!(
                "  block {block}: typed {text:?}, {count} pixels of {} differ (up to {worst:.3}) in {bbox:?}; the commit said {:?}",
                shown.pixels.len(),
                editor.status()
            );
        }
        editor.undo();
        if editor.leaf(page).is_none() {
            read(&mut editor, page);
        }
    }
    let mean = |values: &[f64]| {
        #[allow(clippy::cast_precision_loss)]
        let count = values.len().max(1) as f64;
        values.iter().sum::<f64>() / count
    };
    let worst = |values: &[f64]| values.iter().copied().fold(0.0, f64::max);
    println!(
        "page {page}: {blocks} blocks -- live and committed the same {same}, different {differ}, \
         declined {declined}, live but refused {refused}"
    );
    println!(
        "  first live     mean {:6.2} ms  worst {:6.2} ms",
        mean(&first_ms),
        worst(&first_ms)
    );
    println!(
        "  lay out live   mean {:6.2} ms  worst {:6.2} ms",
        mean(&live_ms),
        worst(&live_ms)
    );
    println!(
        "  draw region    mean {:6.2} ms  worst {:6.2} ms",
        mean(&draw_ms),
        worst(&draw_ms)
    );
    println!(
        "  commit         mean {:6.2} ms  worst {:6.2} ms",
        mean(&commit_ms),
        worst(&commit_ms)
    );
    for (why, count) in skipped {
        println!("  skipped  {count:3}: {why}");
    }
    for (why, count) in reasons {
        println!("  declined {count:3}: {why}");
    }
}
