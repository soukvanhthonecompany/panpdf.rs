use std::sync::Arc;

use pdf_app::Editor;
use pdf_bytes::{ByteStore, SourceId};

fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = arguments.next().expect("a PDF path");
    let page: usize = arguments
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let block: usize = arguments
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let bytes = std::fs::read(&path).expect("readable");
    let mut editor =
        Editor::open(ByteStore::new(SourceId::new(7), Arc::<[u8]>::from(bytes))).expect("opens");
    let source = editor.source().expect("a source").clone();
    let view = pdf_session::interpret_page_fully(
        &source,
        page,
        b"",
        editor.grouping(page).as_deref(),
        pdf_cli::font_provider(),
    )
    .expect("page reads");
    editor.adopt_page(page, Arc::new(view));
    let leaf = editor.leaf(page).expect("adopted").clone();
    let owner = &leaf.view.index.blocks[block];
    let mut seen = std::collections::BTreeSet::new();
    for line in &owner.lines {
        for cluster in &leaf.view.index.lines[*line].clusters {
            let atom = leaf.view.index.clusters[*cluster].atom;
            if !seen.insert(atom) {
                continue;
            }
            let pdf_paint::PaintAtomKind::Text(text) = &leaf.view.graph.atoms[atom].kind else {
                continue;
            };
            let ctm = text.state.ctm.value;
            let tm = text.matrices.text.value;
            let said: String = text
                .glyphs
                .iter()
                .filter_map(|glyph| {
                    text.text
                        .text_of(pdf_content::Code {
                            value: glyph.code.value,
                            byte_len: glyph.code.bytes.len(),
                        })
                        .map(|meaning| meaning.text.clone())
                })
                .collect();
            println!(
                "line {line} atom {atom}: ctm [{:.4} {:.4} {:.4} {:.4} {:.2} {:.2}] tm [{:.4} {:.4} {:.4} {:.4}] size {} Tz {} Ts {} font {} {:?}",
                ctm.a,
                ctm.b,
                ctm.c,
                ctm.d,
                ctm.e,
                ctm.f,
                tm.a,
                tm.b,
                tm.c,
                tm.d,
                text.state.text.font_size.value,
                text.state.text.horizontal_scaling.value,
                text.state.text.rise.value,
                text.font_request
                    .as_ref()
                    .map_or_else(String::new, |request| String::from_utf8_lossy(
                        &request.base_font
                    )
                    .into_owned()),
                said.chars().take(30).collect::<String>(),
            );
            let pen = text.glyphs.first().map(|glyph| {
                ctm.transform(pdf_paint::Point {
                    x: glyph.text_matrix.e,
                    y: glyph.text_matrix.f,
                })
                .y
            });
            let request = text.font_request.as_deref().or_else(|| {
                text.substitution
                    .as_deref()
                    .map(|found| found.request.as_ref())
            });
            println!(
                "    baseline {:?} TL {} line_metrics {:?} request ascent {:?} descent {:?} substituted {:?}",
                pen,
                text.state.text.leading.value,
                text.line_metrics(),
                request.and_then(|request| request.ascent),
                request.and_then(|request| request.descent),
                text.substitution
                    .as_ref()
                    .map(|found| found.primary.identity.family.clone()),
            );
        }
    }
}
