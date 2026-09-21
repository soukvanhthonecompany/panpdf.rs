use std::fmt::Write as _;
use std::sync::Arc;

use pdf_app::Editor;
use pdf_bytes::{ByteStore, SourceId};

fn escaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if u32::from(control) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(control));
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

fn list(items: &[Vec<usize>]) -> String {
    let inner: Vec<String> = items
        .iter()
        .map(|group| {
            let ids: Vec<String> = group.iter().map(ToString::to_string).collect();
            format!("[{}]", ids.join(","))
        })
        .collect();
    format!("[{}]", inner.join(","))
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let path = arguments.next().expect("a PDF path");
    let page: usize = arguments
        .next()
        .and_then(|value| value.parse().ok())
        .expect("a page number, from 0");
    let scale: f64 = arguments
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1.0);
    let bytes = std::fs::read(&path).expect("readable");
    let mut editor =
        Editor::open(ByteStore::new(SourceId::new(7), Arc::<[u8]>::from(bytes))).expect("opens");
    let view = pdf_session::interpret_page_grouped(
        editor.source().expect("idle"),
        page,
        b"",
        editor.grouping(page).as_deref(),
    )
    .expect("page reads");
    editor.adopt_page(page, Arc::new(view));
    let leaf = editor.leaf(page).expect("adopted").clone();
    let device = pdf_render::DeviceTransform::for_page(
        &leaf.view.program.geometry,
        scale,
        pdf_render::RenderLimits::default(),
    )
    .expect("a page with a size");
    let lines = line_records(&leaf, &device);
    let (frames, paragraphs, unread) = frames_and_paragraphs(&editor, page, &leaf);
    println!(
        "{{\"file\":{},\"page\":{page},\"scale\":{scale},\"width\":{},\"height\":{},\"unread_blocks\":{unread},\"lines\":[{}],\"frames\":{},\"paragraphs\":{}}}",
        escaped(&path),
        device.width,
        device.height,
        lines.join(","),
        list(&frames),
        list(&paragraphs)
    );
}

fn line_records(leaf: &pdf_app::Leaf, device: &pdf_render::DeviceTransform) -> Vec<String> {
    let index = &leaf.view.index;
    let mut lines = Vec::with_capacity(index.lines.len());
    for (id, line) in index.lines.iter().enumerate() {
        let Some(first) = line
            .clusters
            .first()
            .map(|cluster| &index.clusters[*cluster])
        else {
            continue;
        };
        let text: String = leaf
            .overlay
            .clusters
            .iter()
            .filter(|cluster| cluster.line == id)
            .map(|cluster| cluster.text.as_deref().unwrap_or("\u{FFFD}"))
            .collect();
        let boxed = index.line_layout(id).map_or_else(
            || "null".to_owned(),
            |[x0, y0, x1, y1]| {
                let one = device.matrix.transform(pdf_paint::Point { x: x0, y: y0 });
                let other = device.matrix.transform(pdf_paint::Point { x: x1, y: y1 });
                format!(
                    "[{:.1},{:.1},{:.1},{:.1}]",
                    one.x.min(other.x),
                    one.y.min(other.y),
                    one.x.max(other.x),
                    one.y.max(other.y)
                )
            },
        );
        let atoms = line
            .clusters
            .iter()
            .map(|cluster| index.clusters[*cluster].atom);
        let painted = (atoms.clone().min().unwrap_or(0), atoms.max().unwrap_or(0));
        lines.push(format!(
            "{{\"id\":{id},\"x\":{:.2},\"y\":{:.2},\"em\":{:.2},\"painted\":[{},{}],\"box\":{boxed},\"text\":{}}}",
            first.baseline.x,
            first.baseline.y,
            first.em,
            painted.0,
            painted.1,
            escaped(&text)
        ));
    }
    lines
}

fn frames_and_paragraphs(
    editor: &Editor,
    page: usize,
    leaf: &pdf_app::Leaf,
) -> (Vec<Vec<usize>>, Vec<Vec<usize>>, usize) {
    let index = &leaf.view.index;
    let mut frames = Vec::with_capacity(index.blocks.len());
    let mut paragraphs = Vec::new();
    let mut unread = 0;
    for (block, owner) in index.blocks.iter().enumerate() {
        frames.push(owner.lines.clone());
        let Some(reading) = editor.block_reading(page, block) else {
            unread += 1;
            paragraphs.extend(owner.lines.iter().map(|line| vec![*line]));
            continue;
        };
        let mut current = Vec::new();
        for line in &reading.lines {
            if let Some(row) = line.row
                && let Some(id) = owner.lines.get(row)
            {
                current.push(*id);
            }
            if line.end != pdf_edit::LineEnd::Wrap
                && line.end != pdf_edit::LineEnd::WrapWithSpace
                && !current.is_empty()
            {
                paragraphs.push(std::mem::take(&mut current));
            }
        }
        if !current.is_empty() {
            paragraphs.push(current);
        }
    }
    (frames, paragraphs, unread)
}
