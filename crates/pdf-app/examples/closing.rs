use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::Command;

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

fn shape(source: &ByteStore) -> Option<(usize, usize)> {
    let view = pdf_session::interpret_page(source, 0).ok()?;
    Some((view.index.lines.len(), view.index.blocks.len()))
}

fn attempt(
    source: &ByteStore,
    view: &pdf_cli::PageView,
    line: usize,
    from: usize,
    to: usize,
) -> (Option<(bool, ByteStore)>, Option<String>) {
    let mut why = None;
    for close_gap in [true, false] {
        let target = match pdf_cli::page_selection_between_view(view, line, from, to, close_gap) {
            Ok(target) => target,
            Err(reason) => {
                why = Some(reason);
                continue;
            }
        };
        let mut session = pdf_cli::Session::new(source.clone(), b"");
        match session.plan(&Command::RewriteText {
            page_index: 0,
            runs: target.runs,
        }) {
            Ok(plan) => match plan.commit(source, b"") {
                Ok(edited) => return (Some((close_gap, edited)), None),
                Err(error) => why = Some(error.to_string()),
            },
            Err(error) => why = Some(error.to_string()),
        }
    }
    (None, why)
}

fn main() {
    let dir = std::env::args().nth(1).expect("dir");
    let take: usize = std::env::args()
        .skip_while(|argument| argument != "--clusters")
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(10);
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .expect("readdir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")))
        .collect();
    paths.sort();

    let (mut asked, mut closed, mut left, mut refused) = (0_usize, 0, 0, 0);
    let (mut kept_shape, mut split) = (0_usize, 0_usize);
    let mut reasons: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for path in &paths {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
        let Ok(view) = pdf_session::interpret_page(&source, 0) else {
            continue;
        };
        let Some((line, width)) = view
            .index
            .lines
            .iter()
            .enumerate()
            .map(|(line, row)| (line, row.clusters.len()))
            .max_by_key(|(_, width)| *width)
            .filter(|(_, width)| *width > take + 2)
        else {
            continue;
        };
        let from = width / 4;
        let to = from + take;
        asked += 1;
        let Some(before) = shape(&source) else {
            continue;
        };

        let (done, why) = attempt(&source, &view, line, from, to);
        let Some((close_gap, edited)) = done else {
            refused += 1;
            *reasons
                .entry(why.unwrap_or_else(|| "?".to_owned()))
                .or_insert(0) += 1;
            continue;
        };
        if close_gap {
            closed += 1;
        } else {
            left += 1;
        }
        match shape(&edited) {
            Some(after) if after == before => kept_shape += 1,
            Some(_) => split += 1,
            None => {}
        }
    }
    println!("pages asked to delete {take} clusters from the middle of their longest row: {asked}");
    println!(
        "  the gap closed:        {closed} ({:.1}%)",
        percent(closed, asked)
    );
    println!(
        "  the gap was left:      {left} ({:.1}%)",
        percent(left, asked)
    );
    println!(
        "  refused entirely:      {refused} ({:.1}%)",
        percent(refused, asked)
    );
    let mut named: Vec<(&String, &usize)> = reasons.iter().collect();
    named.sort_by_key(|(_, count)| std::cmp::Reverse(**count));
    for (reason, count) in named {
        println!("    {count} x {reason}");
    }
    let edited = closed + left;
    println!(
        "of the {edited} that were edited, {kept_shape} kept the page's rows and blocks exactly \
         ({:.1}%) and {split} split one of them",
        percent(kept_shape, edited)
    );
}
