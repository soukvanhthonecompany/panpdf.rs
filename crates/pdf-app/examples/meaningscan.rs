use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;

use pdf_app::Editor;
use pdf_bytes::{ByteStore, SourceId};
use pdf_content::{Code, GlyphProgram};

#[derive(Default)]
struct Tally {
    glyphs: usize,
    missing: usize,
    empty: usize,
    mapped: usize,
    reachable: usize,
    matched: usize,
    samples: BTreeMap<String, usize>,
    context: String,
}

fn reference() -> Option<&'static pdf_content::outline_match::Reference> {
    static REFERENCE: std::sync::OnceLock<Option<pdf_content::outline_match::Reference>> =
        std::sync::OnceLock::new();
    REFERENCE
        .get_or_init(|| {
            let path = std::env::var_os("MEANINGSCAN_REFERENCE")?;
            let program = GlyphProgram::parse(std::fs::read(path).ok()?).ok()?;
            Some(pdf_content::outline_match::Reference::of(&program))
        })
        .as_ref()
}

fn escape(text: &str) -> String {
    let mut out = String::new();
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            character if character.is_control() => {
                let _ = write!(out, "\\u{:04x}", u32::from(character));
            }
            character => out.push(character),
        }
    }
    out
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = arguments.next().expect("a PDF path");
    let page_argument = arguments.next().unwrap_or_else(|| "0".to_owned());
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };
    let Ok(mut editor) = Editor::open(ByteStore::new(SourceId::new(7), Arc::<[u8]>::from(bytes)))
    else {
        return;
    };
    let pages: Vec<usize> = if page_argument == "spread" {
        let count = editor.page_count();
        let mut chosen: Vec<usize> = [1, 2, 3]
            .iter()
            .map(|quarter| count * quarter / 4)
            .collect();
        chosen.dedup();
        chosen
    } else {
        vec![page_argument.parse().unwrap_or(0)]
    };
    for page in pages {
        scan(&mut editor, &path, page);
    }
}

#[expect(clippy::too_many_lines, reason = "one diagnostic, step by step")]
fn scan(editor: &mut Editor, path: &str, page: usize) {
    let Some(source) = editor.source() else {
        return;
    };
    let Ok(view) =
        pdf_session::interpret_page_fully(source, page, b"", None, pdf_cli::font_provider())
    else {
        return;
    };
    let mut fonts: BTreeMap<String, Tally> = BTreeMap::new();
    for atom in &view.graph.atoms {
        let pdf_paint::PaintAtomKind::Text(text) = &atom.kind else {
            continue;
        };
        let name = text.font_request.as_ref().map_or_else(
            || "type3".to_owned(),
            |request| {
                format!(
                    "{} {} {} tech={} tounicode={}",
                    String::from_utf8_lossy(&request.base_font),
                    String::from_utf8_lossy(&request.subtype),
                    request.encoding.clone().unwrap_or_default(),
                    text.program
                        .as_deref()
                        .map_or("none", GlyphProgram::technology),
                    text.text.len(),
                )
            },
        );
        let by_glyph = text.program.as_deref().and_then(|program| match program {
            GlyphProgram::TrueType(font) => Some(font.characters()),
            GlyphProgram::Cff { sfnt, .. } => {
                sfnt.as_deref().map(pdf_content::TrueTypeFont::characters)
            }
            GlyphProgram::Type1 { .. } => None,
        });
        if std::env::var_os("MEANINGSCAN_TABLE").is_some() {
            let table: Vec<String> = (0..=255_u32)
                .filter_map(|value| {
                    let meaning = text.text.text_of(Code { value, byte_len: 1 })?;
                    Some(format!(
                        "{value:02x}={:?}:{:?}",
                        meaning.text, meaning.confidence
                    ))
                })
                .collect();
            eprintln!("{name}: {}", table.join(" "));
        }
        if std::env::var_os("MEANINGSCAN_DUPLICATES").is_some() {
            let mut by_text: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for value in 0..=255_u32 {
                if let Some(meaning) = text.text.text_of(Code { value, byte_len: 1 }) {
                    by_text
                        .entry(meaning.text.clone())
                        .or_default()
                        .push(format!("{value:02x}:{:?}", meaning.confidence));
                }
            }
            for (said, codes) in by_text.iter().filter(|(_, codes)| codes.len() > 1) {
                eprintln!("{name}: {said:?} <- {}", codes.join(" "));
            }
        }
        let tally = fonts.entry(name).or_default();
        for glyph in &text.glyphs {
            tally.glyphs += 1;
            let meaning = text.text.text_of(Code {
                value: glyph.code.value,
                byte_len: glyph.code.bytes.len(),
            });
            match meaning {
                Some(meaning) if !meaning.text.is_empty() => {
                    tally.mapped += 1;
                    if tally.context.chars().count() < 30 {
                        tally.context.push_str(&meaning.text);
                    }
                    continue;
                }
                Some(_) => tally.empty += 1,
                None => tally.missing += 1,
            }
            let characters: String = match (&by_glyph, glyph.glyph) {
                (Some(table), Some(id)) => table
                    .iter()
                    .filter(|(found, _)| *found == id)
                    .map(|(_, character)| *character)
                    .collect(),
                _ => String::new(),
            };
            if !characters.is_empty() {
                tally.reachable += 1;
            }
            let named = match (text.program.as_deref(), glyph.glyph) {
                (Some(program), Some(id)) => program
                    .glyph_name(id)
                    .map(|name| String::from_utf8_lossy(name).into_owned())
                    .unwrap_or_default(),
                _ => String::new(),
            };
            let matched = match (reference(), text.program.as_deref(), glyph.glyph) {
                (Some(reference), Some(program), Some(id)) => {
                    pdf_content::outline_match::Raster::of(program, id)
                        .and_then(|raster| {
                            let hint = program
                                .glyph_name(id)
                                .and_then(pdf_content::outline_match::index_in_name)
                                .or_else(|| glyph.code.cid.and_then(|cid| u16::try_from(cid).ok()));
                            reference.character_of(&raster, hint)
                        })
                        .map_or_else(|| "-".to_owned(), String::from)
                }
                _ => String::new(),
            };
            if matched.chars().count() == 1 && matched != "-" {
                tally.matched += 1;
            }
            let key = format!(
                "{:0width$x} gid={} name={} matched={matched} cmap={}",
                glyph.code.value,
                glyph
                    .glyph
                    .map_or_else(|| "-".to_owned(), |id| id.to_string()),
                named,
                characters,
                width = glyph.code.bytes.len() * 2
            );
            *tally.samples.entry(key).or_default() += 1;
        }
    }
    report(path, page, fonts);
}

fn report(path: &str, page: usize, fonts: BTreeMap<String, Tally>) {
    for (font, tally) in fonts {
        if tally.missing + tally.empty == 0 {
            continue;
        }
        let mut samples: Vec<(String, usize)> = tally.samples.into_iter().collect();
        samples.sort_by_key(|sample| std::cmp::Reverse(sample.1));
        let samples: Vec<String> = samples
            .iter()
            .take(12)
            .map(|(key, count)| format!("{count}x {key}"))
            .collect();
        println!(
            "{{\"file\":\"{}\",\"page\":{page},\"font\":\"{}\",\"glyphs\":{},\"mapped\":{},\"missing\":{},\"empty\":{},\"reachable\":{},\"matched\":{},\"context\":\"{}\",\"samples\":\"{}\"}}",
            escape(path),
            escape(&font),
            tally.glyphs,
            tally.mapped,
            tally.missing,
            tally.empty,
            tally.reachable,
            tally.matched,
            escape(&tally.context),
            escape(&samples.join(" | ")),
        );
    }
}
