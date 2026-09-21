use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use pdf_bytes::{ByteStore, SourceId};
use pdf_session::Session;

impl Tally {
    fn absorb(&mut self, other: &Self) {
        self.pages += other.pages;
        self.strict += other.strict;
        self.display_only += other.display_only;
        self.refused += other.refused;
        self.files_with_display_only += other.files_with_display_only;
        self.files_unreadable += other.files_unreadable;
    }
}

#[derive(Default)]
struct Tally {
    pages: u64,
    strict: u64,
    display_only: u64,
    refused: u64,
    files_with_display_only: u64,
    files_unreadable: u64,
}

fn report(path: &PathBuf, limit: usize, tally: &mut Tally, said: &mut Vec<String>) {
    let Ok(bytes) = std::fs::read(path) else {
        tally.files_unreadable += 1;
        said.push(format!(
            "{}: UNREADABLE -- the file cannot be read",
            path.display()
        ));
        return;
    };
    let source = ByteStore::new(SourceId::new(0), bytes);
    let mut session = Session::with_fonts(source, b"", pdf_cli::font_provider());
    let count = match session.page_count() {
        Ok(count) => count,
        Err(error) => {
            tally.files_unreadable += 1;
            said.push(format!("{}: NO PAGE COUNT -- {error}", path.display()));
            return;
        }
    };
    let mut named = false;
    for index in 0..count.min(limit) {
        tally.pages += 1;
        session.forget_pages();
        if session.page(index).is_ok() {
            tally.strict += 1;
            continue;
        }
        match session.page_for_display(index) {
            Ok(view) => {
                tally.display_only += 1;
                if !named {
                    named = true;
                    tally.files_with_display_only += 1;
                }
                let dropped: std::collections::BTreeSet<String> = view
                    .graph
                    .skipped
                    .iter()
                    .map(|skip| format!("{skip}"))
                    .collect();
                said.push(format!(
                    "{} page {}: shows, {} dropped -- {}",
                    path.display(),
                    index + 1,
                    view.graph.skipped.len(),
                    dropped.into_iter().collect::<Vec<_>>().join("; ")
                ));
            }
            Err(error) => {
                tally.refused += 1;
                said.push(format!(
                    "{} page {}: REFUSED -- {error}",
                    path.display(),
                    index + 1
                ));
            }
        }
    }
}

fn main() {
    let mut arguments = std::env::args_os().skip(1).map(PathBuf::from);
    let Some(target) = arguments.next() else {
        eprintln!("usage: viewerdoor <file.pdf|directory> [max pages per file]");
        std::process::exit(2);
    };
    let limit: usize = arguments
        .next()
        .and_then(|value| value.to_str().and_then(|text| text.parse().ok()))
        .unwrap_or(usize::MAX);

    let mut paths = Vec::new();
    if target.is_dir() {
        for entry in std::fs::read_dir(&target).into_iter().flatten().flatten() {
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
            {
                paths.push(path);
            }
        }
        paths.sort();
    } else {
        paths.push(target);
    }

    let workers = std::thread::available_parallelism()
        .map_or(4, std::num::NonZeroUsize::get)
        .min(paths.len().max(1));
    let next = AtomicUsize::new(0);
    let shared: Mutex<(Tally, Vec<String>)> = Mutex::new((Tally::default(), Vec::new()));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                let mut mine = Tally::default();
                let mut said = Vec::new();
                loop {
                    let at = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = paths.get(at) else { break };
                    report(path, limit, &mut mine, &mut said);
                }
                let mut shared = shared
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                shared.0.absorb(&mine);
                shared.1.extend(said);
            });
        }
    });
    let (tally, mut said) = shared
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    said.sort();
    for line in &said {
        println!("{line}");
    }
    println!(
        "\n=== the two doors, over {} files on {workers} workers ===",
        paths.len()
    );
    println!("pages walked:                    {}", tally.pages);
    println!("opened by the strict door:       {}", tally.strict);
    println!(
        "opened only by the display door: {} over {} files",
        tally.display_only, tally.files_with_display_only
    );
    println!("opened by neither:               {}", tally.refused);
    println!(
        "files that would not open:       {}",
        tally.files_unreadable
    );
}
