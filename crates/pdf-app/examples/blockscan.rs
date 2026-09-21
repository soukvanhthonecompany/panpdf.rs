use std::fmt::Write as _;
use std::sync::Arc;

use pdf_app::wording::{Done, Layout, Message};
use pdf_app::{Applied, Editor};
use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::BlockRange;

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

fn outcome(applied: &Applied) -> String {
    match applied {
        Applied::Changed { .. } => "changed".to_owned(),
        Applied::Unchanged => "unchanged".to_owned(),
        Applied::Refused(reason) => format!("refused: {reason}"),
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

fn undo_if_changed(editor: &mut Editor, page: usize, applied: &Applied) {
    if matches!(applied, Applied::Changed { .. }) {
        let _ = editor.undo();
        read(editor, page);
    }
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = arguments.next().expect("a PDF path");
    let page_argument = arguments.next().unwrap_or_else(|| "0".to_owned());
    let most: usize = arguments
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(usize::MAX);
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
        scan_page(&mut editor, &path, page, most);
    }
}

#[expect(clippy::too_many_lines, reason = "one scan, step by step")]
fn scan_page(editor: &mut Editor, path: &str, page: usize, most: usize) {
    if !read(editor, page) {
        println!(
            "{{\"file\":\"{}\",\"page\":{page},\"error\":\"page does not read\"}}",
            escape(path)
        );
        return;
    }
    let blocks = editor
        .leaf(page)
        .map_or(0, |leaf| leaf.overlay.blocks.len());
    if blocks == 0 {
        println!(
            "{{\"file\":\"{}\",\"page\":{page},\"error\":\"no text blocks\"}}",
            escape(path)
        );
        return;
    }
    let stride = blocks.div_ceil(most.max(1)).max(1);
    for block in (0..blocks).step_by(stride) {
        let Some(leaf) = editor.leaf(page).cloned() else {
            break;
        };
        let owner = &leaf.overlay.blocks[block];
        let Some(&last_line) = owner.lines.last() else {
            continue;
        };
        let rows = owner.lines.len();
        let stops = leaf
            .view
            .index
            .lines
            .get(last_line)
            .map_or(0, |line| line.clusters.len());
        let text = editor
            .copy_text(page, block, (0, 0), (rows - 1, stops))
            .unwrap_or_default();
        let last_font = leaf
            .view
            .index
            .lines
            .get(last_line)
            .and_then(|line| line.clusters.last())
            .and_then(|cluster| leaf.view.index.clusters.get(*cluster))
            .and_then(
                |cluster| match &leaf.view.graph.atoms.get(cluster.atom)?.kind {
                    pdf_paint::PaintAtomKind::Text(text) => Some(Arc::clone(&text.text)),
                    _ => None,
                },
            );
        let typable = |character: &char| !character.is_whitespace() && *character != '\u{fffd}';
        let letter = last_font
            .as_ref()
            .and_then(|font| {
                text.chars()
                    .rev()
                    .filter(typable)
                    .find(|character| font.codes_to_write(&character.to_string()).len() == 1)
            })
            .or_else(|| text.chars().rev().find(typable))
            .map(String::from)
            .unwrap_or_default();
        let anchors = owner.anchors.clone();
        let mut fonts: Vec<String> = Vec::new();
        for line in &owner.lines {
            let Some(row) = leaf.view.index.lines.get(*line) else {
                continue;
            };
            for cluster in &row.clusters {
                let Some(found) = leaf.view.index.clusters.get(*cluster) else {
                    continue;
                };
                if let Some(pdf_paint::PaintAtomKind::Text(text)) =
                    leaf.view.graph.atoms.get(found.atom).map(|atom| &atom.kind)
                {
                    let request = text.font_request.as_deref().or_else(|| {
                        text.substitution
                            .as_deref()
                            .map(|found| found.request.as_ref())
                    });
                    let name = request.map_or_else(
                        || if text.type3 { "type3" } else { "unnamed" }.to_owned(),
                        |request| {
                            format!(
                                "{} {} {}",
                                String::from_utf8_lossy(&request.base_font),
                                String::from_utf8_lossy(&request.subtype),
                                request.encoding.clone().unwrap_or_default()
                            )
                        },
                    );
                    let entry = format!(
                        "{name} embedded={} substituted={} tounicode={}",
                        text.program.is_some(),
                        text.substitution
                            .as_ref()
                            .map_or_else(String::new, |found| found
                                .primary
                                .identity
                                .family
                                .clone()),
                        text.text.len()
                    );
                    if !fonts.contains(&entry) {
                        fonts.push(entry);
                    }
                }
            }
        }

        let moved = editor.move_block(page, &anchors, 5.0, 0.0);
        let move_result = outcome(&moved);
        undo_if_changed(editor, page, &moved);

        let end = (rows - 1, stops);
        let (type_result, typed_path) = if letter.is_empty() {
            ("no letter".to_owned(), String::new())
        } else {
            let typed = editor.edit(
                page,
                block,
                BlockRange::Between { from: end, to: end },
                &letter,
            );
            let path = if matches!(
                editor.status(),
                Message::Done(
                    Done::Styled
                        | Done::Typed {
                            layout: Layout::InFrame,
                            ..
                        }
                )
            ) {
                "block"
            } else {
                "row"
            };
            let result = outcome(&typed);
            undo_if_changed(editor, page, &typed);
            (result, path.to_owned())
        };

        let block_reason = if letter.is_empty() {
            None
        } else {
            editor.block_refusal(
                page,
                block,
                BlockRange::Between { from: end, to: end },
                &letter,
            )
        };
        let entered = editor.edit(
            page,
            block,
            BlockRange::Between { from: end, to: end },
            "\n",
        );
        let enter_result = outcome(&entered);
        undo_if_changed(editor, page, &entered);

        println!(
            "{{\"file\":\"{}\",\"page\":{page},\"block\":{block},\"rows\":{rows},\"chars\":{},\"text\":\"{}\",\"move\":\"{}\",\"type\":\"{}\",\"type_path\":\"{}\",\"enter\":\"{}\",\"block_reason\":\"{}\",\"fonts\":\"{}\"}}",
            escape(path),
            text.chars().count(),
            escape(&text.chars().take(40).collect::<String>()),
            escape(&move_result),
            escape(&type_result),
            typed_path,
            escape(&enter_result),
            escape(block_reason.as_deref().unwrap_or("")),
            escape(&fonts.join(" | ")),
        );
    }
}
