use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use pdf_bytes::{ByteStore, SourceId};
use pdf_content::{PageContentLimits, count_pages_with_password};
use pdf_session::interpret_page_with;

#[derive(Default)]
struct Sweep {
    documents: usize,
    unopenable: usize,
    pages_read: usize,
    pages_failed: usize,
    documents_clean: usize,
    first_page_read: usize,
    first_page_failed: usize,
    reasons: BTreeMap<String, usize>,
    noticed: Vec<(f64, usize, usize, String)>,
}

fn main() {
    let mut arguments = std::env::args_os().skip(1);
    let Some(root) = arguments.next().map(PathBuf::from) else {
        eprintln!("usage: everypage <directory|file.pdf> [--limit N]");
        std::process::exit(2);
    };
    let mut limit = usize::MAX;
    while let Some(argument) = arguments.next() {
        if argument == "--limit" {
            limit = arguments
                .next()
                .and_then(|value| value.to_str().and_then(|value| value.parse().ok()))
                .unwrap_or(usize::MAX);
        }
    }

    let sweep = sweep(&paths_under(&root), limit);
    report(&sweep);
}

fn paths_under(root: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = if root.is_dir() {
        std::fs::read_dir(root)
            .expect("the directory reads")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().is_some_and(|kind| kind == "pdf"))
            .collect()
    } else {
        vec![root.to_path_buf()]
    };
    paths.sort();
    paths
}

fn sweep(paths: &[PathBuf], limit: usize) -> Sweep {
    let mut found = Sweep::default();
    for path in paths {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let source = ByteStore::new(SourceId::new(0), bytes);
        let Ok(count) = count_pages_with_password(&source, PageContentLimits::default(), b"")
        else {
            found.unopenable += 1;
            continue;
        };
        found.documents += 1;
        let wanted = count.min(limit);
        let mut failed_here = 0_usize;
        for page in 0..wanted {
            if page == 0 {
                found.first_page_read += 1;
            }
            if let Err(error) = interpret_page_with(&source, page, b"") {
                failed_here += 1;
                if page == 0 {
                    found.first_page_failed += 1;
                }
                *found
                    .reasons
                    .entry(shorten(&error.to_string()))
                    .or_default() += 1;
            }
            found.pages_read += 1;
        }
        found.pages_failed += failed_here;
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if failed_here == 0 {
            found.documents_clean += 1;
        } else {
            found.noticed.push((
                share(failed_here, wanted),
                failed_here,
                wanted,
                name.clone().into_owned(),
            ));
        }
        println!("{failed_here:>4}/{wanted:<4} failed  {name}");
    }
    found
        .noticed
        .sort_by(|left, right| right.0.total_cmp(&left.0));
    found
}

fn report(found: &Sweep) {
    println!("\n=== every page, not only the first ===");
    println!(
        "documents opened:              {} ({} would not open at all)",
        found.documents, found.unopenable
    );
    println!("pages read:                    {}", found.pages_read);
    println!(
        "pages that interpret:          {} ({:.1}%)",
        found.pages_read - found.pages_failed,
        100.0 * share(found.pages_read - found.pages_failed, found.pages_read)
    );
    println!(
        "pages that FAIL:               {} ({:.1}%)",
        found.pages_failed,
        100.0 * share(found.pages_failed, found.pages_read)
    );
    println!(
        "documents with no failed page: {} of {} ({:.1}%)",
        found.documents_clean,
        found.documents,
        100.0 * share(found.documents_clean, found.documents)
    );
    println!(
        "first pages that interpret:    {} ({} fail) -- the number every other measurement here uses",
        found.first_page_read - found.first_page_failed,
        found.first_page_failed
    );

    println!("\nwhy a page fails, over every page read:");
    let mut ranked: Vec<_> = found.reasons.iter().collect();
    ranked.sort_by_key(|(_, count)| std::cmp::Reverse(**count));
    for (reason, count) in ranked {
        println!("  {count:>6} x {reason}");
    }

    println!("\nthe documents a reader would notice, worst first:");
    for (share, failed, total, name) in found.noticed.iter().take(25) {
        println!(
            "  {:>5.1}%  {failed:>4} of {total:<5} {name}",
            share * 100.0
        );
    }
}

fn share(part: usize, whole: usize) -> f64 {
    let whole = whole.max(1);
    let part = u32::try_from(part).unwrap_or(u32::MAX);
    let whole = u32::try_from(whole).unwrap_or(u32::MAX);
    f64::from(part) / f64::from(whole)
}

fn shorten(reason: &str) -> String {
    reason
        .split(" at byte ")
        .next()
        .unwrap_or(reason)
        .to_owned()
}
