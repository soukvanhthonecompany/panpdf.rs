use std::sync::Arc;

use pdf_app::{Applied, Editor};
use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::BlockRange;

const SAMPLES: &[(&str, &str)] = &[
    ("lao", " ພາສາລາວ"),
    ("thai", " ภาษาไทย"),
    ("latin", " English café"),
    ("cyrillic", " Русский"),
    ("greek", " Ελληνικά"),
    ("han", " 中文 日本語 한국어"),
    ("arabic", " العربية"),
    ("devanagari", " हिन्दी"),
];

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

fn end_of(editor: &Editor, page: usize, block: usize) -> Option<(usize, usize)> {
    let leaf = editor.leaf(page)?;
    let owner = leaf.overlay.blocks.get(block)?;
    let last = *owner.lines.last()?;
    let stops = leaf.view.index.lines.get(last)?.clusters.len();
    Some((owner.lines.len() - 1, stops))
}

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
        .unwrap_or(5);
    let out = arguments.next();
    let bytes = std::fs::read(&path).expect("the file reads");
    let mut editor = Editor::open(ByteStore::new(SourceId::new(7), Arc::<[u8]>::from(bytes)))
        .expect("the file opens");
    assert!(read(&mut editor, page), "the page reads");
    let blocks = editor
        .leaf(page)
        .map_or(0, |leaf| leaf.overlay.blocks.len());
    let stride = blocks.div_ceil(most.max(1)).max(1);
    for (position, block) in (0..blocks).step_by(stride).enumerate() {
        let keep = position == 0 && out.is_some();
        for (language, text) in SAMPLES {
            let Some(end) = end_of(&editor, page, block) else {
                break;
            };
            let applied = editor.edit(
                page,
                block,
                BlockRange::Between { from: end, to: end },
                text,
            );
            let result = match &applied {
                Applied::Changed { .. } => "changed".to_owned(),
                Applied::Unchanged => "unchanged".to_owned(),
                Applied::Refused(reason) => format!("refused: {reason}"),
            };
            println!("block {block} {language}: {result} | {}", editor.status());
            if matches!(applied, Applied::Changed { .. }) {
                if !keep {
                    let _ = editor.undo();
                }
                read(&mut editor, page);
            }
        }
    }
    if let (Some(out), Some(source)) = (out, editor.source()) {
        std::fs::write(out, source.as_bytes()).expect("written");
    }
}
