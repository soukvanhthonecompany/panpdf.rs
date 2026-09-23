use std::collections::BTreeSet;

use crate::connect::{Attachment, AttachmentKind, Provider, Raw, ToolCall, ToolResult, Turn};
use crate::json::Json;

const VERSION: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Chat {
    pub id: String,
    pub title: String,
    pub changed: u64,
    pub model: String,
    pub documents: Vec<String>,
    pub places: Vec<String>,
    pub turns: Vec<Turn>,
}

#[must_use]
pub fn chat_about<'a>(chats: &'a [Chat], place: &str) -> Option<&'a Chat> {
    if place.is_empty() {
        return None;
    }
    chats
        .iter()
        .find(|chat| chat.places.iter().any(|kept| kept == place))
}

#[must_use]
pub fn title_of(turns: &[Turn]) -> String {
    const MOST: usize = 60;
    let said = turns
        .iter()
        .find_map(|turn| match turn {
            Turn::Person { text, .. } if !text.trim().is_empty() => Some(text.trim()),
            _ => None,
        })
        .unwrap_or_default();
    let first = said.lines().next().unwrap_or_default().trim();
    if first.chars().count() <= MOST {
        return first.to_owned();
    }
    let clipped: String = first.chars().take(MOST).collect();
    let cut = clipped
        .rfind(char::is_whitespace)
        .filter(|at| *at > MOST / 2)
        .unwrap_or(clipped.len());
    let mut title = clipped[..cut].trim_end().to_owned();
    title.push('\u{2026}');
    title
}

#[must_use]
pub fn write(chat: &Chat) -> String {
    let value = Json::object([
        ("version", Json::count(VERSION as usize)),
        ("id", Json::text(&chat.id)),
        ("title", Json::text(&chat.title)),
        (
            "changed",
            Json::count(usize::try_from(chat.changed).unwrap_or(usize::MAX)),
        ),
        ("model", Json::text(&chat.model)),
        (
            "documents",
            Json::List(chat.documents.iter().map(Json::text).collect()),
        ),
        (
            "places",
            Json::List(chat.places.iter().map(Json::text).collect()),
        ),
        (
            "turns",
            Json::List(chat.turns.iter().map(turn_out).collect()),
        ),
    ]);
    value.write()
}

#[must_use]
pub fn read(text: &str) -> Option<Chat> {
    let value = Json::parse(text).ok()?;
    let version = value.get("version")?.as_count()?;
    if version > VERSION as usize {
        return None;
    }
    let word = |key: &str| {
        value
            .get(key)
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let turns = value
        .get("turns")?
        .as_list()?
        .iter()
        .map(turn_in)
        .collect::<Option<Vec<Turn>>>()?;
    Some(Chat {
        id: word("id"),
        title: word("title"),
        changed: value
            .get("changed")
            .and_then(Json::as_count)
            .and_then(|count| u64::try_from(count).ok())
            .unwrap_or_default(),
        model: word("model"),
        documents: value
            .get("documents")
            .and_then(Json::as_list)
            .unwrap_or_default()
            .iter()
            .filter_map(Json::as_str)
            .map(str::to_owned)
            .collect(),
        places: value
            .get("places")
            .and_then(Json::as_list)
            .unwrap_or_default()
            .iter()
            .filter_map(Json::as_str)
            .map(str::to_owned)
            .collect(),
        turns,
    })
}

fn turn_out(turn: &Turn) -> Json {
    match turn {
        Turn::Person { text, attachments } => Json::object([
            ("said", Json::text("person")),
            ("text", Json::text(text)),
            (
                "attached",
                Json::List(attachments.iter().map(attachment_out).collect()),
            ),
        ]),
        Turn::Model { text, calls, raw } => Json::object([
            ("said", Json::text("model")),
            ("text", Json::text(text)),
            ("calls", Json::List(calls.iter().map(call_out).collect())),
            ("raw", raw.as_ref().map_or(Json::Null, raw_out)),
        ]),
        Turn::Results { results } => Json::object([
            ("said", Json::text("results")),
            (
                "results",
                Json::List(results.iter().map(result_out).collect()),
            ),
        ]),
    }
}

fn turn_in(value: &Json) -> Option<Turn> {
    let text = || {
        value
            .get("text")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let list = |key: &str| value.get(key).and_then(Json::as_list).unwrap_or_default();
    match value.get("said").and_then(Json::as_str)? {
        "person" => Some(Turn::Person {
            text: text(),
            attachments: list("attached").iter().filter_map(attachment_in).collect(),
        }),
        "model" => Some(Turn::Model {
            text: text(),
            calls: list("calls")
                .iter()
                .map(call_in)
                .collect::<Option<Vec<ToolCall>>>()?,
            raw: value.get("raw").and_then(raw_in),
        }),
        "results" => Some(Turn::Results {
            results: list("results")
                .iter()
                .map(result_in)
                .collect::<Option<Vec<ToolResult>>>()?,
        }),
        _ => None,
    }
}

fn attachment_out(attachment: &Attachment) -> Json {
    let kind = match &attachment.kind {
        AttachmentKind::Text => "text".to_owned(),
        AttachmentKind::Image { media_type } => media_type.clone(),
    };
    Json::object([
        ("name", Json::text(&attachment.name)),
        ("kind", Json::text(kind)),
    ])
}

fn attachment_in(value: &Json) -> Option<Attachment> {
    let name = value.get("name")?.as_str()?.to_owned();
    let kind = value.get("kind").and_then(Json::as_str).unwrap_or("text");
    Some(Attachment {
        name,
        kind: if kind == "text" {
            AttachmentKind::Text
        } else {
            AttachmentKind::Image {
                media_type: kind.to_owned(),
            }
        },
        bytes: Vec::new(),
    })
}

fn call_out(call: &ToolCall) -> Json {
    Json::object([
        ("id", Json::text(&call.id)),
        ("name", Json::text(&call.name)),
        ("arguments", call.arguments.clone()),
    ])
}

fn call_in(value: &Json) -> Option<ToolCall> {
    Some(ToolCall {
        id: value.get("id")?.as_str()?.to_owned(),
        name: value.get("name")?.as_str()?.to_owned(),
        arguments: value.get("arguments").cloned().unwrap_or(Json::Null),
    })
}

fn result_out(result: &ToolResult) -> Json {
    Json::object([
        ("call_id", Json::text(&result.call_id)),
        ("text", Json::text(&result.text)),
        ("is_error", Json::Bool(result.is_error)),
    ])
}

fn result_in(value: &Json) -> Option<ToolResult> {
    Some(ToolResult {
        call_id: value.get("call_id")?.as_str()?.to_owned(),
        text: value
            .get("text")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_owned(),
        is_error: matches!(value.get("is_error"), Some(Json::Bool(true))),
        picture: None,
    })
}

fn raw_out(raw: &Raw) -> Json {
    Json::object([
        ("provider", Json::text(provider_name(raw.provider))),
        ("items", Json::List(raw.items.clone())),
    ])
}

fn raw_in(value: &Json) -> Option<Raw> {
    let provider = provider_of(value.get("provider")?.as_str()?)?;
    Some(Raw {
        provider,
        items: value.get("items")?.as_list()?.to_vec(),
    })
}

const fn provider_name(provider: Provider) -> &'static str {
    match provider {
        Provider::OpenAi => "openai",
        Provider::Anthropic => "anthropic",
        Provider::Gemini => "gemini",
        Provider::Ollama => "ollama",
        Provider::LmStudio => "lmstudio",
        Provider::Custom => "custom",
    }
}

fn provider_of(name: &str) -> Option<Provider> {
    Some(match name {
        "openai" => Provider::OpenAi,
        "anthropic" => Provider::Anthropic,
        "gemini" => Provider::Gemini,
        "ollama" => Provider::Ollama,
        "lmstudio" => Provider::LmStudio,
        "custom" => Provider::Custom,
        _ => return None,
    })
}

#[must_use]
pub fn newest_first(files: impl IntoIterator<Item = String>) -> Vec<Chat> {
    let mut chats: Vec<Chat> = files.into_iter().filter_map(|text| read(&text)).collect();
    chats.sort_by(|a, b| b.changed.cmp(&a.changed).then_with(|| b.id.cmp(&a.id)));
    chats
}

#[must_use]
pub fn name_for(second: u64, taken: &BTreeSet<String>) -> String {
    let mut nth = 0_u32;
    loop {
        let name = if nth == 0 {
            format!("{second:012}")
        } else {
            format!("{second:012}-{nth}")
        };
        if !taken.contains(&name) {
            return name;
        }
        nth = nth.saturating_add(1);
        if nth == u32::MAX {
            return name;
        }
    }
}

#[cfg(test)]
mod tests;
