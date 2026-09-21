use std::error::Error;
use std::path::{Path, PathBuf};

use pdf_bytes::{ByteStore, SourceId};
use pdf_edit::spike_move_text::{self, MovedTextRun};

fn signatures(bytes: &ByteStore) -> Result<Vec<String>, String> {
    pdf_cli::inspect_page_paint_strict(bytes, 0)
        .map(|page| page.paint_signatures)
        .map_err(|error| error.to_string())
}

fn write(directory: &Path, name: &str, lines: &[String]) -> Result<(), Box<dyn Error>> {
    let mut text = lines.join("\n");
    text.push('\n');
    std::fs::write(directory.join(name), text)?;
    Ok(())
}

fn differences(actual: &[String], expected: &[String]) -> Vec<String> {
    let mut found = Vec::new();
    for (at, (one, other)) in actual.iter().zip(expected.iter()).enumerate() {
        if one != other {
            found.push(format!("atom {at}\n  was: {other}\n  now: {one}"));
        }
    }
    if actual.len() != expected.len() {
        found.push(format!(
            "atom count {} against the {} it had",
            actual.len(),
            expected.len()
        ));
    }
    found
}

fn edit(which: &str, source: &ByteStore) -> Result<MovedTextRun, Box<dyn Error>> {
    let provider = pdf_cli::font_provider();
    Ok(match which {
        "move-text" => {
            spike_move_text::move_last_text_run_with_fonts(source, 0, 12.0, 0.0, b"", provider)?
        }
        "move-cluster" => {
            spike_move_text::move_last_cluster_with_fonts(source, 0, 12.0, 0.0, b"", provider)?
        }
        "delete-cluster" => {
            spike_move_text::delete_last_cluster_with_fonts(source, 0, b"", provider)?
        }
        "delete-selection" => {
            spike_move_text::delete_last_row_selection_with_fonts(source, 0, 3, b"", provider)?
        }
        other => return Err(format!("unknown spike: {other}").into()),
    })
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args().skip(1);
    let which = arguments
        .next()
        .ok_or("usage: undowalk <spike> <file.pdf> [--dump DIR]")?;
    let path = PathBuf::from(
        arguments
            .next()
            .ok_or("usage: undowalk <spike> <file.pdf> [--dump DIR]")?,
    );
    let mut dump: Option<PathBuf> = None;
    while let Some(flag) = arguments.next() {
        if flag == "--dump" {
            dump = Some(PathBuf::from(
                arguments.next().ok_or("--dump needs a directory")?,
            ));
        } else {
            return Err(format!("unknown option: {flag}").into());
        }
    }
    if let Some(directory) = &dump {
        std::fs::create_dir_all(directory)?;
    }

    let source = ByteStore::new(SourceId::new(0), std::fs::read(&path)?);
    let moved = edit(&which, &source)?;

    let document =
        pdf_edit::Document::open_strict(source.clone(), pdf_syntax::XrefLimits::default())?;
    let plan = document.begin_transaction().plan_with_fonts(
        &moved.command,
        b"",
        pdf_cli::font_provider(),
    )?;

    let at_rest = signatures(&source)?;
    let mut history = pdf_edit::History::new(source.clone(), b"");
    history.apply(plan)?;
    let after_edit = signatures(history.source())?;

    let mut stops = vec![("at-rest", at_rest.clone()), ("moved", after_edit.clone())];
    let mut broken = 0usize;
    for (name, step, expected) in [
        ("undo-1", "undo", &at_rest),
        ("redo", "redo", &after_edit),
        ("undo-2", "undo", &at_rest),
    ] {
        let walked = if step == "undo" {
            history.undo()?
        } else {
            history.redo()?
        };
        if !walked {
            return Err(format!("{name}: nothing to walk").into());
        }
        let actual = signatures(history.source())?;
        let found = differences(&actual, expected);
        println!("{name}: {} atoms, {} differ", actual.len(), found.len());
        for one in &found {
            println!("{one}");
        }
        broken += found.len();
        stops.push((name, actual));
    }

    if let Some(directory) = &dump {
        for (name, lines) in &stops {
            write(directory, name, lines)?;
        }
        println!("dumped: {}", directory.display());
    }
    println!(
        "walk: {}",
        if broken == 0 {
            "identical at every stop"
        } else {
            "differs"
        }
    );
    std::process::exit(i32::from(broken != 0));
}
