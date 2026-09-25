use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Instant;

use pdf_app::{Applied, Editor};
use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::BlockRange;

const SAMPLES: &[(&str, &str)] = &[
    ("latin", "Hello"),
    ("vietnamese", "Tiếng Việt"),
    ("cyrillic", "Привет"),
    ("digits", "12,345.67 (%)"),
    ("thai", "สวัสดีครับ"),
    ("lao", "ສະບາຍດີ"),
    ("khmer", "សួស្តី"),
    ("myanmar", "မင်္ဂလာပါ"),
    ("devanagari", "नमस्ते"),
    ("arabic", "مرحبا"),
    ("hebrew", "שלום"),
    ("chinese", "你好世界"),
    ("japanese", "こんにちは"),
    ("korean", "안녕하세요"),
];

const WEB_FACES: &str = "DejaVu Sans,Liberation Mono,Liberation Sans,Liberation Serif,Noto Sans Lao,Noto Sans Thai,Noto Serif Lao,Saysettha OT";

fn read(editor: &mut Editor, page: usize) -> bool {
    let Some(source) = editor.source() else {
        return false;
    };
    match pdf_session::interpret_page_fully(
        source,
        page,
        b"",
        editor.grouping(page).as_deref(),
        pdf_cli::font_provider(),
    ) {
        Ok(view) => {
            editor.adopt_page(page, Arc::new(view));
            true
        }
        Err(_) => false,
    }
}

fn positions(editor: &Editor, page: usize, block: usize) -> Option<Vec<usize>> {
    if let Some(reading) = editor.block_reading(page, block) {
        return Some(
            reading
                .lines
                .iter()
                .map(|line| line.clusters.len())
                .collect(),
        );
    }
    let leaf = editor.leaf(page)?;
    let owner = leaf.overlay.blocks.get(block)?;
    Some(
        owner
            .lines
            .iter()
            .map(|line| {
                leaf.view
                    .index
                    .lines
                    .get(*line)
                    .map_or(0, |row| row.clusters.len())
            })
            .collect(),
    )
}

fn whole(editor: &Editor, page: usize, block: usize) -> Option<(String, (usize, usize))> {
    let lines = positions(editor, page, block)?;
    let last = lines.len().checked_sub(1)?;
    let end = (last, lines[last]);
    Some((editor.copy_text(page, block, (0, 0), end)?, end))
}

fn normal(text: &str) -> String {
    text.replace('\u{0e33}', "\u{0e4d}\u{0e32}")
        .replace('\u{0eb3}', "\u{0ecd}\u{0eb2}")
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

fn escape(text: &str) -> String {
    let mut out = String::new();
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            character if character.is_control() => {
                let _ = write!(out, "\\u{:04x}", u32::from(character));
            }
            character => out.push(character),
        }
    }
    out
}

fn judge(
    editor: &mut Editor,
    page: usize,
    block: usize,
    applied: &Applied,
    wanted: &str,
) -> (&'static str, String) {
    match applied {
        Applied::Refused(reason) => ("refused", reason.to_string()),
        Applied::Unchanged => ("wrong", "unchanged".to_owned()),
        Applied::Changed { .. } => {
            read(editor, page);
            let got = whole(editor, page, block).map(|(text, _)| text);
            let said = editor.status().to_string();
            let _ = editor.undo();
            read(editor, page);
            match got {
                Some(text) if normal(&text) == normal(wanted) => ("ok", said),
                Some(text) => {
                    let tail: String = {
                        let count = text.chars().count();
                        text.chars().skip(count.saturating_sub(24)).collect()
                    };
                    ("wrong", format!("reads …{tail}"))
                }
                None => ("wrong", "the block no longer reads".to_owned()),
            }
        }
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "a probe's one loop, read top to bottom"
)]
fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = arguments.next().expect("a PDF path");
    let page: usize = arguments
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let most: usize = arguments
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(6);
    let faces: Vec<String> = std::env::var("CENSUS_FACES")
        .unwrap_or_else(|_| WEB_FACES.to_owned())
        .split(',')
        .map(str::trim)
        .filter(|face| !face.is_empty())
        .map(str::to_owned)
        .collect();
    let bytes = std::fs::read(&path).expect("the file reads");
    let Ok(mut editor) = Editor::open(ByteStore::new(SourceId::new(7), Arc::<[u8]>::from(bytes)))
    else {
        println!(
            "{{\"file\":\"{}\",\"page\":{page},\"error\":\"does not open\"}}",
            escape(&path)
        );
        return;
    };
    let _ = editor.set_aside_restrictions();
    if !read(&mut editor, page) {
        println!(
            "{{\"file\":\"{}\",\"page\":{page},\"error\":\"page does not read\"}}",
            escape(&path)
        );
        return;
    }
    let blocks: Vec<usize> = {
        let count = editor
            .leaf(page)
            .map_or(0, |leaf| leaf.overlay.blocks.len());
        (0..count)
            .filter(|&block| {
                whole(&editor, page, block).is_some_and(|(text, _)| text.chars().count() >= 2)
            })
            .collect()
    };
    let stride = blocks.len().div_ceil(most.max(1)).max(1);
    let file = escape(&path);
    for &block in blocks.iter().step_by(stride) {
        let emit = |sample: &str, face: &str, verdict: &str, detail: &str, ms: u128| {
            println!(
                "{{\"file\":\"{file}\",\"page\":{page},\"block\":{block},\"sample\":\"{sample}\",\"face\":\"{}\",\"verdict\":\"{verdict}\",\"detail\":\"{}\",\"ms\":{ms}}}",
                escape(face),
                escape(detail)
            );
        };
        let choices: Vec<Option<&String>> = std::iter::once(None)
            .chain(faces.iter().map(Some))
            .collect();
        for (sample, word) in SAMPLES {
            for face in &choices {
                let Some((before, end)) = whole(&editor, page, block) else {
                    break;
                };
                let typed = format!(" {word}");
                let range = BlockRange::Between { from: end, to: end };
                let started = Instant::now();
                let applied = match face {
                    None => editor.edit(page, block, range, &typed),
                    Some(family) => editor.edit_in_style(
                        page,
                        block,
                        range,
                        &typed,
                        pdf_edit::TextStyle {
                            family: Some((*family).clone()),
                            ..pdf_edit::TextStyle::default()
                        },
                    ),
                };
                let ms = started.elapsed().as_millis();
                let (verdict, detail) = judge(
                    &mut editor,
                    page,
                    block,
                    &applied,
                    &format!("{before}{typed}"),
                );
                emit(
                    sample,
                    face.map_or("own", String::as_str),
                    verdict,
                    &detail,
                    ms,
                );
            }
        }
        if let Some((before, end)) = whole(&editor, page, block) {
            let started = Instant::now();
            let applied = editor.edit(
                page,
                block,
                BlockRange::Between { from: end, to: end },
                "\nA",
            );
            let ms = started.elapsed().as_millis();
            let (verdict, detail) =
                judge(&mut editor, page, block, &applied, &format!("{before}\nA"));
            emit("enter", "own", verdict, &detail, ms);
        }
        if let Some((before, end)) = whole(&editor, page, block) {
            let started = Instant::now();
            let applied = editor.edit(
                page,
                block,
                BlockRange::Units {
                    at: end,
                    backwards: true,
                    count: 1,
                },
                "",
            );
            let ms = started.elapsed().as_millis();
            let mut wanted: Vec<char> = before.chars().collect();
            wanted.pop();
            let wanted: String = wanted.into_iter().collect();
            let (verdict, detail) = judge(&mut editor, page, block, &applied, &wanted);
            emit("backspace", "own", verdict, &detail, ms);
        }
    }
}
