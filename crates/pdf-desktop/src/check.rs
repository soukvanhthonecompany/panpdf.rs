use std::collections::BTreeSet;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use pdf_app::files::is_a_copy;
use pdf_app::painter::{draw_page, draw_region, rgba};
use pdf_app::strip::{TILE, tile_box, tiles_across};
use pdf_app::{Applied, Editor};

use crate::fail;
use pdf_window::ZOOMS;
use pdf_window::save_file;

const VIEWPORT: (u32, u32) = (1400, 900);

pub(crate) fn report(
    mut editor: Editor,
    path: &Path,
    page: usize,
    destination: Option<&Path>,
) -> ExitCode {
    println!("file: {}", path.display());
    let (width, height) = editor.strip().size();
    println!(
        "document: {} pages, laid out as {width:.0}x{height:.0} points",
        editor.page_count()
    );
    if page >= editor.page_count() {
        return fail("--page is past the end of this document");
    }

    let read = Instant::now();
    let view = match editor.source().map(|source| {
        pdf_session::interpret_page_fully(
            source,
            page,
            editor.credential(),
            None,
            pdf_cli::font_provider(),
        )
    }) {
        Some(Ok(view)) => Arc::new(view),
        Some(Err(error)) => return fail(&error.to_string()),
        None => return fail("the document is busy"),
    };
    let read = read.elapsed();
    editor.adopt_page(page, Arc::clone(&view));
    let Some(leaf) = editor.leaf(page) else {
        return fail("the page was read but not taken in");
    };
    println!(
        "page {}: read in {:.3} s -- {} runs, {} caret stops, {} blocks",
        page + 1,
        read.as_secs_f64(),
        leaf.overlay.runs.len(),
        leaf.overlay.carets.len(),
        leaf.overlay.blocks.len()
    );
    let first_run = leaf.overlay.runs.first().map(|run| run.anchor.clone());
    let block = leaf
        .overlay
        .blocks
        .first()
        .map(|block| (block.anchors.clone(), block.box_pixels));

    report_tiles_match_the_page(&view, page, &editor);
    report_zoom(&view, page, &editor);

    let movable: Vec<(usize, Vec<String>, BTreeSet<usize>)> = leaf
        .overlay
        .blocks
        .iter()
        .enumerate()
        .filter(|(_, block)| block.anchors.len() > 1)
        .map(|(which, block)| {
            (
                which,
                block.anchors.clone(),
                runs_of(&leaf.view, &block.lines),
            )
        })
        .collect();
    report_block_identity(&editor, page, &movable);

    let edited = match (block, first_run) {
        (Some((anchors, _)), _) if anchors.len() > 1 => {
            report_block_move(&mut editor, page, &anchors)
        }
        (_, Some(anchor)) => report_move(&mut editor, page, &anchor),
        (_, None) => {
            println!("no run to move");
            false
        }
    };
    if !edited {
        return ExitCode::SUCCESS;
    }
    report_export(&mut editor, path, destination)
}

fn report_tiles_match_the_page(view: &pdf_session::PageView, page: usize, editor: &Editor) {
    for scale in [0.5_f64, 1.0, 2.0] {
        let Some((width, height)) = editor.page_pixels(page, scale) else {
            println!("tiles at {scale}: this page has no device transform");
            continue;
        };
        let Ok((whole, _)) = draw_page(view, scale) else {
            println!("tiles at {scale}: the page would not draw in one go");
            continue;
        };
        let expected = rgba(&whole);
        let (across, down) = tiles_across((width, height));
        let mut differing = 0_u64;
        let mut worst = 0_u8;
        let started = Instant::now();
        for row in 0..down {
            for col in 0..across {
                let Some(window) = tile_box((width, height), col, row) else {
                    continue;
                };
                let Ok((tile, _)) = draw_region(view, scale, window) else {
                    differing += u64::from(TILE) * u64::from(TILE);
                    continue;
                };
                let drawn = rgba(&tile);
                for y in 0..tile.height {
                    for x in 0..tile.width {
                        let here = (y as usize * tile.width as usize + x as usize) * 4;
                        let there = ((window[1] + y) as usize * width as usize
                            + (window[0] + x) as usize)
                            * 4;
                        for channel in 0..3 {
                            let (one, other) = (drawn[here + channel], expected[there + channel]);
                            if one != other {
                                differing += 1;
                                worst = worst.max(one.abs_diff(other));
                            }
                        }
                    }
                }
            }
        }
        let pixels = u64::from(width) * u64::from(height);
        println!(
            "tiles at {scale}: {across}x{down} tiles over {width}x{height} in {:.3} s -- \
             {differing} of {} channels differ from one whole-page render (worst {worst})",
            started.elapsed().as_secs_f64(),
            pixels * 3
        );
    }
}

fn report_zoom(view: &pdf_session::PageView, page: usize, editor: &Editor) {
    println!("one viewport of tiles, by zoom:");
    let mut worst = 0.0_f64;
    for (rung, scale) in ZOOMS.iter().enumerate() {
        let Some((width, height)) = editor.page_pixels(page, *scale) else {
            continue;
        };
        let window = [0, 0, VIEWPORT.0.min(width), VIEWPORT.1.min(height)];
        let (across, down) = tiles_across((width, height));
        let started = Instant::now();
        let mut drawn = 0;
        for row in 0..down {
            for col in 0..across {
                let Some(box_pixels) = tile_box((width, height), col, row) else {
                    continue;
                };
                if box_pixels[0] >= window[2] || box_pixels[1] >= window[3] {
                    continue;
                }
                if draw_region(view, *scale, box_pixels).is_ok() {
                    drawn += 1;
                }
            }
        }
        let spent = started.elapsed().as_secs_f64();
        worst = worst.max(spent);
        println!(
            "  rung {rung:>2} at {scale:>4}: {drawn:>3} tiles for a {}x{} window : {spent:.3} s",
            window[2], window[3]
        );
    }
    println!("  worst rung: {worst:.3} s");
}

fn runs_of(view: &pdf_session::PageView, lines: &[usize]) -> BTreeSet<usize> {
    runs_of_index(&view.index, lines)
}

fn runs_of_index(index: &pdf_semantics::SemanticIndex, lines: &[usize]) -> BTreeSet<usize> {
    let mut named = BTreeSet::new();
    for line in lines {
        let Some(line) = index.lines.get(*line) else {
            continue;
        };
        for cluster in &line.clusters {
            named.insert(index.clusters[*cluster].atom);
        }
    }
    named
}

fn report_block_identity(
    editor: &Editor,
    page: usize,
    blocks: &[(usize, Vec<String>, BTreeSet<usize>)],
) {
    let (Some(original), credential) = (editor.source().cloned(), editor.credential().to_vec())
    else {
        println!("block identity: the document is busy");
        return;
    };
    let mut tally = Identity::default();
    for (which, anchors, before) in blocks.iter().take(BLOCKS_ASKED) {
        let Ok(mut fresh) = Editor::open_with(original.clone(), &credential) else {
            println!("block identity: the document does not re-open");
            return;
        };
        let Some(Ok(view)) = fresh.source().map(|source| {
            pdf_session::interpret_page_fully(
                source,
                page,
                &credential,
                None,
                pdf_cli::font_provider(),
            )
        }) else {
            println!("block identity: the page does not re-read");
            return;
        };
        fresh.adopt_page(page, Arc::new(view));
        check_one_block(&mut fresh, page, (*which, anchors, before), &mut tally);
    }
    if tally.asked == 0 {
        println!("block identity: no block on this page can be moved");
        return;
    }
    println!(
        "block identity: {} of {} blocks moved and read back -- {} kept every run they went in \
         with, {} did not. Inferring the grouping again instead would have changed {}. \
         Clusters the grouping did not name: {}.",
        tally.asked,
        blocks.len().min(BLOCKS_ASKED),
        tally.kept,
        tally.changed,
        tally.inference_would_change,
        tally.strangers
    );
}

const BLOCKS_ASKED: usize = 10;

#[derive(Default)]
struct Identity {
    asked: usize,
    kept: usize,
    changed: usize,
    inference_would_change: usize,
    strangers: usize,
}

fn check_one_block(
    editor: &mut Editor,
    page: usize,
    block: (usize, &[String], &BTreeSet<usize>),
    tally: &mut Identity,
) {
    let (which, anchors, before) = block;
    let mut moved = false;
    for offset in [120.0, 60.0, 30.0, 12.0] {
        let Some(job) = editor.begin_move_block(page, anchors, 0.0, offset) else {
            return;
        };
        if matches!(editor.adopt(job.run()), Applied::Changed { .. }) {
            moved = true;
            break;
        }
    }
    if !moved {
        return;
    }
    tally.asked += 1;
    let grouping = editor.grouping(page);
    let after = match editor.source().map(|source| {
        pdf_session::interpret_page_fully(
            source,
            page,
            editor.credential(),
            grouping.as_deref(),
            pdf_cli::font_provider(),
        )
    }) {
        Some(Ok(view)) => Arc::new(view),
        Some(Err(error)) => {
            println!(
                "  block {}: the edited page does not re-read -- {error}",
                which + 1
            );
            return;
        }
        None => return,
    };
    tally.strangers += after.index.report.clusters_outside_the_grouping;
    let inferred = pdf_semantics::SemanticIndex::of(&after.graph);
    let widest = (0..inferred.blocks.len())
        .map(|block| runs_of_index(&inferred, &inferred.blocks[block].lines))
        .max_by_key(|held| held.intersection(before).count())
        .unwrap_or_default();
    if &widest != before {
        tally.inference_would_change += 1;
        println!(
            "  block {}: {} runs -> inferring again gives {} of them plus {} that are not its own",
            which + 1,
            before.len(),
            widest.intersection(before).count(),
            widest.difference(before).count()
        );
    }
    editor.adopt_page(page, after);
    let held = editor
        .leaf(page)
        .and_then(|leaf| {
            let block = leaf.overlay.blocks.get(which)?;
            Some(runs_of(&leaf.view, &block.lines))
        })
        .unwrap_or_default();
    if &held == before {
        tally.kept += 1;
    } else {
        tally.changed += 1;
        println!(
            "  block {}: GROUPING FAILED, {} runs before, {} after",
            which + 1,
            before.len(),
            held.len()
        );
    }
}

fn report_block_move(editor: &mut Editor, page: usize, anchors: &[String]) -> bool {
    let begun = Instant::now();
    let Some(job) = editor.begin_move_block(page, anchors, 7.0, -5.0) else {
        println!("the editor was busy");
        return false;
    };
    let begun = begun.elapsed();
    let ran = Instant::now();
    let outcome = job.run();
    let ran = ran.elapsed();
    let adopted = Instant::now();
    let applied = editor.adopt(outcome);
    let adopted = adopted.elapsed();
    report_applied(
        "moved a block of",
        anchors.len(),
        &applied,
        begun,
        ran,
        adopted,
    );
    if matches!(applied, Applied::Changed { .. }) {
        let undone = Instant::now();
        let applied = editor.undo();
        println!(
            "undone: {} in {:.3} s",
            describe(&applied),
            undone.elapsed().as_secs_f64()
        );
        return true;
    }
    false
}

fn report_move(editor: &mut Editor, page: usize, anchor: &str) -> bool {
    let begun = Instant::now();
    let Some(job) = editor.begin_move(page, anchor, 10.0, 0.0) else {
        println!("the editor was busy");
        return false;
    };
    let begun = begun.elapsed();
    let ran = Instant::now();
    let outcome = job.run();
    let ran = ran.elapsed();
    let adopted = Instant::now();
    let applied = editor.adopt(outcome);
    let adopted = adopted.elapsed();
    report_applied("moved", 1, &applied, begun, ran, adopted);
    if matches!(applied, Applied::Changed { .. }) {
        let undone = Instant::now();
        let applied = editor.undo();
        println!(
            "undone: {} in {:.3} s",
            describe(&applied),
            undone.elapsed().as_secs_f64()
        );
        return true;
    }
    false
}

fn report_applied(
    what: &str,
    runs: usize,
    applied: &Applied,
    begun: std::time::Duration,
    ran: std::time::Duration,
    adopted: std::time::Duration,
) {
    match applied {
        Applied::Changed { page, region } => {
            println!(
                "{what} {runs} run(s) on page {}: declared {}",
                page + 1,
                match region {
                    Some([x0, y0, x1, y1]) => format!("{:.1}x{:.1} points", x1 - x0, y1 - y0),
                    None => "no region".to_owned(),
                }
            );
            println!(
                "the interface pays {:.3} ms of it (begin {:.3} + adopt {:.3}); a worker pays {:.0} ms",
                (begun + adopted).as_secs_f64() * 1000.0,
                begun.as_secs_f64() * 1000.0,
                adopted.as_secs_f64() * 1000.0,
                ran.as_secs_f64() * 1000.0
            );
        }
        Applied::Refused(reason) => println!("refused: {reason}"),
        Applied::Unchanged => println!("{what}: nothing changed"),
    }
}

fn report_export(editor: &mut Editor, original: &Path, destination: Option<&Path>) -> ExitCode {
    let exported = Instant::now();
    match editor.export() {
        Ok(export) => {
            println!(
                "export: {} bytes, {} glyph placements checked, in {:.3} s",
                export.bytes.len(),
                export.placements,
                exported.elapsed().as_secs_f64()
            );
            let Some(destination) = destination else {
                return ExitCode::SUCCESS;
            };
            if destination.exists() && !is_a_copy(destination) {
                return fail(&format!(
                    "{} exists and was not written by this tool",
                    destination.display()
                ));
            }
            match save_file::save(original, destination, &export.bytes, None) {
                Ok(_) => {
                    println!("written: {}", destination.display());
                    ExitCode::SUCCESS
                }
                Err(error) => fail(&format!("{}: {error}", destination.display())),
            }
        }
        Err(reason) => fail(&reason),
    }
}

fn describe(applied: &Applied) -> String {
    match applied {
        Applied::Changed { page, .. } => format!("page {} changed", page + 1),
        Applied::Unchanged => "nothing changed".to_owned(),
        Applied::Refused(reason) => format!("refused: {reason}"),
    }
}
