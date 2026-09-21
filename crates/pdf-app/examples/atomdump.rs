use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};

fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = arguments.next().expect("a PDF path");
    let page: usize = arguments.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let from: usize = arguments.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let to: usize = arguments
        .next()
        .and_then(|v| v.parse().ok())
        .unwrap_or(usize::MAX);
    let stream = arguments.next().is_some_and(|word| word == "stream");
    let bytes = std::fs::read(&path).expect("the file reads");
    let source = ByteStore::new(SourceId::new(7), Arc::<[u8]>::from(bytes));
    let view =
        pdf_session::interpret_page_fully(&source, page, b"", None, pdf_cli::font_provider())
            .expect("the page reads");
    if stream {
        let spans: Vec<_> = view
            .graph
            .atoms
            .iter()
            .enumerate()
            .filter(|(ordinal, _)| *ordinal >= from && *ordinal <= to)
            .map(|(_, atom)| atom.id.operator_span)
            .collect();
        if let Some(first) = spans.first() {
            let start = spans.iter().map(|span| span.start()).min().unwrap_or(0);
            let end = spans.iter().map(|span| span.end()).max().unwrap_or(0);
            if let Some(decoded) = view
                .program
                .streams
                .iter()
                .find(|candidate| candidate.bytes.id() == first.source())
            {
                let bytes = decoded.bytes.as_bytes();
                let shown = &bytes[start.saturating_sub(150)..(end + 20).min(bytes.len())];
                println!("{}", String::from_utf8_lossy(shown));
            }
        }
        return;
    }
    for (ordinal, atom) in view.graph.atoms.iter().enumerate() {
        if ordinal < from || ordinal > to {
            continue;
        }
        let pdf_paint::PaintAtomKind::Text(text) = &atom.kind else {
            continue;
        };
        let state = &text.state.text;
        let font = state
            .font
            .as_ref()
            .map(|font| String::from_utf8_lossy(&font.value.name).into_owned())
            .unwrap_or_default();
        println!(
            "atom {ordinal} font {font} size {} Tc {} Tw {} Tz {} Ts {} mode {:?} tm {:?}",
            state.font_size.value,
            state.character_spacing.value,
            state.word_spacing.value,
            state.horizontal_scaling.value,
            state.rise.value,
            state.rendering_mode.value,
            text.matrices.text.value,
        );
        for glyph in &text.glyphs {
            let pen = text.state.ctm.value.transform(pdf_paint::Point {
                x: glyph.text_matrix.e,
                y: glyph.text_matrix.f,
            });
            println!(
                "    code {} bytes {:02X?} pen {:.2},{:.2}",
                glyph.code.value, glyph.code.bytes, pen.x, pen.y
            );
        }
    }
}
