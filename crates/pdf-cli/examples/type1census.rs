use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};

#[derive(Default)]
struct Tally {
    by_evidence: BTreeMap<&'static str, (u64, u64)>,
    unreadable_fonts: BTreeMap<String, (u64, u64)>,
    files_with_unreadable: BTreeMap<String, u64>,
    pages: u64,
}

fn walk(atoms: &[pdf_paint::PaintAtom], file: &str, tally: &mut Tally) {
    for atom in atoms {
        match &atom.kind {
            pdf_paint::PaintAtomKind::TransparencyGroup(group) => {
                walk(&group.graph.atoms, file, tally);
            }
            pdf_paint::PaintAtomKind::Text(text) => {
                if text.type3 || text.glyphs.is_empty() {
                    continue;
                }
                let Some(request) = text.font_request.as_ref() else {
                    continue;
                };
                let glyphs = u64::try_from(text.glyphs.len()).unwrap_or(0);
                let slot = tally
                    .by_evidence
                    .entry(request.program.label())
                    .or_default();
                slot.0 += 1;
                slot.1 += glyphs;
                if matches!(
                    request.program,
                    pdf_content::ProgramEvidence::Unreadable { .. }
                ) {
                    let name = String::from_utf8_lossy(&request.base_font).into_owned();
                    let slot = tally.unreadable_fonts.entry(name).or_default();
                    slot.0 += 1;
                    slot.1 += glyphs;
                    *tally
                        .files_with_unreadable
                        .entry(file.to_owned())
                        .or_default() += glyphs;
                }
            }
            _ => {}
        }
    }
}

fn main() {
    let directory = std::env::args().nth(1).expect("usage: type1census <dir>");
    let pages: usize = std::env::args()
        .nth(2)
        .and_then(|value| value.parse().ok())
        .unwrap_or(1);
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&directory)
        .expect("readdir")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.extension()
                .is_some_and(|kind| kind.eq_ignore_ascii_case("pdf"))
        })
        .collect();
    paths.sort();

    let mut tally = Tally::default();
    for path in &paths {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let file = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
        for page in 0..pages {
            let Ok(view) = pdf_session::interpret_page(&source, page) else {
                break;
            };
            tally.pages += 1;
            walk(&view.graph.atoms, &file, &mut tally);
        }
    }

    println!("{} files, {} pages read", paths.len(), tally.pages);
    println!("\ntext runs by what the file says about the font's own program:");
    for (label, (runs, glyphs)) in &tally.by_evidence {
        println!("  {runs:>7} runs {glyphs:>9} glyphs  {label}");
    }
    println!(
        "\nfonts whose embedded program this build cannot read ({} of them):",
        tally.unreadable_fonts.len()
    );
    let mut named: Vec<_> = tally.unreadable_fonts.iter().collect();
    named.sort_by_key(|(_, (_, glyphs))| std::cmp::Reverse(*glyphs));
    for (name, (runs, glyphs)) in named.iter().take(25) {
        println!("  {glyphs:>8} glyphs {runs:>6} runs  {name}");
    }
    let mut files: Vec<_> = tally.files_with_unreadable.iter().collect();
    files.sort_by_key(|(_, glyphs)| std::cmp::Reverse(**glyphs));
    println!(
        "\n{} files draw at least one such run on the pages read; the worst:",
        files.len()
    );
    for (file, glyphs) in files.iter().take(15) {
        println!("  {glyphs:>8} glyphs  {file}");
    }
}
