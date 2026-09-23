use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::{Command, TextRunSelection};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: editstack <file.pdf> [steps]");
        return;
    };
    let steps: usize = args.next().and_then(|n| n.parse().ok()).unwrap_or(100);
    let bytes = std::fs::read(&path).expect("the file");
    let opened = bytes.len();
    let source = ByteStore::new(SourceId::new(1), Arc::<[u8]>::from(bytes));
    let mut session = pdf_session::Session::new(source, b"");

    println!("opened {} KB, {} steps", opened / 1024, steps);
    println!(
        "{:>6}  {:>12}  {:>12}  {:>10}",
        "step", "document KB", "resident MB", "per step KB"
    );
    let first = resident();
    for step in 1..=steps {
        let dx = if step % 2 == 0 { -1.0 } else { 1.0 };
        let command = Command::MoveTextRun {
            page_index: 0,
            selection: TextRunSelection::Last,
            dx,
            dy: 0.0,
        };
        let plan = match session.plan(&command) {
            Ok(plan) => plan,
            Err(error) => {
                eprintln!("step {step}: refused: {error:?}");
                return;
            }
        };
        if let Err(error) = session.apply(plan) {
            eprintln!("step {step}: not committed: {error:?}");
            return;
        }
        if step % 10 == 0 || step == 1 {
            let now = resident();
            let grown = now.saturating_sub(first);
            println!(
                "{step:>6}  {:>12}  {:>12.1}  {:>10.1}",
                session.source().as_bytes().len() / 1024,
                megabytes(now),
                kilobytes(grown) / f64::from(u32::try_from(step).unwrap_or(u32::MAX)),
            );
        }
    }
}

fn resident() -> usize {
    std::fs::read_to_string("/proc/self/status")
        .unwrap_or_default()
        .lines()
        .find(|line| line.starts_with("VmRSS:"))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

fn megabytes(kb: usize) -> f64 {
    kilobytes(kb) / 1024.0
}

fn kilobytes(kb: usize) -> f64 {
    f64::from(u32::try_from(kb).unwrap_or(u32::MAX))
}
