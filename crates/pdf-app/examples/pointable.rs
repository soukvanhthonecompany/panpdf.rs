use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_paint::PaintAtomKind;

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

fn main() {
    let target = std::env::args().nth(1).expect("a directory or a file");
    let page_number = std::env::args()
        .skip_while(|argument| argument != "--page")
        .nth(1)
        .and_then(|value| value.parse::<usize>().ok())
        .map_or(0, |value| value.saturating_sub(1));
    let target = std::path::PathBuf::from(target);
    let mut paths: Vec<_> = if target.is_dir() {
        std::fs::read_dir(&target)
            .expect("readdir")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")))
            .collect()
    } else {
        vec![target]
    };
    paths.sort();

    let (mut pages, mut with_type3) = (0_usize, 0_usize);
    let (mut rows, mut blocks, mut clusters) = (0_usize, 0_usize, 0_usize);
    let (mut lost_rows, mut lost_blocks, mut type3_clusters) = (0_usize, 0_usize, 0_usize);
    let mut worst: Option<(String, usize, usize)> = None;
    for path in &paths {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
        let Ok(view) = pdf_session::interpret_page(&source, page_number) else {
            continue;
        };
        pages += 1;
        let index = &view.index;
        let unmeasurable: Vec<bool> = view
            .graph
            .atoms
            .iter()
            .map(|atom| match &atom.kind {
                PaintAtomKind::Text(text) => text.type3 && text.program.is_none(),
                _ => false,
            })
            .collect();
        let all_type3 = |line: &usize| {
            index.lines[*line]
                .clusters
                .iter()
                .all(|cluster| unmeasurable[index.clusters[*cluster].atom])
        };
        let here_rows = index
            .lines
            .iter()
            .enumerate()
            .filter(|(l, _)| all_type3(l))
            .count();
        let here_blocks = index
            .blocks
            .iter()
            .filter(|block| block.lines.iter().all(all_type3))
            .count();
        rows += index.lines.len();
        blocks += index.blocks.len();
        clusters += index.clusters.len();
        type3_clusters += index
            .clusters
            .iter()
            .filter(|cluster| unmeasurable[cluster.atom])
            .count();
        lost_rows += here_rows;
        lost_blocks += here_blocks;
        if here_blocks > 0 {
            with_type3 += 1;
            let name = path
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
            if worst
                .as_ref()
                .is_none_or(|(_, blocks, _)| here_blocks > *blocks)
            {
                worst = Some((name, here_blocks, index.blocks.len()));
            }
        }
    }
    println!("pages: {pages} (page {} of each)", page_number + 1);
    println!(
        "clusters drawn by a Type 3 font: {type3_clusters} of {clusters} ({:.1}%)",
        percent(type3_clusters, clusters)
    );
    println!(
        "rows that would have no box without Type 3 geometry: {lost_rows} of {rows} ({:.1}%)",
        percent(lost_rows, rows)
    );
    println!(
        "blocks that would have no box, and so no frame, no hit target and no caret: \
         {lost_blocks} of {blocks} ({:.1}%), on {with_type3} pages",
        percent(lost_blocks, blocks)
    );
    if let Some((name, lost, all)) = worst {
        println!("worst page: {name}, {lost} of its {all} blocks");
    }
}
