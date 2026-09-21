use std::fmt::Write as _;

use crate::desk::Desk;
use crate::json::Json;
use crate::tools::Answer;

struct Report {
    said: pdf_edit::info::DocumentFacts,
    sizes: Vec<[f64; 2]>,
    bookmarks: Vec<pdf_edit::outline::Bookmark>,
    fields: Vec<(usize, pdf_edit::form::FormField)>,
    signatures: Vec<pdf_edit::signature::Signature>,
    set_aside: bool,
}

pub fn document_info(desk: &mut Desk, handle: &str) -> Result<Answer, String> {
    let sizes = desk.page_sizes(handle)?;
    let set_aside = desk.restrictions_set_aside(handle)?;
    let (source, credential) = desk.source(handle)?;
    describe(&source, &credential, sizes, set_aside)
}

pub fn describe(
    source: &pdf_bytes::ByteStore,
    credential: &[u8],
    sizes: Vec<[f64; 2]>,
    set_aside: bool,
) -> Result<Answer, String> {
    let facts = Report {
        said: pdf_edit::info::document_facts(source, credential).map_err(|error| {
            format!("what this document says about itself cannot be read: {error}")
        })?,
        sizes,
        bookmarks: pdf_edit::outline::read_outline(source, credential).unwrap_or_default(),
        fields: pdf_edit::form::fields_of_document(source, credential).unwrap_or_default(),
        signatures: pdf_edit::signature::signatures(source, credential).unwrap_or_default(),
        set_aside,
    };
    Ok(Answer {
        text: told(&facts),
        data: data(&facts),
        picture: None,
    })
}

fn told(facts: &Report) -> String {
    let info = &facts.said.info;
    let mut said = String::new();
    for (label, value) in [
        ("Title", &info.title),
        ("Author", &info.author),
        ("Subject", &info.subject),
        ("Keywords", &info.keywords),
        ("Made with", &info.creator),
        ("Written by", &info.producer),
    ] {
        if !value.is_empty() {
            let _ = writeln!(said, "{label}: {value}");
        }
    }
    let _ = writeln!(said, "Pages: {}", facts.sizes.len());
    if let Some(protection) = &facts.said.protection {
        let _ = writeln!(
            said,
            "Protected with a password (security revision {}).",
            protection.revision
        );
    }
    if facts.set_aside {
        said.push_str("Its author's editing restrictions were set aside at the person's word.\n");
    }
    let mut runs: Vec<(usize, usize, [f64; 2])> = Vec::new();
    for (page, size) in facts.sizes.iter().enumerate() {
        match runs.last_mut() {
            Some((_, last, held)) if same_size(*held, *size) => *last = page,
            _ => runs.push((page, page, *size)),
        }
    }
    for (first, last, [width, height]) in &runs {
        let pages = if first == last {
            format!("Page {}", first + 1)
        } else {
            format!("Pages {}-{}", first + 1, last + 1)
        };
        let _ = writeln!(said, "{pages}: {width:.0} x {height:.0} pt");
    }
    if !facts.bookmarks.is_empty() {
        said.push_str("Bookmarks:\n");
        for bookmark in facts.bookmarks.iter().take(200) {
            let _ = writeln!(
                said,
                "{}{}{}",
                "  ".repeat(bookmark.depth),
                bookmark.title,
                bookmark
                    .page
                    .map_or(String::new(), |page| format!(" (page {})", page + 1))
            );
        }
    }
    if !facts.fields.is_empty() {
        said.push_str("Form fields:\n");
        for (page, field) in &facts.fields {
            let _ = writeln!(
                said,
                "  {} ({}, page {}): {}{}",
                field.name,
                kind(field.kind),
                page + 1,
                value(&field.value),
                if field.states.is_empty() {
                    String::new()
                } else {
                    format!(" [states: {}]", field.states.join(", "))
                }
            );
        }
    }
    for signature in &facts.signatures {
        let _ = writeln!(said, "{}", signed(signature));
    }
    if !facts.signatures.is_empty() {
        said.push_str("Changing a signed document breaks what the signature can vouch for: tell the person before editing.\n");
    }
    said
}

fn signed(signature: &pdf_edit::signature::Signature) -> String {
    use pdf_edit::signature::{Covers, Integrity};
    let verdict =
        signature
            .checked
            .as_ref()
            .map_or("not checked", |checked| match checked.integrity {
                Integrity::Intact => "intact",
                Integrity::ContentChanged => "BROKEN: what it covers was changed after signing",
                Integrity::SignatureWrong => "BROKEN: the signature does not verify",
                Integrity::CannotCheck(_) => "cannot be checked",
            });
    let signer = signature
        .checked
        .as_ref()
        .and_then(|checked| checked.signer.as_ref())
        .map_or_else(|| signature.name.clone(), |who| who.common.clone());
    let covers = match signature.covers {
        Covers::WholeDocument => "covers the whole file",
        Covers::UpTo { .. } => "the file was added to after it",
        Covers::Unstated => "does not say what it covers",
    };
    format!("Signature by {signer}: {verdict}; {covers}.")
}

fn data(facts: &Report) -> Json {
    let info = &facts.said.info;
    let stamp = |stamp: Option<pdf_edit::info::Stamp>| {
        stamp.map_or(Json::Null, |stamp| Json::text(stamp.write()))
    };
    let texts = |items: &[String]| Json::List(items.iter().cloned().map(Json::Text).collect());
    Json::object([
        ("title", Json::text(info.title.clone())),
        ("author", Json::text(info.author.clone())),
        ("subject", Json::text(info.subject.clone())),
        ("keywords", Json::text(info.keywords.clone())),
        ("creator", Json::text(info.creator.clone())),
        ("producer", Json::text(info.producer.clone())),
        ("created", stamp(info.created)),
        ("modified", stamp(info.modified)),
        ("pages", Json::count(facts.sizes.len())),
        (
            "page_sizes",
            Json::List(
                facts
                    .sizes
                    .iter()
                    .map(|[width, height]| {
                        Json::List(vec![Json::Number(*width), Json::Number(*height)])
                    })
                    .collect(),
            ),
        ),
        ("protected", Json::Bool(facts.said.protection.is_some())),
        (
            "bookmarks",
            Json::List(
                facts
                    .bookmarks
                    .iter()
                    .map(|bookmark| {
                        Json::object([
                            ("title", Json::text(bookmark.title.clone())),
                            ("depth", Json::count(bookmark.depth)),
                            (
                                "page",
                                bookmark
                                    .page
                                    .map_or(Json::Null, |page| Json::count(page + 1)),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "fields",
            Json::List(
                facts
                    .fields
                    .iter()
                    .map(|(page, field)| {
                        Json::object([
                            ("name", Json::text(field.name.clone())),
                            ("kind", Json::text(kind(field.kind))),
                            ("page", Json::count(page + 1)),
                            ("value", Json::text(value(&field.value))),
                            ("read_only", Json::Bool(field.read_only)),
                            ("states", texts(&field.states)),
                            ("options", texts(&field.options)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "signatures",
            Json::List(
                facts
                    .signatures
                    .iter()
                    .map(|signature| Json::text(signed(signature)))
                    .collect(),
            ),
        ),
    ])
}

fn kind(kind: pdf_edit::form::FieldKind) -> &'static str {
    use pdf_edit::form::FieldKind;
    match kind {
        FieldKind::Text => "text",
        FieldKind::Checkbox => "check box",
        FieldKind::Radio => "radio button",
        FieldKind::Push => "button",
        FieldKind::Combo => "combo box",
        FieldKind::List => "list",
        FieldKind::Signature => "signature",
    }
}

fn value(value: &pdf_edit::form::FieldValue) -> String {
    match value {
        pdf_edit::form::FieldValue::Empty => String::new(),
        pdf_edit::form::FieldValue::Text(text) | pdf_edit::form::FieldValue::State(text) => {
            text.clone()
        }
    }
}

pub fn fill_field(
    desk: &mut Desk,
    handle: &str,
    name: &str,
    wanted: &Json,
) -> Result<Answer, String> {
    let (source, credential) = desk.source(handle)?;
    let command = field_to_fill(&source, &credential, name, wanted)?;
    desk.command(handle, &command)?;
    Ok(Answer {
        text: format!("{name} is filled."),
        data: Json::Null,
        picture: None,
    })
}

pub fn field_to_fill(
    source: &pdf_bytes::ByteStore,
    credential: &[u8],
    name: &str,
    wanted: &Json,
) -> Result<pdf_edit::Command, String> {
    use pdf_edit::form::{FieldKind, FieldValue};
    let fields = pdf_edit::form::fields_of_document(source, credential)
        .map_err(|error| format!("this document's form cannot be read: {error}"))?;
    let widgets: Vec<&(usize, pdf_edit::form::FormField)> = fields
        .iter()
        .filter(|(_, field)| field.name == name)
        .collect();
    let Some((page, field)) = widgets.first().copied() else {
        return Err(format!(
            "the form has no field called {name:?}: document_info lists them"
        ));
    };
    if field.read_only {
        return Err(format!("{name} is read-only"));
    }
    let (widget, page, value) = match field.kind {
        FieldKind::Text | FieldKind::Combo | FieldKind::List => {
            let text = match wanted {
                Json::Text(text) => text.clone(),
                Json::Bool(_) => return Err(format!("{name} takes text")),
                _ => return Err("`value` must be text or true/false".to_owned()),
            };
            if matches!(field.kind, FieldKind::Combo | FieldKind::List)
                && !field.options.is_empty()
                && !field.options.contains(&text)
            {
                return Err(format!("{name} takes one of: {}", field.options.join(", ")));
            }
            let value = if text.is_empty() {
                FieldValue::Empty
            } else {
                FieldValue::Text(text)
            };
            (field.widget, *page, value)
        }
        FieldKind::Checkbox | FieldKind::Radio => {
            let state = match wanted {
                Json::Bool(false) => "Off".to_owned(),
                Json::Bool(true) => field
                    .states
                    .iter()
                    .find(|state| *state != "Off")
                    .cloned()
                    .ok_or_else(|| format!("{name} has no state to turn on"))?,
                Json::Text(state) => state.clone(),
                _ => return Err("`value` must be text or true/false".to_owned()),
            };
            let chosen = if state == "Off" {
                widgets
                    .iter()
                    .find(|(_, widget)| widget.value != FieldValue::State("Off".to_owned()))
                    .or_else(|| widgets.first())
                    .ok_or_else(|| format!("{name} has no button to clear"))?
            } else {
                widgets
                    .iter()
                    .find(|(_, widget)| widget.states.contains(&state))
                    .ok_or_else(|| {
                        let states: Vec<&String> = widgets
                            .iter()
                            .flat_map(|(_, widget)| widget.states.iter())
                            .collect();
                        format!(
                            "{name} has no state {state:?}; it has: {}",
                            states
                                .iter()
                                .map(|state| state.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    })?
            };
            (chosen.1.widget, chosen.0, FieldValue::State(state))
        }
        FieldKind::Push | FieldKind::Signature => {
            return Err(format!(
                "{name} is a {} and is not filled in",
                kind(field.kind)
            ));
        }
    };
    Ok(pdf_edit::Command::FillField {
        page_index: page,
        widget,
        value,
    })
}

#[must_use]
pub fn family_for(text: &str) -> Option<String> {
    let families = pdf_cli::font_families();
    let has = |wanted: &str| families.iter().any(|family| family == wanted);
    let within = |from: u32, to: u32| text.chars().any(|c| (from..=to).contains(&u32::from(c)));
    let wanted: &[&str] = if within(0x0E80, 0x0EFF) {
        &[
            "Noto Sans Lao",
            "Phetsarath OT",
            "Saysettha OT",
            "Noto Serif Lao",
        ]
    } else if within(0x0E00, 0x0E7F) {
        &["Noto Sans Thai", "Sarabun", "Noto Serif Thai", "Loma"]
    } else if within(0x3000, 0x9FFF) || within(0xAC00, 0xD7AF) {
        &["Noto Sans CJK SC", "Noto Sans CJK JP", "Noto Sans CJK KR"]
    } else {
        &["Noto Sans", "DejaVu Sans", "Liberation Sans", "Helvetica"]
    };
    wanted
        .iter()
        .find(|family| has(family))
        .map(|family| (*family).to_owned())
        .or_else(|| families.first().cloned())
}

fn same_size(one: [f64; 2], other: [f64; 2]) -> bool {
    (one[0] - other[0]).abs() < 0.01 && (one[1] - other[1]).abs() < 0.01
}

#[cfg(test)]
mod tests;
