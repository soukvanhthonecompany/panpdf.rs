use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};

const COLUMN_GAP_EM: f64 = 2.5;

fn piece_starts(index: &pdf_semantics::SemanticIndex, line: &pdf_semantics::Line) -> Vec<f64> {
    let mut starts = Vec::new();
    let mut last: Option<(f64, f64)> = None;
    for cluster in &line.clusters {
        let Some(cluster) = index.clusters.get(*cluster) else {
            continue;
        };
        let Some([x0, _, x1, _]) = cluster.bounds else {
            continue;
        };
        match last {
            None => starts.push(x0),
            Some((end, em)) if em > 0.0 && (x0 - end) / em > COLUMN_GAP_EM => starts.push(x0),
            Some(_) => {}
        }
        last = Some((x1, cluster.advance));
    }
    starts
}

fn main() {
    let target = std::env::args().nth(1).expect("a directory or a file");
    let page_number = std::env::args()
        .nth(2)
        .and_then(|value| value.parse::<usize>().ok())
        .map_or(0, |value| value.saturating_sub(1));
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
        for (row, line) in index.lines.iter().enumerate() {
            if line.widest_gap <= COLUMN_GAP_EM {
                continue;
            }
            let starts = piece_starts(index, line);
            let pieces = starts.len();
            let em = line
                .clusters
                .iter()
                .filter_map(|cluster| index.clusters.get(*cluster))
                .map(|cluster| cluster.em)
                .fold(0.0_f64, f64::max)
                .max(f64::MIN_POSITIVE);
            let mut aligned = 0_usize;
            for start in &starts {
                let repeats = index
                    .lines
                    .iter()
                    .enumerate()
                    .filter(|(other, _)| *other != row)
                    .filter(|(_, other)| {
                        piece_starts(index, other)
                            .iter()
                            .any(|at| (at - start).abs() < 0.25 * em)
                    })
                    .count();
                if repeats > 0 {
                    aligned += 1;
                }
            }
            println!(
                "{{\"file\":\"{name}\",\"page\":{},\"row\":{row},\"pieces\":{pieces},\
                 \"widest_gap\":{:.2},\"clusters\":{},\"aligned\":{aligned}}}",
                page_number + 1,
                line.widest_gap,
                line.clusters.len(),
            );
        }
    }
}
