use std::collections::BTreeSet;
use std::sync::Arc;

use pdf_app::view::{Step, caret_at, caret_at_in, caret_step, caret_step_in};
use pdf_bytes::{ByteStore, SourceId};
use pdf_cli::{CaretStop, PageOverlay, TextBlockBox};

fn percent(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        return 0.0;
    }
    let (part, whole) = (
        u32::try_from(part).unwrap_or(u32::MAX),
        u32::try_from(whole).unwrap_or(u32::MAX),
    );
    100.0 * f64::from(part) / f64::from(whole)
}

const ROW_APART: f64 = 0.5;

const BLANK_MARGIN: f64 = 20.0;

#[derive(Default)]
struct Tally {
    blocks: usize,
    clicks_astray: usize,
    scoped_clicks_astray: usize,
    clicks_on_ink: usize,
    clicks_landed: usize,
    missed_stacked: usize,
    missed_tiny: usize,
    missed_row: usize,
    steps: usize,
    steps_astray: usize,
    tall_steps: usize,
    tall_astray: usize,
    scoped_steps_astray: usize,
    offered: usize,
    moved: usize,
    backwards: usize,
    blank_cases: usize,
    blank_old_wrong: usize,
    blank_new_wrong: usize,
}

impl Tally {
    fn absorb(&mut self, other: &Self) {
        self.blocks += other.blocks;
        self.clicks_astray += other.clicks_astray;
        self.scoped_clicks_astray += other.scoped_clicks_astray;
        self.clicks_on_ink += other.clicks_on_ink;
        self.clicks_landed += other.clicks_landed;
        self.missed_stacked += other.missed_stacked;
        self.missed_tiny += other.missed_tiny;
        self.missed_row += other.missed_row;
        self.steps += other.steps;
        self.steps_astray += other.steps_astray;
        self.tall_steps += other.tall_steps;
        self.tall_astray += other.tall_astray;
        self.scoped_steps_astray += other.scoped_steps_astray;
        self.offered += other.offered;
        self.moved += other.moved;
        self.backwards += other.backwards;
        self.blank_cases += other.blank_cases;
        self.blank_old_wrong += other.blank_old_wrong;
        self.blank_new_wrong += other.blank_new_wrong;
    }
}

fn old_nearest_middle_in(stops: &[CaretStop], rows: &[usize], point: (f64, f64)) -> Option<usize> {
    let middle = |stop: &CaretStop| {
        (
            stop.up[0].mul_add(0.5, stop.at[0]),
            stop.up[1].mul_add(0.5, stop.at[1]),
        )
    };
    stops
        .iter()
        .enumerate()
        .filter(|(_, stop)| rows.contains(&stop.line))
        .min_by(|(_, a), (_, b)| {
            let (ma, mb) = (middle(a), middle(b));
            let (da, db) = (
                (ma.0 - point.0).hypot(ma.1 - point.1),
                (mb.0 - point.0).hypot(mb.1 - point.1),
            );
            da.total_cmp(&db)
        })
        .map(|(index, _)| index)
}

fn blank_space(overlay: &PageOverlay, block: &TextBlockBox, tally: &mut Tally) {
    let widest = block
        .lines
        .iter()
        .flat_map(|&line| overlay.carets.iter().filter(move |s| s.line == line))
        .map(|s| s.at[0])
        .fold(f64::NEG_INFINITY, f64::max);
    if !widest.is_finite() {
        return;
    }
    for &line in &block.lines {
        let on_row: Vec<&CaretStop> = overlay.carets.iter().filter(|s| s.line == line).collect();
        let Some(last) = on_row.iter().max_by_key(|s| s.offset) else {
            continue;
        };
        if last.up[0].abs() > last.up[1].abs() * 0.05 {
            continue;
        }
        if widest - last.at[0] < BLANK_MARGIN {
            continue;
        }
        let point = (widest - 1.0, last.at[1]);
        tally.blank_cases += 1;
        if let Some(old) = old_nearest_middle_in(&overlay.carets, &block.lines, point)
            && overlay.carets[old].line != line
        {
            tally.blank_old_wrong += 1;
        }
        if let Some(new) = caret_at_in(&overlay.carets, &block.lines, point)
            && overlay.carets[new].line != line
        {
            tally.blank_new_wrong += 1;
        }
    }
}

fn clicks(overlay: &PageOverlay, block: &TextBlockBox, rows: &BTreeSet<usize>, tally: &mut Tally) {
    let b = block.box_pixels;
    let centre = (f64::midpoint(b[0], b[2]), f64::midpoint(b[1], b[3]));
    if let Some(index) = caret_at(&overlay.carets, centre)
        && !rows.contains(&overlay.carets[index].line)
    {
        tally.clicks_astray += 1;
    }
    if let Some(index) = caret_at_in(&overlay.carets, &block.lines, centre)
        && !rows.contains(&overlay.carets[index].line)
    {
        tally.scoped_clicks_astray += 1;
    }
    for cluster in overlay
        .clusters
        .iter()
        .filter(|cluster| rows.contains(&cluster.line))
    {
        let Some(box_of) = cluster.box_pixels else {
            continue;
        };
        tally.clicks_on_ink += 1;
        let on_it = (
            f64::midpoint(box_of[0], box_of[2]),
            f64::midpoint(box_of[1], box_of[3]),
        );
        let Some(index) = caret_at_in(&overlay.carets, &block.lines, on_it) else {
            continue;
        };
        let stop = &overlay.carets[index];
        if stop.line == cluster.line
            && (stop.offset == cluster.index_in_line || stop.offset == cluster.index_in_line + 1)
        {
            tally.clicks_landed += 1;
        } else {
            if cluster.stacked {
                tally.missed_stacked += 1;
            }
            if box_of[2] - box_of[0] < 3.0 || box_of[3] - box_of[1] < 3.0 {
                tally.missed_tiny += 1;
            }
            if stop.line != cluster.line {
                tally.missed_row += 1;
            }
        }
    }
}

fn steps(overlay: &PageOverlay, block: &TextBlockBox, rows: &BTreeSet<usize>, tally: &mut Tally) {
    for (index, stop) in overlay.carets.iter().enumerate() {
        if !rows.contains(&stop.line) {
            continue;
        }
        for step in [Step::Up, Step::Down] {
            tally.steps += 1;
            if rows.len() > 1 {
                tally.tall_steps += 1;
            }
            let unscoped = caret_step(&overlay.carets, index, step);
            if unscoped != index && !rows.contains(&overlay.carets[unscoped].line) {
                tally.steps_astray += 1;
                if rows.len() > 1 {
                    tally.tall_astray += 1;
                }
            }
            let scoped = caret_step_in(&overlay.carets, &block.lines, index, step);
            if scoped != index && !rows.contains(&overlay.carets[scoped].line) {
                tally.scoped_steps_astray += 1;
            }
            let somewhere = overlay.carets.iter().any(|other| {
                rows.contains(&other.line)
                    && other.line != stop.line
                    && match step {
                        Step::Up => other.at[1] < stop.at[1] - ROW_APART,
                        _ => other.at[1] > stop.at[1] + ROW_APART,
                    }
            });
            if rows.len() > 1 && somewhere {
                tally.offered += 1;
                if scoped != index {
                    tally.moved += 1;
                }
            }
            if scoped == index {
                continue;
            }
            let gap = overlay.carets[scoped].at[1] - stop.at[1];
            if gap.abs() > ROW_APART && ((step == Step::Up) != (gap < 0.0)) {
                tally.backwards += 1;
            }
        }
    }
}

fn main() {
    let dir = std::env::args().nth(1).expect("dir");
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .expect("readdir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")))
        .collect();
    paths.sort();

    let mut total = Tally::default();
    let mut pages = 0_usize;
    for path in &paths {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
        let Ok(view) = pdf_session::interpret_page(&source, 0) else {
            continue;
        };
        let Ok(overlay) = pdf_cli::page_overlay_view(&view, 1.0) else {
            continue;
        };
        pages += 1;
        for block in &overlay.blocks {
            let rows: BTreeSet<usize> = block.lines.iter().copied().collect();
            let mut tally = Tally {
                blocks: 1,
                ..Tally::default()
            };
            clicks(&overlay, block, &rows, &mut tally);
            steps(&overlay, block, &rows, &mut tally);
            blank_space(&overlay, block, &mut tally);
            total.absorb(&tally);
        }
    }
    println!("pages: {pages}   blocks: {}", total.blocks);
    println!("searching the whole page, as the window used to:");
    println!(
        "   clicks in a block's middle landing outside it : {}  ({:.2}%)",
        total.clicks_astray,
        percent(total.clicks_astray, total.blocks)
    );
    println!(
        "   up/down steps leaving the block               : {} of {}  ({:.2}%)",
        total.steps_astray,
        total.steps,
        percent(total.steps_astray, total.steps)
    );
    println!(
        "      of those, in blocks of more than one row   : {} of {}",
        total.tall_astray, total.tall_steps
    );
    println!("\nscoped to the block, as the window does now:");
    println!(
        "   clicks landing outside the block              : {} (a restatement of the filter)",
        total.scoped_clicks_astray
    );
    println!(
        "   clicks on a glyph landing beside it           : {} of {}  ({:.1}%)",
        total.clicks_landed,
        total.clicks_on_ink,
        percent(total.clicks_landed, total.clicks_on_ink)
    );
    println!(
        "      of the misses: {} stacked, {} tiny (<3px), {} on another row",
        total.missed_stacked, total.missed_tiny, total.missed_row
    );
    println!(
        "   up/down steps leaving the block               : {}",
        total.scoped_steps_astray
    );
    println!(
        "   steps with a row to reach that reached it     : {} of {}  ({:.1}%)",
        total.moved,
        total.offered,
        percent(total.moved, total.offered)
    );
    println!(
        "   steps that moved the wrong way down the page  : {}",
        total.backwards
    );
    println!("\nblank space beside a short row (DEFECT 1, 2026-09-22):");
    println!("   clicks tried: {}", total.blank_cases);
    println!(
        "   old rule (nearest to any stop's middle)  landed on the wrong row: {} ({:.2}%)",
        total.blank_old_wrong,
        percent(total.blank_old_wrong, total.blank_cases)
    );
    println!(
        "   new rule (nearest row, then nearest stop) landed on the wrong row: {} ({:.2}%)",
        total.blank_new_wrong,
        percent(total.blank_new_wrong, total.blank_cases)
    );
}
