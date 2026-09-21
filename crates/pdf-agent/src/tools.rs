use std::path::Path;

use std::fmt::Write as _;

use crate::connect::ToolOffer;
use crate::desk::{Block, Desk, MOST_CHARACTERS};

pub mod request;

const MOST_HITS: usize = 200;

const MOST_HIT_CHARACTERS: usize = 600;

fn keep(so_far: &[(Block, usize)]) -> bool {
    so_far.len() < MOST_HITS
        && so_far
            .iter()
            .map(|(block, _)| block.text.chars().count().min(MOST_HIT_CHARACTERS) + 60)
            .sum::<usize>()
            < MOST_CHARACTERS
}
use crate::json::Json;

pub const INSTRUCTIONS: &str = "PanPDF reads and edits PDF documents on this computer, with the same engine as the PanPDF window. \
Start with open_document, which gives a handle every other tool takes. Read with read_text (text in blocks, each named like p3-b12) \
or find_text, and look at a page with render_page when layout, pictures or scanned pages matter. \
Change text with replace_text, naming a block; to change a few words, pass `find` so only those are replaced and the rest keeps its style. \
Every change is checked by the engine before it is written, and a refusal says why. Changes stay in memory until save_document; \
undo takes back the last one. Pages are counted from 1. Positions are points from the top-left corner of the page as shown. \
Never set replace or set_aside_restrictions without the person's agreement.";

struct Tool {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    input: &'static str,
    read_only: bool,
    destructive: bool,
}

const DOCUMENT: &str =
    r#""document":{"type":"string","description":"The handle open_document gave, like doc-1."}"#;

#[expect(
    clippy::too_many_lines,
    reason = "a table of the tools offered, one entry each"
)]
fn tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "open_document",
            title: "Open a PDF",
            description: "Opens a PDF file and returns a handle for the other tools, with its page count and title. \
Opening changes nothing on disk.",
            input: r#"{"type":"object","properties":{
"path":{"type":"string","description":"The file's path. A path that starts with ~ is taken from the home folder."},
"password":{"type":"string","description":"The document's password, when it asks for one. Ask the person; never guess."},
"set_aside_restrictions":{"type":"boolean","description":"Open a document whose author restricted editing so that it can be edited anyway. Only when the person says they have the right to."}},
"required":["path"],"additionalProperties":false}"#,
            read_only: true,
            destructive: false,
        },
        Tool {
            name: "list_documents",
            title: "Open documents",
            description: "Lists the documents open now: handle, file, pages, and whether there are changes not saved.",
            input: r#"{"type":"object","additionalProperties":false}"#,
            read_only: true,
            destructive: false,
        },
        Tool {
            name: "close_document",
            title: "Close a document",
            description: "Closes a document. Changes not saved are lost, and the reply says whether there were any.",
            input: r#"{"type":"object","properties":{DOCUMENT},"required":["document"],"additionalProperties":false}"#,
            read_only: false,
            destructive: true,
        },
        Tool {
            name: "document_info",
            title: "What a document says about itself",
            description: "Title, author and the other properties; each page's size in points; the bookmarks; \
the form's fields with their values; and the signatures with whether each is intact.",
            input: r#"{"type":"object","properties":{DOCUMENT},"required":["document"],"additionalProperties":false}"#,
            read_only: true,
            destructive: false,
        },
        Tool {
            name: "read_text",
            title: "Read text",
            description: "Reads the text of a range of pages in blocks -- paragraphs, headings, cells -- in reading order. \
Each block has a name like p3-b12 that replace_text takes, its box on the page in points [left, top, right, bottom], \
and its size. Long documents are read in parts: the reply says where to continue. A page with no blocks may be a scan: render_page shows it.",
            input: r#"{"type":"object","properties":{DOCUMENT,
"first_page":{"type":"integer","minimum":1,"description":"The first page to read. Default 1."},
"last_page":{"type":"integer","minimum":1,"description":"The last page to read. Default: as far as the reply has room for."}},
"required":["document"],"additionalProperties":false}"#,
            read_only: true,
            destructive: false,
        },
        Tool {
            name: "find_text",
            title: "Find text",
            description: "Finds every block holding a piece of text, with the block's name, its page and box, and its text. At most 200 blocks come back, each block's text cut to 600 characters; when there are more, the answer says so, and a longer piece of text or a page range finds them.",
            input: r#"{"type":"object","properties":{DOCUMENT,
"text":{"type":"string","description":"What to look for."},
"match_case":{"type":"boolean","description":"Whether capitals must match. Default false."},
"first_page":{"type":"integer","minimum":1,"description":"The first page to search. Default 1."},
"last_page":{"type":"integer","minimum":1,"description":"The last page to search. Default the last page."}},
"required":["document","text"],"additionalProperties":false}"#,
            read_only: true,
            destructive: false,
        },
        Tool {
            name: "render_page",
            title: "See a page",
            description: "Draws a page as it is shown and returns it as a PNG picture, to see layout, pictures, drawings, or a scanned page.",
            input: r#"{"type":"object","properties":{DOCUMENT,
"page":{"type":"integer","minimum":1},
"dpi":{"type":"number","minimum":10,"maximum":300,"description":"Resolution. Default 96. The picture's longer side is at most 2400 pixels."}},
"required":["document","page"],"additionalProperties":false}"#,
            read_only: true,
            destructive: false,
        },
        Tool {
            name: "replace_text",
            title: "Change text",
            description: "Replaces the text of a block, laid out again in its own font, size and width, as the window does when a person types. \
With `find`, only that piece of the block is replaced -- it must occur in the block exactly once -- and the rest keeps its style. \
An empty `text` deletes. Use \\n for a new paragraph. The reply is the block as it now reads.",
            input: r#"{"type":"object","properties":{DOCUMENT,
"block":{"type":"string","description":"A block name from read_text or find_text, like p3-b12."},
"text":{"type":"string","description":"The new text."},
"find":{"type":"string","description":"The piece of the block to replace. Leave out to replace the whole block."}},
"required":["document","block","text"],"additionalProperties":false}"#,
            read_only: false,
            destructive: false,
        },
        Tool {
            name: "add_text",
            title: "Write new text",
            description: "Writes new text on a page in a frame: `left` and `top` place its top-left corner, `width` is how wide it wraps.",
            input: r#"{"type":"object","properties":{DOCUMENT,
"page":{"type":"integer","minimum":1},
"left":{"type":"number"},"top":{"type":"number"},
"width":{"type":"number","exclusiveMinimum":0},
"text":{"type":"string"},
"size":{"type":"number","exclusiveMinimum":0,"description":"In points. Default 12."},
"font":{"type":"string","description":"A family list_fonts names. Default: one that has every character of the text."},
"bold":{"type":"boolean"},"italic":{"type":"boolean"},
"color":{"type":"string","description":"As #rrggbb. Default black."}},
"required":["document","page","left","top","width","text"],"additionalProperties":false}"#,
            read_only: false,
            destructive: false,
        },
        Tool {
            name: "list_fonts",
            title: "Fonts",
            description: "The font families new text can be set in, optionally only those whose name holds `name`.",
            input: r#"{"type":"object","properties":{"name":{"type":"string"}},"additionalProperties":false}"#,
            read_only: true,
            destructive: false,
        },
        Tool {
            name: "set_properties",
            title: "Set document properties",
            description: "Sets the document's title, author, subject or keywords. What is left out is kept.",
            input: r#"{"type":"object","properties":{DOCUMENT,
"title":{"type":"string"},"author":{"type":"string"},"subject":{"type":"string"},"keywords":{"type":"string"}},
"required":["document"],"additionalProperties":false}"#,
            read_only: false,
            destructive: false,
        },
        Tool {
            name: "fill_field",
            title: "Fill a form field",
            description: "Fills a field of the document's form, named as document_info lists it. A text field takes text; \
a check box or radio button takes the state to show (document_info lists them) or true/false; a list or combo box takes one of its options.",
            input: r#"{"type":"object","properties":{DOCUMENT,
"name":{"type":"string"},
"value":{"type":["string","boolean"]}},
"required":["document","name","value"],"additionalProperties":false}"#,
            read_only: false,
            destructive: false,
        },
        Tool {
            name: "add_blank_page",
            title: "Add a blank page",
            description: "Puts in a blank page after `after_page` (0 puts it first), the size of the page beside it unless a size is given.",
            input: r#"{"type":"object","properties":{DOCUMENT,
"after_page":{"type":"integer","minimum":0},
"width":{"type":"number","exclusiveMinimum":0},"height":{"type":"number","exclusiveMinimum":0}},
"required":["document","after_page"],"additionalProperties":false}"#,
            read_only: false,
            destructive: false,
        },
        Tool {
            name: "delete_pages",
            title: "Delete pages",
            description: "Takes pages out of the document. Every page cannot be taken out.",
            input: r#"{"type":"object","properties":{DOCUMENT,
"pages":{"type":"array","items":{"type":"integer","minimum":1},"minItems":1}},
"required":["document","pages"],"additionalProperties":false}"#,
            read_only: false,
            destructive: true,
        },
        Tool {
            name: "move_pages",
            title: "Move pages",
            description: "Moves pages so the first of them becomes page `to` and the rest follow it in the order given.",
            input: r#"{"type":"object","properties":{DOCUMENT,
"pages":{"type":"array","items":{"type":"integer","minimum":1},"minItems":1},
"to":{"type":"integer","minimum":1}},
"required":["document","pages","to"],"additionalProperties":false}"#,
            read_only: false,
            destructive: false,
        },
        Tool {
            name: "rotate_pages",
            title: "Turn pages",
            description: "Turns pages clockwise by 90, 180 or 270 degrees (negative turns the other way).",
            input: r#"{"type":"object","properties":{DOCUMENT,
"pages":{"type":"array","items":{"type":"integer","minimum":1},"minItems":1},
"degrees":{"type":"integer","enum":[90,180,270,-90,-180,-270]}},
"required":["document","pages","degrees"],"additionalProperties":false}"#,
            read_only: false,
            destructive: false,
        },
        Tool {
            name: "insert_pages",
            title: "Put in pages from another PDF",
            description: "Copies pages of another PDF file in after `after_page` (0 puts them first). The other file must not be password-protected.",
            input: r#"{"type":"object","properties":{DOCUMENT,
"from":{"type":"string","description":"The other file's path."},
"pages":{"type":"array","items":{"type":"integer","minimum":1},"description":"Its pages to copy. Default: all."},
"after_page":{"type":"integer","minimum":0}},
"required":["document","from","after_page"],"additionalProperties":false}"#,
            read_only: false,
            destructive: false,
        },
        Tool {
            name: "undo",
            title: "Undo",
            description: "Takes back the last change.",
            input: r#"{"type":"object","properties":{DOCUMENT},"required":["document"],"additionalProperties":false}"#,
            read_only: false,
            destructive: false,
        },
        Tool {
            name: "redo",
            title: "Redo",
            description: "Puts back the last change undo took back.",
            input: r#"{"type":"object","properties":{DOCUMENT},"required":["document"],"additionalProperties":false}"#,
            read_only: false,
            destructive: false,
        },
        Tool {
            name: "save_document",
            title: "Save",
            description: "Writes the document to a file. Without `path` it is saved as a new file beside the original, named ...-edited.pdf. \
An existing file is written over only with replace: true, which needs the person's agreement; the original is never written over if it changed on disk since it was opened.",
            input: r#"{"type":"object","properties":{DOCUMENT,
"path":{"type":"string"},
"replace":{"type":"boolean"}},
"required":["document"],"additionalProperties":false}"#,
            read_only: false,
            destructive: true,
        },
    ]
}

#[must_use]
pub fn listed() -> Json {
    Json::List(
        tools()
            .into_iter()
            .map(|tool| {
                let schema = tool.input.replace("DOCUMENT", DOCUMENT);
                Json::object([
                    ("name", Json::text(tool.name)),
                    ("title", Json::text(tool.title)),
                    ("description", Json::text(tool.description)),
                    (
                        "inputSchema",
                        Json::parse(&schema).expect("every schema above is JSON"),
                    ),
                    (
                        "annotations",
                        Json::object([
                            ("title", Json::text(tool.title)),
                            ("readOnlyHint", Json::Bool(tool.read_only)),
                            ("destructiveHint", Json::Bool(tool.destructive)),
                            ("idempotentHint", Json::Bool(tool.read_only)),
                            ("openWorldHint", Json::Bool(false)),
                        ]),
                    ),
                ])
            })
            .collect(),
    )
}

#[must_use]
pub fn exists(name: &str) -> bool {
    tools().iter().any(|tool| tool.name == name)
}

const NOT_IN_A_WINDOW: [&str; 4] = [
    "open_document",
    "list_documents",
    "close_document",
    "save_document",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolFacts {
    pub read_only: bool,
    pub destructive: bool,
}

#[must_use]
pub fn facts(name: &str) -> Option<ToolFacts> {
    tools()
        .iter()
        .find(|tool| tool.name == name)
        .map(|tool| ToolFacts {
            read_only: tool.read_only,
            destructive: tool.destructive,
        })
}

#[must_use]
pub fn offered_to_a_window() -> Vec<ToolOffer> {
    tools()
        .into_iter()
        .filter(|tool| !NOT_IN_A_WINDOW.contains(&tool.name))
        .map(|tool| ToolOffer {
            name: tool.name.to_owned(),
            description: tool.description.to_owned(),
            schema: Json::parse(&tool.input.replace("DOCUMENT", DOCUMENT))
                .expect("every schema above is JSON"),
        })
        .collect()
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DocumentBrief {
    pub file_name: String,
    pub title: String,
    pub pages: usize,
}

#[must_use]
pub fn window_instructions(brief: &DocumentBrief) -> String {
    let mut about = String::new();
    if !brief.title.is_empty() {
        let _ = write!(about, ", titled \"{}\"", brief.title);
    }
    if !brief.file_name.is_empty() {
        let _ = write!(about, ", the file {}", brief.file_name);
    }
    let pages = if brief.pages == 1 {
        "1 page long".to_owned()
    } else {
        format!("{} pages long", brief.pages)
    };
    format!(
        "You are helping the person at the PanPDF window with the PDF they have open.\n\
\n\
The open document is `doc-1`{about}, {pages}. Pass \"doc-1\" as `document` to every \
tool; there is no other document, and none to open, list or close.\n\
\n\
What you change appears in their window at once, as one step they can undo, and nothing is \
written to the file: **the person saves**, with Ctrl+S, and you never do. Read before you \
change: read_text or find_text first, so that you change the text that is really there.\n\
\n\
A block is named `p<page>-b<index>` -- p3-b12 is the twelfth block of page 3 -- and a name is \
only good until that page changes. After any change to a page, read it again before naming a \
block on it.\n\
\n\
The person may refuse an action. A refusal is their answer: do not try it again in another \
way, and ask them what they would like instead. Pages are counted from 1, and positions are \
points from the top-left corner of the page as it is shown.",
    )
}

pub struct Answer {
    pub text: String,
    pub data: Json,
    pub picture: Option<Vec<u8>>,
}

impl Answer {
    fn of(text: impl Into<String>, data: Json) -> Self {
        Self {
            text: text.into(),
            data,
            picture: None,
        }
    }
}

pub(crate) struct Args<'a>(pub(crate) &'a Json);

impl Args<'_> {
    pub(crate) fn text(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(Json::as_str)
    }

    pub(crate) fn required(&self, key: &str) -> Result<&str, String> {
        self.text(key)
            .ok_or_else(|| format!("`{key}` is needed, as text"))
    }

    pub(crate) fn flag(&self, key: &str) -> bool {
        self.0.get(key).and_then(Json::as_bool).unwrap_or(false)
    }

    pub(crate) fn number(&self, key: &str) -> Option<f64> {
        self.0.get(key).and_then(Json::as_f64)
    }

    pub(crate) fn has(&self, key: &str) -> bool {
        self.0.get(key).is_some()
    }

    pub(crate) fn page(&self, key: &str) -> Result<usize, String> {
        self.0
            .get(key)
            .and_then(Json::as_count)
            .filter(|page| *page >= 1)
            .map(|page| page - 1)
            .ok_or_else(|| format!("`{key}` is needed, as a page number from 1"))
    }

    pub(crate) fn after(&self, key: &str) -> Result<usize, String> {
        self.0
            .get(key)
            .and_then(Json::as_count)
            .ok_or_else(|| format!("`{key}` is needed: 0 puts it first"))
    }

    pub(crate) fn pages(&self, key: &str) -> Result<Vec<usize>, String> {
        self.0
            .get(key)
            .and_then(Json::as_list)
            .ok_or_else(|| format!("`{key}` is needed, as a list of page numbers"))?
            .iter()
            .map(|item| {
                item.as_count()
                    .filter(|page| *page >= 1)
                    .map(|page| page - 1)
                    .ok_or_else(|| format!("`{key}` holds something that is not a page number"))
            })
            .collect()
    }
}

pub fn call(desk: &mut Desk, name: &str, arguments: &Json) -> Result<Answer, String> {
    let empty = Json::Object(std::collections::BTreeMap::new());
    let args = Args(if matches!(arguments, Json::Null) {
        &empty
    } else {
        arguments
    });
    match name {
        "open_document" => open_document(desk, &args),
        "list_documents" => Ok(list_documents(desk)),
        "close_document" => close_document(desk, &args),
        "document_info" => crate::about::document_info(desk, args.required("document")?),
        "read_text" => read_text(desk, &args),
        "find_text" => find_text(desk, &args),
        "render_page" => render_page(desk, &args),
        "replace_text" => replace_text(desk, &args),
        "add_text" => add_text(desk, &args),
        "list_fonts" => Ok(list_fonts(&args)),
        "set_properties" => set_properties(desk, &args),
        "fill_field" => crate::about::fill_field(
            desk,
            args.required("document")?,
            args.required("name")?,
            args.0.get("value").ok_or("`value` is needed")?,
        ),
        "add_blank_page" => add_blank_page(desk, &args),
        "delete_pages" => delete_pages(desk, &args),
        "move_pages" => {
            let handle = args.required("document")?;
            let pages = args.pages("pages")?;
            check_pages(desk, handle, &pages)?;
            let to = args.page("to")?;
            desk.command(handle, &pdf_edit::Command::MovePages { pages, to })?;
            Ok(Answer::of("Moved.", Json::Null))
        }
        "rotate_pages" => rotate_pages(desk, &args),
        "insert_pages" => insert_pages(desk, &args),
        "undo" | "redo" => walk(desk, &args, name == "undo"),
        "save_document" => save_document(desk, &args),
        _ => Err(format!("there is no tool called {name}")),
    }
}

fn open_document(desk: &mut Desk, args: &Args) -> Result<Answer, String> {
    let path = expand(args.required("path")?);
    let set_aside = args.flag("set_aside_restrictions");
    let summary = desk.open(&path, args.text("password").unwrap_or_default(), set_aside)?;
    let mut said = format!(
        "Opened {} as {}: {} page{}",
        path.display(),
        summary.handle,
        summary.pages,
        plural(summary.pages)
    );
    if !summary.title.is_empty() {
        let _ = write!(said, ", titled {:?}", summary.title);
    }
    said.push('.');
    if summary.restricted {
        said.push_str(if set_aside {
            " Its author restricted editing; that was set aside at the person's word."
        } else {
            " Its author restricted editing, so changes will be refused: it can be read. \
If the person says they have the right to edit it, open it again with set_aside_restrictions."
        });
    }
    Ok(Answer::of(
        said,
        Json::object([
            ("document", Json::text(summary.handle)),
            ("pages", Json::count(summary.pages)),
            ("title", Json::text(summary.title)),
            ("protected", Json::Bool(summary.protected)),
            ("editing_restricted", Json::Bool(summary.restricted)),
        ]),
    ))
}

fn list_documents(desk: &Desk) -> Answer {
    let open = desk.handles();
    let said = if open.is_empty() {
        "No document is open.".to_owned()
    } else {
        open.iter()
            .map(|(handle, path, pages, changed)| {
                format!(
                    "{handle}: {} ({pages} page{}{})",
                    path.display(),
                    plural(*pages),
                    if *changed { ", changed, not saved" } else { "" }
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    Answer::of(
        said,
        Json::List(
            open.into_iter()
                .map(|(handle, path, pages, changed)| {
                    Json::object([
                        ("document", Json::text(handle)),
                        ("path", Json::text(path.display().to_string())),
                        ("pages", Json::count(pages)),
                        ("unsaved_changes", Json::Bool(changed)),
                    ])
                })
                .collect(),
        ),
    )
}

fn close_document(desk: &mut Desk, args: &Args) -> Result<Answer, String> {
    let handle = args.required("document")?;
    let lost = desk.close(handle)?;
    Ok(Answer::of(
        if lost {
            format!("Closed {handle}. It had changes that were not saved; they are gone.")
        } else {
            format!("Closed {handle}.")
        },
        Json::object([("unsaved_changes_lost", Json::Bool(lost))]),
    ))
}

fn read_text(desk: &mut Desk, args: &Args) -> Result<Answer, String> {
    let handle = args.required("document")?;
    let count = desk.page_count(handle)?;
    let (first, last) = page_range(args, count)?;
    read_pages(desk, handle, first, last)
}

fn page_range(args: &Args, count: usize) -> Result<(usize, usize), String> {
    let first = if args.has("first_page") {
        Some(args.page("first_page")?)
    } else {
        None
    };
    let last = if args.has("last_page") {
        Some(args.page("last_page")?)
    } else {
        None
    };
    page_span(first, last, count)
}

pub fn page_span(
    first: Option<usize>,
    last: Option<usize>,
    count: usize,
) -> Result<(usize, usize), String> {
    let first = first.unwrap_or(0);
    let last = last
        .unwrap_or_else(|| count.saturating_sub(1))
        .min(count.saturating_sub(1));
    if first >= count {
        return Err(format!(
            "there is no page {}: the document has {count}",
            first + 1
        ));
    }
    Ok((first, last.max(first)))
}

fn find_text(desk: &mut Desk, args: &Args) -> Result<Answer, String> {
    let handle = args.required("document")?;
    let wanted = args.required("text")?.to_owned();
    if wanted.is_empty() {
        return Err("`text` is empty".to_owned());
    }
    let match_case = args.flag("match_case");
    let count = desk.page_count(handle)?;
    let (first, last) = page_range(args, count)?;
    format_hits((&wanted, match_case), (first, last, count), &mut |page| {
        Ok(desk.blocks(handle, page).unwrap_or_default())
    })
}

pub fn format_hits(
    (wanted, match_case): (&str, bool),
    (first, last, count): (usize, usize, usize),
    blocks: &mut dyn FnMut(usize) -> Result<Vec<Block>, String>,
) -> Result<Answer, String> {
    let fold = |text: &str| {
        if match_case {
            text.to_owned()
        } else {
            text.to_lowercase()
        }
    };
    let wanted = fold(wanted);
    let mut hits = Vec::new();
    let mut more = 0;
    for page in first..=last {
        for block in blocks(page)? {
            let times = fold(&block.text).matches(&wanted).count();
            if times == 0 {
                continue;
            }
            if keep(&hits) {
                hits.push((block, times));
            } else {
                more += 1;
            }
        }
    }
    let total: usize = hits.iter().map(|(_, times)| times).sum();
    let mut said = format!(
        "{total} match{} in {} block{}{}.",
        if total == 1 { "" } else { "es" },
        hits.len(),
        plural(hits.len()),
        if first == 0 && last + 1 == count {
            String::new()
        } else {
            format!(" of pages {} to {}", first + 1, last + 1)
        }
    );
    if more > 0 {
        let _ = write!(
            said,
            "\n{more} more block{} hold it and are not listed: look for a longer piece \
             of text, or search a range of pages with first_page and last_page.",
            plural(more)
        );
    }
    for (block, times) in &hits {
        let _ = write!(
            said,
            "\n{} (page {}, {}x): {}",
            block.name(),
            block.page + 1,
            times,
            clip(&block.text, 300)
        );
    }
    Ok(Answer::of(
        said,
        Json::List(
            hits.iter()
                .map(|(block, times)| {
                    let mut described = described(&Block {
                        text: clip(&block.text, MOST_HIT_CHARACTERS),
                        ..block.clone()
                    });
                    if let Json::Object(members) = &mut described {
                        members.insert("matches".to_owned(), Json::count(*times));
                    }
                    described
                })
                .collect(),
        ),
    ))
}

fn render_page(desk: &mut Desk, args: &Args) -> Result<Answer, String> {
    let handle = args.required("document")?;
    let at = args.page("page")?;
    let dpi = args.number("dpi").unwrap_or(96.0).clamp(10.0, 300.0);
    let (png, width, height) = desk.picture(handle, at, dpi)?;
    Ok(Answer {
        text: format!("Page {} as shown, {width} x {height} pixels.", at + 1),
        data: Json::object([
            ("page", Json::count(at + 1)),
            ("width", Json::Number(f64::from(width))),
            ("height", Json::Number(f64::from(height))),
        ]),
        picture: Some(png),
    })
}

fn replace_text(desk: &mut Desk, args: &Args) -> Result<Answer, String> {
    let handle = args.required("document")?;
    let block = args.required("block")?;
    let replacement = args.required("text")?;
    let now = desk.rewrite(handle, block, args.text("find"), replacement)?;
    Ok(Answer::of(
        format!("Done. {} now reads: {}", now.name(), clip(&now.text, 2_000)),
        described(&now),
    ))
}

fn add_text(desk: &mut Desk, args: &Args) -> Result<Answer, String> {
    let handle = args.required("document")?;
    let at = args.page("page")?;
    let left = args.number("left").ok_or("`left` is needed, in points")?;
    let top = args.number("top").ok_or("`top` is needed, in points")?;
    let width = args
        .number("width")
        .filter(|width| *width > 0.0)
        .ok_or("`width` is needed, in points")?;
    let written = args.required("text")?;
    let size = args
        .number("size")
        .filter(|size| *size > 0.0)
        .unwrap_or(12.0);
    let family = match args.text("font") {
        Some(family) => family.to_owned(),
        None => crate::about::family_for(written)
            .ok_or("no installed font has every character of this text: pass `font`")?,
    };
    let fill = args.text("color").map(colour).transpose()?;
    let lines = written.lines().count().max(1);
    #[expect(clippy::cast_precision_loss, reason = "a count of lines")]
    let height = size * 1.4 * lines as f64;
    desk.place_text(
        handle,
        at,
        [left, top, left + width, top + height],
        written,
        (&family, size, args.flag("bold"), args.flag("italic"), fill),
    )?;
    Ok(Answer::of(
        format!("Written on page {} in {family}, {size} pt.", at + 1),
        Json::object([("font", Json::text(family))]),
    ))
}

fn list_fonts(args: &Args) -> Answer {
    list_fonts_named(args.text("name"))
}

#[must_use]
pub fn list_fonts_named(name: Option<&str>) -> Answer {
    let wanted = name.map(str::to_lowercase);
    let families: Vec<&String> = pdf_cli::font_families()
        .iter()
        .filter(|family| {
            wanted
                .as_deref()
                .is_none_or(|wanted| family.to_lowercase().contains(wanted))
        })
        .collect();
    Answer::of(
        families
            .iter()
            .map(|family| family.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        Json::List(
            families
                .into_iter()
                .map(|family| Json::text(family.clone()))
                .collect(),
        ),
    )
}

fn set_properties(desk: &mut Desk, args: &Args) -> Result<Answer, String> {
    let handle = args.required("document")?;
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
    desk.command(handle, &pdf_edit::Command::SetDocumentInfo { edit })?;
    Ok(Answer::of("Properties set.", Json::Null))
}

#[must_use]
pub const fn beside_after(after: usize) -> (usize, bool) {
    if after == 0 {
        (0, true)
    } else {
        (after - 1, false)
    }
}

fn add_blank_page(desk: &mut Desk, args: &Args) -> Result<Answer, String> {
    let handle = args.required("document")?;
    let after = args.after("after_page")?;
    let sizes = desk.page_sizes(handle)?;
    if after > sizes.len() {
        return Err(format!(
            "there is no page {after}: the document has {}",
            sizes.len()
        ));
    }
    let (beside, before) = beside_after(after);
    let size = match (args.number("width"), args.number("height")) {
        (Some(width), Some(height)) if width > 0.0 && height > 0.0 => [width, height],
        _ => sizes[beside],
    };
    desk.command(
        handle,
        &pdf_edit::Command::AddBlankPage {
            beside,
            before,
            size,
        },
    )?;
    Ok(Answer::of(
        format!("A blank page is now page {}.", after + 1),
        Json::object([("page", Json::count(after + 1))]),
    ))
}

fn delete_pages(desk: &mut Desk, args: &Args) -> Result<Answer, String> {
    let handle = args.required("document")?;
    let pages = args.pages("pages")?;
    check_pages(desk, handle, &pages)?;
    let many = pages.len();
    desk.command(handle, &pdf_edit::Command::RemovePages { pages })?;
    Ok(Answer::of(
        format!("Took out {many} page{}.", plural(many)),
        Json::object([("pages", Json::count(desk.page_count(handle)?))]),
    ))
}

fn rotate_pages(desk: &mut Desk, args: &Args) -> Result<Answer, String> {
    let handle = args.required("document")?;
    let pages = args.pages("pages")?;
    check_pages(desk, handle, &pages)?;
    let degrees = args.number("degrees").ok_or("`degrees` is needed")?;
    if degrees.fract() != 0.0 || degrees % 90.0 != 0.0 || degrees.abs() > 270.0 {
        return Err("`degrees` is 90, 180 or 270, or the same negative".to_owned());
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a multiple of 90 within 270 either way, checked above"
    )]
    let quarter_turns = (degrees / 90.0) as i32;
    desk.command(
        handle,
        &pdf_edit::Command::RotatePages {
            pages,
            quarter_turns,
        },
    )?;
    Ok(Answer::of("Turned.", Json::Null))
}

fn insert_pages(desk: &mut Desk, args: &Args) -> Result<Answer, String> {
    let handle = args.required("document")?;
    let from = expand(args.required("from")?);
    let bytes: std::sync::Arc<[u8]> = std::fs::read(&from)
        .map_err(|error| format!("{} cannot be read: {error}", from.display()))?
        .into();
    let other =
        pdf_bytes::ByteStore::new(pdf_bytes::SourceId::new(1), std::sync::Arc::clone(&bytes));
    if pdf_edit::info::lock(&other, b"") == pdf_edit::info::Lock::Refused {
        return Err(format!(
            "{} is protected by a password, and pages cannot be taken out of a protected \
             file yet -- not even with the password. Open it in PanPDF, save an unprotected \
             copy, and take the pages from that.",
            from.display()
        ));
    }
    let available = pdf_session::Session::new(other, b"")
        .page_count()
        .map_err(|error| format!("{} has no pages this can read: {error}", from.display()))?;
    let chosen = if args.has("pages") {
        args.pages("pages")?
    } else {
        (0..available).collect()
    };
    if let Some(missing) = chosen.iter().find(|page| **page >= available) {
        return Err(format!(
            "{} has no page {}: it has {available}",
            from.display(),
            missing + 1
        ));
    }
    let after = args.after("after_page")?;
    let count = desk.page_count(handle)?;
    if after > count {
        return Err(format!(
            "there is no page {after}: the document has {count}"
        ));
    }
    let (beside, before) = beside_after(after);
    let many = chosen.len();
    desk.command(
        handle,
        &pdf_edit::Command::InsertPages {
            beside,
            before,
            document: bytes,
            pages: chosen,
        },
    )?;
    Ok(Answer::of(
        format!("Put in {many} page{} after page {after}.", plural(many)),
        Json::object([("pages", Json::count(desk.page_count(handle)?))]),
    ))
}

fn walk(desk: &mut Desk, args: &Args, back: bool) -> Result<Answer, String> {
    let handle = args.required("document")?;
    let walked = desk.walk(handle, back)?;
    Ok(Answer::of(
        match (walked, back) {
            (true, true) => "Took back the last change.",
            (true, false) => "Put the change back.",
            (false, true) => "There is nothing to undo.",
            (false, false) => "There is nothing to redo.",
        },
        Json::object([("changed", Json::Bool(walked))]),
    ))
}

fn save_document(desk: &mut Desk, args: &Args) -> Result<Answer, String> {
    let handle = args.required("document")?;
    let destination = match args.text("path") {
        Some(path) => expand(path),
        None => beside(&desk.handles(), handle)?,
    };
    let bytes = desk.save(handle, &destination, args.flag("replace"))?;
    Ok(Answer::of(
        format!("Saved {} ({bytes} bytes).", destination.display()),
        Json::object([
            ("path", Json::text(destination.display().to_string())),
            #[expect(clippy::cast_precision_loss, reason = "a file's length")]
            ("bytes", Json::Number(bytes as f64)),
        ]),
    ))
}

#[must_use]
pub fn reading_size(blocks: &[Block]) -> usize {
    blocks
        .iter()
        .map(|block| block.text.chars().count() + 40)
        .sum()
}

fn read_pages(desk: &mut Desk, handle: &str, first: usize, last: usize) -> Result<Answer, String> {
    format_pages((first, last), &mut |page| desk.blocks(handle, page))
}

pub fn format_pages(
    (first, last): (usize, usize),
    blocks: &mut dyn FnMut(usize) -> Result<Vec<Block>, String>,
) -> Result<Answer, String> {
    let mut said = String::new();
    let mut pages = Vec::new();
    let mut characters = 0;
    let mut stopped_before = None;
    let mut clipped = Vec::new();
    for page in first..=last {
        let mut blocks = blocks(page)?;
        let size = reading_size(&blocks);
        if characters > 0 && characters + size > MOST_CHARACTERS {
            stopped_before = Some(page);
            break;
        }
        if size > MOST_CHARACTERS && !blocks.is_empty() {
            let share = MOST_CHARACTERS / blocks.len();
            for block in &mut blocks {
                block.text = clip(&block.text, share);
            }
            clipped.push(page + 1);
        }
        characters += size.min(MOST_CHARACTERS);
        let _ = write!(said, "\n=== Page {} ===\n", page + 1);
        if blocks.is_empty() {
            said.push_str(
                "(no text: this page may be a picture or a scan; render_page shows it)\n",
            );
        }
        for block in &blocks {
            let [left, top, right, bottom] = block.area;
            let _ = write!(
                said,
                "[{} | {left:.0},{top:.0},{right:.0},{bottom:.0} | {} pt{}]\n{}\n",
                block.name(),
                block.size,
                block
                    .fixed
                    .map_or(String::new(), |why| format!(" | read-only: {why}")),
                block.text
            );
        }
        pages.push(Json::object([
            ("page", Json::count(page + 1)),
            ("blocks", Json::List(blocks.iter().map(described).collect())),
        ]));
    }
    for page in &clipped {
        let _ = write!(
            said,
            "\n(Page {page} holds more text than one reply does, so each of its blocks \
             is cut short. read_text that page on its own, or find_text in it, to read a \
             block whole.)"
        );
    }
    if let Some(next) = stopped_before {
        let _ = write!(
            said,
            "\n(The reply is full. Continue with first_page: {}.)",
            next + 1
        );
    }
    Ok(Answer::of(
        said.trim_start().to_owned(),
        Json::object([
            ("pages", Json::List(pages)),
            (
                "clipped_pages",
                Json::List(clipped.iter().map(|page| Json::count(*page)).collect()),
            ),
            (
                "continue_from_page",
                stopped_before.map_or(Json::Null, |next| Json::count(next + 1)),
            ),
        ]),
    ))
}

fn described(block: &Block) -> Json {
    Json::object([
        ("block", Json::text(block.name())),
        ("page", Json::count(block.page + 1)),
        ("text", Json::text(block.text.clone())),
        (
            "box",
            Json::List(
                block
                    .area
                    .iter()
                    .map(|value| Json::Number(*value))
                    .collect(),
            ),
        ),
        ("size", Json::Number(block.size)),
        (
            "read_only",
            block.fixed.map_or(Json::Bool(false), Json::text),
        ),
    ])
}

fn check_pages(desk: &mut Desk, handle: &str, pages: &[usize]) -> Result<(), String> {
    let count = desk.page_count(handle)?;
    match pages.iter().find(|page| **page >= count) {
        Some(page) => Err(format!(
            "there is no page {}: the document has {count}",
            page + 1
        )),
        None => Ok(()),
    }
}

pub(crate) fn expand(path: &str) -> std::path::PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map_or_else(
                || Path::new(path).to_owned(),
                |home| Path::new(&home).join(rest),
            ),
        None => Path::new(path).to_owned(),
    }
}

fn beside(
    open: &[(String, std::path::PathBuf, usize, bool)],
    handle: &str,
) -> Result<std::path::PathBuf, String> {
    let (_, path, _, _) = open
        .iter()
        .find(|(held, ..)| held == handle)
        .ok_or_else(|| format!("no document is open as {handle}"))?;
    let stem = path.file_stem().map_or_else(
        || "document".to_owned(),
        |stem| stem.to_string_lossy().into_owned(),
    );
    for number in 1..10_000 {
        let name = if number == 1 {
            format!("{stem}-edited.pdf")
        } else {
            format!("{stem}-edited-{number}.pdf")
        };
        let candidate = path.with_file_name(name);
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err("no free name beside the original: pass `path`".to_owned())
}

pub(crate) fn colour(text: &str) -> Result<[f64; 3], String> {
    let hex = text.trim().trim_start_matches('#');
    if hex.len() != 6 || !hex.chars().all(|character| character.is_ascii_hexdigit()) {
        return Err(format!("{text:?} is not a colour like #1a2b3c"));
    }
    let part = |from: usize| {
        u8::from_str_radix(&hex[from..from + 2], 16).map_or(0.0, |value| f64::from(value) / 255.0)
    };
    Ok([part(0), part(2), part(4)])
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

fn clip(text: &str, most: usize) -> String {
    if text.chars().count() <= most {
        text.to_owned()
    } else {
        let mut clipped: String = text.chars().take(most).collect();
        clipped.push_str(" ...");
        clipped
    }
}

#[cfg(test)]
mod tests;
