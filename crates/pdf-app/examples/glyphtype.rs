use std::sync::Arc;

use pdf_app::{Applied, Editor};
use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::BlockRange;

const TYPED: &str = "ທົດສອບ";

fn read(editor: &mut Editor, page: usize) -> bool {
    let Some(source) = editor.source() else {
        return false;
    };
    match pdf_session::interpret_page_fully(
        source,
        page,
        b"",
        editor.grouping(page).as_deref(),
        editor.fonts().or_else(pdf_cli::font_provider),
    ) {
        Ok(view) => {
            editor.adopt_page(page, Arc::new(view));
            true
        }
        Err(why) => {
            println!("the page does not read: {why}");
            false
        }
    }
}

fn last_stop(editor: &Editor, page: usize, block: usize) -> Option<(usize, usize)> {
    let overlay = &editor.leaf(page)?.overlay;
    let lines = &overlay.blocks.get(block)?.lines;
    let row = lines.len().checked_sub(1)?;
    let offset = overlay
        .carets
        .iter()
        .filter(|stop| stop.line == lines[row])
        .map(|stop| stop.offset)
        .max()?;
    Some((row, offset))
}

fn stops_on(editor: &Editor, page: usize, block: usize, row: usize) -> Vec<usize> {
    let Some(overlay) = editor.leaf(page).map(|leaf| &leaf.overlay) else {
        return Vec::new();
    };
    let Some(line) = overlay
        .blocks
        .get(block)
        .and_then(|owner| owner.lines.get(row).copied())
    else {
        return Vec::new();
    };
    let mut offsets: Vec<usize> = overlay
        .carets
        .iter()
        .filter(|stop| stop.line == line)
        .map(|stop| stop.offset)
        .collect();
    offsets.sort_unstable();
    offsets.dedup();
    offsets
}

fn whole(editor: &Editor, page: usize, block: usize) -> Option<String> {
    let last = last_stop(editor, page, block)?;
    if last == (0, 0) {
        return Some(String::new());
    }
    editor.copy_text(page, block, (0, 0), last)
}

fn plain(text: &str) -> String {
    pdf_edit::in_compatibility_form(text)
        .chars()
        .filter(|letter| !letter.is_whitespace())
        .collect()
}

fn first_difference(one: &str, two: &str) -> (String, String) {
    let at = one
        .chars()
        .zip(two.chars())
        .position(|(a, b)| a != b)
        .unwrap_or(0);
    let window =
        |text: &str| -> String { text.chars().skip(at.saturating_sub(20)).take(60).collect() };
    (window(one), window(two))
}

fn saved_reading(editor: &mut Editor, page: usize, block: usize) -> Option<String> {
    let export = editor.export().ok()?;
    let source = ByteStore::new(SourceId::new(0), export.bytes);
    let mut reopened = Editor::open(source).ok()?;
    if reopened.editing_restricted() {
        reopened.set_aside_restrictions();
    }
    if !read(&mut reopened, page) {
        return None;
    }
    whole(&reopened, page, block)
}

fn prepared(path: &str, page: usize, block: usize) -> Option<(Editor, String)> {
    let bytes = std::fs::read(path).expect("the file reads");
    let source = ByteStore::new(SourceId::new(0), bytes);
    let mut editor = Editor::open(source).expect("the document opens");
    if editor.editing_restricted() {
        editor.set_aside_restrictions();
    }
    if !read(&mut editor, page) {
        return None;
    }
    if !editor.read_block_by_glyphs(page, block) {
        println!("block {block} is not read by its glyphs");
        return None;
    }
    let text = whole(&editor, page, block)?;
    Some((editor, text))
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = arguments.next().expect("a file");
    let page: usize = arguments.next().expect("a page").parse().expect("a number");
    let block: usize = arguments
        .next()
        .expect("a block")
        .parse()
        .expect("a number");
    let row: usize = arguments
        .next()
        .map_or(0, |it| it.parse().expect("a number"));
    let Some((editor, before)) = prepared(&path, page, block) else {
        return;
    };
    println!("block {block} read by its glyphs:\n  {before}");
    let offsets = stops_on(&editor, page, block, row);
    drop(editor);
    println!("row {row} has {} caret stops", offsets.len());
    for offset in offsets {
        let Some((mut editor, before)) = prepared(&path, page, block) else {
            continue;
        };
        let at = (row, offset);
        match editor.edit(page, block, BlockRange::Between { from: at, to: at }, TYPED) {
            Applied::Changed { .. } => {}
            Applied::Refused(why) => {
                println!("  {offset}: refused: {why}");
                continue;
            }
            Applied::Unchanged => {
                println!("  {offset}: nothing changed");
                continue;
            }
        }
        if !read(&mut editor, page) {
            continue;
        }
        let after = whole(&editor, page, block).unwrap_or_default();
        let put_back = after.replacen(TYPED, "", 1);
        if put_back != before {
            println!("  {offset}: the block is not what it was with the text put in");
            println!("    was:  {before}");
            println!("    now:  {after}");
        }
        let Some(saved) = saved_reading(&mut editor, page, block) else {
            println!("  {offset}: the saved copy does not read");
            continue;
        };
        let (shown, kept) = (plain(&after), plain(&saved));
        if shown != kept {
            println!("  {offset}: the saved copy says something else");
            println!("    screen: {}", first_difference(&shown, &kept).0);
            println!("    saved:  {}", first_difference(&shown, &kept).1);
        }
    }
}
