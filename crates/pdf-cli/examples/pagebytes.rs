use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_paint::{PaintAtom, PaintAtomKind, PathSegment};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: pagebytes <file.pdf> [page]");
        return;
    };
    let at: usize = args.next().and_then(|n| n.parse().ok()).unwrap_or(0);
    let bytes = std::fs::read(&path).expect("the file");
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
    let before = resident();
    let view = pdf_session::interpret_page(&source, at).expect("an interpreted page");
    let holding = resident();

    let (mut own, mut clips, mut clip_segments, mut elements, mut marks) = (0, 0, 0, 0, 0);
    let mut kinds: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for atom in &view.graph.atoms {
        marks += atom.marks.len();
        let state = match &atom.kind {
            PaintAtomKind::Path(paint) => {
                own += paint.path.segments.len();
                *kinds.entry("path").or_default() += 1;
                &paint.state
            }
            PaintAtomKind::Text(paint) => {
                elements += paint.elements.len();
                *kinds.entry("text").or_default() += 1;
                &paint.state
            }
            PaintAtomKind::Image(paint) => {
                *kinds.entry("image").or_default() += 1;
                &paint.state
            }
            PaintAtomKind::Shading(paint) => {
                *kinds.entry("shading").or_default() += 1;
                &paint.state
            }
            PaintAtomKind::TransparencyGroup(paint) => {
                *kinds.entry("group").or_default() += 1;
                &paint.state
            }
        };
        clips += state.clip_paths.len();
        for clip in &state.clip_paths {
            clip_segments += clip.path.segments.len();
        }
    }

    let atoms = view.graph.atoms.len();
    let segment = std::mem::size_of::<PathSegment>();
    let each = std::mem::size_of::<PaintAtom>();
    println!("atoms          {atoms} {kinds:?}");
    println!("atom structs   {} MB ({each} bytes each)", mb(atoms * each));
    println!("own segments   {own} = {} MB", mb(own * segment));
    println!("clip paths     {clips} on {atoms} atoms");
    println!(
        "clip segments  {clip_segments} = {} MB",
        mb(clip_segments * segment)
    );
    println!("text elements  {elements}");
    println!("marks          {marks}");
    let (now, peak) = holding;
    println!(
        "resident       {} MB before, {} MB holding the page, {} MB high water",
        mb_of(before.0),
        mb_of(now),
        mb_of(peak)
    );
    drop(view);
    let (after, _) = resident();
    println!(
        "               {} MB once the page is dropped: {} MB was the page itself",
        mb_of(after),
        mb_of(now.saturating_sub(after))
    );
}

fn resident() -> (usize, usize) {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let field = |name: &str| {
        status
            .lines()
            .find(|line| line.starts_with(name))
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|n| n.parse::<usize>().ok())
            .unwrap_or(0)
    };
    (field("VmRSS:"), field("VmHWM:"))
}

fn mb_of(kb: usize) -> String {
    let kb = u32::try_from(kb).unwrap_or(u32::MAX);
    format!("{:.1}", f64::from(kb) / 1024.0)
}

fn mb(bytes: usize) -> String {
    let bytes = u32::try_from(bytes).unwrap_or(u32::MAX);
    format!("{:.1}", f64::from(bytes) / 1_048_576.0)
}
