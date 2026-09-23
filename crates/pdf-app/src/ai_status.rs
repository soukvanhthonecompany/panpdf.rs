use pdf_agent::tools::request::Request;

use crate::wording::Lang;

#[must_use]
pub fn doing(request: &Request, lang: Lang) -> String {
    match lang {
        Lang::English => doing_in_english(request),
    }
}

#[must_use]
pub fn did(name: &str, lang: Lang) -> String {
    match lang {
        Lang::English => did_in_english(name),
    }
}

fn pages(first: usize, last: usize) -> String {
    if first == last {
        format!("page {}", first + 1)
    } else {
        format!("pages {} to {}", first + 1, last + 1)
    }
}

fn some_pages(list: &[usize]) -> String {
    match list {
        [one] => format!("page {}", one + 1),
        [one, two] => format!("pages {} and {}", one + 1, two + 1),
        many => format!("{} pages", many.len()),
    }
}

fn quoted(text: &str) -> String {
    const MOST: usize = 40;
    let line = text.lines().next().unwrap_or_default().trim();
    if line.chars().count() <= MOST {
        format!("\u{201c}{line}\u{201d}")
    } else {
        let cut: String = line.chars().take(MOST).collect();
        format!("\u{201c}{}\u{2026}\u{201d}", cut.trim_end())
    }
}

fn doing_in_english(request: &Request) -> String {
    match request {
        Request::DocumentInfo => "Reading what the document says about itself".to_owned(),
        Request::ReadText { first, last } => match (first, last) {
            (None, None) => "Reading the document's text".to_owned(),
            (Some(first), Some(last)) => format!("Reading {}", pages(*first, *last)),
            (Some(first), None) => format!("Reading from page {}", first + 1),
            (None, Some(last)) => format!("Reading up to page {}", last + 1),
        },
        Request::FindText { text, .. } => format!("Searching for {}", quoted(text)),
        Request::RenderPage { page, .. } => format!("Looking at page {}", page + 1),
        Request::ListFonts { .. } => "Looking through the fonts".to_owned(),
        Request::ReplaceText { block, .. } => format!("Changing the text of {block}"),
        Request::AddText { page, .. } => format!("Adding text to page {}", page + 1),
        Request::WritePages { from_page, .. } => {
            format!("Writing the document from page {}", from_page + 1)
        }
        Request::SetProperties(_) => "Changing the document's properties".to_owned(),
        Request::FillField { name, .. } => format!("Filling in {}", quoted(name)),
        Request::AddBlankPage { .. } => "Adding a blank page".to_owned(),
        Request::DeletePages(list) => format!("Deleting {}", some_pages(list)),
        Request::MovePages { pages: list, .. } => format!("Moving {}", some_pages(list)),
        Request::RotatePages { pages: list, .. } => format!("Turning {}", some_pages(list)),
        Request::InsertPages { from, .. } => format!(
            "Putting in pages from {}",
            from.file_name().map_or_else(
                || from.display().to_string(),
                |name| name.to_string_lossy().into_owned()
            )
        ),
        Request::Undo => "Taking back the last change".to_owned(),
        Request::Redo => "Putting back the last change".to_owned(),
        Request::AskPerson { .. } => "Asking you".to_owned(),
    }
}

fn did_in_english(name: &str) -> String {
    match name {
        "document_info" => "Read the document's facts",
        "read_text" => "Read the text",
        "find_text" => "Searched",
        "render_page" => "Looked at the page",
        "list_fonts" => "Looked through the fonts",
        "replace_text" => "Changed the text",
        "add_text" => "Added text",
        "write_pages" => "Wrote the document",
        "set_properties" => "Changed the properties",
        "fill_field" => "Filled in a field",
        "add_blank_page" => "Added a page",
        "delete_pages" => "Deleted pages",
        "move_pages" => "Moved pages",
        "rotate_pages" => "Turned pages",
        "insert_pages" => "Put in pages",
        "undo" => "Took back a change",
        "redo" => "Put back a change",
        "ask_person" => "Asked you",
        other => return other.to_owned(),
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use pdf_agent::tools::request::Request;

    use super::{did, doing};
    use crate::wording::Lang;

    #[test]
    fn a_status_says_what_it_is_about() {
        let say = |request: Request| doing(&request, Lang::English);
        assert_eq!(
            say(Request::ReadText {
                first: Some(0),
                last: Some(2)
            }),
            "Reading pages 1 to 3"
        );
        assert_eq!(
            say(Request::ReadText {
                first: Some(1),
                last: Some(1)
            }),
            "Reading page 2"
        );
        assert_eq!(
            say(Request::FindText {
                text: "total due".to_owned(),
                match_case: false,
                first: None,
                last: None,
            }),
            "Searching for \u{201c}total due\u{201d}"
        );
        assert_eq!(
            say(Request::DeletePages(vec![1, 4])),
            "Deleting pages 2 and 5"
        );
        assert_eq!(
            say(Request::AskPerson {
                question: "Which?".to_owned(),
                options: Vec::new()
            }),
            "Asking you"
        );
    }

    #[test]
    fn a_status_stays_one_line_and_a_done_line_is_words() {
        let long = doing(
            &Request::FindText {
                text: "a".repeat(100),
                match_case: false,
                first: None,
                last: None,
            },
            Lang::English,
        );
        assert!(long.chars().count() < 70, "{long}");
        assert!(long.ends_with("\u{2026}\u{201d}"));
        assert_eq!(did("ask_person", Lang::English), "Asked you");
        assert_eq!(did("read_text", Lang::English), "Read the text");
        assert_eq!(did("something_new", Lang::English), "something_new");
    }
}
