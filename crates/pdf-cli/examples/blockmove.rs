use std::collections::BTreeSet;
use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::plan::{Capability, Command, SourceAnchor};
use pdf_paint::{Matrix, PaintAtomKind, PaintGraph, Point};

fn origins(graph: &PaintGraph) -> Vec<(usize, Point)> {
    let mut found = Vec::new();
    for (ordinal, atom) in graph.atoms.iter().enumerate() {
        let PaintAtomKind::Text(text) = &atom.kind else {
            continue;
        };
        for glyph in &text.glyphs {
            let ctm: Matrix = text.state.ctm.value;
            found.push((
                ordinal,
                ctm.transform(Point {
                    x: glyph.matrix.e,
                    y: glyph.matrix.f,
                }),
            ));
        }
    }
    found
}

const OFFSET: (f64, f64) = (7.0, -5.0);

fn main() {
    let dir = std::env::args().nth(1).expect("dir");
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .expect("readdir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")))
        .collect();
    paths.sort();
    for path in paths {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
        let Ok(view) = pdf_session::interpret_page(&source, 0) else {
            println!("FILEERR\t{name}");
            continue;
        };
        report_page(&name, &source, &view);
    }
}

fn report_page(name: &str, source: &ByteStore, view: &pdf_session::PageView) {
    let index = pdf_semantics::SemanticIndex::of(&view.graph);
    let before = origins(&view.graph);
    for (bi, block) in index.blocks.iter().enumerate() {
        let mut named: BTreeSet<usize> = BTreeSet::new();
        for &li in &block.lines {
            for &ci in &index.lines[li].clusters {
                named.insert(index.clusters[ci].atom);
            }
        }
        if named.is_empty() {
            continue;
        }
        report_block(name, source, view, &before, bi, block, &named);
    }
}

fn report_block(
    name: &str,
    source: &ByteStore,
    view: &pdf_session::PageView,
    before: &[(usize, Point)],
    bi: usize,
    block: &pdf_semantics::Block,
    named: &BTreeSet<usize>,
) {
    let (dx, dy) = OFFSET;
    {
        let runs: Vec<SourceAnchor> = named
            .iter()
            .map(|o| SourceAnchor::of(&view.graph.atoms[*o].id))
            .collect();
        let planned = pdf_edit::spike_move_text::plan_command_in(
            source,
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
        );
        let plan = match planned {
            Ok(plan) => plan,
            Err(error) => {
                println!(
                    "REFUSED\t{name}\t{bi}\tnruns={}\tnlines={}\t{error:?}",
                    named.len(),
                    block.lines.len()
                );
                return;
            }
        };
        let capability = match plan.capability() {
            Capability::Exact => "Exact",
            Capability::Normalized => "Normalized",
        };
        let region = plan.effect().declared_region;
        let committed = match plan.commit(source, b"") {
            Ok(committed) => committed,
            Err(error) => {
                println!("COMMITERR\t{name}\t{bi}\t{error:?}");
                return;
            }
        };
        let Ok(after_view) = pdf_session::interpret_page(&committed, 0) else {
            println!("REREADERR\t{name}\t{bi}");
            return;
        };
        let after = origins(&after_view.graph);
        if after.len() != before.len() {
            println!(
                "GLYPHCOUNT\t{name}\t{bi}\t{} -> {}",
                before.len(),
                after.len()
            );
            return;
        }
        let mut worst_named = 0.0_f64;
        let mut worst_other = 0.0_f64;
        for ((ordinal, was), (_, now)) in before.iter().zip(&after) {
            let (mx, my) = (now.x - was.x, now.y - was.y);
            let (wx, wy) = if named.contains(ordinal) {
                (dx, dy)
            } else {
                (0.0, 0.0)
            };
            let error = (mx - wx).abs().max((my - wy).abs());
            if named.contains(ordinal) {
                worst_named = worst_named.max(error);
            } else {
                worst_other = worst_other.max(error);
            }
        }
        let contains = region.map(|r| {
            let mut ok = true;
            for ordinal in named {
                for graph in [&view.graph, &after_view.graph] {
                    if let PaintAtomKind::Text(t) = &graph.atoms[*ordinal].kind
                        && let Some(b) = t.outline_bounds()
                    {
                        ok &= r[0] <= b[0] && r[1] <= b[1] && r[2] >= b[2] && r[3] >= b[3];
                    }
                }
            }
            ok
        });
        println!(
            "MOVED\t{name}\t{bi}\tnruns={}\tnlines={}\tcap={capability}\tworst_named={worst_named:.3e}\tworst_other={worst_other:.3e}\tregion={}\tcontains={contains:?}",
            named.len(),
            block.lines.len(),
            region.is_some(),
        );
    }
}
