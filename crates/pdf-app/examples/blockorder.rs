use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};

fn spread(values: &[f64]) -> f64 {
    match (
        values.iter().copied().fold(f64::INFINITY, f64::min),
        values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    ) {
        (low, high) if low.is_finite() && high.is_finite() => high - low,
        _ => 0.0,
    }
}

fn main() {
    let target = std::env::args().nth(1).expect("a directory or a file");
    let page_number = std::env::args()
        .skip_while(|argument| argument != "--page")
        .nth(1)
        .and_then(|value| value.parse::<usize>().ok())
        .map_or(0, |value| value.saturating_sub(1));
    let page_number = std::env::args()
        .nth(2)
        .filter(|value| value != "--page")
        .and_then(|value| value.parse::<usize>().ok())
        .map_or(page_number, |value| value.saturating_sub(1));
    let target = std::path::PathBuf::from(target);
    let mut paths: Vec<_> = if target.is_dir() {
        std::fs::read_dir(&target)
            .expect("readdir")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
            })
            .collect()
    } else {
        vec![target]
    };
    paths.sort();

    for path in &paths {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
        let Ok(view) = pdf_session::interpret_page(&source, page_number) else {
            continue;
        };
        let index = &view.index;
        let name = path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        for (number, block) in index.blocks.iter().enumerate() {
            report(index, &name, page_number, number, block);
        }
    }
}

fn report(
    index: &pdf_semantics::SemanticIndex,
    name: &str,
    page_number: usize,
    number: usize,
    block: &pdf_semantics::Block,
) {
    let rows = block.lines.len();
    let mut lefts = Vec::new();
    let mut widths = Vec::new();
    let mut baselines = Vec::new();
    let mut pieces = 0_usize;
    let mut atoms = std::collections::BTreeSet::new();
    let mut em = 0.0_f64;
    for line in &block.lines {
        let Some(row) = index.lines.get(*line) else {
            continue;
        };
        let mut edges: Option<(f64, f64)> = None;
        let mut last_end: Option<f64> = None;
        let mut here = 1_usize;
        for cluster in &row.clusters {
            let Some(cluster) = index.clusters.get(*cluster) else {
                continue;
            };
            atoms.insert(cluster.atom);
            em = em.max(cluster.advance);
            let Some([x0, y0, x1, _]) = cluster.bounds else {
                continue;
            };
            baselines.push(y0);
            edges = Some(match edges {
                Some((low, high)) => (low.min(x0), high.max(x1)),
                None => (x0, x1),
            });
            if let Some(end) = last_end
                && cluster.advance > 0.0
                && x0 - end > 2.5 * cluster.advance
            {
                here += 1;
            }
            last_end = Some(x1);
        }
        if let Some((low, high)) = edges {
            lefts.push(low);
            widths.push(high - low);
        }
        pieces += here;
    }
    let em = if em > 0.0 { em } else { 1.0 };
    let widest = widths.iter().copied().fold(0.0_f64, f64::max).max(1.0);
    let rights: Vec<f64> = lefts
        .iter()
        .zip(&widths)
        .map(|(left, width)| left + width)
        .collect();
    let centres: Vec<f64> = lefts
        .iter()
        .zip(&widths)
        .map(|(left, width)| left + width / 2.0)
        .collect();
    let edge = spread(&lefts).min(spread(&centres)).min(spread(&rights)) / em;
    baselines.sort_by(f64::total_cmp);
    baselines.dedup_by(|a, b| (*a - *b).abs() < 0.5);
    let steps: Vec<f64> = baselines.windows(2).map(|pair| pair[1] - pair[0]).collect();
    println!(
        "{{\"file\":\"{name}\",\"page\":{},\"block\":{number},\"rows\":{rows},\
                 \"pieces\":{pieces},\"atoms\":{},\"left_spread\":{:.2},\"width_spread\":{:.2},\
                 \"pitch_spread\":{:.2},\"edge\":{edge:.2}}}",
        page_number + 1,
        atoms.len(),
        spread(&lefts) / em,
        spread(&widths) / widest,
        spread(&steps) / em,
    );
}
