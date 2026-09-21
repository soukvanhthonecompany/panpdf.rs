use std::sync::Arc;

use pdf_app::{Applied, Editor};
use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::BlockRange;

const SAMPLES: &[(&str, &str)] = &[
    ("lao", " ພາສາລາວ ນ້ຳໃຈ"),
    ("thai", " ภาษาไทย น้ำใจ"),
    ("english", " English text"),
    ("vietnamese", " Tiếng Việt"),
    ("french", " café déjà"),
    ("russian", " Русский"),
    ("greek", " Ελληνικά"),
    ("chinese", " 中文"),
    ("japanese", " 日本語かな"),
    ("korean", " 한국어"),
    ("arabic", " العربية"),
    ("hebrew", " עברית"),
    ("hindi", " हिन्दी"),
    ("khmer", " ភាសាខ្មែរ"),
    ("burmese", " မြန်မာ"),
    ("digits", " 12345"),
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

fn upto(editor: &Editor, page: usize, block: usize, stop: (usize, usize)) -> Option<String> {
    if stop == (0, 0) {
        return Some(String::new());
    }
    editor.copy_text(page, block, (0, 0), stop)
}

fn last_stop(editor: &Editor, page: usize, block: usize) -> Option<(usize, usize)> {
    let rows = editor.leaf(page)?.overlay.blocks.get(block)?.lines.len();
    let mut last = None;
    for line in 0..rows * 2 + 64 {
        let mut stop = 0;
        while stop <= 8192 && upto(editor, page, block, (line, stop)).is_some() {
            last = Some((line, stop));
            stop += 1;
        }
        if stop == 0 {
            break;
        }
    }
    last
}

fn whole(editor: &Editor, page: usize, block: usize) -> Option<String> {
    let last = last_stop(editor, page, block)?;
    upto(editor, page, block, last)
}

fn plain(text: &str) -> String {
    pdf_edit::in_compatibility_form(text)
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

fn every_block(editor: &Editor, page: usize) -> Vec<Option<String>> {
    let count = editor
        .leaf(page)
        .map_or(0, |leaf| leaf.overlay.blocks.len());
    (0..count).map(|block| whole(editor, page, block)).collect()
}

#[derive(Default)]
struct Tally {
    steps: usize,
    changed: usize,
    refused: usize,
    wrong: usize,
    others_moved: usize,
}

#[derive(Clone, Copy)]
enum Place {
    End,
    Start,
}

fn scan(editor: &mut Editor, page: usize, block: usize, place: Place, tally: &mut Tally) {
    for (language, sample) in SAMPLES {
        let Some(before) = whole(editor, page, block) else {
            println!("  block {block}: no longer reads");
            return;
        };
        let others = every_block(editor, page);
        let (stop, sample) = match place {
            Place::End => (last_stop(editor, page, block), (*sample).to_owned()),
            Place::Start => (Some((0, 0)), format!("{} ", sample.trim_start())),
        };
        let Some(stop) = stop else {
            return;
        };
        tally.steps += 1;
        let applied = editor.edit(
            page,
            block,
            BlockRange::Between {
                from: stop,
                to: stop,
            },
            &sample,
        );
        match &applied {
            Applied::Changed { .. } => tally.changed += 1,
            Applied::Refused(reason) => {
                tally.refused += 1;
                println!("  block {block} {language}: refused: {reason}");
                continue;
            }
            Applied::Unchanged => {
                println!("  block {block} {language}: unchanged");
                continue;
            }
        }
        if !read(editor, page) {
            println!("  block {block} {language}: the page no longer reads");
            return;
        }
        let after = whole(editor, page, block).unwrap_or_default();
        let wanted = match place {
            Place::End => format!("{before}{sample}"),
            Place::Start => format!("{sample}{before}"),
        };
        if plain(&after) != plain(&wanted) {
            tally.wrong += 1;
            println!(
                "  block {block} {language}: reads back wrong\n    got    {:?}\n    wanted {:?}",
                after.replace('\n', "⏎"),
                wanted.replace('\n', "⏎")
            );
        }
        let now = every_block(editor, page);
        for (other, (was, is)) in others.iter().zip(&now).enumerate() {
            if other != block && was.as_deref().map(plain) != is.as_deref().map(plain) {
                tally.others_moved += 1;
                println!(
                    "  block {block} {language}: block {other} changed\n    was {was:?}\n    is  {is:?}"
                );
            }
        }
        if now.len() != others.len() {
            tally.others_moved += 1;
            println!(
                "  block {block} {language}: the page had {} blocks and has {}",
                others.len(),
                now.len()
            );
        }
    }
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
        .unwrap_or(3);
    let out = arguments.next();
    let bytes = std::fs::read(&path).expect("the file reads");
    let source = || ByteStore::new(SourceId::new(7), Arc::<[u8]>::from(bytes.clone()));
    let mut tally = Tally::default();
    let mut editor = Editor::open(source()).expect("the file opens");
    assert!(read(&mut editor, page), "the page reads");
    let texts = every_block(&editor, page);
    let candidates: Vec<usize> = texts
        .iter()
        .enumerate()
        .filter(|(_, text)| {
            text.as_ref()
                .is_some_and(|text| plain(text).chars().count() >= 3)
        })
        .map(|(block, _)| block)
        .collect();
    let stride = candidates.len().div_ceil(most.max(1)).max(1);
    let chosen: Vec<usize> = candidates.iter().copied().step_by(stride).collect();
    println!(
        "{path} page {page}: {} blocks, trying {chosen:?}",
        texts.len()
    );
    for (index, block) in chosen.iter().enumerate() {
        for place in [Place::End, Place::Start] {
            let keep = index + 1 == chosen.len() && matches!(place, Place::Start);
            editor = Editor::open(source()).expect("the file opens");
            assert!(read(&mut editor, page), "the page reads");
            let text = texts[*block].clone().unwrap_or_default();
            println!(
                "block {block} at its {}: {:?}",
                match place {
                    Place::End => "end",
                    Place::Start => "start",
                },
                text.chars().take(50).collect::<String>().replace('\n', "⏎")
            );
            scan(&mut editor, page, *block, place, &mut tally);
            if keep && let (Some(out), Some(source)) = (out.as_ref(), editor.source()) {
                std::fs::write(out, source.as_bytes()).expect("written");
            }
        }
    }
    println!(
        "summary: {} steps, {} changed, {} refused, {} read back wrong, {} other blocks changed",
        tally.steps, tally.changed, tally.refused, tally.wrong, tally.others_moved
    );
}
