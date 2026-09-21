use std::sync::Arc;

use pdf_app::{Applied, Editor};
use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::BlockRange;

fn read(editor: &mut Editor, page: usize) {
    let source = editor.source().expect("idle").clone();
    let view = pdf_session::interpret_page_fully(
        &source,
        page,
        b"",
        editor.grouping(page).as_deref(),
        pdf_cli::font_provider(),
    )
    .expect("page reads");
    editor.adopt_page(page, Arc::new(view));
}

fn argument<'a>(arguments: &'a str, key: &str) -> Option<&'a str> {
    let start = arguments.find(&format!("{key}="))? + key.len() + 1;
    let rest = &arguments[start..];
    if let Some(quoted) = rest.strip_prefix('"') {
        let mut escaped = false;
        for (at, character) in quoted.char_indices() {
            match character {
                '\\' if !escaped => escaped = true,
                '"' if !escaped => return Some(&rest[..at + 2]),
                _ => escaped = false,
            }
        }
        return Some(rest);
    }
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    Some(&rest[..end])
}

fn unquote(debug: &str) -> String {
    let inner = debug.trim_start_matches('"').trim_end_matches('"');
    let mut out = String::new();
    let mut characters = inner.chars().peekable();
    while let Some(character) = characters.next() {
        if character != '\\' {
            out.push(character);
            continue;
        }
        match characters.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('u') => {
                let code: String = characters
                    .by_ref()
                    .skip_while(|c| *c == '{')
                    .take_while(|c| *c != '}')
                    .collect();
                if let Some(decoded) = u32::from_str_radix(&code, 16).ok().and_then(char::from_u32)
                {
                    out.push(decoded);
                }
            }
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

fn numbers(text: &str) -> Vec<f64> {
    text.split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .filter_map(|part| part.parse().ok())
        .collect()
}

fn range(arguments: &str) -> Option<BlockRange> {
    let start = arguments.find("range=")? + 6;
    let end = arguments.find(" text=")?;
    let text = &arguments[start..end];
    let values = numbers(text);
    if text.starts_with("Between") {
        let [a, b, c, d] = values[..] else {
            return None;
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        return Some(BlockRange::Between {
            from: (a as usize, b as usize),
            to: (c as usize, d as usize),
        });
    }
    let [a, b, count] = values[..] else {
        return None;
    };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some(BlockRange::Units {
        at: (a as usize, b as usize),
        backwards: text.contains("backwards: true"),
        count: count as usize,
    })
}

fn anchors_of(editor: &Editor, page: usize, arguments: &str) -> Vec<String> {
    let count: usize = argument(arguments, "anchors")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    editor
        .leaf(page)
        .and_then(|leaf| {
            leaf.overlay
                .blocks
                .iter()
                .find(|block| block.anchors.len() == count)
                .map(|block| block.anchors.clone())
        })
        .unwrap_or_default()
}

fn show(editor: &Editor, page: usize) {
    let leaf = editor.leaf(page).expect("read");
    let frames = editor.frame_boxes(page);
    for (number, block) in leaf.overlay.blocks.iter().enumerate() {
        let rows: Vec<String> = leaf.view.index.blocks[number]
            .lines
            .iter()
            .map(|line| {
                let clusters = &leaf.view.index.lines[*line].clusters;
                let seed = &leaf.view.index.clusters[clusters[0]];
                let text: String = leaf
                    .overlay
                    .clusters
                    .iter()
                    .filter(|cluster| cluster.line == *line)
                    .map(|cluster| cluster.text.clone().unwrap_or_default())
                    .collect();
                format!(
                    "({:.1},{:.1}) {:?}",
                    seed.baseline.x,
                    seed.baseline.y,
                    text.chars().take(30).collect::<String>()
                )
            })
            .collect();
        println!(
            "  block {number} turn {:.3} frame {:?}\n    {}",
            block.turn,
            frames
                .get(number)
                .map(|f| f.map(|v| (v * 10.0).round() / 10.0)),
            rows.join("\n    ")
        );
    }
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = arguments.next().expect("a PDF path");
    let trace = arguments.next().expect("a trace");
    let out = arguments.next();
    let bytes = std::fs::read(&path).expect("readable");
    let mut editor =
        Editor::open(ByteStore::new(SourceId::new(7), Arc::<[u8]>::from(bytes))).expect("opens");
    let mut page = 0;
    read(&mut editor, page);
    println!("opened");
    show(&editor, page);
    for line in std::fs::read_to_string(&trace).expect("trace").lines() {
        if !line.contains("\"record\":\"command\"") {
            continue;
        }
        let field = |name: &str| {
            let key = format!("\"{name}\":\"");
            let start = line.find(&key)? + key.len();
            let rest = &line[start..];
            let mut escaped = false;
            for (at, character) in rest.char_indices() {
                match character {
                    '\\' if !escaped => escaped = true,
                    '"' if !escaped => {
                        return Some(rest[..at].replace("\\\"", "\"").replace("\\\\", "\\"));
                    }
                    _ => escaped = false,
                }
            }
            None
        };
        let (Some(command), Some(arguments)) = (field("command"), field("arguments")) else {
            continue;
        };
        if let Some(number) = argument(&arguments, "page").and_then(|v| v.parse().ok()) {
            page = number;
        }
        let applied = match command.as_str() {
            "Type" => {
                let block = argument(&arguments, "block").and_then(|v| v.parse().ok());
                let text = argument(&arguments, "text")
                    .map(unquote)
                    .unwrap_or_default();
                match (block, range(&arguments)) {
                    (Some(block), Some(range)) => editor.edit(page, block, range, &text),
                    _ => Applied::Refused(format!("unread: {arguments}").into()),
                }
            }
            "MoveBlock" => {
                let anchors = anchors_of(&editor, page, &arguments);
                let dx = argument(&arguments, "dx")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0.0);
                let dy = argument(&arguments, "dy")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0.0);
                editor.move_block(page, &anchors, dx, dy)
            }
            "SetAngles" | "SetSize" => {
                let anchors = anchors_of(&editor, page, &arguments);
                let job = if command == "SetSize" {
                    let points = argument(&arguments, "points")
                        .and_then(|v| numbers(v).first().copied())
                        .unwrap_or(12.0);
                    editor.begin_set_size(page, &anchors, points)
                } else {
                    let turn =
                        argument(&arguments, "turn").and_then(|v| numbers(v).first().copied());
                    let slant =
                        argument(&arguments, "slant").and_then(|v| numbers(v).first().copied());
                    editor.begin_set_angles(page, &anchors, turn, slant)
                };
                match job {
                    Some(job) => editor.adopt(job.run()),
                    None => Applied::Refused("busy".into()),
                }
            }
            "Undo" => editor.undo(),
            other => {
                println!("skipped {other} {arguments}");
                continue;
            }
        };
        println!(
            "\n{command} {arguments}\n  -> {applied:?} · {}",
            editor.status()
        );
        read(&mut editor, page);
        show(&editor, page);
    }
    if let Some(out) = out {
        std::fs::write(out, editor.export().expect("exports").bytes).expect("written");
    }
}
