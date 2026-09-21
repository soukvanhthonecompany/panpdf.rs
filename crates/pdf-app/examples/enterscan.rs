use std::collections::BTreeSet;
use std::sync::Arc;

use pdf_app::{Applied, Editor};
use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::BlockRange;

fn read(editor: &mut Editor) {
    let view = pdf_session::interpret_page_grouped(
        editor.source().expect("idle"),
        0,
        b"",
        editor.grouping(0).as_deref(),
    )
    .expect("page reads");
    editor.adopt_page(0, Arc::new(view));
}

fn row_stops(editor: &Editor, block: usize) -> Vec<usize> {
    let Some(leaf) = editor.leaf(0) else {
        return Vec::new();
    };
    let Some(owner) = leaf.overlay.blocks.get(block) else {
        return Vec::new();
    };
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
        .collect()
}

fn offset_of(editor: &Editor, block: usize, stop: (usize, usize)) -> Option<usize> {
    if stop == (0, 0) {
        return Some(0);
    }
    editor
        .copy_text(0, block, (0, 0), stop)
        .map(|text| text.chars().count())
}

fn reading_stops(editor: &Editor, block: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let most = row_stops(editor, block).len() * 2 + 64;
    for line in 0..most {
        let mut stop = 0;
        while stop <= 4096 && offset_of(editor, block, (line, stop)).is_some() {
            out.push((line, stop));
            stop += 1;
        }
        if stop == 0 {
            break;
        }
    }
    out
}

fn whole(editor: &Editor, block: usize) -> Option<String> {
    let last = *reading_stops(editor, block).last()?;
    editor.copy_text(0, block, (0, 0), last)
}

fn stops(editor: &Editor, block: usize) -> Vec<((usize, usize), usize)> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for stop in reading_stops(editor, block) {
        if let Some(offset) = offset_of(editor, block, stop)
            && seen.insert(offset)
        {
            out.push((stop, offset));
        }
    }
    out
}

fn past_frame(editor: &Editor, block: usize) -> Option<f64> {
    let leaf = editor.leaf(0)?;
    let laid = leaf.overlay.blocks.get(block)?.box_pixels;
    let frame = editor.frame_boxes(0).get(block)?;
    (laid[2] > frame[2] + 1.0).then_some(laid[2] - frame[2])
}

fn insert(text: &str, at: usize, what: &str) -> String {
    let mut chars: Vec<char> = text.chars().collect();
    let tail: Vec<char> = chars.split_off(at.min(chars.len()));
    let mut out: String = chars.into_iter().collect();
    out.push_str(what);
    out.extend(tail);
    out
}

#[derive(Default)]
struct Tally {
    positions: usize,
    refused: Vec<String>,
    wrong_text: usize,
    wrong_caret: usize,
    not_restored: usize,
    past_frame: usize,
    lost_caret: usize,
    typing_refused: usize,
}

fn show(text: &str) -> String {
    text.replace('\n', "⏎")
}

#[expect(clippy::too_many_lines, reason = "one scenario, step by step")]
fn scan_block(editor: &mut Editor, block: usize, name: &str) -> Tally {
    let mut tally = Tally::default();
    let Some(original) = whole(editor, block) else {
        println!("{name} block {block}: no text");
        return tally;
    };
    println!(
        "{name} block {block}: {} rows, {} chars: {}",
        row_stops(editor, block).len(),
        original.chars().count(),
        show(&original.chars().take(60).collect::<String>())
    );
    let positions = stops(editor, block);
    for (index, (_, offset)) in positions.iter().enumerate() {
        let current = stops(editor, block);
        let Some(&(stop, _)) = current.iter().find(|(_, at)| at == offset) else {
            println!("  @{offset}: position no longer exists");
            tally.lost_caret += 1;
            continue;
        };
        tally.positions += 1;
        let before = whole(editor, block).unwrap_or_default();
        let applied = editor.edit(
            0,
            block,
            BlockRange::Between {
                from: stop,
                to: stop,
            },
            "\n",
        );
        if let Applied::Refused(reason) = &applied {
            if tally.refused.len() < 3 {
                println!("  @{offset} {stop:?}: Enter refused: {reason}");
            }
            tally.refused.push(reason.to_string());
            continue;
        }
        read(editor);
        let after = whole(editor, block).unwrap_or_default();
        let wanted = insert(&before, *offset, "\n");
        if after != wanted {
            tally.wrong_text += 1;
            if tally.wrong_text <= 5 {
                println!(
                    "  @{offset} {stop:?}: Enter gave {:?}\n      wanted {:?}",
                    show(&after),
                    show(&wanted)
                );
            }
        }
        if let Some(over) = past_frame(editor, block) {
            tally.past_frame += 1;
            if tally.past_frame <= 3 {
                println!("  @{offset}: a line {over:.1} px past the frame after Enter");
            }
        }
        let Some(caret) = editor.landed_caret() else {
            tally.lost_caret += 1;
            println!("  @{offset}: no caret after Enter");
            continue;
        };
        if offset_of(editor, block, caret) != Some(offset + 1) {
            tally.wrong_caret += 1;
            if tally.wrong_caret <= 5 {
                println!(
                    "  @{offset}: caret after Enter at {caret:?} = char {:?}, wanted {}",
                    offset_of(editor, block, caret),
                    offset + 1
                );
            }
        }
        let applied = editor.edit(
            0,
            block,
            BlockRange::Between {
                from: caret,
                to: caret,
            },
            "x",
        );
        read(editor);
        let typed = matches!(applied, Applied::Changed { .. });
        let typed_caret = if typed { editor.landed_caret() } else { None };
        if !typed || typed_caret.is_none() {
            println!("  @{offset}: typing after Enter: {applied:?}");
            tally.typing_refused += 1;
        }
        let at = typed_caret.unwrap_or(caret);
        let applied = editor.edit(
            0,
            block,
            BlockRange::Units {
                at,
                backwards: true,
                count: if typed { 2 } else { 1 },
            },
            "",
        );
        read(editor);
        let restored = whole(editor, block).unwrap_or_default();
        if restored != before {
            tally.not_restored += 1;
            if tally.not_restored <= 5 {
                println!(
                    "  @{offset}: Enter, x, Backspace x2 ({applied:?}) left {:?}\n      was {:?}",
                    show(&restored),
                    show(&before)
                );
            }
            while whole(editor, block).as_deref() != Some(original.as_str())
                && matches!(editor.undo(), Applied::Changed { .. })
            {
                read(editor);
            }
            read(editor);
        }
        if index % 25 == 0 {
            eprint!(".");
        }
    }
    eprintln!();

    let rows = row_stops(editor, block);
    if let Some(&end) = reading_stops(editor, block).last() {
        let paste = format!(" {original} {original} {original}").replace('\n', " ");
        let applied = editor.edit(0, block, BlockRange::Between { from: end, to: end }, &paste);
        read(editor);
        let rows_after = row_stops(editor, block).len();
        println!(
            "  paste {} chars: {} -> rows {} -> {}, past frame {:?}, status {:?}",
            paste.chars().count(),
            match &applied {
                Applied::Changed { .. } => "changed".to_owned(),
                other => format!("{other:?}"),
            },
            rows.len(),
            rows_after,
            past_frame(editor, block),
            editor.status()
        );
        if matches!(applied, Applied::Changed { .. }) {
            let got = whole(editor, block).unwrap_or_default();
            let wanted = format!("{original}{paste}");
            if got != wanted {
                println!(
                    "  paste text differs: got {} chars, wanted {}; first difference at char {:?}",
                    got.chars().count(),
                    wanted.chars().count(),
                    got.chars().zip(wanted.chars()).position(|(a, b)| a != b)
                );
            }
            let _ = editor.undo();
            read(editor);
            if whole(editor, block).as_deref() != Some(original.as_str()) {
                println!("  undo after paste did not give the block back");
            }
        }
    }
    tally
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = arguments.next().expect("a PDF path");
    let only: Option<usize> = arguments.next().and_then(|value| value.parse().ok());
    let bytes = std::fs::read(&path).expect("readable");
    let mut editor =
        Editor::open(ByteStore::new(SourceId::new(7), Arc::<[u8]>::from(bytes))).expect("opens");
    read(&mut editor);
    let blocks = editor.leaf(0).map_or(0, |leaf| leaf.overlay.blocks.len());
    for block in 0..blocks {
        if only.is_some_and(|wanted| wanted != block) {
            continue;
        }
        let tally = scan_block(&mut editor, block, &path);
        let mut reasons: Vec<&String> = tally.refused.iter().collect();
        reasons.sort();
        reasons.dedup();
        println!(
            "SUMMARY block {block}: positions {} · Enter refused {} {:?} · wrong text {} · wrong caret {} · not restored {} · past frame {} · lost caret {} · typing refused {}",
            tally.positions,
            tally.refused.len(),
            reasons,
            tally.wrong_text,
            tally.wrong_caret,
            tally.not_restored,
            tally.past_frame,
            tally.lost_caret,
            tally.typing_refused
        );
    }
}
