use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId, SourceSpan};
use pdf_paint::{PaintAtomKind, PaintGraph};

#[derive(Clone, Debug)]
struct Seen {
    is_path: bool,
    section: Option<SourceSpan>,
    extent: Option<[f64; 4]>,
}

#[derive(Debug, Default, PartialEq)]
struct Verdict {
    paths: usize,
    unmarked: usize,
    mixed: usize,
    grouped: usize,
    scoped: usize,
    scoped_alone: usize,
    runs: usize,
    rules: usize,
    substantial: usize,
    sections: Vec<usize>,
    scopes: Vec<usize>,
}

fn classify(seen: &[Seen], scopes: &[Range<usize>]) -> Verdict {
    let mut verdict = Verdict::default();

    let mut holds: HashMap<SourceSpan, (usize, usize)> = HashMap::new();
    for atom in seen {
        let Some(key) = atom.section else { continue };
        let entry = holds.entry(key).or_insert((0, 0));
        if atom.is_path {
            entry.0 += 1;
        } else {
            entry.1 += 1;
        }
    }

    let mut innermost: HashMap<usize, Range<usize>> = HashMap::new();
    for scope in scopes {
        if scope.end > seen.len() || !seen[scope.clone()].iter().all(|atom| atom.is_path) {
            continue;
        }
        for atom in scope.clone() {
            innermost
                .entry(atom)
                .and_modify(|held| {
                    if scope.len() < held.len() {
                        *held = scope.clone();
                    }
                })
                .or_insert_with(|| scope.clone());
        }
    }

    let mut previous_was_path = false;
    for (position, atom) in seen.iter().enumerate() {
        if atom.is_path && !previous_was_path {
            verdict.runs += 1;
        }
        previous_was_path = atom.is_path;
        if !atom.is_path {
            continue;
        }
        verdict.paths += 1;
        match atom.section.and_then(|key| holds.get(&key)) {
            None => verdict.unmarked += 1,
            Some((_, 0)) => verdict.grouped += 1,
            Some(_) => verdict.mixed += 1,
        }
        if let Some(scope) = innermost.get(&position) {
            verdict.scoped += 1;
            if scope.len() == 1 {
                verdict.scoped_alone += 1;
            }
        }
        if let Some([x0, y0, x1, y1]) = atom.extent {
            let (width, height) = (x1 - x0, y1 - y0);
            if width < 3.0 || height < 3.0 {
                verdict.rules += 1;
            } else if width * height >= 72.0 * 72.0 {
                verdict.substantial += 1;
            }
        }
    }

    for (paths_in, others_in) in holds.values() {
        if *others_in == 0 && *paths_in > 0 {
            verdict.sections.push(*paths_in);
        }
    }
    let mut distinct: Vec<Range<usize>> = innermost.into_values().collect();
    distinct.sort_by_key(|scope| (scope.start, scope.end));
    distinct.dedup();
    verdict.scopes = distinct.iter().map(Range::len).collect();
    verdict
}

fn seen_of(graph: &PaintGraph) -> Vec<Seen> {
    graph
        .atoms
        .iter()
        .map(|atom| Seen {
            is_path: matches!(atom.kind, PaintAtomKind::Path(_)),
            section: atom.marks.last().map(|mark| mark.operator_span),
            extent: atom.kind.user_bounds(),
        })
        .collect()
}

#[derive(Default)]
struct Tally {
    pages: usize,
    counted: Verdict,
    tags: HashMap<String, usize>,
}

impl Tally {
    fn add(&mut self, verdict: Verdict) {
        let into = &mut self.counted;
        into.paths += verdict.paths;
        into.unmarked += verdict.unmarked;
        into.mixed += verdict.mixed;
        into.grouped += verdict.grouped;
        into.scoped += verdict.scoped;
        into.scoped_alone += verdict.scoped_alone;
        into.runs += verdict.runs;
        into.rules += verdict.rules;
        into.substantial += verdict.substantial;
        into.sections.extend(verdict.sections);
        into.scopes.extend(verdict.scopes);
    }
}

fn measure(path: &PathBuf, tally: &mut Tally, verbose: bool) {
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
    let Ok(view) = pdf_session::interpret_page(&source, 0) else {
        return;
    };
    tally.pages += 1;
    let seen = seen_of(&view.graph);
    let scopes: Vec<Range<usize>> = view
        .graph
        .object_scopes
        .iter()
        .map(|scope| scope.atoms.clone())
        .collect();
    let verdict = classify(&seen, &scopes);
    for atom in &view.graph.atoms {
        if !matches!(atom.kind, PaintAtomKind::Path(_)) {
            continue;
        }
        if let Some(mark) = atom.marks.last() {
            *tally
                .tags
                .entry(String::from_utf8_lossy(&mark.tag).into_owned())
                .or_default() += 1;
        }
    }
    if verbose {
        println!(
            "{}: {} paths -- {} in a pure section, {} in a mixed one, {} unmarked; \
             {} in a pure q/Q scope, {} of those alone in it",
            path.display(),
            verdict.paths,
            verdict.grouped,
            verdict.mixed,
            verdict.unmarked,
            verdict.scoped,
            verdict.scoped_alone
        );
    }
    tally.add(verdict);
}

fn percent(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        return 0.0;
    }
    let count = |value: usize| f64::from(u32::try_from(value).unwrap_or(u32::MAX));
    100.0 * count(part) / count(whole)
}

fn spread(mut sizes: Vec<usize>) -> (usize, usize, usize) {
    sizes.sort_unstable();
    (
        sizes.len(),
        sizes.get(sizes.len() / 2).copied().unwrap_or(0),
        sizes.last().copied().unwrap_or(0),
    )
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let target = PathBuf::from(arguments.next().expect("a pdf file or a directory"));
    let summary = arguments.any(|flag| flag == "--summary");
    let mut paths: Vec<PathBuf> = if target.is_dir() {
        let mut found: Vec<PathBuf> = std::fs::read_dir(&target)
            .expect("readdir")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
            })
            .collect();
        found.sort();
        found
    } else {
        vec![target]
    };
    paths.dedup();

    let mut tally = Tally::default();
    for path in &paths {
        measure(path, &mut tally, !summary);
    }

    let counted = tally.counted;
    println!(
        "\n{} pages, {} path atoms; paint order alone would make {} runs of them",
        tally.pages, counted.paths, counted.runs
    );

    let (sections, section_median, section_largest) = spread(counted.sections);
    println!(
        "\n  in a marked-content section holding only paths: {} ({:.0}%)",
        counted.grouped,
        percent(counted.grouped, counted.paths)
    );
    println!(
        "  in a section holding text or a picture too:     {}",
        counted.mixed
    );
    println!(
        "  inside no marked-content section at all:        {}",
        counted.unmarked
    );
    println!("  such sections: {sections}; median {section_median}, largest {section_largest}");

    let (scopes, scope_median, scope_largest) = spread(counted.scopes);
    println!(
        "\n  in a q/Q scope holding only paths:              {} ({:.0}%)",
        counted.scoped,
        percent(counted.scoped, counted.paths)
    );
    println!(
        "  alone in it, and so grouped with nothing:       {} ({:.0}% of those)",
        counted.scoped_alone,
        percent(counted.scoped_alone, counted.scoped)
    );
    println!("  such scopes: {scopes}; median {scope_median}, largest {scope_largest}");

    println!(
        "\n  thinner than 3 points in one direction (a rule): {} ({:.0}%)",
        counted.rules,
        percent(counted.rules, counted.paths)
    );
    println!(
        "  covering at least a square inch:                 {}",
        counted.substantial
    );

    let mut tags: Vec<(&String, &usize)> = tally.tags.iter().collect();
    tags.sort_by(|one, other| other.1.cmp(one.1).then(one.0.cmp(other.0)));
    if !tags.is_empty() {
        println!("\n  the tags those paths are marked with:");
        for (tag, count) in tags.iter().take(6) {
            println!("  {count:>6} x /{tag}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Seen, classify};
    use pdf_bytes::{SourceId, SourceSpan};
    use std::ops::Range;

    fn span(start: usize) -> SourceSpan {
        SourceSpan::new(SourceId::new(1), start, start + 1).expect("a forward span")
    }

    fn atom(is_path: bool, section: Option<SourceSpan>, extent: [f64; 4]) -> Seen {
        Seen {
            is_path,
            section,
            extent: Some(extent),
        }
    }

    fn page() -> (Vec<Seen>, Vec<Range<usize>>) {
        let (pure, shared) = (span(10), span(20));
        let rule = [0.0, 0.0, 200.0, 1.0];
        let big = [0.0, 0.0, 100.0, 100.0];
        (
            vec![
                atom(true, Some(pure), big),
                atom(true, Some(pure), rule),
                atom(true, Some(shared), rule),
                atom(false, Some(shared), big),
                atom(true, None, big),
                atom(false, None, big),
            ],
            vec![0..2, 2..3, 2..4, 0..6],
        )
    }

    #[test]
    fn every_count_is_the_one_worked_out_by_hand() {
        let (seen, scopes) = page();
        let verdict = classify(&seen, &scopes);
        assert_eq!(verdict.paths, 4);
        assert_eq!(verdict.runs, 2, "atoms 0 to 2, and atom 4");
        assert_eq!(verdict.grouped, 2, "the section holding only atoms 0 and 1");
        assert_eq!(verdict.mixed, 1, "atom 2 shares its section with text");
        assert_eq!(verdict.unmarked, 1, "atom 4");
        assert_eq!(verdict.rules, 2, "atoms 1 and 2 are one point tall");
        assert_eq!(
            verdict.substantial, 2,
            "atoms 0 and 4; a picture is not a path"
        );
        assert_eq!(verdict.sections, vec![2]);
    }

    #[test]
    fn a_scope_that_holds_anything_else_is_not_evidence_about_paths() {
        let (seen, scopes) = page();
        let verdict = classify(&seen, &scopes);
        assert_eq!(verdict.scoped, 3);
        assert_eq!(verdict.scoped_alone, 1);
        assert_eq!(verdict.scopes, vec![2, 1]);

        let reaches_the_text = 0..4;
        assert_eq!(
            classify(&seen, std::slice::from_ref(&reaches_the_text)).scoped,
            0
        );
        let nested = classify(&seen, &[0..3, 2..3]);
        assert_eq!((nested.scoped, nested.scoped_alone), (3, 1));
    }

    #[test]
    fn a_scope_reaching_past_the_page_is_refused_rather_than_indexing_it() {
        let (seen, _) = page();
        let past_the_end = 0..99;
        assert_eq!(
            classify(&seen, std::slice::from_ref(&past_the_end)).scoped,
            0
        );
    }
}
