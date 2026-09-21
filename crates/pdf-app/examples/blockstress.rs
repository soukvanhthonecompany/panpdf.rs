use std::sync::Arc;

use pdf_app::{Applied, Editor};
use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::BlockRange;

const ONE_AT_A_TIME: usize = 40;

const TYPED: &str = "ทดสอบ test 123";

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
        .filter(|character| !character.is_whitespace())
        .collect()
}

fn every_block(editor: &Editor, page: usize) -> Vec<Option<String>> {
    let count = editor
        .leaf(page)
        .map_or(0, |leaf| leaf.overlay.blocks.len());
    (0..count)
        .map(|block| whole(editor, page, block).map(|text| plain(&text)))
        .collect()
}

fn size_of(editor: &Editor, page: usize, block: usize) -> Option<f64> {
    editor
        .leaf(page)?
        .overlay
        .blocks
        .get(block)?
        .shape
        .map(|shape| shape.size)
}

#[derive(Default)]
struct Tally {
    steps: usize,
    refused: usize,
    wrong: usize,
    others: usize,
    unread: usize,
}

struct Run<'a> {
    editor: Editor,
    page: usize,
    block: usize,
    tally: &'a mut Tally,
    others: Vec<Option<String>>,
}

impl Run<'_> {
    fn step(&mut self, name: &str, applied: &Applied, wanted: Option<&str>) -> bool {
        self.tally.steps += 1;
        match applied {
            Applied::Changed { .. } => {}
            Applied::Refused(reason) => {
                self.tally.refused += 1;
                println!("  b{} {name}: refused: {reason}", self.block);
                return false;
            }
            Applied::Unchanged => {
                self.tally.refused += 1;
                println!("  b{} {name}: nothing changed", self.block);
                return false;
            }
        }
        if !read(&mut self.editor, self.page) {
            self.tally.unread += 1;
            println!("  b{} {name}: the page no longer reads", self.block);
            return false;
        }
        let got = whole(&self.editor, self.page, self.block).map(|text| plain(&text));
        if let Some(wanted) = wanted
            && got.as_deref() != Some(&plain(wanted))
        {
            self.tally.wrong += 1;
            println!(
                "  b{} {name}: reads {:?}, wanted {:?}",
                self.block,
                got,
                plain(wanted)
            );
            return false;
        }
        let now = every_block(&self.editor, self.page);
        let moved: Vec<usize> = (0..self.others.len().max(now.len()))
            .filter(|other| *other != self.block && self.others.get(*other) != now.get(*other))
            .collect();
        if !moved.is_empty() {
            self.tally.others += 1;
            println!("  b{} {name}: other blocks changed: {moved:?}", self.block);
            for other in &moved {
                println!(
                    "    b{other} was {:?}\n    b{other} now {:?}",
                    self.others.get(*other),
                    now.get(*other)
                );
            }
            self.others = now;
        }
        true
    }

    fn style_all(&mut self, name: &str, size: f64) -> bool {
        let Some(end) = last_stop(&self.editor, self.page, self.block) else {
            return false;
        };
        let before = whole(&self.editor, self.page, self.block).unwrap_or_default();
        let applied = self.editor.style(
            self.page,
            self.block,
            BlockRange::Between {
                from: (0, 0),
                to: end,
            },
            pdf_edit::TextStyle {
                size: Some(size),
                ..pdf_edit::TextStyle::default()
            },
        );
        self.step(name, &applied, Some(&before))
    }

    fn type_at_end(&mut self, name: &str, text: &str) -> bool {
        let before = whole(&self.editor, self.page, self.block).unwrap_or_default();
        let end = last_stop(&self.editor, self.page, self.block).unwrap_or((0, 0));
        let applied = self.editor.edit(
            self.page,
            self.block,
            BlockRange::Between { from: end, to: end },
            text,
        );
        self.step(name, &applied, Some(&format!("{before}{text}")))
    }

    fn delete_all(&mut self, name: &str) -> bool {
        let Some(end) = last_stop(&self.editor, self.page, self.block) else {
            return false;
        };
        let applied = self.editor.edit(
            self.page,
            self.block,
            BlockRange::Between {
                from: (0, 0),
                to: end,
            },
            "",
        );
        self.step(name, &applied, Some(""))
    }

    fn backspace_away(&mut self) -> bool {
        let mut text = whole(&self.editor, self.page, self.block).unwrap_or_default();
        let letters = plain(&text).chars().count();
        if letters > ONE_AT_A_TIME {
            let Some(end) = last_stop(&self.editor, self.page, self.block) else {
                return false;
            };
            let mut keep = end;
            let mut kept = 0;
            while kept < ONE_AT_A_TIME && keep != (0, 0) {
                keep = if keep.1 > 0 {
                    (keep.0, keep.1 - 1)
                } else {
                    let row = keep.0 - 1;
                    let offset = self
                        .editor
                        .leaf(self.page)
                        .and_then(|leaf| {
                            let line = *leaf.overlay.blocks[self.block].lines.get(row)?;
                            leaf.overlay
                                .carets
                                .iter()
                                .filter(|stop| stop.line == line)
                                .map(|stop| stop.offset)
                                .max()
                        })
                        .unwrap_or(0);
                    (row, offset)
                };
                kept += 1;
            }
            let rest = self
                .editor
                .copy_text(self.page, self.block, keep, end)
                .unwrap_or_default();
            let applied = self.editor.edit(
                self.page,
                self.block,
                BlockRange::Between {
                    from: (0, 0),
                    to: keep,
                },
                "",
            );
            if !self.step("delete the head", &applied, Some(&rest)) {
                return false;
            }
            text = rest;
        }
        for count in 0..=ONE_AT_A_TIME * 4 {
            let Some(at) = last_stop(&self.editor, self.page, self.block) else {
                return false;
            };
            if at == (0, 0) && plain(&text).is_empty() {
                return true;
            }
            let applied = self.editor.edit(
                self.page,
                self.block,
                BlockRange::Units {
                    at,
                    backwards: true,
                    count: 1,
                },
                "",
            );
            let expect = text.clone();
            if !self.step(&format!("backspace {count}"), &applied, None) {
                return false;
            }
            text = whole(&self.editor, self.page, self.block).unwrap_or_default();
            let (was, now) = (plain(&expect), plain(&text));
            if !(now.len() < was.len() || text.len() < expect.len()) || !was.starts_with(&now) {
                self.tally.wrong += 1;
                println!(
                    "  b{} backspace {count}: {:?} became {:?}",
                    self.block, was, now
                );
                return false;
            }
        }
        println!("  b{}: Backspace never emptied it", self.block);
        false
    }
}

fn open(source: ByteStore) -> Result<(Editor, bool), String> {
    let mut editor = Editor::open(source)?;
    let restricted = editor.editing_restricted();
    if restricted {
        assert!(editor.set_aside_restrictions());
    }
    Ok((editor, restricted))
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
        .unwrap_or(4);
    let bytes = std::fs::read(&path).expect("the file reads");
    let source = || ByteStore::new(SourceId::new(7), Arc::<[u8]>::from(bytes.clone()));
    let open = || open(source());
    let Ok((mut editor, restricted)) = open() else {
        println!("{path}: does not open");
        return;
    };
    if restricted {
        println!("{path}: editing restricted by its author, set aside");
    }
    if editor.editing_restricted() {
        println!("{path}: restrictions were not set aside");
        return;
    }
    if !read(&mut editor, page) {
        println!("{path} page {page}: does not read");
        return;
    }
    let texts = every_block(&editor, page);
    let candidates: Vec<usize> = texts
        .iter()
        .enumerate()
        .filter(|(_, text)| text.as_ref().is_some_and(|text| text.chars().count() >= 2))
        .map(|(block, _)| block)
        .collect();
    let stride = candidates.len().div_ceil(most.max(1)).max(1);
    let chosen: Vec<usize> = candidates.iter().copied().step_by(stride).collect();
    println!(
        "{path} page {page}: {} blocks, trying {chosen:?}",
        texts.len()
    );
    let mut total = Tally::default();
    for block in chosen {
        let mut tally = Tally::default();
        let (mut editor, _) = open().expect("the file opens");
        assert!(read(&mut editor, page), "the page reads");
        let others = every_block(&editor, page);
        let size = size_of(&editor, page, block).unwrap_or(12.0);
        let mut run = Run {
            editor,
            page,
            block,
            tally: &mut tally,
            others,
        };
        let mut reached = "start";
        'steps: {
            if !run.style_all("larger", (size * 1.25).round()) {
                break 'steps;
            }
            reached = "larger";
            if !run.backspace_away() {
                break 'steps;
            }
            reached = "emptied";
            if !run.type_at_end("type into the empty block", TYPED) {
                break 'steps;
            }
            reached = "retyped";
            if !run.style_all("smaller", (size * 0.8).round().max(4.0)) {
                break 'steps;
            }
            reached = "smaller";
            if !run.delete_all("select all and delete") {
                break 'steps;
            }
            if !run.type_at_end("type again", "ok") {
                break 'steps;
            }
            reached = "done";
        }
        println!(
            "{{\"file\": {:?}, \"page\": {page}, \"block\": {block}, \"reached\": {reached:?}, \"steps\": {}, \"refused\": {}, \"wrong\": {}, \"others\": {}, \"unread\": {}}}",
            path, tally.steps, tally.refused, tally.wrong, tally.others, tally.unread
        );
        total.steps += tally.steps;
        total.refused += tally.refused;
        total.wrong += tally.wrong;
        total.others += tally.others;
        total.unread += tally.unread;
    }
    println!(
        "summary: {} steps, {} refused, {} read back wrong, {} other blocks changed, {} unreadable",
        total.steps, total.refused, total.wrong, total.others, total.unread
    );
}
