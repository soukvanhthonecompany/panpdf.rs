use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_paint::PaintAtomKind;

fn near(one: [f64; 4], two: [f64; 4], slack: f64) -> bool {
    let gap = |a0: f64, a1: f64, b0: f64, b1: f64| (b0 - a1).max(a0 - b1).max(0.0);
    gap(one[0], one[2], two[0], two[2]) <= slack && gap(one[1], one[3], two[1], two[3]) <= slack
}

fn state(atom: &pdf_paint::PaintAtom) -> Option<&pdf_paint::GraphicsState> {
    match &atom.kind {
        PaintAtomKind::Path(paint) => Some(&paint.state),
        _ => None,
    }
}

fn runs(ends: &[bool]) -> usize {
    1 + ends.iter().filter(|end| **end).count()
}

fn main() {
    let path = std::env::args().nth(1).expect("a PDF path");
    let page = std::env::args()
        .nth(2)
        .and_then(|value| value.parse::<usize>().ok())
        .map_or(0, |value| value.saturating_sub(1));
    let bytes = std::fs::read(&path).expect("read");
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
    let Ok(view) = pdf_session::interpret_page(&source, page) else {
        println!("the page does not read");
        return;
    };

    let mut paths: Vec<(usize, &pdf_paint::PaintAtom)> = Vec::new();
    for (position, atom) in view.graph.atoms.iter().enumerate() {
        if matches!(atom.kind, PaintAtomKind::Path(_)) {
            paths.push((position, atom));
        }
    }
    if paths.len() < 2 {
        println!("{} paths; nothing to group", paths.len());
        return;
    }

    let (mut adjacent, mut clip, mut marks, mut touch) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for pair in paths.windows(2) {
        let ((one_at, one), (two_at, two)) = (pair[0], pair[1]);
        adjacent.push(two_at != one_at + 1);
        clip.push(match (state(one), state(two)) {
            (Some(a), Some(b)) => a.clip_paths.len() != b.clip_paths.len(),
            _ => true,
        });
        marks.push(one.marks.len() != two.marks.len());
        touch.push(match (one.kind.user_bounds(), two.kind.user_bounds()) {
            (Some(a), Some(b)) => {
                let side = (a[2] - a[0]).abs().max(a[3] - a[1]).abs().max(1.0);
                !near(a, b, side / 2.0)
            }
            _ => true,
        });
    }
    let mut both = Vec::with_capacity(adjacent.len());
    let mut box_so_far = paths[0].1.kind.user_bounds();
    for (step, pair) in paths.windows(2).enumerate() {
        let next = pair[1].1.kind.user_bounds();
        let apart = adjacent[step];
        let far = match (box_so_far, next) {
            (Some(held), Some(next)) => {
                let side = (held[2] - held[0])
                    .abs()
                    .max((held[3] - held[1]).abs())
                    .max(1.0);
                let _ = side;
                !near(held, next, 1.0)
            }
            _ => true,
        };
        let _ = apart;
        both.push(far);
        box_so_far = if far {
            next
        } else {
            match (box_so_far, next) {
                (Some(a), Some(b)) => Some([
                    a[0].min(b[0]),
                    a[1].min(b[1]),
                    a[2].max(b[2]),
                    a[3].max(b[3]),
                ]),
                (Some(held), None) => Some(held),
                (None, held) => held,
            }
        };
    }
    println!(
        "{path} p{}: {} paths -> adjacent {} runs, clip {} runs, marks {} runs, touch {} runs, \
         both {} runs",
        page + 1,
        paths.len(),
        runs(&adjacent),
        runs(&clip),
        runs(&marks),
        runs(&touch),
        runs(&both),
    );
    show_runs(&paths, &both);
}

fn show_runs(paths: &[(usize, &pdf_paint::PaintAtom)], both: &[bool]) {
    let mut start = 0_usize;
    for (step, end) in both.iter().chain(std::iter::once(&true)).enumerate() {
        if !*end {
            continue;
        }
        let members = &paths[start..=step.min(paths.len() - 1)];
        let bounds = members
            .iter()
            .filter_map(|(_, atom)| atom.kind.user_bounds())
            .reduce(|a, b| {
                [
                    a[0].min(b[0]),
                    a[1].min(b[1]),
                    a[2].max(b[2]),
                    a[3].max(b[3]),
                ]
            });
        if let Some([x0, y0, x1, y1]) = bounds {
            println!(
                "   run of {:2} paths  {:7.1} {:7.1} .. {:7.1} {:7.1}",
                members.len(),
                x0,
                y0,
                x1,
                y1
            );
        }
        start = step + 1;
    }
}
