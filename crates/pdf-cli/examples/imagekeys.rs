use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pdf_bytes::ByteStore;
use pdf_content::{
    PageContentLimits, PageResources, count_pages_with_password, load_page_program_with_password,
};
use pdf_syntax::{Object, ObjectKind, Reference};

const MAX_DEPTH: usize = 16;

const ACCEPTED: &[&[u8]] = &[
    b"/Type",
    b"/Subtype",
    b"/Width",
    b"/Height",
    b"/BitsPerComponent",
    b"/ColorSpace",
    b"/Decode",
    b"/ImageMask",
    b"/Interpolate",
    b"/SMask",
    b"/Matte",
    b"/Length",
    b"/Filter",
    b"/DecodeParms",
    b"/DL",
    b"/Name",
    b"/Intent",
    b"/ColorTransform",
    b"/Metadata",
    b"/ImageName",
];

fn text(source: &ByteStore, object: &Object) -> String {
    String::from_utf8_lossy(
        source
            .as_bytes()
            .get(object.span().start()..object.span().end())
            .unwrap_or_default(),
    )
    .into_owned()
}

fn describe(source: &ByteStore, object: &Object) -> String {
    match object.kind() {
        ObjectKind::Boolean(_) | ObjectKind::Number(_) | ObjectKind::Name | ObjectKind::Null => {
            text(source, object)
        }
        ObjectKind::Array(items) => format!("[{} items]", items.len()),
        ObjectKind::Dictionary(entries) => format!("<<{} entries>>", entries.len()),
        ObjectKind::Reference(_) => "N N R".to_owned(),
        ObjectKind::LiteralString | ObjectKind::HexString => "(string)".to_owned(),
    }
}

fn entry<'a>(source: &ByteStore, object: &'a Object, key: &[u8]) -> Option<&'a Object> {
    let ObjectKind::Dictionary(entries) = object.kind() else {
        return None;
    };
    entries
        .iter()
        .find(|entry| entry.key_equals(source, key))
        .map(pdf_syntax::DictionaryEntry::value)
}

fn is_image(source: &ByteStore, object: &Object) -> bool {
    entry(source, object, b"/Subtype").is_some_and(|value| value.name_equals(source, b"/Image"))
}

fn is_form(source: &ByteStore, object: &Object) -> bool {
    entry(source, object, b"/Subtype").is_some_and(|value| value.name_equals(source, b"/Form"))
}

#[derive(Default)]
struct Tally {
    uses: BTreeMap<String, u64>,
    values: BTreeMap<String, BTreeSet<String>>,
    stops: BTreeMap<String, BTreeSet<String>>,
    images: u64,
}

impl Tally {
    fn absorb(&mut self, other: &Self) {
        for (key, count) in &other.uses {
            *self.uses.entry(key.clone()).or_default() += count;
        }
        for (key, values) in &other.values {
            let slot = self.values.entry(key.clone()).or_default();
            for value in values {
                if slot.len() < 12 {
                    slot.insert(value.clone());
                }
            }
        }
        for (key, pages) in &other.stops {
            self.stops
                .entry(key.clone())
                .or_default()
                .extend(pages.iter().cloned());
        }
        self.images += other.images;
    }
}

fn walk(
    resources: &PageResources,
    label: &str,
    page: usize,
    depth: usize,
    seen: &mut Vec<Reference>,
    tally: &mut Tally,
) {
    if depth > MAX_DEPTH {
        return;
    }
    for resource in resources.xobjects() {
        let Some(reference) = resource.reference() else {
            continue;
        };
        if seen.contains(&reference) {
            continue;
        }
        let source = resource.source();
        if is_image(source, resource.value()) {
            seen.push(reference);
            let ObjectKind::Dictionary(entries) = resource.value().kind() else {
                continue;
            };
            tally.images += 1;
            for entry in entries {
                let key = String::from_utf8_lossy(
                    source
                        .as_bytes()
                        .get(entry.key().span().start()..entry.key().span().end())
                        .unwrap_or_default(),
                )
                .into_owned();
                *tally.uses.entry(key.clone()).or_default() += 1;
                let values = tally.values.entry(key.clone()).or_default();
                if values.len() < 12 {
                    values.insert(describe(source, entry.value()));
                }
                if !ACCEPTED.contains(&key.as_bytes()) {
                    tally
                        .stops
                        .entry(key)
                        .or_default()
                        .insert(format!("{label} / {page}"));
                }
            }
        } else if is_form(source, resource.value())
            && let Some(form) = resource.form()
            && let Some(inner) = form.resources.as_ref()
        {
            seen.push(reference);
            walk(inner, label, page, depth + 1, seen, tally);
        }
    }
}

fn file_tally(path: &Path, all: bool) -> Tally {
    let mut tally = Tally::default();
    let Ok(bytes) = std::fs::read(path) else {
        return tally;
    };
    let source = ByteStore::new(pdf_bytes::SourceId::new(1), Arc::<[u8]>::from(bytes));
    let limits = PageContentLimits::default();
    let count = if all {
        count_pages_with_password(&source, limits, b"").unwrap_or(1)
    } else {
        1
    };
    let label = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into(),
    );
    for page in 0..count {
        let Ok(program) = load_page_program_with_password(&source, page, limits, b"") else {
            continue;
        };
        let mut seen = Vec::new();
        walk(
            &program.resources,
            &label,
            page + 1,
            0,
            &mut seen,
            &mut tally,
        );
    }
    tally
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let Some(target) = arguments.next().map(PathBuf::from) else {
        eprintln!("usage: imagekeys <file.pdf|directory> [--all] [--summary]");
        std::process::exit(2);
    };
    let rest: Vec<String> = arguments.collect();
    let all = rest.iter().any(|a| a == "--all");
    let summary = rest.iter().any(|a| a == "--summary");

    let mut total = Tally::default();
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
    for path in &paths {
        let tally = file_tally(path, all);
        if !summary && tally.images > 0 {
            println!("{}: {} image dictionaries", path.display(), tally.images);
        }
        total.absorb(&tally);
    }

    println!("\n=== image dictionary keys over {} files ===", paths.len());
    println!("image dictionaries read: {}", total.images);
    let mut refused: Vec<_> = total
        .stops
        .iter()
        .map(|(key, pages)| {
            (
                pages.len(),
                key.clone(),
                total.uses.get(key).copied().unwrap_or(0),
            )
        })
        .collect();
    refused.sort_by_key(|(pages, key, _)| (std::cmp::Reverse(*pages), key.clone()));
    println!("\nkeys this engine refuses, by pages they would stop:");
    for (pages, key, uses) in &refused {
        let values = total
            .values
            .get(key)
            .map(|set| set.iter().cloned().collect::<Vec<_>>().join(", "))
            .unwrap_or_default();
        println!("  {pages:>5} pages  {uses:>6} uses  {key}   values: {values}");
    }
    println!("\nkeys this engine accepts, by uses:");
    let mut accepted: Vec<_> = total
        .uses
        .iter()
        .filter(|(key, _)| ACCEPTED.contains(&key.as_bytes()))
        .collect();
    accepted.sort_by_key(|(key, uses)| (std::cmp::Reverse(**uses), (*key).clone()));
    for (key, uses) in accepted {
        println!("  {uses:>8}  {key}");
    }
    if !refused.is_empty() {
        println!("\npages, for the largest refused keys:");
        for (_, key, _) in refused.iter().take(6) {
            let pages = &total.stops[key];
            let shown: Vec<_> = pages.iter().take(6).cloned().collect();
            println!("  {key}: {}", shown.join(" | "));
        }
    }
}
