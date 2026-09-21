use std::sync::Arc;

use pdf_app::{Applied, Editor};
use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::{BlockRange, TextStyle};

const TYPED: &[(&str, &str)] = &[
    ("lao-date", " ວັນທີ 14 ກັນຍາ 2026 ນ້ຳໃຈ ເຈົ້າ ປີໃໝ່ ຂໍ້ມູນ ກິ່ງ"),
    ("thai-date", " วันที่ 14 กันยายน พ.ศ. 2569 น้ำใจ ผู้ใหญ่ ปั๊ม กี่ ฤๅษี"),
    ("vietnamese", " Thứ Hai, ngày 14 tháng 9 năm 2026 Việt"),
    ("devanagari", " १४ सितंबर २०२६ क्षत्रिय"),
    ("others", " 14 сентября Σεπτεμβρίου 9月14日 9월 Ñoño"),
];

fn styles() -> Vec<(&'static str, TextStyle)> {
    vec![
        (
            "red",
            TextStyle {
                fill: Some([0.8, 0.0, 0.0]),
                ..TextStyle::default()
            },
        ),
        (
            "size",
            TextStyle {
                size: Some(20.0),
                ..TextStyle::default()
            },
        ),
        (
            "bold",
            TextStyle {
                bold: Some(true),
                ..TextStyle::default()
            },
        ),
        (
            "italic",
            TextStyle {
                italic: Some(true),
                ..TextStyle::default()
            },
        ),
        (
            "underline",
            TextStyle {
                underline: Some(true),
                ..TextStyle::default()
            },
        ),
    ]
}

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

fn report(editor: &mut Editor, page: usize, block: usize, op: &str, applied: &Applied, out: &str) {
    let result = match applied {
        Applied::Changed { .. } => "changed".to_owned(),
        Applied::Unchanged => "unchanged".to_owned(),
        Applied::Refused(reason) => format!("refused: {reason}"),
    };
    println!("block {block} {op}: {result}");
    if matches!(applied, Applied::Changed { .. }) {
        if let Some(source) = editor.source() {
            std::fs::write(format!("{out}/b{block}-{op}.pdf"), source.as_bytes()).expect("written");
        }
        let _ = editor.undo();
        read(editor, page);
    }
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = arguments.next().expect("a PDF path");
    let page: usize = arguments.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let most: usize = arguments.next().and_then(|v| v.parse().ok()).unwrap_or(5);
    let out = arguments.next().expect("an output directory");
    std::fs::create_dir_all(&out).expect("output directory");
    let bytes = std::fs::read(&path).expect("the file reads");
    let mut editor = Editor::open(ByteStore::new(SourceId::new(7), Arc::<[u8]>::from(bytes)))
        .expect("the file opens");
    assert!(read(&mut editor, page), "the page reads");
    if let Some(source) = editor.source() {
        std::fs::write(format!("{out}/original.pdf"), source.as_bytes()).expect("written");
    }
    let blocks = editor
        .leaf(page)
        .map_or(0, |leaf| leaf.overlay.blocks.len());
    let stride = blocks.div_ceil(most.max(1)).max(1);
    let chosen: Vec<usize> = match std::env::var("STYLESCAN_BLOCKS") {
        Ok(list) => list
            .split(',')
            .filter_map(|v| v.parse().ok())
            .filter(|block| *block < blocks)
            .collect(),
        Err(_) => (0..blocks).step_by(stride).collect(),
    };
    for block in chosen {
        if let Some(end) = end_of(&editor, page, block) {
            let text = editor
                .copy_text(page, block, (0, 0), end)
                .unwrap_or_default()
                .replace('\n', " / ");
            println!(
                "block {block} text: {}",
                text.chars().take(80).collect::<String>()
            );
        }
        if let Ok(words) = std::env::var("STYLESCAN_WORDS") {
            for (index, word) in words.split('|').enumerate() {
                let Some(end) = end_of(&editor, page, block) else {
                    break;
                };
                let text = format!(" {word}");
                let applied = editor.edit(
                    page,
                    block,
                    BlockRange::Between { from: end, to: end },
                    &text,
                );
                let op = format!("w{index}");
                print!("[{word}] ");
                report(&mut editor, page, block, &op, &applied, &out);
            }
            continue;
        }
        for (op, style) in styles() {
            let Some(end) = end_of(&editor, page, block) else {
                break;
            };
            let applied = editor.style(
                page,
                block,
                BlockRange::Between {
                    from: (0, 0),
                    to: end,
                },
                style,
            );
            report(&mut editor, page, block, op, &applied, &out);
        }
        for (op, text) in TYPED {
            let Some(end) = end_of(&editor, page, block) else {
                break;
            };
            let applied = editor.edit(
                page,
                block,
                BlockRange::Between { from: end, to: end },
                text,
            );
            report(&mut editor, page, block, op, &applied, &out);
        }
    }
}
