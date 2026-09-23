#![forbid(unsafe_code)]
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use pdf_app::Editor;
use pdf_bytes::{ByteStore, SourceId};

mod check;

use pdf_window::app;

fn main() -> ExitCode {
    pdf_window::startup::began();
    let mut page = 0_usize;
    let mut path: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut check = false;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--scale" => {
                if arguments.next().is_none() {
                    return usage("--scale needs a number");
                }
            }
            "--page" => match arguments
                .next()
                .and_then(|value| value.parse::<usize>().ok())
            {
                Some(value) if value > 0 => page = value - 1,
                _ => return usage("--page needs a page number, counted from 1"),
            },
            "--out" => match arguments.next() {
                Some(value) => out = Some(PathBuf::from(value)),
                None => return usage("--out needs a path"),
            },
            "--check" => check = true,
            "--version" => {
                println!("PanPDF {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            other if path.is_none() => path = Some(PathBuf::from(other)),
            other => return usage(&format!("unexpected argument {other:?}")),
        }
    }
    let Some(path) = path else {
        if check {
            return usage("--check needs a file");
        }
        let editor = match Editor::stand_in() {
            Ok(editor) => editor,
            Err(error) => return fail(&error),
        };
        return match app::run(editor, PathBuf::new(), Vec::new(), None) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => fail(&error),
        };
    };
    let (path, library) = match resolve(&path) {
        Ok(both) => both,
        Err(reason) => return fail(&reason),
    };
    let source = match std::fs::read(&path) {
        Ok(bytes) => ByteStore::owning(SourceId::next_document(), bytes),
        Err(error) => return fail(&format!("{}: {error}", path.display())),
    };
    if !check && pdf_edit::info::lock(&source, b"") == pdf_edit::info::Lock::Refused {
        let editor = match Editor::stand_in() {
            Ok(editor) => editor,
            Err(error) => return fail(&error),
        };
        let locked = app::Locked { path, source };
        return match app::run(editor, PathBuf::new(), library, Some(locked)) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => fail(&error),
        };
    }
    let editor = match Editor::open(source) {
        Ok(editor) => editor,
        Err(error) => return fail(&error),
    };
    if out.as_deref() == Some(path.as_path()) {
        return fail("--out is the file that was opened; an edit is saved as a copy");
    }
    if check {
        return check::report(editor, &path, page, out.as_deref());
    }
    match app::run(editor, path, library, None) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => fail(&error),
    }
}

fn resolve(asked: &Path) -> Result<(PathBuf, Vec<PathBuf>), String> {
    if asked.is_dir() {
        let library = pdf_window::pdfs_in(asked);
        let first = library
            .first()
            .cloned()
            .ok_or_else(|| format!("no PDF in {}", asked.display()))?;
        return Ok((first, library));
    }
    let mut library = asked.parent().map(pdf_window::pdfs_in).unwrap_or_default();
    if !library.contains(&asked.to_path_buf()) {
        library.push(asked.to_path_buf());
        library.sort();
    }
    Ok((asked.to_path_buf(), library))
}

fn usage(reason: &str) -> ExitCode {
    eprintln!("pdf-app: {reason}");
    eprintln!(
        "usage: pdf-app [--scale N] [--page N] [--out <copy.pdf>] [--check] [--version] [<file.pdf>]"
    );
    ExitCode::FAILURE
}

fn fail(reason: &str) -> ExitCode {
    eprintln!("pdf-app: {reason}");
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::resolve;

    #[test]
    fn a_file_is_opened_with_its_own_directory_beside_it() {
        let corpus = Path::new("../../documents");
        if !corpus.is_dir() {
            return;
        }
        let asked = corpus.join("book.pdf");
        if !asked.is_file() {
            return;
        }
        let (opened, library) = resolve(&asked).expect("a file resolves to itself");
        assert_eq!(opened, asked);
        assert!(library.contains(&asked), "the shelf must hold what is open");
        assert!(library.len() > 1, "the corpus has more than one document");
        assert!(
            library.windows(2).all(|pair| pair[0] <= pair[1]),
            "the shelf is in order"
        );
        assert!(
            !library.iter().any(|path| pdf_app::files::is_a_copy(path)),
            "copies this editor wrote are not offered"
        );
    }

    #[test]
    fn a_directory_opens_the_first_document_in_it() {
        let corpus = Path::new("../../documents");
        if !corpus.is_dir() {
            return;
        }
        let (opened, library) = resolve(corpus).expect("a corpus resolves");
        assert_eq!(Some(&opened), library.first());
    }

    #[test]
    fn a_directory_with_no_pdf_says_so() {
        let empty = std::env::temp_dir().join("panpdf-empty-library");
        let _ = std::fs::create_dir_all(&empty);
        let refused = resolve(&empty).expect_err("there is nothing to open");
        assert!(refused.contains("no PDF"), "{refused}");
        let _ = std::fs::remove_dir(&empty);
    }

    #[test]
    fn a_file_named_directly_is_opened_whatever_the_shelf_would_offer() {
        let stage = std::env::temp_dir().join("panpdf-library-copy");
        let _ = std::fs::create_dir_all(&stage);
        let copy = stage.join("book-edited.pdf");
        let _ = std::fs::write(&copy, b"%PDF-1.7\n");
        let (opened, library) = resolve(&copy).expect("a named file resolves");
        assert_eq!(opened, copy);
        assert!(
            library.contains(&copy),
            "a person who named a file has said what they want"
        );
        let _ = std::fs::remove_file(&copy);
        let _ = std::fs::remove_dir(&stage);
    }

    #[test]
    fn the_shelf_holds_whole_paths() {
        let corpus = Path::new("../../documents");
        if !corpus.is_dir() {
            return;
        }
        let (_, library) = resolve(corpus).expect("a corpus resolves");
        assert!(
            library.iter().all(|path| path.parent() == Some(corpus)),
            "every entry must name where it is, not only what it is called"
        );
        let unique: std::collections::HashSet<&PathBuf> = library.iter().collect();
        assert_eq!(
            unique.len(),
            library.len(),
            "the shelf lists each file once"
        );
    }
}
