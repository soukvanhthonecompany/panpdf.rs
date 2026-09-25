use std::path::PathBuf;

use crate::json::Json;
use crate::tools::{Args, colour, expand};

pub const HANDLE: &str = "doc-1";

#[derive(Clone, Debug, PartialEq)]
pub struct NewText {
    pub family: String,
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    pub fill: Option<[f64; 3]>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    DocumentInfo,
    ReadText {
        first: Option<usize>,
        last: Option<usize>,
    },
    FindText {
        text: String,
        match_case: bool,
        first: Option<usize>,
        last: Option<usize>,
    },
    RenderPage {
        page: usize,
        dpi: f64,
    },
    ListFonts {
        name: Option<String>,
    },
    ReplaceText {
        block: String,
        find: Option<String>,
        text: String,
    },
    AddText {
        page: usize,
        area: [f64; 4],
        text: String,
        style: NewText,
    },
    WritePages {
        from_page: usize,
        markdown: String,
        replace: bool,
        size: f64,
        family: String,
        margin: f64,
        theme: String,
    },
    SetProperties(pdf_edit::info::InfoEdit),
    FillField {
        name: String,
        value: Json,
    },
    AddBlankPage {
        after: usize,
        size: Option<[f64; 2]>,
    },
    DeletePages(Vec<usize>),
    MovePages {
        pages: Vec<usize>,
        to: usize,
    },
    RotatePages {
        pages: Vec<usize>,
        quarter_turns: i32,
    },
    InsertPages {
        from: PathBuf,
        pages: Option<Vec<usize>>,
        after: usize,
        password: Option<String>,
    },
    Undo,
    Redo,
    AskPerson {
        question: String,
        options: Vec<(String, String)>,
    },
}

impl Request {
    #[must_use]
    pub const fn only_reads(&self) -> bool {
        matches!(
            self,
            Self::DocumentInfo
                | Self::ReadText { .. }
                | Self::FindText { .. }
                | Self::RenderPage { .. }
                | Self::ListFonts { .. }
                | Self::AskPerson { .. }
        )
    }
}

pub fn parse(name: &str, arguments: &Json) -> Result<Request, String> {
    let empty = Json::Object(std::collections::BTreeMap::new());
    let args = Args(if matches!(arguments, Json::Null) {
        &empty
    } else {
        arguments
    });
    if let Some(handle) = args.text("document")
        && handle != HANDLE
    {
        return Err(format!(
            "there is no document called {handle}: the document open in this window is \
             {HANDLE}, and it is the only one"
        ));
    }
    match name {
        "document_info" => Ok(Request::DocumentInfo),
        "read_text" => Ok(Request::ReadText {
            first: optional_page(&args, "first_page")?,
            last: optional_page(&args, "last_page")?,
        }),
        "find_text" => {
            let text = args.required("text")?;
            if text.is_empty() {
                return Err("`text` is empty".to_owned());
            }
            Ok(Request::FindText {
                text: text.to_owned(),
                match_case: args.flag("match_case"),
                first: optional_page(&args, "first_page")?,
                last: optional_page(&args, "last_page")?,
            })
        }
        "render_page" => Ok(Request::RenderPage {
            page: args.page("page")?,
            dpi: args.number("dpi").unwrap_or(96.0).clamp(10.0, 300.0),
        }),
        "list_fonts" => Ok(Request::ListFonts {
            name: args.text("name").map(str::to_owned),
        }),
        "replace_text" => Ok(Request::ReplaceText {
            block: args.required("block")?.to_owned(),
            find: args.text("find").map(str::to_owned),
            text: args.required("text")?.to_owned(),
        }),
        "add_text" => add_text(&args),
        "write_pages" => write_pages(&args),
        "set_properties" => set_properties(&args),
        "fill_field" => Ok(Request::FillField {
            name: args.required("name")?.to_owned(),
            value: args.0.get("value").ok_or("`value` is needed")?.clone(),
        }),
        "add_blank_page" => Ok(Request::AddBlankPage {
            after: args.after("after_page")?,
            size: match (args.number("width"), args.number("height")) {
                (Some(width), Some(height)) if width > 0.0 && height > 0.0 => Some([width, height]),
                _ => None,
            },
        }),
        "delete_pages" => Ok(Request::DeletePages(args.pages("pages")?)),
        "move_pages" => Ok(Request::MovePages {
            pages: args.pages("pages")?,
            to: args.page("to")?,
        }),
        "rotate_pages" => Ok(Request::RotatePages {
            pages: args.pages("pages")?,
            quarter_turns: quarter_turns(&args)?,
        }),
        "insert_pages" => Ok(Request::InsertPages {
            from: expand(args.required("from")?),
            pages: if args.has("pages") {
                Some(args.pages("pages")?)
            } else {
                None
            },
            after: args.after("after_page")?,
            password: args.text("password").map(str::to_owned),
        }),
        "undo" => Ok(Request::Undo),
        "redo" => Ok(Request::Redo),
        "ask_person" => ask_person(&args),
        _ => Err(format!("there is no tool called {name}")),
    }
}

const MOST_OPTIONS: usize = 4;

fn ask_person(args: &Args) -> Result<Request, String> {
    let question = args.required("question")?.trim().to_owned();
    if question.is_empty() {
        return Err("`question` is empty".to_owned());
    }
    let listed = args
        .0
        .get("options")
        .and_then(Json::as_list)
        .ok_or("`options` is needed: two to four answers, each with a `label`")?;
    let mut options = Vec::new();
    for option in listed {
        let label = option
            .get("label")
            .and_then(Json::as_str)
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .ok_or("every option needs a `label`")?;
        let means = option
            .get("description")
            .and_then(Json::as_str)
            .map(str::trim)
            .unwrap_or_default();
        options.push((label.to_owned(), means.to_owned()));
    }
    if !(2..=MOST_OPTIONS).contains(&options.len()) {
        return Err(format!(
            "give two to {MOST_OPTIONS} options, not {}; the person can always type their own",
            options.len()
        ));
    }
    Ok(Request::AskPerson { question, options })
}

fn optional_page(args: &Args, key: &str) -> Result<Option<usize>, String> {
    if args.has(key) {
        args.page(key).map(Some)
    } else {
        Ok(None)
    }
}

fn add_text(args: &Args) -> Result<Request, String> {
    let page = args.page("page")?;
    let left = args.number("left").ok_or("`left` is needed, in points")?;
    let top = args.number("top").ok_or("`top` is needed, in points")?;
    let width = args
        .number("width")
        .filter(|width| *width > 0.0)
        .ok_or("`width` is needed, in points")?;
    let text = args.required("text")?.to_owned();
    let size = args
        .number("size")
        .filter(|size| *size > 0.0)
        .unwrap_or(12.0);
    let family = match args.text("font") {
        Some(family) => family.to_owned(),
        None => crate::about::family_for(&text)
            .ok_or("no installed font has every character of this text: pass `font`")?,
    };
    let fill = args.text("color").map(colour).transpose()?;
    let lines = text.lines().count().max(1);
    #[expect(clippy::cast_precision_loss, reason = "a count of lines")]
    let height = size * 1.4 * lines as f64;
    Ok(Request::AddText {
        page,
        area: [left, top, left + width, top + height],
        text,
        style: NewText {
            family,
            size,
            bold: args.flag("bold"),
            italic: args.flag("italic"),
            fill,
        },
    })
}

fn write_pages(args: &Args) -> Result<Request, String> {
    let markdown = args.required("markdown")?.to_owned();
    if markdown.trim().is_empty() {
        return Err("`markdown` is empty: there is nothing to write".to_owned());
    }
    let size = args
        .number("size")
        .filter(|size| (4.0..=96.0).contains(size))
        .unwrap_or(11.0);
    let margin = args
        .number("margin")
        .filter(|margin| (0.0..=300.0).contains(margin))
        .unwrap_or(56.0);
    let family = match args.text("font") {
        Some(family) => family.to_owned(),
        None => crate::about::family_for(&markdown)
            .ok_or("no installed font has every character of this text: pass `font`")?,
    };
    let theme = match args.text("theme") {
        None => crate::composing::theme::default_theme().name.to_owned(),
        Some(name) => crate::composing::theme::named(name)
            .ok_or_else(|| {
                format!(
                    "there is no theme \"{name}\": {}",
                    crate::composing::theme::names().join(", ")
                )
            })?
            .name
            .to_owned(),
    };
    Ok(Request::WritePages {
        from_page: args.page("from_page").unwrap_or(0),
        markdown,
        replace: args.flag("replace"),
        size,
        family,
        margin,
        theme,
    })
}

fn set_properties(args: &Args) -> Result<Request, String> {
    let edit = pdf_edit::info::InfoEdit {
        title: args.text("title").map(str::to_owned),
        author: args.text("author").map(str::to_owned),
        subject: args.text("subject").map(str::to_owned),
        keywords: args.text("keywords").map(str::to_owned),
        ..pdf_edit::info::InfoEdit::default()
    };
    if edit.asks_for_nothing() {
        return Err("nothing to set: pass title, author, subject or keywords".to_owned());
    }
    Ok(Request::SetProperties(edit))
}

fn quarter_turns(args: &Args) -> Result<i32, String> {
    let degrees = args.number("degrees").ok_or("`degrees` is needed")?;
    if degrees.fract() != 0.0 || degrees % 90.0 != 0.0 || degrees.abs() > 270.0 {
        return Err("`degrees` is 90, 180 or 270, or the same negative".to_owned());
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a multiple of 90 within 270 either way, checked above"
    )]
    Ok((degrees / 90.0) as i32)
}

#[cfg(test)]
mod tests;
