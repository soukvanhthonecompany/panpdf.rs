use std::error::Error;
use std::sync::Arc;

use pdf_app::Editor;
use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::plan::SourceAnchor;

fn holding(bytes: &[u8], marker: &[u8]) -> usize {
    bytes
        .windows(marker.len())
        .filter(|window| *window == marker)
        .count()
}

fn open(path: &str, id: u64) -> Result<ByteStore, Box<dyn Error>> {
    Ok(ByteStore::new(
        SourceId::new(id),
        Arc::<[u8]>::from(std::fs::read(path)?),
    ))
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let from = args
        .next()
        .ok_or("usage: crosspaste <from.pdf> <into.pdf> <out.pdf>")?;
    let into = args.next().ok_or("usage: crosspaste needs <into.pdf>")?;
    let out = args.next().ok_or("usage: crosspaste needs <out.pdf>")?;
    let from_page: usize = args.next().map_or(Ok(0), |text| text.parse())?;
    let into_page: usize = args.next().map_or(Ok(0), |text| text.parse())?;

    let source = open(&from, 9_001)?;
    let target = open(&into, 9_002)?;
    let source_bytes = source.as_bytes().to_vec();
    let target_bytes = target.as_bytes().to_vec();

    let view = pdf_session::interpret_page(&source, from_page)
        .map_err(|error| format!("reading the page copied from: {error:?}"))?;
    let anchors: Vec<String> = view
        .graph
        .atoms
        .iter()
        .map(|atom| SourceAnchor::of(&atom.id).encode())
        .collect();
    println!(
        "copied\tobjects={}\tfrom={from}\tpage={from_page}",
        anchors.len()
    );

    let mut editor =
        Editor::open(target).map_err(|reason| format!("opening the target: {reason}"))?;
    editor.set_aside_restrictions();
    let before =
        pdf_session::interpret_page(editor.source().ok_or("the target is busy")?, into_page)
            .map_err(|error| format!("reading the page pasted into: {error:?}"))?;
    println!(
        "before\tatoms={}\timages={}\tfontfiles={}",
        before.graph.atoms.len(),
        holding(&target_bytes, b"/Subtype /Image"),
        holding(&target_bytes, b"/FontFile")
    );

    let copied = {
        let mut lending = Editor::open(source.clone())
            .map_err(|reason| format!("opening the document copied from: {reason}"))?;
        lending.adopt_page(from_page, Arc::new(view));
        let mut taken = Vec::new();
        let mut refusals: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for anchor in &anchors {
            match lending.copy_objects(from_page, std::slice::from_ref(anchor)) {
                Ok(one) => taken.extend(one.objects),
                Err(reason) => *refusals.entry(reason).or_default() += 1,
            }
        }
        for (reason, count) in &refusals {
            println!("refused\tcount={count}\t{reason}");
        }
        if taken.is_empty() {
            return Err("nothing on that page can be copied yet".into());
        }
        println!("taking\tobjects={}", taken.len());
        pdf_edit::Copied {
            objects: taken,
            from: source.id(),
        }
    };

    let applied = editor.paste_objects(
        into_page,
        copied,
        (0.0, 0.0),
        Some((source, pdf_edit::Password::default())),
    );
    println!("pasted\t{applied:?}");

    let after = editor
        .source()
        .ok_or("the target is busy after the paste")?;
    let now = pdf_session::interpret_page(after, into_page)
        .map_err(|error| format!("reading the page back: {error:?}"))?;
    println!(
        "after\tatoms={}\timages={}\tfontfiles={}\tbytes={}\tgrew={}",
        now.graph.atoms.len(),
        holding(after.as_bytes(), b"/Subtype /Image"),
        holding(after.as_bytes(), b"/FontFile"),
        after.len(),
        after.len() - target_bytes.len()
    );
    println!("source\tbytes={}", source_bytes.len());
    std::fs::write(&out, after.as_bytes())?;
    println!("wrote\t{out}");
    Ok(())
}
