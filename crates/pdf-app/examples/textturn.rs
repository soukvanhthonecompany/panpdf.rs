use std::sync::Arc;

use pdf_app::Editor;
use pdf_bytes::{ByteStore, SourceId};

const NEAR: f64 = 1e-6;

fn escape(text: &str) -> String {
    let mut out = String::new();
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' | '\r' | '\t' => out.push(' '),
            other if (other as u32) < 0x20 => out.push(' '),
            other => out.push(other),
        }
    }
    out
}

fn named(turn: f64) -> String {
    let quarter = std::f64::consts::FRAC_PI_2;
    for (steps, name) in [
        (0, "upright"),
        (1, "quarter"),
        (2, "upside down"),
        (3, "three quarters"),
    ] {
        let want = f64::from(steps) * quarter;
        if (turn - want).abs() < NEAR || (turn + std::f64::consts::TAU - want).abs() < NEAR {
            return name.to_owned();
        }
    }
    "at an angle".to_owned()
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = arguments.next().expect("a PDF path");
    let page_argument = arguments.next().unwrap_or_else(|| "spread".to_owned());
    let Ok(bytes) = std::fs::read(&path) else {
        println!(
            "{{\"file\":\"{}\",\"error\":\"unreadable\"}}",
            escape(&path)
        );
        return;
    };
    let source = ByteStore::new(SourceId::new(7), Arc::<[u8]>::from(bytes));
    let Ok(mut editor) = Editor::open(source) else {
        println!(
            "{{\"file\":\"{}\",\"error\":\"the file does not open\"}}",
            escape(&path)
        );
        return;
    };
    editor.set_aside_restrictions();
    let count = editor.page_count();
    let pages: Vec<usize> = if page_argument == "spread" {
        let mut chosen: Vec<usize> = [1, 2, 3].iter().map(|part| count * part / 4).collect();
        chosen.dedup();
        chosen
    } else {
        vec![
            page_argument
                .parse::<usize>()
                .unwrap_or(1)
                .saturating_sub(1),
        ]
    };
    for page in pages {
        scan_page(&mut editor, &path, page);
    }
}

fn scan_page(editor: &mut Editor, path: &str, page: usize) {
    let Some(source) = editor.source().cloned() else {
        return;
    };
    let Ok(view) =
        pdf_session::interpret_page_fully(&source, page, b"", None, pdf_cli::font_provider())
    else {
        println!(
            "{{\"file\":\"{}\",\"page\":{page},\"error\":\"the page does not read\"}}",
            escape(path)
        );
        return;
    };
    let turns: Vec<Option<f64>> = view
        .graph
        .atoms
        .iter()
        .map(|atom| match &atom.kind {
            pdf_paint::PaintAtomKind::Text(text) => Some(pdf_edit::text_turn(text)),
            _ => None,
        })
        .collect();
    editor.adopt_page(page, Arc::new(view));
    let Some(leaf) = editor.leaf(page) else {
        return;
    };
    let blocks = leaf.overlay.blocks.len();
    let index = leaf.view.index.clone();
    for block in 0..blocks {
        let turn = index
            .blocks
            .get(block)
            .and_then(|held| held.lines.first())
            .and_then(|line| index.lines[*line].clusters.first())
            .and_then(|cluster| turns.get(index.clusters[*cluster].atom).copied().flatten())
            .unwrap_or(0.0);
        let reading = editor.block_reading(page, block);
        println!(
            "{{\"file\":\"{}\",\"page\":{page},\"block\":{block},\"turn\":{turn:.6},\"set\":\"{}\",\"read\":{}}}",
            escape(path),
            named(turn),
            if reading.is_some() { "true" } else { "false" }
        );
    }
}
