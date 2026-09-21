use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};

fn main() {
    let mut arguments = std::env::args_os().skip(1).map(PathBuf::from);
    let Some(target) = arguments.next() else {
        eprintln!("usage: glyphgaps <file.pdf|directory> [max pages per file]");
        std::process::exit(2);
    };
    let limit: usize = arguments
        .next()
        .and_then(|value| value.to_str().and_then(|text| text.parse().ok()))
        .unwrap_or(usize::MAX);

    let mut files: Vec<PathBuf> = Vec::new();
    if target.is_dir() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&target)
            .expect("the directory can be read")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().is_some_and(|kind| kind == "pdf"))
            .collect();
        entries.sort();
        files.extend(entries);
    } else {
        files.push(target);
    }

    for file in &files {
        report(file, limit);
    }
}

struct Tally<'a> {
    no_program: &'a mut usize,
    no_glyph_id: &'a mut usize,
    missing_outline: &'a mut usize,
    blank_outline: &'a mut usize,
    drawn: &'a mut usize,
    absent: &'a mut BTreeMap<u16, usize>,
    unmapped: &'a mut BTreeMap<String, usize>,
    programs: &'a mut BTreeMap<String, usize>,
}

fn walk(atoms: &[pdf_paint::PaintAtom], tally: &mut Tally<'_>) {
    for atom in atoms {
        match &atom.kind {
            pdf_paint::PaintAtomKind::TransparencyGroup(group) => walk(&group.graph.atoms, tally),
            pdf_paint::PaintAtomKind::Text(text) => {
                if text.type3 {
                    continue;
                }
                let Some(program) = text.program.as_ref() else {
                    *tally.no_program += text.glyphs.len();
                    continue;
                };
                for glyph in &text.glyphs {
                    let Some(id) = glyph.glyph else {
                        *tally.no_glyph_id += 1;
                        let code = &glyph.code;
                        let named = match code.cid {
                            Some(cid) => format!("code {:#x} cid {cid}", code.value),
                            None => format!("code {:#x}", code.value),
                        };
                        *tally.unmapped.entry(named).or_default() += 1;
                        continue;
                    };
                    match program.path(id) {
                        None => {
                            *tally.missing_outline += 1;
                            *tally.absent.entry(id).or_default() += 1;
                            let kind = match program.as_ref() {
                                pdf_paint::GlyphProgram::TrueType(font) => format!(
                                    "TrueType, {} glyphs, {} upem",
                                    font.glyph_count(),
                                    font.units_per_em()
                                ),
                                pdf_paint::GlyphProgram::Cff { font, .. } => format!(
                                    "CFF, {} glyphs, {} upem",
                                    font.glyph_count(),
                                    font.units_per_em()
                                ),
                                pdf_paint::GlyphProgram::Type1 { font, .. } => format!(
                                    "Type 1, {} glyphs, {} upem",
                                    font.glyph_count(),
                                    font.units_per_em()
                                ),
                            };
                            *tally.programs.entry(kind).or_default() += 1;
                        }
                        Some(outline) if outline.is_empty() => *tally.blank_outline += 1,
                        Some(_) => *tally.drawn += 1,
                    }
                }
            }
            _ => {}
        }
    }
}

fn report(path: &Path, limit: usize) {
    let Ok(bytes) = std::fs::read(path) else {
        println!("{}: unreadable", path.display());
        return;
    };
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
    let session = pdf_session::Session::new(source.clone(), b"");
    let Ok(count) = session.page_count() else {
        println!("{}: no page tree", path.display());
        return;
    };

    let mut no_program = 0_usize;
    let mut no_glyph_id = 0_usize;
    let mut missing_outline = 0_usize;
    let mut blank_outline = 0_usize;
    let mut drawn = 0_usize;
    let mut absent: BTreeMap<u16, usize> = BTreeMap::new();
    let mut unmapped: BTreeMap<String, usize> = BTreeMap::new();
    let mut programs: BTreeMap<String, usize> = BTreeMap::new();
    let mut pages_touched = 0_usize;

    for index in 0..count.min(limit) {
        let Ok(page) = pdf_session::interpret_page(&source, index) else {
            continue;
        };
        pages_touched += 1;
        walk(
            &page.graph.atoms,
            &mut Tally {
                no_program: &mut no_program,
                no_glyph_id: &mut no_glyph_id,
                missing_outline: &mut missing_outline,
                blank_outline: &mut blank_outline,
                drawn: &mut drawn,
                absent: &mut absent,
                unmapped: &mut unmapped,
                programs: &mut programs,
            },
        );
    }

    let asked = no_program + no_glyph_id + missing_outline + blank_outline + drawn;
    if asked == 0 {
        return;
    }
    println!("{}", path.display());
    println!("  pages read: {pages_touched} of {count}");
    println!("  glyphs asked for: {asked}");
    println!("  drawn: {drawn}");
    println!("  no font program at all: {no_program}");
    println!("  code mapped to no glyph: {no_glyph_id}");
    println!("  glyph id with no outline: {missing_outline}");
    println!("  outline present but empty: {blank_outline}");
    if !unmapped.is_empty() {
        let mut worst: Vec<(String, usize)> = unmapped.into_iter().collect();
        worst.sort_by_key(|entry| std::cmp::Reverse(entry.1));
        let named = worst
            .iter()
            .take(12)
            .map(|(code, times)| format!("{code} x{times}"))
            .collect::<Vec<_>>()
            .join(", ");
        println!("  codes that selected no glyph: {named}");
    }
    if !absent.is_empty() {
        let mut worst: Vec<(u16, usize)> = absent.into_iter().collect();
        worst.sort_by_key(|entry| std::cmp::Reverse(entry.1));
        let named = worst
            .iter()
            .take(12)
            .map(|(id, times)| format!("{id} x{times}"))
            .collect::<Vec<_>>()
            .join(", ");
        println!("  glyph ids with no outline: {named}");
        for (kind, times) in &programs {
            println!("    from: {kind} x{times}");
        }
    }
}
