use std::error::Error;
use std::path::PathBuf;

use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::{Command, History};
use pdf_syntax::XrefLimits;

fn resident() -> (u64, u64) {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let field = |name: &str| -> u64 {
        status
            .lines()
            .find(|line| line.starts_with(name))
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|value| value.parse().ok())
            .unwrap_or(0)
    };
    (field("VmRSS:"), field("VmHWM:"))
}

fn report(stage: &str, source: Option<&ByteStore>) {
    let (rss, peak) = resident();
    let (len, at) = source.map_or((0, 0), |store| {
        (store.len(), store.as_bytes().as_ptr().addr())
    });
    println!("{stage}\trss_kb={rss}\tpeak_kb={peak}\tlen={len}\tat=0x{at:x}");
}

fn info_edit(nth: usize) -> Command {
    Command::SetDocumentInfo {
        edit: pdf_edit::info::InfoEdit {
            title: Some(format!("memorycost {nth}")),
            ..pdf_edit::info::InfoEdit::default()
        },
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args().skip(1);
    let path = PathBuf::from(
        arguments
            .next()
            .ok_or("usage: memorycost <file.pdf> [edits]")?,
    );
    let edits: usize = match arguments.next() {
        Some(value) => value.parse()?,
        None => 10,
    };

    report("start", None);
    let held = std::fs::read(&path)?;
    let on_disc = held.len();
    report("read", None);
    let source = ByteStore::new(SourceId::new(0), held);
    report("bytestore", Some(&source));

    let mut history = History::new(source, b"");
    history.set_aside_restrictions();
    report("history", Some(history.source()));

    for nth in 0..edits {
        let current = history.source().clone();
        let document = pdf_edit::Document::open_strict(current, XrefLimits::default())?;
        let plan = document
            .begin_transaction()
            .plan(&info_edit(nth), b"")
            .map_err(|error| format!("{error:?}"))?;
        drop(document);
        history.apply(plan).map_err(|error| format!("{error:?}"))?;
        report(&format!("edit{}", nth + 1), Some(history.source()));
    }

    if edits > 0 {
        history.undo().map_err(|error| format!("{error:?}"))?;
        report("undo", Some(history.source()));
    }

    let (rss, peak) = resident();
    let per_file = |kilobytes: u64| -> u64 {
        let on_disc = u64::try_from(on_disc).unwrap_or(1).max(1);
        kilobytes.saturating_mul(1024).saturating_mul(1000) / on_disc
    };
    println!(
        "summary\ton_disc={on_disc}\tcopies_resident_per_mille={}\tcopies_peak_per_mille={}",
        per_file(rss),
        per_file(peak)
    );
    Ok(())
}
