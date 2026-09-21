use std::collections::BTreeSet;
use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::plan::{Command, SourceAnchor};
use pdf_session::PageView;

fn runs_of(index: &pdf_semantics::SemanticIndex, block: usize) -> BTreeSet<usize> {
    let mut named = BTreeSet::new();
    for &line in &index.blocks[block].lines {
        for &cluster in &index.lines[line].clusters {
            named.insert(index.clusters[cluster].atom);
        }
    }
    named
}

fn anchors_of(view: &PageView, runs: &BTreeSet<usize>) -> BTreeSet<String> {
    runs.iter()
        .map(|atom| SourceAnchor::of(&view.graph.atoms[*atom].id).encode())
        .collect()
}

fn grouping_keeps(
    after: &PageView,
    grouping: &pdf_semantics::Grouping,
    block: usize,
    named: &BTreeSet<usize>,
    strangers: &mut usize,
) -> bool {
    let grouped = pdf_semantics::SemanticIndex::of_grouped(&after.graph, grouping);
    *strangers += grouped.report.clusters_outside_the_grouping;
    let held = runs_of(&grouped, block);
    if grouped.blocks.len() == grouping.block_count() && &held == named {
        return true;
    }
    println!(
        "  block {block:>3}: GROUPING FAILED, {} runs -> {}",
        named.len(),
        held.len()
    );
    false
}

fn widest_block(
    index: &pdf_semantics::SemanticIndex,
    named: &BTreeSet<usize>,
) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize)> = None;
    for candidate in 0..index.blocks.len() {
        let shared = runs_of(index, candidate).intersection(named).count();
        if best.is_none_or(|(_, most)| shared > most) {
            best = Some((candidate, shared));
        }
    }
    best
}

fn main() {
    let path = std::env::args().nth(1).expect("path");
    let dx: f64 = std::env::args().nth(2).map_or(0.0, |a| a.parse().unwrap());
    let dy: f64 = std::env::args()
        .nth(3)
        .map_or(-120.0, |a| a.parse().unwrap());
    let bytes = std::fs::read(&path).expect("read");
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
    let view = pdf_session::interpret_page(&source, 0).expect("interpret");
    let index = pdf_semantics::SemanticIndex::of(&view.graph);
    println!(
        "{path}: {} blocks, offset ({dx}, {dy}) in user space",
        index.blocks.len()
    );

    let grouping = pdf_semantics::Grouping::of(&index);
    let (mut planned, mut kept, mut grew, mut shrank, mut gone) = (0, 0, 0, 0, 0);
    let (mut kept_by_grouping, mut wrong_by_grouping, mut strangers) = (0, 0, 0);
    let mut renamed = 0;
    for block in 0..index.blocks.len() {
        let named = runs_of(&index, block);
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
                page_index: 0,
                runs,
                dx,
                dy,
            },
        ) else {
            continue;
        };
        planned += 1;
        let Ok(committed) = plan.commit(&source, b"") else {
            continue;
        };
        let Ok(after) = pdf_session::interpret_page(&committed, 0) else {
            continue;
        };
        let after_index = pdf_semantics::SemanticIndex::of(&after.graph);
        if grouping_keeps(&after, &grouping, block, &named, &mut strangers) {
            kept_by_grouping += 1;
        } else {
            wrong_by_grouping += 1;
        }
        let best = widest_block(&after_index, &named);
        let Some((candidate, shared)) = best.filter(|(_, shared)| *shared > 0) else {
            gone += 1;
            println!(
                "  block {block:>3}: {} runs -> LOST (no block holds any of them)",
                named.len()
            );
            continue;
        };
        let held = runs_of(&after_index, candidate);
        if anchors_of(&view, &named) != anchors_of(&after, &named) {
            renamed += 1;
        }
        if held == named {
            kept += 1;
        } else if held.len() > named.len() {
            grew += 1;
            println!(
                "  block {block:>3}: {} runs -> ABSORBED {} more ({} runs after)",
                named.len(),
                held.len() - shared,
                held.len()
            );
        } else {
            shrank += 1;
            println!(
                "  block {block:>3}: {} runs -> SPLIT, largest piece holds {}",
                named.len(),
                held.len()
            );
        }
    }
    println!(
        "\ninferred again after the edit -- planned {planned}   identity kept {kept}   \
         absorbed other text {grew}   split {shrank}   lost {gone}\n\
         carried by the grouping    -- identity kept {kept_by_grouping}   \
         wrong {wrong_by_grouping}   clusters the grouping did not name {strangers}\n\
         blocks whose runs answer to different anchors after the edit: {renamed}"
    );
    if wrong_by_grouping > 0 {
        std::process::exit(1);
    }
}
