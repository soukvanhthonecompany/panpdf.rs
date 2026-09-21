use std::collections::BTreeMap;
use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_paint::PaintAtomKind;

fn main() {
    let path = std::env::args().nth(1).expect("a PDF path");
    let page = std::env::args()
        .nth(2)
        .and_then(|value| value.parse::<usize>().ok())
        .map_or(0, |value| value.saturating_sub(1));
    let quiet = std::env::args().any(|value| value == "--tally");
    let bytes = std::fs::read(&path).expect("read");
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
    let view =
        pdf_session::interpret_page_fully(&source, page, b"", None, pdf_cli::font_provider())
            .expect("the page reads");

    if !quiet
        && let Some(resources) = view
            .program
            .resources
            .fonts()
            .first()
            .map(|_| &view.program.resources)
    {
        for resource in resources.fonts() {
            let name = String::from_utf8_lossy(resource.name()).into_owned();
            let Ok(Some(program)) = resource.glyph_program() else {
                println!("{name}: no program");
                continue;
            };
            let named: Vec<String> = (0..120_u16)
                .filter_map(|glyph| {
                    program
                        .glyph_name(glyph)
                        .map(|name| format!("g{glyph}={}", String::from_utf8_lossy(name)))
                })
                .take(16)
                .collect();
            println!(
                "{name}: units/em {}, post names: {}",
                program.units_per_em(),
                if named.is_empty() {
                    "(none)".to_owned()
                } else {
                    named.join(" ")
                }
            );
        }
    }

    let mut fonts: BTreeMap<String, BTreeMap<u32, (String, Option<u16>)>> = BTreeMap::new();
    for atom in &view.graph.atoms {
        let PaintAtomKind::Text(text) = &atom.kind else {
            continue;
        };
        let name = text.state.text.font.as_ref().map_or_else(
            || "(none)".to_owned(),
            |font| String::from_utf8_lossy(&font.value.name).into_owned(),
        );
        let seen = fonts.entry(name).or_default();
        for glyph in &text.glyphs {
            let meaning = text
                .text
                .text_of(pdf_content::Code {
                    value: glyph.code.value,
                    byte_len: glyph.code.bytes.len(),
                })
                .map_or_else(String::new, |meaning| meaning.text.clone());
            seen.entry(glyph.code.value)
                .or_insert((meaning, glyph.glyph));
        }
    }

    for (font, codes) in &fonts {
        let unread = codes
            .values()
            .filter(|(text, _)| {
                text.is_empty()
                    || text.chars().all(|character| {
                        character == '\u{FFFD}' || (0xE000..=0xF8FF).contains(&u32::from(character))
                    })
            })
            .count();
        if quiet {
            println!(
                "{{\"file\":{path:?},\"font\":{font:?},\"codes\":{},\"unread\":{unread}}}",
                codes.len()
            );
            continue;
        }
        println!("{font}: {} codes, {unread} unread", codes.len());
        let shown: Vec<String> = codes
            .iter()
            .take(24)
            .map(|(code, (text, glyph))| {
                let glyph = glyph.map_or_else(|| "-".to_owned(), |id| id.to_string());
                format!("{code:#04x}=>{text:?}/g{glyph}")
            })
            .collect();
        println!("  {}", shown.join(" "));
    }
}
