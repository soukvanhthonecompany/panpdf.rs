use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;

use pdf_app::{Applied, Editor};
use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::BlockRange;

const PLACED: f64 = 0.01;

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
            '\n' => out.push_str("\\n"),
            character if character.is_control() => {
                let _ = write!(out, "\\u{:04x}", u32::from(character));
            }
            character => out.push(character),
        }
    }
    out
}

fn positions(editor: &Editor, page: usize, block: usize) -> Option<Vec<usize>> {
    if let Some(reading) = editor.block_reading(page, block) {
        return Some(
            reading
                .lines
                .iter()
                .map(|line| line.clusters.len())
                .collect(),
        );
    }
    let leaf = editor.leaf(page)?;
    let owner = leaf.overlay.blocks.get(block)?;
    Some(
        owner
            .lines
            .iter()
            .map(|line| {
                leaf.view
                    .index
                    .lines
                    .get(*line)
                    .map_or(0, |row| row.clusters.len())
            })
            .collect(),
    )
}

fn whole(editor: &Editor, page: usize, block: usize) -> Option<String> {
    let lines = positions(editor, page, block)?;
    let last = lines.len().checked_sub(1)?;
    editor.copy_text(page, block, (0, 0), (last, lines[last]))
}

fn clusters_by_block(editor: &Editor, page: usize) -> BTreeMap<usize, Vec<(i64, i64)>> {
    let mut out: BTreeMap<usize, Vec<(i64, i64)>> = BTreeMap::new();
    let Some(leaf) = editor.leaf(page) else {
        return out;
    };
    let index = &leaf.view.index;
    for (block, owner) in index.blocks.iter().enumerate() {
        let points = out.entry(block).or_default();
        for line in &owner.lines {
            for cluster in &index.lines[*line].clusters {
                let point = index.clusters[*cluster].baseline;
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "page coordinates in thousandths of a point"
                )]
                points.push((
                    (point.x * 1000.0).round() as i64,
                    (point.y * 1000.0).round() as i64,
                ));
            }
        }
        points.sort_unstable();
    }
    out
}

fn attempt(
    editor: &mut Editor,
    page: usize,
    act: impl FnOnce(&mut Editor) -> Applied,
    check: impl FnOnce(&mut Editor) -> Result<(), String>,
) -> String {
    let applied = act(editor);
    let verdict = match &applied {
        Applied::Refused(reason) => format!("refused: {reason}"),
        Applied::Unchanged => "refused: nothing changed".to_owned(),
        Applied::Changed { .. } => {
            let digest = editor.source().map_or(0, |source| {
                use std::hash::{Hash, Hasher};
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                source.as_bytes().hash(&mut hasher);
                hasher.finish()
            });
            let verdict_digest = format!(" #{digest:016x}");
            if read(editor, page) {
                match check(editor) {
                    Ok(()) => format!("ok{verdict_digest}"),
                    Err(what) => format!("wrong: {what}{verdict_digest}"),
                }
            } else {
                "wrong: the edited page does not read".to_owned()
            }
        }
    };
    if matches!(applied, Applied::Changed { .. }) {
        let _ = editor.undo();
        read(editor, page);
    }
    verdict
}

fn expect_text(editor: &Editor, page: usize, block: usize, wanted: &str) -> Result<(), String> {
    match whole(editor, page, block) {
        Some(text) if text == wanted => Ok(()),
        Some(text) => Err(format!(
            "reads {:?}, wanted {:?}",
            tail(&text),
            tail(wanted)
        )),
        None => Err("the block's text does not read".to_owned()),
    }
}

fn tail(text: &str) -> String {
    let count = text.chars().count();
    text.chars().skip(count.saturating_sub(24)).collect()
}

fn insert_at(text: &str, at: usize, what: &str) -> String {
    let mut out: String = text.chars().take(at).collect();
    out.push_str(what);
    out.extend(text.chars().skip(at));
    out
}

fn letter_of(editor: &Editor, page: usize, block: usize, text: &str) -> Option<String> {
    let leaf = editor.leaf(page)?;
    let owner = leaf.overlay.blocks.get(block)?;
    let font = owner
        .lines
        .last()
        .and_then(|line| leaf.view.index.lines.get(*line))
        .and_then(|line| line.clusters.last())
        .and_then(|cluster| leaf.view.index.clusters.get(*cluster))
        .and_then(
            |cluster| match &leaf.view.graph.atoms.get(cluster.atom)?.kind {
                pdf_paint::PaintAtomKind::Text(text) => Some(Arc::clone(&text.text)),
                _ => None,
            },
        );
    let typable = |character: &char| {
        !character.is_whitespace()
            && *character != '\u{fffd}'
            && !pdf_content::is_combining_mark(*character)
    };
    font.as_ref()
        .and_then(|font| {
            text.chars()
                .rev()
                .filter(typable)
                .find(|character| font.codes_to_write(&character.to_string()).len() == 1)
        })
        .or_else(|| text.chars().rev().find(typable))
        .map(String::from)
}

#[expect(clippy::too_many_lines, reason = "one block, action by action")]
fn scan_block(editor: &mut Editor, path: &str, page: usize, block: usize) {
    let Some(text) = whole(editor, page, block) else {
        return;
    };
    if text.trim().is_empty() {
        return;
    }
    let Some(lines) = positions(editor, page, block) else {
        return;
    };
    let last = lines.len() - 1;
    let end = (last, lines[last]);
    let stops = editor.leaf(page).map_or(0, |leaf| {
        let owned = &leaf.overlay.blocks[block].lines;
        leaf.overlay
            .carets
            .iter()
            .filter(|stop| owned.contains(&stop.line))
            .count()
    });
    let stops_verdict = if stops > 0 {
        "ok".to_owned()
    } else {
        "wrong: no caret stop".to_owned()
    };
    let letter = letter_of(editor, page, block, &text).unwrap_or_else(|| "a".to_owned());
    let chars = text.chars().count();
    let (mid_line, mut mid_stop) = lines
        .iter()
        .enumerate()
        .find(|(_, count)| **count >= 2)
        .map_or((0, 0), |(line, count)| (line, count / 2));
    let after_a_letter = editor
        .copy_text(page, block, (mid_line, 0), (mid_line, mid_stop))
        .is_some_and(|before| before.chars().any(|c| !pdf_content::is_combining_mark(c)));
    while after_a_letter
        && mid_stop < lines.get(mid_line).copied().unwrap_or(0)
        && editor
            .copy_text(page, block, (mid_line, mid_stop), (mid_line, mid_stop + 1))
            .and_then(|next| next.chars().next())
            .is_some_and(pdf_content::is_combining_mark)
    {
        mid_stop += 1;
    }
    let mid_offset = editor
        .copy_text(page, block, (0, 0), (mid_line, mid_stop))
        .map_or(0, |prefix| prefix.chars().count());

    let range = |at: (usize, usize)| BlockRange::Between { from: at, to: at };
    let space_mid = attempt(
        editor,
        page,
        |editor| editor.edit(page, block, range((mid_line, mid_stop)), " "),
        |editor| expect_text(editor, page, block, &insert_at(&text, mid_offset, " ")),
    );
    let space_end = {
        let typed = format!(" {letter}");
        attempt(
            editor,
            page,
            |editor| editor.edit(page, block, range(end), &typed),
            |editor| expect_text(editor, page, block, &format!("{text}{typed}")),
        )
    };
    let block_why = editor
        .block_refusal(page, block, range(end), &letter)
        .unwrap_or_else(|| "ok".to_owned());
    let letter_end = attempt(
        editor,
        page,
        |editor| editor.edit(page, block, range(end), &letter),
        |editor| expect_text(editor, page, block, &format!("{text}{letter}")),
    );
    let without_last = {
        let before = editor
            .copy_text(page, block, (0, 0), (last, lines[last].saturating_sub(1)))
            .unwrap_or_default();
        let cluster = editor
            .copy_text(
                page,
                block,
                (last, lines[last].saturating_sub(1)),
                (last, lines[last]),
            )
            .unwrap_or_default();
        let mut characters: Vec<char> = cluster.chars().collect();
        let joins = |character: char| matches!(u32::from(character), 0x200C | 0x200D | 0xFE00..=0xFE0F | 0xE0100..=0xE01EF);
        let whole = characters.len() < 2
            || cluster.contains('\u{FFFD}')
            || characters.last().copied().is_some_and(joins)
            || characters
                .get(characters.len().wrapping_sub(2))
                .copied()
                .is_some_and(joins);
        if whole {
            before
        } else {
            characters.pop();
            format!("{before}{}", characters.into_iter().collect::<String>())
        }
    };
    let backspace_end = if chars < 2 {
        "n/a".to_owned()
    } else {
        attempt(
            editor,
            page,
            |editor| {
                editor.edit(
                    page,
                    block,
                    BlockRange::Units {
                        at: end,
                        backwards: true,
                        count: 1,
                    },
                    "",
                )
            },
            |editor| expect_text(editor, page, block, &without_last),
        )
    };
    let enter_mid = attempt(
        editor,
        page,
        |editor| editor.edit(page, block, range((mid_line, mid_stop)), "\n"),
        |editor| expect_text(editor, page, block, &insert_at(&text, mid_offset, "\n")),
    );
    let before = clusters_by_block(editor, page);
    let moved = attempt(
        editor,
        page,
        |editor| editor.move_text_block(page, block, 5.0, 0.0),
        |editor| {
            let after = clusters_by_block(editor, page);
            for (other, points) in &before {
                let now = after.get(other).cloned().unwrap_or_default();
                let wanted: Vec<(i64, i64)> = if *other == block {
                    let mut shifted: Vec<(i64, i64)> =
                        points.iter().map(|(x, y)| (x + 5000, *y)).collect();
                    shifted.sort_unstable();
                    shifted
                } else {
                    points.clone()
                };
                let close = now.len() == wanted.len()
                    && now.iter().zip(&wanted).all(|(one, other)| {
                        #[expect(clippy::cast_precision_loss, reason = "thousandths of a point")]
                        let apart = ((one.0 - other.0).abs().max((one.1 - other.1).abs())) as f64;
                        apart <= PLACED * 1000.0
                    });
                if !close {
                    return Err(if *other == block {
                        "the block's clusters are not all 5 pt right".to_owned()
                    } else {
                        format!("block {other}, not moved, changed")
                    });
                }
            }
            Ok(())
        },
    );

    let foreign = attempt(
        editor,
        page,
        |editor| editor.edit(page, block, range(end), "กA ລ"),
        |editor| expect_text(editor, page, block, &format!("{text}กA ລ")),
    );
    println!("{{\"foreign\":\"{}\"}}", escape(&foreign));
    println!(
        "{{\"file\":\"{}\",\"page\":{page},\"block\":{block},\"chars\":{chars},\"lines\":{},\"text\":\"{}\",\"stops\":\"{}\",\"space_mid\":\"{}\",\"space_end\":\"{}\",\"letter_end\":\"{}\",\"backspace_end\":\"{}\",\"enter_mid\":\"{}\",\"move\":\"{}\",\"block_why\":\"{}\"}}",
        escape(path),
        lines.len(),
        escape(&text.chars().take(40).collect::<String>()),
        escape(&stops_verdict),
        escape(&space_mid),
        escape(&space_end),
        escape(&letter_end),
        escape(&backspace_end),
        escape(&enter_mid),
        escape(&moved),
        escape(&block_why),
    );
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = arguments.next().expect("a PDF path");
    let page_argument = arguments.next().unwrap_or_else(|| "spread".to_owned());
    let most: usize = arguments
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(40);
    let Ok(bytes) = std::fs::read(&path) else {
        println!(
            "{{\"file\":\"{}\",\"error\":\"unreadable\"}}",
            escape(&path)
        );
        return;
    };
    let mut editor = match Editor::open(ByteStore::new(SourceId::new(7), Arc::<[u8]>::from(bytes)))
    {
        Ok(editor) => editor,
        Err(error) => {
            println!(
                "{{\"file\":\"{}\",\"error\":\"open: {}\"}}",
                escape(&path),
                escape(&error)
            );
            return;
        }
    };
    editor.set_aside_restrictions();
    let pages: Vec<usize> = if page_argument == "spread" {
        let count = editor.page_count();
        let mut chosen: Vec<usize> = [1, 2, 3]
            .iter()
            .map(|quarter| count * quarter / 4)
            .collect();
        chosen.dedup();
        chosen
    } else {
        vec![page_argument.parse().unwrap_or(0)]
    };
    for page in pages {
        if !read(&mut editor, page) {
            println!(
                "{{\"file\":\"{}\",\"page\":{page},\"error\":\"page does not read\"}}",
                escape(&path)
            );
            continue;
        }
        let blocks = editor
            .leaf(page)
            .map_or(0, |leaf| leaf.overlay.blocks.len());
        if blocks == 0 {
            println!(
                "{{\"file\":\"{}\",\"page\":{page},\"error\":\"no text blocks\"}}",
                escape(&path)
            );
            continue;
        }
        let stride = blocks.div_ceil(most.max(1)).max(1);
        for block in (0..blocks).step_by(stride) {
            scan_block(&mut editor, &path, page, block);
        }
    }
}
