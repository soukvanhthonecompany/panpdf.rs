use std::fmt::Write as _;
use std::sync::Arc;

use pdf_app::Editor;
use pdf_bytes::{ByteStore, SourceId};

const COVERED: f64 = 0.5;

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

fn is_mark(character: char) -> bool {
    matches!(u32::from(character),
        0x0300..=0x036F | 0x0E31 | 0x0E34..=0x0E3A | 0x0E47..=0x0E4E
        | 0x0EB1 | 0x0EB4..=0x0EBC | 0x0EC8..=0x0ECD
        | 0x0900..=0x0903 | 0x093A..=0x094F | 0x1AB0..=0x1AFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}

fn shared(one: [f64; 4], other: [f64; 4]) -> f64 {
    let width = one[2].min(other[2]) - one[0].max(other[0]);
    let height = one[3].min(other[3]) - one[1].max(other[1]);
    let area = |b: [f64; 4]| (b[2] - b[0]) * (b[3] - b[1]);
    let smaller = area(one).min(area(other));
    if width <= 0.0 || height <= 0.0 || smaller <= 0.0 {
        return 0.0;
    }
    width * height / smaller
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = arguments.next().expect("a PDF path");
    let page_argument = arguments.next().unwrap_or_else(|| "spread".to_owned());
    let least: f64 = arguments
        .next()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.05);
    let Ok(bytes) = std::fs::read(&path) else {
        println!(
            "{{\"file\":\"{}\",\"error\":\"unreadable\"}}",
            escape(&path)
        );
        return;
    };
    let Ok(mut editor) = Editor::open(ByteStore::new(SourceId::new(7), Arc::<[u8]>::from(bytes)))
    else {
        println!("{{\"file\":\"{}\",\"error\":\"open\"}}", escape(&path));
        return;
    };
    let count = editor.page_count();
    let pages: Vec<usize> = match page_argument.as_str() {
        "spread" => {
            let mut chosen: Vec<usize> = [1, 2, 3].iter().map(|q| count * q / 4).collect();
            chosen.dedup();
            chosen
        }
        "all" => (0..count).collect(),
        page => vec![page.parse().unwrap_or(0)],
    };
    for page in pages {
        if !read(&mut editor, page) {
            println!(
                "{{\"file\":\"{}\",\"page\":{page},\"error\":\"page does not read\"}}",
                escape(&path)
            );
            continue;
        }
        let overlay = &editor.leaf(page).expect("adopted").overlay;
        let mut overlapping = 0;
        for (number, block) in overlay.blocks.iter().enumerate() {
            let worst = measure(overlay, number, block, (&path, page), least);
            if worst >= COVERED {
                overlapping += 1;
            }
        }
        println!(
            "{{\"file\":\"{}\",\"page\":{page},\"blocks\":{},\"overlapping\":{overlapping}}}",
            escape(&path),
            overlay.blocks.len()
        );
    }
}

fn measure(
    overlay: &pdf_app::Overlay,
    number: usize,
    block: &pdf_cli::TextBlockBox,
    (path, page): (&str, usize),
    least: f64,
) -> f64 {
    let letters: Vec<_> = overlay
        .clusters
        .iter()
        .filter(|c| block.lines.contains(&c.line))
        .filter_map(|c| {
            let text = c.text.clone().unwrap_or_default();
            let blank = text.chars().all(|ch| ch.is_whitespace() || is_mark(ch));
            let ink = c.box_pixels?;
            (!blank || text.is_empty()).then_some((c.line, c.index_in_line, text, ink))
        })
        .collect();
    let mut most_across: Option<(f64, usize, usize)> = None;
    let mut most_along: Option<(f64, usize, usize)> = None;
    for (at, one) in letters.iter().enumerate() {
        for (offset, other) in letters[at + 1..].iter().enumerate() {
            let hair = 0.2 * (one.3[2] - one.3[0]).max(one.3[3] - one.3[1]);
            if one.2 == other.2
                && !one.2.is_empty()
                && (0..4).all(|side| (one.3[side] - other.3[side]).abs() <= hair)
            {
                continue;
            }
            let covered = shared(one.3, other.3);
            let slot = if one.0 == other.0 {
                &mut most_along
            } else {
                &mut most_across
            };
            if slot.is_none_or(|(most, _, _)| covered > most) {
                *slot = Some((covered, at, at + 1 + offset));
            }
        }
    }
    let row_text = |line: usize| {
        overlay
            .clusters
            .iter()
            .filter(|c| c.line == line)
            .map(|c| c.text.clone().unwrap_or_default())
            .collect::<String>()
    };
    let mut worst = 0.0_f64;
    for (what, most) in [("two rows", most_across), ("one row", most_along)] {
        let Some((covered, one, other)) = most else {
            continue;
        };
        worst = worst.max(covered);
        if covered < least {
            continue;
        }
        let (one, other) = (&letters[one], &letters[other]);
        println!(
            "{{\"file\":\"{}\",\"page\":{page},\"block\":{number},\"rows\":{},\"kind\":\"{what}\",\"covered\":{covered:.3},\"at\":\"{:?} {:?}\",\"one\":\"{}\",\"other\":\"{}\",\"row_one\":\"{}\",\"row_other\":\"{}\"}}",
            escape(path),
            block.lines.len(),
            (one.0, one.1, one.3),
            (other.0, other.1, other.3),
            escape(&one.2),
            escape(&other.2),
            escape(&row_text(one.0).chars().take(60).collect::<String>()),
            escape(&row_text(other.0).chars().take(60).collect::<String>()),
        );
    }
    worst
}
