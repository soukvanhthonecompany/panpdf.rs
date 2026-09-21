use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_paint::{PaintAtom, PaintAtomKind};

fn count(atoms: &[PaintAtom], nested: bool, tally: &mut [usize; 4]) {
    for atom in atoms {
        match &atom.kind {
            PaintAtomKind::Text(text) => {
                let slot = usize::from(nested) * 2;
                tally[slot] += 1;
                tally[slot + 1] += text.glyphs.len();
            }
            PaintAtomKind::TransparencyGroup(group) => count(&group.graph.atoms, true, tally),
            _ => {}
        }
    }
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = arguments.next().expect("a PDF path");
    let which = arguments.next().unwrap_or_else(|| "spread".to_owned());
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };
    let Ok(editor) =
        pdf_app::Editor::open(ByteStore::new(SourceId::new(7), Arc::<[u8]>::from(bytes)))
    else {
        return;
    };
    let pages = editor.page_count();
    let chosen: Vec<usize> = if which == "spread" {
        let mut chosen: Vec<usize> = [1, 2, 3].iter().map(|q| pages * q / 4).collect();
        chosen.dedup();
        chosen
    } else {
        vec![which.parse().unwrap_or(0)]
    };
    for page in chosen {
        let Some(source) = editor.source() else {
            return;
        };
        let Ok(view) =
            pdf_session::interpret_page_fully(source, page, b"", None, pdf_cli::font_provider())
        else {
            continue;
        };
        let mut tally = [0; 4];
        count(&view.graph.atoms, false, &mut tally);
        println!(
            "{{\"file\":{:?},\"page\":{page},\"top_runs\":{},\"top_glyphs\":{},\"group_runs\":{},\"group_glyphs\":{}}}",
            path, tally[0], tally[1], tally[2], tally[3]
        );
    }
}
