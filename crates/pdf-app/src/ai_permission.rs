#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Mode {
    ChatOnly,
    #[default]
    AskBeforeChanges,
    DoIt,
    Free,
}

impl Mode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ChatOnly => "chat_only",
            Self::AskBeforeChanges => "ask_before_changes",
            Self::DoIt => "do_it",
            Self::Free => "free",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "chat_only" => Some(Self::ChatOnly),
            "ask_before_changes" => Some(Self::AskBeforeChanges),
            "do_it" => Some(Self::DoIt),
            "free" => Some(Self::Free),
            _ => None,
        }
    }
}

pub const ALWAYS_ASK: [&str; 1] = ["insert_pages"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Why {
    ToolsAreOff,
    UnknownTool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Decision {
    Run,
    Ask { may_allow_for_chat: bool },
    Refuse(Why),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Answer {
    Once,
    ForThisChat,
    Refuse,
}

#[must_use]
pub const fn refusal_text() -> &'static str {
    "The person refused this action. Do not retry it; ask what they would like instead."
}

#[must_use]
pub fn decide(
    mode: Mode,
    name: &str,
    facts: Option<(bool, bool)>,
    allowed_for_chat: bool,
) -> Decision {
    let Some((read_only, _destructive)) = facts else {
        return Decision::Refuse(Why::UnknownTool);
    };
    if mode == Mode::ChatOnly {
        return Decision::Refuse(Why::ToolsAreOff);
    }
    if read_only {
        return Decision::Run;
    }
    let always = ALWAYS_ASK.contains(&name);
    match mode {
        Mode::ChatOnly => Decision::Refuse(Why::ToolsAreOff),
        Mode::AskBeforeChanges => {
            if allowed_for_chat && !always {
                Decision::Run
            } else {
                Decision::Ask {
                    may_allow_for_chat: !always,
                }
            }
        }
        Mode::DoIt => {
            if always {
                Decision::Ask {
                    may_allow_for_chat: false,
                }
            } else {
                Decision::Run
            }
        }
        Mode::Free => Decision::Run,
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use words::describe_call;

#[cfg(not(target_arch = "wasm32"))]
mod words {
    use pdf_agent::tools::request::Request;

    use crate::wording::Lang;

    const MOST: usize = 200;

    #[must_use]
    pub fn describe_call(request: &Request, lang: Lang) -> String {
        match lang {
            Lang::English => describe_in_english(request),
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one sentence per tool, read as a table"
    )]
    fn describe_in_english(request: &Request) -> String {
        match request {
            Request::DocumentInfo => "Read what the document says about itself".to_owned(),
            Request::ReadText { first, last } => match (first, last) {
                (None, None) => "Read the document's text".to_owned(),
                (first, last) => {
                    let from = first.unwrap_or(0) + 1;
                    match last {
                        Some(last) if *last + 1 != from => {
                            format!("Read the text of pages {from} to {}", last + 1)
                        }
                        Some(_) => {
                            format!("Read the text of page {from}")
                        }
                        None => {
                            format!("Read the text from page {from} on")
                        }
                    }
                }
            },
            Request::FindText { text, .. } => {
                let text = clip(text);
                format!("Find \u{201c}{text}\u{201d} in the document")
            }
            Request::RenderPage { page, .. } => {
                format!("Look at page {} as a picture", page + 1)
            }
            Request::ListFonts { .. } => "List the fonts new text can be set in".to_owned(),
            Request::ReplaceText { block, find, text } if text.trim().is_empty() => match find {
                Some(find) => {
                    let find = clip(find);
                    format!("Delete \u{201c}{find}\u{201d} from block {block}")
                }
                None => format!("Delete all the text of block {block}"),
            },
            Request::ReplaceText { block, find, text } => {
                let text = clip(text);
                match find {
                    Some(find) => {
                        let find = clip(find);
                        format!("Replace \u{201c}{find}\u{201d} in block {block} with: {text}")
                    }
                    None => {
                        format!("Replace the text of block {block} with: {text}")
                    }
                }
            }
            Request::AddText { page, text, .. } if text.trim().is_empty() => {
                format!("Write nothing at all on page {}", page + 1)
            }
            Request::AddText { page, text, .. } => {
                let text = clip(text);
                format!("Write new text on page {}: {text}", page + 1)
            }
            Request::WritePages {
                from_page,
                markdown,
                replace,
                ..
            } => {
                let words = markdown.split_whitespace().count();
                let doing = if *replace {
                    "Rewrite"
                } else {
                    "Write a document onto"
                };
                let first = clip(
                    markdown
                        .lines()
                        .find(|line| !line.trim().is_empty())
                        .unwrap_or_default(),
                );
                format!(
                    "{doing} the pages from page {}, {words} words, starting \u{201c}{first}\u{201d}",
                    from_page + 1
                )
            }
            Request::SetProperties(edit) => {
                let named = properties(edit);
                format!("Set the document's properties ({named})")
            }
            Request::FillField { name, value } => {
                let said = clip(&match value {
                    pdf_agent::json::Json::Text(text) => text.clone(),
                    other => other.write(),
                });
                format!("Fill the form field \u{201c}{name}\u{201d} with: {said}")
            }
            Request::AddBlankPage { after, .. } => {
                if *after == 0 {
                    "Add a blank page at the front".to_owned()
                } else {
                    format!("Add a blank page after page {after}")
                }
            }
            Request::DeletePages(pages) => {
                let said = pages_said(pages);
                format!("Delete page{} {said}", plural(pages.len()))
            }
            Request::MovePages { pages, to } => {
                let said = pages_said(pages);
                format!(
                    "Move page{} {said} so that the first becomes page {}",
                    plural(pages.len()),
                    to + 1
                )
            }
            Request::RotatePages {
                pages,
                quarter_turns,
            } => {
                let said = pages_said(pages);
                let degrees = quarter_turns * 90;
                format!(
                    "Turn page{} {said} by {degrees} degrees",
                    plural(pages.len())
                )
            }
            Request::InsertPages {
                from, pages, after, ..
            } => {
                let file = from.display();
                let which = match pages {
                    Some(pages) => {
                        let said = pages_said(pages);
                        format!("page{} {said} of", plural(pages.len()))
                    }
                    None => "every page of".to_owned(),
                };
                let place = if *after == 0 {
                    "at the front".to_owned()
                } else {
                    format!("after page {after}")
                };
                format!("Put {which} the file {file} in {place}")
            }
            Request::Undo => "Take back the last change".to_owned(),
            Request::Redo => "Put back the last change that was taken back".to_owned(),
            Request::AskPerson { question, .. } => format!("Ask you: {question}"),
        }
    }

    fn properties(edit: &pdf_edit::info::InfoEdit) -> String {
        let named: Vec<&str> = [
            (edit.title.is_some(), "title"),
            (edit.author.is_some(), "author"),
            (edit.subject.is_some(), "subject"),
            (edit.keywords.is_some(), "keywords"),
        ]
        .into_iter()
        .filter_map(|(asked, word)| asked.then_some(word))
        .collect();
        named.join(", ")
    }

    fn pages_said(pages: &[usize]) -> String {
        let numbers: Vec<String> = pages.iter().map(|page| (page + 1).to_string()).collect();
        let and = " and ";
        match numbers.split_last() {
            None => String::new(),
            Some((last, [])) => last.clone(),
            Some((last, rest)) => format!("{}{and}{last}", rest.join(", ")),
        }
    }

    fn plural(count: usize) -> &'static str {
        if count == 1 { "" } else { "s" }
    }

    fn clip(text: &str) -> String {
        let flat = text.replace(['\n', '\r'], " ");
        if flat.chars().count() <= MOST {
            flat
        } else {
            let mut clipped: String = flat.chars().take(MOST).collect();
            clipped.push('\u{2026}');
            clipped
        }
    }
}

#[cfg(test)]
mod tests;
