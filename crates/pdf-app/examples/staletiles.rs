use std::collections::BTreeSet;
use std::sync::Arc;

use pdf_app::document::OVERLAY_SCALE;
use pdf_app::painter::rgba;
use pdf_app::strip::{boxes_overlap, tile_box, tiles_across};
use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::plan::{Command, SourceAnchor};
use pdf_session::PageView;

const ZOOMS: [f64; 4] = [0.5, 1.0, 2.0, 4.0];

const OFFSET: (f64, f64) = (40.0, -90.0);

fn touched_by_hand(region: [f64; 4], page_height_at_one: u32, tile: [u32; 4], scale: f64) -> bool {
    let k = OVERLAY_SCALE / scale;
    let [x0, y0, x1, y1] = tile.map(f64::from);
    let [rx0, ry0, rx1, ry1] = region;
    let height = f64::from(page_height_at_one);
    let one = [x0 * k, y0 * k, x1 * k, y1 * k];
    let other = [rx0, height - ry1, rx1, height - ry0];
    one[0] < other[2] && other[0] < one[2] && one[1] < other[3] && other[1] < one[3]
}

fn main() {
    let path = std::env::args().nth(1).expect("path");
    let page: usize = std::env::args()
        .nth(2)
        .map_or(0, |value| value.parse().expect("page"));
    let bytes = std::fs::read(&path).expect("read");
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
    let view = pdf_session::interpret_page(&source, page).expect("interpret");
    let index = &view.index;
    println!(
        "{path} page {}: {} blocks, moved by {:?} points",
        page + 1,
        index.blocks.len(),
        OFFSET
    );

    let editor = pdf_app::Editor::open(source.clone()).expect("open");
    let Some((_, height_at_one)) = editor.page_pixels(page, OVERLAY_SCALE) else {
        println!("this page has no device transform");
        return;
    };

    let mut tally = Tally::default();
    let mut first = String::new();
    for block in 0..index.blocks.len() {
        let mut named: BTreeSet<usize> = BTreeSet::new();
        for &line in &index.blocks[block].lines {
            for &cluster in &index.lines[line].clusters {
                named.insert(index.clusters[cluster].atom);
            }
        }
        if named.is_empty() {
            continue;
        }
        let runs: Vec<SourceAnchor> = named
            .iter()
            .map(|atom| SourceAnchor::of(&view.graph.atoms[*atom].id))
            .collect();
        let Ok(plan) = pdf_edit::spike_move_text::plan_command_in(
            &source,
            pdf_edit::spike_move_text::PlannerPage {
                program: &view.program,
                operations: &view.operations,
                graph: &view.graph,
                fonts: None,
                restrictions: pdf_edit::Restrictions::Respect,
                credential: b"",
            },
            &Command::MoveTextBlock {
                page_index: page,
                runs,
                dx: OFFSET.0,
                dy: OFFSET.1,
            },
        ) else {
            continue;
        };
        let Some(region) = plan.effect().declared_region else {
            continue;
        };
        let Ok(committed) = plan.commit(&source, b"") else {
            continue;
        };
        let Ok(after) = pdf_session::interpret_page(&committed, page) else {
            continue;
        };
        tally.moved += 1;
        check_block(
            &editor,
            Sides {
                before: &view,
                after: &after,
            },
            Named {
                page,
                block,
                region,
                height_at_one,
            },
            &mut tally,
            &mut first,
        );
    }
    println!(
        "blocks moved {}, tiles examined {}, tiles kept {}",
        tally.moved, tally.tiles, tally.kept
    );
    println!(
        "kept-but-changed, by the hand-written flip : {}",
        tally.wrong_by_hand
    );
    println!(
        "kept-but-changed, by the device transform  : {}",
        tally.wrong
    );
    if first.is_empty() {
        println!("every tile the window keeps is the same picture it was");
    } else {
        println!("for instance -- {first}");
        std::process::exit(1);
    }
}

#[derive(Default)]
struct Tally {
    moved: u64,
    tiles: u64,
    kept: u64,
    wrong: u64,
    wrong_by_hand: u64,
}

#[derive(Clone, Copy)]
struct Sides<'a> {
    before: &'a PageView,
    after: &'a PageView,
}

#[derive(Clone, Copy)]
struct Named {
    page: usize,
    block: usize,
    region: [f64; 4],
    height_at_one: u32,
}

fn check_block(
    editor: &pdf_app::Editor,
    sides: Sides<'_>,
    named: Named,
    tally: &mut Tally,
    first: &mut String,
) {
    let Named {
        page,
        block,
        region,
        height_at_one,
    } = named;
    {
        for scale in ZOOMS {
            let Some(touched) = editor.region_in_pixels(page, scale, region) else {
                continue;
            };
            let Some((width, down)) = editor.page_pixels(page, scale) else {
                continue;
            };
            let (across, rows) = tiles_across((width, down));
            for row in 0..rows {
                for col in 0..across {
                    let Some(box_pixels) = tile_box((width, down), col, row) else {
                        continue;
                    };
                    tally.tiles += 1;
                    let now = boxes_overlap(box_pixels, touched);
                    let by_hand = touched_by_hand(region, height_at_one, box_pixels, scale);
                    if now && by_hand {
                        continue;
                    }
                    let same = same_pixels(sides.before, sides.after, scale, box_pixels);
                    if !now {
                        tally.kept += 1;
                        if !same {
                            tally.wrong += 1;
                            if first.is_empty() {
                                *first = format!(
                                    "block {block} at zoom {scale}: tile {box_pixels:?} kept, \
                                     but its pixels changed (region {region:?} -> {touched:?})"
                                );
                            }
                        }
                    }
                    if !by_hand && !same {
                        tally.wrong_by_hand += 1;
                    }
                }
            }
        }
    }
}

fn same_pixels(before: &PageView, after: &PageView, scale: f64, window: [u32; 4]) -> bool {
    let (Ok((one, _)), Ok((other, _))) = (
        pdf_cli::render_region_pixels_view(before, scale, window),
        pdf_cli::render_region_pixels_view(after, scale, window),
    ) else {
        return false;
    };
    rgba(&one) == rgba(&other)
}
