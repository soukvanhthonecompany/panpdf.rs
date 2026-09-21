use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};

#[derive(Default)]
struct Tally {
    not_cid_keyed: u64,
    agree: u64,
    differ: u64,
    absent_from_charset: u64,
    shifts: BTreeMap<i64, u64>,
    files_touched: BTreeMap<String, u64>,
}

impl Tally {
    fn absorb(&mut self, other: &Self) {
        self.not_cid_keyed += other.not_cid_keyed;
        self.agree += other.agree;
        self.differ += other.differ;
        self.absent_from_charset += other.absent_from_charset;
        for (shift, count) in &other.shifts {
            *self.shifts.entry(*shift).or_default() += count;
        }
        for (file, count) in &other.files_touched {
            *self.files_touched.entry(file.clone()).or_default() += count;
        }
    }

    fn considered(&self) -> u64 {
        self.agree + self.differ + self.absent_from_charset
    }
}

fn walk(atoms: &[pdf_paint::PaintAtom], tally: &mut Tally) {
    for atom in atoms {
        match &atom.kind {
            pdf_paint::PaintAtomKind::TransparencyGroup(group) => walk(&group.graph.atoms, tally),
            pdf_paint::PaintAtomKind::Text(text) => {
                if text.type3 {
                    continue;
                }
                let Some(program) = text.program.as_ref() else {
                    continue;
                };
                let pdf_paint::GlyphProgram::Cff { font, .. } = program.as_ref() else {
                    tally.not_cid_keyed += u64::try_from(text.glyphs.len()).unwrap_or(u64::MAX);
                    continue;
                };
                if !font.is_cid() {
                    tally.not_cid_keyed += u64::try_from(text.glyphs.len()).unwrap_or(u64::MAX);
                    continue;
                }
                for glyph in &text.glyphs {
                    let Some(cid) = glyph.code.cid else {
                        continue;
                    };
                    let Ok(cid) = u16::try_from(cid) else {
                        continue;
                    };
                    match font.glyph_for_cid(cid) {
                        None => tally.absent_from_charset += 1,
                        Some(mapped) if mapped == cid => tally.agree += 1,
                        Some(mapped) => {
                            tally.differ += 1;
                            let shift = i64::from(mapped) - i64::from(cid);
                            *tally.shifts.entry(shift).or_default() += 1;
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn report(path: &Path, limit: usize, total: &mut Tally) {
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
    let Ok(pages) = pdf_session::Session::new(source.clone(), b"").page_count() else {
        return;
    };
    let mut session = pdf_session::Session::new(source, b"");
    let mut file = Tally::default();
    for index in 0..pages.min(limit) {
        let Ok(view) = session.page(index) else {
            continue;
        };
        walk(&view.graph.atoms, &mut file);
    }
    if file.considered() == 0 && file.not_cid_keyed == 0 {
        return;
    }
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into(),
    );
    if file.differ > 0 || file.absent_from_charset > 0 {
        file.files_touched.insert(name.clone(), file.differ);
    }
    println!(
        "{name}: cid-keyed glyphs {} (agree {}, differ {}, absent {}), other programs {}",
        file.considered(),
        file.agree,
        file.differ,
        file.absent_from_charset,
        file.not_cid_keyed
    );
    total.absorb(&file);
}

fn main() {
    let mut arguments = std::env::args_os().skip(1).map(PathBuf::from);
    let Some(target) = arguments.next() else {
        eprintln!("usage: cidgid <file.pdf|directory> [max pages per file]");
        std::process::exit(2);
    };
    let limit: usize = arguments
        .next()
        .and_then(|value| value.to_str().and_then(|text| text.parse().ok()))
        .unwrap_or(usize::MAX);

    let mut total = Tally::default();
    if target.is_dir() {
        let mut paths: Vec<_> = std::fs::read_dir(&target)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
            })
            .collect();
        paths.sort();
        for path in paths {
            report(&path, limit, &mut total);
        }
    } else {
        report(&target, limit, &mut total);
    }

    println!("\n=== total ===");
    println!(
        "glyphs shown through a CID-keyed CFF: {}",
        total.considered()
    );
    println!("  charset agrees with the CID:        {}", total.agree);
    println!("  charset says a different glyph:     {}", total.differ);
    println!(
        "  CID not in the charset at all:      {}",
        total.absent_from_charset
    );
    println!(
        "glyphs shown through other programs:  {}",
        total.not_cid_keyed
    );
    println!(
        "files with at least one difference:   {}",
        total.files_touched.len()
    );
    if !total.shifts.is_empty() {
        println!("\nhow far the charset moves a glyph, by count:");
        let mut shifts: Vec<_> = total.shifts.iter().collect();
        shifts.sort_by_key(|(_, count)| std::cmp::Reverse(**count));
        for (shift, count) in shifts.iter().take(20) {
            println!("  {count:>9} x {shift:+}");
        }
        if shifts.len() > 20 {
            println!("  ... and {} more distinct shifts", shifts.len() - 20);
        }
    }
    if !total.files_touched.is_empty() {
        println!("\nfiles by differing glyphs:");
        let mut files: Vec<_> = total.files_touched.iter().collect();
        files.sort_by_key(|(_, count)| std::cmp::Reverse(**count));
        for (file, count) in files.iter().take(25) {
            println!("  {count:>9} {file}");
        }
    }
}
