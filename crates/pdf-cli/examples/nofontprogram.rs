use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};

#[derive(Default)]
struct Tally {
    fonts: BTreeMap<String, (u64, u64, BTreeMap<String, u64>)>,
    scripts: BTreeMap<&'static str, u64>,
    unknown_text: u64,
    runs: u64,
    glyphs: u64,
    pages: u64,
}

impl Tally {
    fn absorb(&mut self, other: &Self) {
        for (name, (runs, glyphs, files)) in &other.fonts {
            let slot = self.fonts.entry(name.clone()).or_default();
            slot.0 += runs;
            slot.1 += glyphs;
            for (file, count) in files {
                *slot.2.entry(file.clone()).or_default() += count;
            }
        }
        for (script, count) in &other.scripts {
            *self.scripts.entry(script).or_default() += count;
        }
        self.unknown_text += other.unknown_text;
        self.runs += other.runs;
        self.glyphs += other.glyphs;
        self.pages += other.pages;
    }
}

fn block(code: char) -> &'static str {
    match u32::from(code) {
        0x0000..=0x007F => "Basic Latin",
        0x0080..=0x024F => "Latin extended",
        0x0370..=0x03FF => "Greek",
        0x0400..=0x04FF => "Cyrillic",
        0x0E00..=0x0E7F => "Thai",
        0x0E80..=0x0EFF => "Lao",
        0x1780..=0x17FF => "Khmer",
        0x2000..=0x206F => "Punctuation",
        0x2070..=0x2BFF => "Symbols",
        0x3000..=0x30FF => "CJK kana/punctuation",
        0x4E00..=0x9FFF => "CJK ideographs",
        0xAC00..=0xD7AF => "Hangul",
        0xF000..=0xF8FF => "Private use",
        _ => "other",
    }
}

fn walk(atoms: &[pdf_paint::PaintAtom], label: &str, tally: &mut Tally) {
    for atom in atoms {
        match &atom.kind {
            pdf_paint::PaintAtomKind::TransparencyGroup(group) => {
                walk(&group.graph.atoms, label, tally);
            }
            pdf_paint::PaintAtomKind::Text(text) => {
                if text.type3 || text.program.is_some() || text.glyphs.is_empty() {
                    continue;
                }
                tally.runs += 1;
                let glyphs = u64::try_from(text.glyphs.len()).unwrap_or(0);
                tally.glyphs += glyphs;
                let name = text.state.text.font.as_ref().map_or_else(
                    || "(no /Tf)".to_owned(),
                    |font| String::from_utf8_lossy(&font.value.name).into_owned(),
                );
                let slot = tally.fonts.entry(name).or_default();
                slot.0 += 1;
                slot.1 += glyphs;
                *slot.2.entry(label.to_owned()).or_default() += 1;
                let mut named = false;
                for glyph in &text.glyphs {
                    let code = pdf_content::Code {
                        value: glyph.code.value,
                        byte_len: glyph.code.bytes.len() + glyph.code.completed_bytes,
                    };
                    if let Some(meaning) = text.text.text_of(code) {
                        for character in meaning.text.chars() {
                            *tally.scripts.entry(block(character)).or_default() += 1;
                            named = true;
                        }
                    }
                }
                if !named {
                    tally.unknown_text += 1;
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
    let label = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into(),
    );
    let mut session = pdf_session::Session::new(source, b"");
    let mut file = Tally::default();
    for index in 0..pages.min(limit) {
        let Ok(view) = session.page(index) else {
            continue;
        };
        let before = file.runs;
        walk(&view.graph.atoms, &label, &mut file);
        if file.runs > before {
            file.pages += 1;
        }
    }
    if file.runs > 0 {
        println!(
            "{label}: {} runs, {} glyphs, on {} pages",
            file.runs, file.glyphs, file.pages
        );
    }
    total.absorb(&file);
}

fn main() {
    let mut arguments = std::env::args_os().skip(1).map(PathBuf::from);
    let Some(target) = arguments.next() else {
        eprintln!("usage: nofontprogram <file.pdf|directory> [max pages per file]");
        std::process::exit(2);
    };
    let limit: usize = arguments
        .next()
        .and_then(|value| value.to_str().and_then(|text| text.parse().ok()))
        .unwrap_or(usize::MAX);

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
        report(path, limit, &mut total);
    }

    println!("\n=== fonts that embed no program ===");
    println!(
        "runs {}, glyphs {}, pages {}, files {}",
        total.runs,
        total.glyphs,
        total.pages,
        total
            .fonts
            .values()
            .flat_map(|slot| slot.2.keys())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    );
    println!(
        "runs whose codes carry no Unicode at all: {}",
        total.unknown_text
    );
    println!("\nby /Tf name, most glyphs first:");
    let mut fonts: Vec<_> = total.fonts.iter().collect();
    fonts.sort_by_key(|(name, slot)| (std::cmp::Reverse(slot.1), (*name).clone()));
    for (name, (runs, glyphs, files)) in fonts.iter().take(40) {
        println!(
            "  {glyphs:>9} glyphs {runs:>7} runs {:>4} files  {name}",
            files.len()
        );
    }
    if fonts.len() > 40 {
        println!("  ... and {} more names", fonts.len() - 40);
    }
    println!("\nby script of the text those runs carry:");
    let mut scripts: Vec<_> = total.scripts.iter().collect();
    scripts.sort_by_key(|(_, count)| std::cmp::Reverse(**count));
    for (script, count) in scripts {
        println!("  {count:>9}  {script}");
    }
}
