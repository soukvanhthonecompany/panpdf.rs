use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_content::outline_match::Raster;
use pdf_paint::PaintAtomKind;

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

fn bits(raster: &Raster) -> String {
    raster.rows().iter().fold(String::new(), |mut out, row| {
        let _ = write!(out, "{row:032x}");
        out
    })
}

#[derive(Default)]
struct Font {
    codes: BTreeMap<u32, (String, String)>,
    runs: Vec<Vec<u32>>,
    placed: Vec<(u32, pdf_paint::reading_order::PlacedInk)>,
}

fn page(path: &str, page: usize) {
    let Ok(bytes) = std::fs::read(path) else {
        eprintln!("{path}: unreadable");
        return;
    };
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
    let Ok(view) =
        pdf_session::interpret_page_fully(&source, page, b"", None, pdf_cli::font_provider())
    else {
        eprintln!("{path} page {page}: does not read");
        return;
    };
    let mut fonts: BTreeMap<String, Font> = BTreeMap::new();
    for atom in &view.graph.atoms {
        let PaintAtomKind::Text(text) = &atom.kind else {
            continue;
        };
        let name = text.font_request.as_ref().map_or_else(
            || "(none)".to_owned(),
            |request| request.family.trim().to_owned(),
        );
        let font = fonts.entry(name).or_default();
        let mut run = Vec::with_capacity(text.glyphs.len());
        for glyph in &text.glyphs {
            let code = glyph.code.value;
            run.push(code);
            let raster = match (text.program.as_deref(), glyph.glyph) {
                (Some(program), Some(id)) => Raster::of(program, id),
                _ => None,
            };
            let placed = text.state.ctm.value.multiply(glyph.matrix);
            let origin = placed.transform(pdf_paint::Point { x: 0.0, y: 0.0 });
            font.placed.push((
                code,
                pdf_paint::reading_order::PlacedInk {
                    origin: (origin.x, origin.y),
                    turn: placed.b.atan2(placed.a),
                    em: placed.a.hypot(placed.b),
                    ink: raster.as_ref().map(Raster::ink_box),
                },
            ));
            font.codes.entry(code).or_insert_with(|| {
                let meaning = text
                    .text
                    .text_of(pdf_content::Code {
                        value: code,
                        byte_len: glyph.code.bytes.len(),
                    })
                    .map_or_else(String::new, |meaning| meaning.text.clone());
                let drawing = match (text.program.as_deref(), glyph.glyph) {
                    (Some(program), Some(id)) => {
                        Raster::of(program, id).map_or_else(String::new, |raster| bits(&raster))
                    }
                    _ => String::new(),
                };
                (meaning, drawing)
            });
        }
        if !run.is_empty() {
            font.runs.push(run);
        }
    }
    for (name, font) in fonts {
        let codes: Vec<String> = font
            .codes
            .iter()
            .map(|(code, (meaning, drawing))| {
                format!("\"{code}\":[\"{}\",\"{drawing}\"]", escape(meaning))
            })
            .collect();
        let inks: Vec<_> = font.placed.iter().map(|(_, ink)| *ink).collect();
        let lines: Vec<Vec<u32>> = pdf_paint::reading_order::reading_order(&inks)
            .into_iter()
            .map(|line| line.into_iter().map(|at| font.placed[at].0).collect())
            .collect();
        let runs: Vec<String> = (if std::env::var_os("GLYPHDUMP_FILE_ORDER").is_some() {
            &font.runs
        } else {
            &lines
        })
        .iter()
        .map(|run| {
            let codes: Vec<String> = run.iter().map(u32::to_string).collect();
            format!("[{}]", codes.join(","))
        })
        .collect();
        println!(
            "{{\"file\":\"{}\",\"page\":{page},\"font\":\"{}\",\"codes\":{{{}}},\"runs\":[{}]}}",
            escape(path),
            escape(&name),
            codes.join(","),
            runs.join(",")
        );
    }
}

fn reference(path: &str) {
    let Ok(program) = std::fs::read(path)
        .map_err(|error| error.to_string())
        .and_then(|bytes| {
            pdf_content::GlyphProgram::parse(bytes).map_err(|error| format!("{error:?}"))
        })
    else {
        eprintln!("{path}: not a font");
        return;
    };
    let blocks = [0x20_u32..0x7F, 0x0E00..0x0E80, 0x0E80..0x0F00];
    for character in blocks.into_iter().flatten().filter_map(char::from_u32) {
        let Some(glyph) = program.glyph_for_char(character) else {
            continue;
        };
        let drawing = Raster::of(&program, glyph).map_or_else(String::new, |raster| bits(&raster));
        println!(
            "{{\"face\":\"{}\",\"char\":\"{}\",\"drawing\":\"{drawing}\"}}",
            escape(path),
            escape(&character.to_string())
        );
    }
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.first().map(String::as_str) {
        Some("reference") => arguments[1..].iter().for_each(|path| reference(path)),
        Some("page") if arguments.len() >= 3 => {
            for number in &arguments[2..] {
                match number.parse::<usize>() {
                    Ok(number) => page(&arguments[1], number),
                    Err(_) => eprintln!("{number}: not a page number"),
                }
            }
        }
        _ => eprintln!("usage: glyphdump page file.pdf page... | glyphdump reference face.ttf..."),
    }
}
