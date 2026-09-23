use std::error::Error;
use std::path::PathBuf;

use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::{Command, History};
use pdf_syntax::XrefLimits;

fn info_edit(nth: usize) -> Command {
    Command::SetDocumentInfo {
        edit: pdf_edit::info::InfoEdit {
            title: Some(format!("savehash {nth}")),
            ..pdf_edit::info::InfoEdit::default()
        },
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Reading {
    Bytes,
    Decrypted,
}

fn stop(name: &str, source: &ByteStore, reading: Reading) {
    let hash = match reading {
        Reading::Bytes => pdf_content::sha256_hex(source.as_bytes()),
        Reading::Decrypted => {
            match pdf_edit::reprotect::rewrite(source, b"", &pdf_edit::reprotect::Wanted::Open) {
                Ok(plain) => pdf_content::sha256_hex(&plain),
                Err(error) => format!("undecipherable: {error:?}"),
            }
        }
    };
    println!("{name}\tlen={}\tsha256={hash}", source.len());
}

fn main() -> Result<(), Box<dyn Error>> {
    let path = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("usage: savehash <file.pdf>")?,
    );
    let reading = if std::env::args().any(|arg| arg == "--decrypted") {
        Reading::Decrypted
    } else {
        Reading::Bytes
    };
    let source = ByteStore::new(SourceId::new(0), std::fs::read(&path)?);
    let mut history = History::new(source.clone(), b"");
    history.set_aside_restrictions();
    stop("open", history.source(), reading);

    for nth in 0..2 {
        let current = history.source().clone();
        let document = pdf_edit::Document::open_strict(current, XrefLimits::default())?;
        let plan = document
            .begin_transaction()
            .plan(&info_edit(nth), b"")
            .map_err(|error| format!("{error:?}"))?;
        drop(document);
        history.apply(plan).map_err(|error| format!("{error:?}"))?;
        stop(&format!("edit{}", nth + 1), history.source(), reading);
    }

    for step in ["undo", "undo", "redo", "redo", "undo", "undo"] {
        let walked = if step == "undo" {
            history.undo().map_err(|error| format!("{error:?}"))?
        } else {
            history.redo().map_err(|error| format!("{error:?}"))?
        };
        stop(&format!("{step}={walked}"), history.source(), reading);
    }
    Ok(())
}
