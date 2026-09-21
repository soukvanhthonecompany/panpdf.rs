use std::path::{Path, PathBuf};

#[must_use]
pub fn copy_beside(original: &Path) -> PathBuf {
    let mut name = original.file_stem().unwrap_or_default().to_os_string();
    name.push("-edited");
    let mut copy = original.to_path_buf();
    match original.extension() {
        Some(extension) => {
            name.push(".");
            name.push(extension);
        }
        None => name.push(".pdf"),
    }
    copy.set_file_name(name);
    copy
}

#[must_use]
pub fn is_a_copy(path: &Path) -> bool {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem.ends_with("-edited"))
}

pub fn named_pdf(folder: &Path, typed: &str) -> Result<PathBuf, NameRefused> {
    named_file(folder, typed, "pdf")
}

pub fn named_file(folder: &Path, typed: &str, ending: &str) -> Result<PathBuf, NameRefused> {
    let name = typed.trim();
    if name.is_empty() {
        return Err(NameRefused::Empty);
    }
    if name == "." || name == ".." {
        return Err(NameRefused::NotAName);
    }
    if name.contains(['/', '\\']) || name.contains('\0') {
        return Err(NameRefused::NotAName);
    }
    let mut file = folder.join(name);
    if !Path::new(name)
        .extension()
        .is_some_and(|had| had.eq_ignore_ascii_case(ending))
    {
        let mut with = std::ffi::OsString::from(name);
        with.push(".");
        with.push(ending);
        file.set_file_name(with);
    }
    Ok(file)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NameRefused {
    Empty,
    NotAName,
}

#[must_use]
pub fn named_for_pages(original: &Path, pages: &[usize]) -> String {
    let stem = original
        .file_stem()
        .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
    let tail = match pages {
        [] => String::new(),
        [one] => format!("-p{}", one + 1),
        [first, .., last] if last - first + 1 == pages.len() => {
            format!("-p{}-{}", first + 1, last + 1)
        }
        many => format!("-{}pages", many.len()),
    };
    format!("{stem}{tail}.pdf")
}

#[must_use]
pub fn files_written(base: &Path, spread: usize) -> Vec<PathBuf> {
    files_written_as(base, spread, "pdf")
}

#[must_use]
pub fn files_written_as(base: &Path, spread: usize, ending: &str) -> Vec<PathBuf> {
    if spread <= 1 {
        return vec![base.to_path_buf()];
    }
    (1..=spread)
        .map(|at| numbered_as(base, at, spread, ending))
        .collect()
}

#[must_use]
pub fn numbered(base: &Path, which: usize, of: usize) -> PathBuf {
    numbered_as(base, which, of, "pdf")
}

#[must_use]
pub fn numbered_as(base: &Path, which: usize, of: usize, ending: &str) -> PathBuf {
    let digits = of.to_string().len();
    let stem = base
        .file_stem()
        .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
    let mut file = base.to_path_buf();
    file.set_file_name(format!("{stem}-{which:0digits$}.{ending}"));
    file
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{
        NameRefused, copy_beside, files_written, files_written_as, is_a_copy, named_file,
        named_for_pages, named_pdf, numbered, numbered_as,
    };

    #[test]
    fn a_copy_is_named_beside_the_original() {
        for (original, expected) in [
            ("/tmp/book.pdf", "/tmp/book-edited.pdf"),
            ("book.pdf", "book-edited.pdf"),
            ("/tmp/a.b.pdf", "/tmp/a.b-edited.pdf"),
            ("/tmp/book", "/tmp/book-edited.pdf"),
            (
                "/tmp/\u{e17}\u{e14}\u{e2a}\u{e2d}\u{e1a}.pdf",
                "/tmp/\u{e17}\u{e14}\u{e2a}\u{e2d}\u{e1a}-edited.pdf",
            ),
        ] {
            assert_eq!(
                copy_beside(Path::new(original)),
                PathBuf::from(expected),
                "{original}"
            );
        }
    }

    #[test]
    fn a_copy_is_never_the_original() {
        for original in ["/tmp/book.pdf", "book.pdf", "/tmp/book", "/tmp/.pdf"] {
            let original = Path::new(original);
            assert_ne!(copy_beside(original), original.to_path_buf());
        }
    }

    #[test]
    fn a_copy_is_recognised_as_one() {
        assert!(is_a_copy(Path::new("/tmp/book-edited.pdf")));
        assert!(is_a_copy(&copy_beside(Path::new("/tmp/book.pdf"))));
        assert!(is_a_copy(&copy_beside(&copy_beside(Path::new(
            "/tmp/book.pdf"
        )))));
        assert!(!is_a_copy(Path::new("/tmp/book.pdf")));
        assert!(!is_a_copy(Path::new("/tmp/edited.pdf")));
    }

    #[test]
    fn a_typed_name_is_one_name_in_the_folder() {
        let folder = Path::new("/tmp/work");
        for (typed, expected) in [
            ("report", "/tmp/work/report.pdf"),
            ("report.pdf", "/tmp/work/report.pdf"),
            ("report.PDF", "/tmp/work/report.PDF"),
            ("  spaced  ", "/tmp/work/spaced.pdf"),
            ("a.b", "/tmp/work/a.b.pdf"),
            (".hidden", "/tmp/work/.hidden.pdf"),
            (
                "\u{e40}\u{e2d}\u{e01}\u{e2a}\u{e32}\u{e23}",
                "/tmp/work/\u{e40}\u{e2d}\u{e01}\u{e2a}\u{e32}\u{e23}.pdf",
            ),
        ] {
            assert_eq!(
                named_pdf(folder, typed),
                Ok(PathBuf::from(expected)),
                "{typed}"
            );
        }
    }

    #[test]
    fn a_path_is_not_a_name() {
        let folder = Path::new("/tmp/work");
        for typed in ["", "   ", "\t"] {
            assert_eq!(
                named_pdf(folder, typed),
                Err(NameRefused::Empty),
                "{typed:?}"
            );
        }
        for typed in [
            "../escape.pdf",
            "sub/report.pdf",
            "/etc/passwd",
            ".",
            "..",
            "back\\slash.pdf",
        ] {
            assert_eq!(
                named_pdf(folder, typed),
                Err(NameRefused::NotAName),
                "{typed:?}"
            );
        }
    }

    #[test]
    fn a_file_of_pages_is_named_after_them() {
        let book = Path::new("/tmp/book.pdf");
        assert_eq!(named_for_pages(book, &[6]), "book-p7.pdf");
        assert_eq!(named_for_pages(book, &[6, 7, 8]), "book-p7-9.pdf");
        assert_eq!(named_for_pages(book, &[0, 2, 4]), "book-3pages.pdf");
        assert_eq!(named_for_pages(book, &[0, 1]), "book-p1-2.pdf");
        assert_eq!(named_for_pages(book, &[]), "book.pdf");
    }

    #[test]
    fn split_files_are_numbered_and_padded() {
        let base = Path::new("/tmp/book.pdf");
        assert_eq!(numbered(base, 1, 3), PathBuf::from("/tmp/book-1.pdf"));
        assert_eq!(numbered(base, 2, 12), PathBuf::from("/tmp/book-02.pdf"));
        assert_eq!(numbered(base, 10, 12), PathBuf::from("/tmp/book-10.pdf"));
        assert_eq!(
            numbered(Path::new("/tmp/a.b.pdf"), 1, 1),
            PathBuf::from("/tmp/a.b-1.pdf")
        );
    }

    #[test]
    fn a_save_says_which_files_it_would_write() {
        let base = Path::new("/tmp/book.pdf");
        assert_eq!(files_written(base, 1), vec![PathBuf::from("/tmp/book.pdf")]);
        assert_eq!(files_written(base, 0), vec![PathBuf::from("/tmp/book.pdf")]);
        assert_eq!(
            files_written(base, 3),
            vec![
                PathBuf::from("/tmp/book-1.pdf"),
                PathBuf::from("/tmp/book-2.pdf"),
                PathBuf::from("/tmp/book-3.pdf"),
            ]
        );
        assert_eq!(files_written(base, 11).len(), 11);
        assert_eq!(
            files_written(base, 11)[0],
            PathBuf::from("/tmp/book-01.pdf")
        );
    }

    #[test]
    fn a_picture_is_named_the_way_a_document_is() {
        let folder = Path::new("/d");
        assert_eq!(
            named_file(folder, "page", "png").expect("a name"),
            PathBuf::from("/d/page.png")
        );
        assert_eq!(
            named_file(folder, "page.PNG", "png").expect("a name"),
            PathBuf::from("/d/page.PNG")
        );
        assert_eq!(
            named_file(folder, "book.pdf", "png").expect("a name"),
            PathBuf::from("/d/book.pdf.png")
        );
        for refused in ["", "  ", ".", "..", "a/b", "a\\b"] {
            assert!(named_file(folder, refused, "png").is_err(), "{refused}");
        }
        assert_eq!(
            named_file(folder, "", "png"),
            Err(NameRefused::Empty),
            "nothing typed is nothing typed, whatever is being written"
        );
        assert_eq!(
            numbered_as(Path::new("/d/book.png"), 2, 12, "png"),
            PathBuf::from("/d/book-02.png")
        );
        assert_eq!(
            files_written_as(Path::new("/d/book.png"), 3, "png"),
            vec![
                PathBuf::from("/d/book-1.png"),
                PathBuf::from("/d/book-2.png"),
                PathBuf::from("/d/book-3.png"),
            ]
        );
        assert_eq!(
            files_written_as(Path::new("/d/book.png"), 1, "png"),
            vec![PathBuf::from("/d/book.png")]
        );
    }
}
