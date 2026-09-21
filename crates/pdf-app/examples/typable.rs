use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};

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
    let dir = std::env::args().nth(1).expect("dir");
    let typed = std::env::args().nth(2).unwrap_or_else(|| "A".to_owned());
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .expect("readdir")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.extension()
                .is_some_and(|kind| kind.eq_ignore_ascii_case("pdf"))
        })
        .collect();
    paths.sort();

    let (mut asked, mut planned, mut committed) = (0_usize, 0_usize, 0_usize);
    let (mut again, mut carets) = (0_usize, 0_usize);
    let mut reasons: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut second: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
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
            .filter(|(_, width)| *width > 3)
        else {
            continue;
        };
        asked += 1;
        let from = width / 2;
        let runs = match pdf_cli::page_replacement_view(&view, line, from, from + 1, &typed) {
            Ok(runs) => runs,
            Err(reason) => {
                *reasons.entry(reason).or_insert(0) += 1;
                continue;
            }
        };
        planned += 1;
        let mut session = pdf_cli::Session::new(source.clone(), b"");
        match session.plan(&pdf_edit::Command::RewriteText {
            page_index: 0,
            runs,
        }) {
            Ok(plan) => match plan.commit(&source, b"") {
                Ok(edited) => {
                    committed += 1;
                    let (accepted, offered) = kept_typing(&edited, line, &typed, &mut second);
                    again += accepted;
                    carets += offered;
                    if std::env::var("TYPABLE_NAMES").is_ok() {
                        println!("  ok {}", path.display());
                    }
                }
                Err(error) => *reasons.entry(error.to_string()).or_insert(0) += 1,
            },
            Err(error) => *reasons.entry(error.to_string()).or_insert(0) += 1,
        }
    }

    println!("pages asked to retype one cluster as {typed:?}: {asked}");
    println!("  planned:   {planned} ({:.1}%)", percent(planned, asked));
    println!(
        "  committed: {committed} ({:.1}%)",
        percent(committed, asked)
    );
    let mut named: Vec<(&String, &usize)> = reasons.iter().collect();
    named.sort_by_key(|(_, count)| std::cmp::Reverse(**count));
    for (reason, count) in named {
        println!("    {count} x {reason}");
    }
    println!(
        "  caret positions on those rows that accept a second character: {again} of {carets} ({:.1}%)",
        percent(again, carets)
    );
    let mut named: Vec<(&String, &usize)> = second.iter().collect();
    named.sort_by_key(|(_, count)| std::cmp::Reverse(**count));
    for (reason, count) in named {
        println!("    {count} x {reason}");
    }
}

fn kept_typing(
    source: &ByteStore,
    line: usize,
    typed: &str,
    reasons: &mut std::collections::BTreeMap<String, usize>,
) -> (usize, usize) {
    let Ok(view) = pdf_session::interpret_page(source, 0) else {
        *reasons
            .entry("the edited page does not interpret".to_owned())
            .or_insert(0) += 1;
        return (0, 1);
    };
    let Some(row) = view.index.lines.get(line) else {
        *reasons
            .entry("the edited page lost the row that was typed on".to_owned())
            .or_insert(0) += 1;
        return (0, 1);
    };
    let stops = row.clusters.len() + 1;
    let mut accepted = 0;
    for at in 0..stops {
        let runs = match pdf_cli::page_replacement_view(&view, line, at, at, typed) {
            Ok(runs) => runs,
            Err(reason) => {
                *reasons.entry(reason).or_insert(0) += 1;
                continue;
            }
        };
        let mut session = pdf_cli::Session::new(source.clone(), b"");
        match session.plan(&pdf_edit::Command::RewriteText {
            page_index: 0,
            runs,
        }) {
            Ok(plan) => match plan.commit(source, b"") {
                Ok(_) => accepted += 1,
                Err(error) => *reasons.entry(error.to_string()).or_insert(0) += 1,
            },
            Err(error) => *reasons.entry(error.to_string()).or_insert(0) += 1,
        }
    }
    (accepted, stops)
}
