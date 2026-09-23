use std::error::Error;

use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::{Command, History};
use pdf_syntax::XrefLimits;

fn edit(nth: usize) -> Command {
    Command::SetDocumentInfo {
        edit: pdf_edit::info::InfoEdit {
            title: Some(format!("undocheck {nth}")),
            ..pdf_edit::info::InfoEdit::default()
        },
    }
}

fn apply(history: &mut History, nth: usize) -> Result<(), String> {
    let current = history.source().clone();
    let document = pdf_edit::Document::open_strict(current, XrefLimits::default())
        .map_err(|error| format!("open_strict: {error:?}"))?;
    let plan = document
        .begin_transaction()
        .plan(&edit(nth), b"")
        .map_err(|error| format!("plan: {error:?}"))?;
    drop(document);
    history
        .apply(plan)
        .map_err(|error| format!("apply: {error:?}"))?;
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: undocheck <file.pdf>")?;
    let source = ByteStore::new(SourceId::new(0), std::fs::read(&path)?);
    let mut history = History::new(source, b"");
    history.set_aside_restrictions();
    println!("edit1 {:?}", apply(&mut history, 1));
    println!(
        "undo  {:?}",
        history.undo().map_err(|error| format!("{error:?}"))
    );
    println!("edit-after-undo {:?}", apply(&mut history, 2));
    Ok(())
}
