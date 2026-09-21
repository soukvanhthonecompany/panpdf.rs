use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};

fn main() {
    let path = std::env::args().nth(1).expect("a PDF path");
    let Ok(bytes) = std::fs::read(&path) else {
        println!("unreadable");
        return;
    };
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
    let view = match pdf_session::interpret_page(&source, 0) {
        Ok(view) => view,
        Err(error) => {
            println!("refused {error}");
            return;
        }
    };
    let index = &view.index;
    println!(
        "clusters {} lines {} blocks {}",
        index.clusters.len(),
        index.lines.len(),
        index.blocks.len()
    );
    if std::env::var_os("GROUPINGDIFF_REPORT").is_some() {
        eprintln!(
            "layered_apart {} refused_by_paint_order {}",
            index.report.lines_layered_apart, index.report.lines_refused_by_paint_order
        );
    }
    if let Some(y) = std::env::var("GROUPINGDIFF_ROW")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
    {
        for cluster in &index.clusters {
            if (cluster.baseline.y - y).abs() < 6.0 {
                let line = index.lines.iter().position(|l| {
                    l.clusters
                        .iter()
                        .any(|c| index.clusters[*c].name() == cluster.name())
                });
                eprintln!(
                    "atom {} glyph {} code {} x {:.1} y {:.1} adv {:.1} blank {} bounds {:?} line {:?}",
                    cluster.atom,
                    cluster.glyphs.start,
                    cluster.code,
                    cluster.baseline.x,
                    cluster.baseline.y,
                    cluster.advance,
                    cluster.blank,
                    cluster.bounds.map(|b| [b[0].round(), b[2].round()]),
                    line
                );
            }
        }
    }
    if std::env::var_os("GROUPINGDIFF_BLOCKS").is_some() {
        for (number, block) in index.blocks.iter().enumerate() {
            let rows: Vec<String> = block
                .lines
                .iter()
                .map(|line| {
                    let seed = &index.clusters[index.lines[*line].clusters[0]];
                    format!("{line}@{:.1}", seed.baseline.y)
                })
                .collect();
            eprintln!("block {number}: {}", rows.join(" "));
        }
    }
    for block in &index.blocks {
        let mut keys: Vec<(usize, usize)> = block
            .lines
            .iter()
            .flat_map(|line| index.lines[*line].clusters.iter())
            .map(|cluster| {
                let name = index.clusters[*cluster].name();
                (name.atom, name.glyph)
            })
            .collect();
        keys.sort_unstable();
        let mut hasher = DefaultHasher::new();
        keys.hash(&mut hasher);
        let bounds = block.bounds.map_or_else(
            || "-".to_owned(),
            |b| format!("{:.1},{:.1},{:.1},{:.1}", b[0], b[1], b[2], b[3]),
        );
        println!(
            "block {:016x} {} {} {bounds}",
            hasher.finish(),
            keys.len(),
            block.lines.len()
        );
    }
}
