use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::json::Json;

const MAX_RESPONSE: usize = 8 * 1024 * 1024;

const MOST_SPOKEN_BYTES: usize = 2 * 1024 * 1024;

const MOST_ATTACHED_BYTES: usize = crate::attach::MOST_TOTAL_BYTES.div_ceil(3) * 4;

const ASK_SECONDS: u64 = 300;

const LIST_SECONDS: u64 = 30;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Provider {
    OpenAi,
    Anthropic,
    Gemini,
    Ollama,
    LmStudio,
    Custom,
}

#[derive(Clone, Eq, PartialEq)]
pub struct Connection {
    pub provider: Provider,
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    pub effort: Effort,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("provider", &self.provider)
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("effort", &self.effort)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Model {
    pub id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Picture {
    pub media_type: String,
    pub base64: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AttachmentKind {
    Text,
    Image { media_type: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Attachment {
    pub name: String,
    pub kind: AttachmentKind,
    pub bytes: Vec<u8>,
}

impl Attachment {
    #[must_use]
    pub fn text(name: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: AttachmentKind::Text,
            bytes: text.into().into_bytes(),
        }
    }

    #[must_use]
    pub fn image(
        name: impl Into<String>,
        media_type: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
    ) -> Self {
        Self {
            name: name.into(),
            kind: AttachmentKind::Image {
                media_type: media_type.into(),
            },
            bytes: bytes.into(),
        }
    }

    #[must_use]
    pub fn as_text(&self) -> std::borrow::Cow<'_, str> {
        match self.kind {
            AttachmentKind::Text => String::from_utf8_lossy(&self.bytes),
            AttachmentKind::Image { .. } => std::borrow::Cow::Borrowed(""),
        }
    }

    fn size(&self) -> usize {
        match self.kind {
            AttachmentKind::Text => self.bytes.len(),
            AttachmentKind::Image { .. } => self.bytes.len().div_ceil(3) * 4,
        }
    }
}

fn attached_words(name: &str, text: &str) -> String {
    format!("Attached file \"{name}\":\n{text}")
}

fn data_url(media_type: &str, bytes: &[u8]) -> String {
    format!("data:{media_type};base64,{}", crate::json::base64(bytes))
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolOffer {
    pub name: String,
    pub description: String,
    pub schema: Json,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Json,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolResult {
    pub call_id: String,
    pub text: String,
    pub is_error: bool,
    pub picture: Option<Picture>,
}

impl ToolResult {
    #[must_use]
    pub fn said(call_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            call_id: call_id.into(),
            text: text.into(),
            is_error: false,
            picture: None,
        }
    }

    #[must_use]
    pub fn failed(call_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            call_id: call_id.into(),
            text: text.into(),
            is_error: true,
            picture: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Raw {
    pub provider: Provider,
    pub items: Vec<Json>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reply {
    pub text: String,
    pub calls: Vec<ToolCall>,
    pub raw: Option<Raw>,
    pub cut_short: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Said {
    Person,
    Model,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Turn {
    Person {
        text: String,
        attachments: Vec<Attachment>,
    },
    Model {
        text: String,
        calls: Vec<ToolCall>,
        raw: Option<Raw>,
    },
    Results {
        results: Vec<ToolResult>,
    },
}

impl Turn {
    #[must_use]
    pub fn person(text: impl Into<String>) -> Self {
        Self::Person {
            text: text.into(),
            attachments: Vec::new(),
        }
    }

    #[must_use]
    pub fn person_with(text: impl Into<String>, attachments: Vec<Attachment>) -> Self {
        Self::Person {
            text: text.into(),
            attachments,
        }
    }

    #[must_use]
    pub fn model(text: impl Into<String>) -> Self {
        Self::Model {
            text: text.into(),
            calls: Vec::new(),
            raw: None,
        }
    }

    #[must_use]
    pub fn answered(reply: &Reply) -> Self {
        Self::Model {
            text: reply.text.clone(),
            calls: reply.calls.clone(),
            raw: reply.raw.clone(),
        }
    }

    #[must_use]
    pub const fn said(&self) -> Said {
        match self {
            Self::Person { .. } | Self::Results { .. } => Said::Person,
            Self::Model { .. } => Said::Model,
        }
    }

    #[must_use]
    pub fn text(&self) -> &str {
        match self {
            Self::Person { text, .. } | Self::Model { text, .. } => text,
            Self::Results { .. } => "",
        }
    }

    #[must_use]
    pub fn attachments(&self) -> &[Attachment] {
        match self {
            Self::Person { attachments, .. } => attachments,
            _ => &[],
        }
    }

    #[must_use]
    pub fn calls(&self) -> &[ToolCall] {
        match self {
            Self::Model { calls, .. } => calls,
            _ => &[],
        }
    }

    const fn role(&self) -> &'static str {
        match self.said() {
            Said::Person => "user",
            Said::Model => "assistant",
        }
    }

    #[must_use]
    pub fn size(&self) -> usize {
        self.words_size() + self.attached_size()
    }

    fn attached_size(&self) -> usize {
        self.attachments().iter().map(Attachment::size).sum()
    }

    fn words_size(&self) -> usize {
        match self {
            Self::Person { text, .. } => text.len(),
            Self::Model { text, calls, .. } => {
                text.len() + calls.iter().map(|call| call.name.len() + 32).sum::<usize>()
            }
            Self::Results { results } => results
                .iter()
                .map(|result| {
                    result.text.len()
                        + result
                            .picture
                            .as_ref()
                            .map_or(0, |picture| picture.base64.len())
                })
                .sum(),
        }
    }
}

#[must_use]
pub fn without_the_old_pictures(turns: &[Turn]) -> Vec<Turn> {
    let newest = turns
        .iter()
        .enumerate()
        .filter_map(|(at, turn)| match turn {
            Turn::Results { results } => results
                .iter()
                .rposition(|result| result.picture.is_some())
                .map(|which| (at, which)),
            _ => None,
        })
        .next_back();
    let mut out = turns.to_vec();
    for (at, turn) in out.iter_mut().enumerate() {
        let Turn::Results { results } = turn else {
            continue;
        };
        for (which, result) in results.iter_mut().enumerate() {
            if newest != Some((at, which)) {
                result.picture = None;
            }
        }
    }
    out
}

const UNANSWERED: &str = "This action was not carried out, and the conversation went on without it. Do not retry it; \
ask the person what they would like instead.";

pub fn settle_dangling_calls(turns: &mut Vec<Turn>) {
    let mut at = 0;
    while at < turns.len() {
        let missing: Vec<ToolResult> = match &turns[at] {
            Turn::Model { calls, .. } => {
                let answered: Vec<&str> = match turns.get(at + 1) {
                    Some(Turn::Results { results }) => {
                        results.iter().map(|it| it.call_id.as_str()).collect()
                    }
                    _ => Vec::new(),
                };
                calls
                    .iter()
                    .filter(|call| !answered.contains(&call.id.as_str()))
                    .map(|call| ToolResult::failed(call.id.clone(), UNANSWERED))
                    .collect()
            }
            _ => Vec::new(),
        };
        if !missing.is_empty() {
            match turns.get_mut(at + 1) {
                Some(Turn::Results { results }) => results.extend(missing),
                _ => turns.insert(at + 1, Turn::Results { results: missing }),
            }
        }
        at += 1;
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Effort {
    #[default]
    Off,
    None,
    Low,
    Medium,
    High,
}

impl Effort {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::None => "none",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "off" => Some(Self::Off),
            "none" => Some(Self::None),
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }

    const fn budget_tokens(self) -> u32 {
        match self {
            Self::Off | Self::None => 0,
            Self::Low => 1024,
            Self::Medium => 4096,
            Self::High => 16000,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThinkingStyle {
    Budget,
    Adaptive,
}

#[must_use]
pub fn anthropic_thinking_style(model: &str) -> ThinkingStyle {
    if model.starts_with("claude-3-")
        || model.contains("-4-5")
        || model.contains("-4-1")
        || model.contains("-4-0")
    {
        ThinkingStyle::Budget
    } else {
        ThinkingStyle::Adaptive
    }
}

#[must_use]
pub fn reasoning_members(
    provider: Provider,
    model: &str,
    effort: Effort,
) -> (Vec<(&'static str, Json)>, u32) {
    const PLAIN_MAX_TOKENS: u32 = 4096;
    const ADAPTIVE_MAX_TOKENS: u32 = 16000;
    if effort == Effort::Off {
        return (Vec::new(), PLAIN_MAX_TOKENS);
    }
    if effort == Effort::None {
        return match provider.wire() {
            Wire::Chat => (
                vec![("reasoning_effort", Json::text("none"))],
                PLAIN_MAX_TOKENS,
            ),
            Wire::Messages => (
                vec![("thinking", Json::object([("type", Json::text("disabled"))]))],
                PLAIN_MAX_TOKENS,
            ),
            Wire::Responses => (
                vec![(
                    "reasoning",
                    Json::object([("effort", Json::text("minimal"))]),
                )],
                PLAIN_MAX_TOKENS,
            ),
        };
    }
    let word = Json::text(effort.as_str());
    match provider.wire() {
        Wire::Responses => (
            vec![("reasoning", Json::object([("effort", word)]))],
            PLAIN_MAX_TOKENS,
        ),
        Wire::Messages => match anthropic_thinking_style(model) {
            ThinkingStyle::Budget => {
                let budget = effort.budget_tokens();
                (
                    vec![(
                        "thinking",
                        Json::object([
                            ("type", Json::text("enabled")),
                            ("budget_tokens", Json::Number(f64::from(budget))),
                        ]),
                    )],
                    budget + PLAIN_MAX_TOKENS,
                )
            }
            ThinkingStyle::Adaptive => (
                vec![
                    ("thinking", Json::object([("type", Json::text("adaptive"))])),
                    ("output_config", Json::object([("effort", word)])),
                ],
                ADAPTIVE_MAX_TOKENS,
            ),
        },
        Wire::Chat => (vec![("reasoning_effort", word)], PLAIN_MAX_TOKENS),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConnectError {
    Invalid(String),
    Cancelled,
    Curl(String),
    ResponseTooLarge,
    Http(String),
    Protocol(String),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(s) => write!(f, "invalid AI connection: {s}"),
            Self::Cancelled => f.write_str("AI request cancelled"),
            Self::Curl(s) => write!(f, "curl could not make the AI request: {s}"),
            Self::ResponseTooLarge => f.write_str("the AI response is too large"),
            Self::Http(s) => write!(f, "the AI service refused the request: {s}"),
            Self::Protocol(s) => write!(f, "the AI service returned an unexpected answer: {s}"),
        }
    }
}

impl std::error::Error for ConnectError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wire {
    Responses,
    Messages,
    Chat,
}

impl Provider {
    #[must_use]
    pub const fn wire(self) -> Wire {
        match self {
            Self::OpenAi => Wire::Responses,
            Self::Anthropic => Wire::Messages,
            _ => Wire::Chat,
        }
    }

    #[must_use]
    pub const fn default_base_url(self) -> &'static str {
        match self {
            Self::OpenAi => "https://api.openai.com/v1",
            Self::Anthropic => "https://api.anthropic.com/v1",
            Self::Gemini => "https://generativelanguage.googleapis.com/v1beta/openai",
            Self::Ollama => "http://127.0.0.1:11434/v1",
            Self::LmStudio => "http://127.0.0.1:1234/v1",
            Self::Custom => "",
        }
    }
}

impl Connection {
    #[must_use]
    pub fn preset(provider: Provider, model: impl Into<String>) -> Self {
        Self {
            provider,
            base_url: provider.default_base_url().to_owned(),
            model: model.into(),
            api_key: String::new(),
            effort: Effort::Off,
        }
    }

    pub fn models(&self, cancel: &AtomicBool) -> Result<Vec<Model>, ConnectError> {
        let body = self.request("GET", "/models", None, (LIST_SECONDS, cancel))?;
        let root = Json::parse(&body).map_err(|e| ConnectError::Protocol(e.to_string()))?;
        let items = root
            .get("data")
            .and_then(Json::as_list)
            .ok_or_else(|| ConnectError::Protocol("models answer has no data list".to_owned()))?;
        Ok(items
            .iter()
            .filter_map(|item| item.get("id").and_then(Json::as_str))
            .map(str::to_owned)
            .map(|id| Model { id })
            .collect())
    }

    pub fn test_connection(&self, cancel: &AtomicBool) -> Result<(), ConnectError> {
        self.models(cancel).map(|_| ())
    }

    pub fn chat(
        &self,
        prompt: &str,
        context: Option<&str>,
        cancel: &AtomicBool,
    ) -> Result<Reply, ConnectError> {
        self.converse(&[Turn::person(prompt)], context, cancel)
    }

    pub fn converse(
        &self,
        turns: &[Turn],
        context: Option<&str>,
        cancel: &AtomicBool,
    ) -> Result<Reply, ConnectError> {
        self.converse_with(turns, context, &[], None, cancel)
    }

    pub fn converse_with(
        &self,
        turns: &[Turn],
        context: Option<&str>,
        tools: &[ToolOffer],
        system: Option<&str>,
        cancel: &AtomicBool,
    ) -> Result<Reply, ConnectError> {
        if cancel.load(Ordering::Relaxed) {
            return Err(ConnectError::Cancelled);
        }
        let spoken = self.spoken(turns, context)?;
        let payload = self.payload(&spoken, tools, system);
        let endpoint = match self.provider.wire() {
            Wire::Responses => "/responses",
            Wire::Messages => "/messages",
            Wire::Chat => "/chat/completions",
        };
        let body = self.request(
            "POST",
            endpoint,
            Some(&payload.write()),
            (ASK_SECONDS, cancel),
        )?;
        let root = Json::parse(&body).map_err(|e| ConnectError::Protocol(e.to_string()))?;
        answer(self.provider, &root)
    }

    pub fn converse_streaming(
        &self,
        turns: &[Turn],
        context: Option<&str>,
        tools: &[ToolOffer],
        system: Option<&str>,
        cancel: &AtomicBool,
        on_partial: &mut dyn FnMut(Progress<'_>),
    ) -> Result<Reply, ConnectError> {
        if cancel.load(Ordering::Relaxed) {
            return Err(ConnectError::Cancelled);
        }
        let spoken = self.spoken(turns, context)?;
        let mut payload = self.payload(&spoken, tools, system);
        put(&mut payload, "stream", Json::Bool(true));
        let endpoint = match self.provider.wire() {
            Wire::Responses => "/responses",
            Wire::Messages => "/messages",
            Wire::Chat => "/chat/completions",
        };
        let mut gathering = Gathering::new(self.provider.wire());
        let body = self.request_streaming(
            endpoint,
            &payload.write(),
            (ASK_SECONDS, cancel),
            &mut |line| {
                if gathering.line(line) {
                    on_partial(Progress {
                        said: gathering.said(),
                        thinking: gathering.thinking(),
                    });
                }
            },
        )?;
        if let Some(whole) = gathering.whole() {
            return answer(self.provider, &whole);
        }
        let root = Json::parse(&body).map_err(|e| ConnectError::Protocol(e.to_string()))?;
        answer(self.provider, &root)
    }

    fn spoken(&self, turns: &[Turn], context: Option<&str>) -> Result<Vec<Turn>, ConnectError> {
        if self.model.is_empty() {
            return Err(ConnectError::Invalid("the model is empty".to_owned()));
        }
        let last = turns
            .last()
            .ok_or_else(|| ConnectError::Invalid("there is nothing to ask".to_owned()))?;
        let asked = match last {
            Turn::Person { text, attachments } => !text.is_empty() || !attachments.is_empty(),
            Turn::Results { results } => !results.is_empty(),
            Turn::Model { .. } => false,
        };
        if !asked {
            return Err(ConnectError::Invalid("the prompt is empty".to_owned()));
        }
        let mut spoken = turns.to_vec();
        if let Some(context) = context.filter(|text| !text.is_empty())
            && let Some(Turn::Person { text, .. }) = spoken
                .iter_mut()
                .find(|turn| matches!(turn, Turn::Person { .. }))
        {
            *text = format!("Document context:\n{context}\n\nUser request:\n{text}");
        }
        let words: usize = spoken.iter().map(Turn::words_size).sum();
        if words > MOST_SPOKEN_BYTES {
            return Err(ConnectError::Invalid(
                "prompt and context are too large".to_owned(),
            ));
        }
        let attached: usize = spoken.iter().map(Turn::attached_size).sum();
        if attached > MOST_ATTACHED_BYTES {
            return Err(ConnectError::Invalid(
                "the attached files are too large".to_owned(),
            ));
        }
        Ok(spoken)
    }

    fn payload(&self, spoken: &[Turn], tools: &[ToolOffer], system: Option<&str>) -> Json {
        let (thinking, max_tokens) = reasoning_members(self.provider, &self.model, self.effort);
        let mut body = match self.provider.wire() {
            Wire::Messages => self.messages_body(spoken, tools, system, max_tokens),
            Wire::Responses => self.responses_body(spoken, tools, system),
            Wire::Chat => self.chat_body(spoken, tools, system),
        };
        for (key, value) in thinking {
            put(&mut body, key, value);
        }
        body
    }

    fn messages_body(
        &self,
        spoken: &[Turn],
        tools: &[ToolOffer],
        system: Option<&str>,
        max_tokens: u32,
    ) -> Json {
        let messages = spoken
            .iter()
            .map(|turn| match turn {
                Turn::Person { text, attachments } => Json::object([
                    ("role", Json::text(turn.role())),
                    ("content", messages_person_content(text, attachments)),
                ]),
                Turn::Model { text, calls, raw } => Json::object([
                    ("role", Json::text("assistant")),
                    ("content", self.assistant_content(text, calls, raw.as_ref())),
                ]),
                Turn::Results { results } => Json::object([
                    ("role", Json::text("user")),
                    (
                        "content",
                        Json::List(results.iter().map(tool_result_block).collect()),
                    ),
                ]),
            })
            .collect();
        let mut body = Json::object([
            ("model", Json::text(self.model.clone())),
            ("max_tokens", Json::Number(f64::from(max_tokens))),
            ("messages", Json::List(messages)),
        ]);
        if let Some(system) = system {
            put(&mut body, "system", Json::text(system));
        }
        if !tools.is_empty() {
            put(
                &mut body,
                "tools",
                Json::List(
                    tools
                        .iter()
                        .map(|tool| {
                            Json::object([
                                ("name", Json::text(tool.name.clone())),
                                ("description", Json::text(tool.description.clone())),
                                ("input_schema", tool.schema.clone()),
                            ])
                        })
                        .collect(),
                ),
            );
        }
        body
    }

    fn assistant_content(&self, text: &str, calls: &[ToolCall], raw: Option<&Raw>) -> Json {
        if let Some(raw) = raw.filter(|raw| raw.provider == self.provider) {
            return Json::List(raw.items.clone());
        }
        if calls.is_empty() {
            return Json::text(text.to_owned());
        }
        let mut blocks = Vec::new();
        if !text.is_empty() {
            blocks.push(Json::object([
                ("type", Json::text("text")),
                ("text", Json::text(text.to_owned())),
            ]));
        }
        for call in calls {
            blocks.push(Json::object([
                ("type", Json::text("tool_use")),
                ("id", Json::text(call.id.clone())),
                ("name", Json::text(call.name.clone())),
                ("input", call.arguments.clone()),
            ]));
        }
        Json::List(blocks)
    }

    fn responses_body(&self, spoken: &[Turn], tools: &[ToolOffer], system: Option<&str>) -> Json {
        let mut input = Vec::new();
        for turn in spoken {
            match turn {
                Turn::Person { text, attachments } => input.push(Json::object([
                    ("role", Json::text(turn.role())),
                    ("content", responses_person_content(text, attachments)),
                ])),
                Turn::Model { text, calls, raw } => {
                    if let Some(raw) = raw.as_ref().filter(|raw| raw.provider == self.provider) {
                        input.extend(raw.items.iter().cloned());
                        continue;
                    }
                    if !text.is_empty() || calls.is_empty() {
                        input.push(Json::object([
                            ("role", Json::text("assistant")),
                            (
                                "content",
                                Json::List(vec![Json::object([
                                    ("type", Json::text("output_text")),
                                    ("text", Json::text(text.clone())),
                                ])]),
                            ),
                        ]));
                    }
                    for call in calls {
                        input.push(Json::object([
                            ("type", Json::text("function_call")),
                            ("call_id", Json::text(call.id.clone())),
                            ("name", Json::text(call.name.clone())),
                            ("arguments", Json::text(call.arguments.write())),
                        ]));
                    }
                }
                Turn::Results { results } => {
                    for result in results {
                        input.push(Json::object([
                            ("type", Json::text("function_call_output")),
                            ("call_id", Json::text(result.call_id.clone())),
                            ("output", Json::text(result.text.clone())),
                        ]));
                    }
                }
            }
        }
        let mut body = Json::object([
            ("model", Json::text(self.model.clone())),
            ("store", Json::Bool(false)),
            ("input", Json::List(input)),
        ]);
        if let Some(system) = system {
            put(&mut body, "instructions", Json::text(system));
        }
        if !tools.is_empty() {
            put(
                &mut body,
                "tools",
                Json::List(
                    tools
                        .iter()
                        .map(|tool| {
                            Json::object([
                                ("type", Json::text("function")),
                                ("name", Json::text(tool.name.clone())),
                                ("description", Json::text(tool.description.clone())),
                                ("parameters", tool.schema.clone()),
                                ("strict", Json::Bool(false)),
                            ])
                        })
                        .collect(),
                ),
            );
            put(&mut body, "tool_choice", Json::text("auto"));
            put(
                &mut body,
                "include",
                Json::List(vec![Json::text("reasoning.encrypted_content")]),
            );
        }
        body
    }

    fn chat_body(&self, spoken: &[Turn], tools: &[ToolOffer], system: Option<&str>) -> Json {
        let mut messages = Vec::new();
        if let Some(system) = system {
            messages.push(Json::object([
                ("role", Json::text("system")),
                ("content", Json::text(system)),
            ]));
        }
        for turn in spoken {
            match turn {
                Turn::Person { text, attachments } => messages.push(Json::object([
                    ("role", Json::text(turn.role())),
                    ("content", chat_person_content(text, attachments)),
                ])),
                Turn::Model { text, calls, raw } => {
                    if let Some(raw) = raw.as_ref().filter(|raw| raw.provider == self.provider) {
                        messages.extend(raw.items.iter().cloned());
                    } else if calls.is_empty() {
                        messages.push(Json::object([
                            ("role", Json::text("assistant")),
                            ("content", Json::text(text.clone())),
                        ]));
                    } else {
                        messages.push(Json::object([
                            ("role", Json::text("assistant")),
                            ("content", Json::text(text.clone())),
                            (
                                "tool_calls",
                                Json::List(
                                    calls
                                        .iter()
                                        .map(|call| {
                                            Json::object([
                                                ("id", Json::text(call.id.clone())),
                                                ("type", Json::text("function")),
                                                (
                                                    "function",
                                                    Json::object([
                                                        ("name", Json::text(call.name.clone())),
                                                        (
                                                            "arguments",
                                                            Json::text(call.arguments.write()),
                                                        ),
                                                    ]),
                                                ),
                                            ])
                                        })
                                        .collect(),
                                ),
                            ),
                        ]));
                    }
                }
                Turn::Results { results } => {
                    for result in results {
                        messages.push(Json::object([
                            ("role", Json::text("tool")),
                            ("tool_call_id", Json::text(result.call_id.clone())),
                            ("content", Json::text(result.text.clone())),
                        ]));
                    }
                }
            }
        }
        let mut body = Json::object([
            ("model", Json::text(self.model.clone())),
            ("messages", Json::List(messages)),
        ]);
        if !tools.is_empty() {
            put(
                &mut body,
                "tools",
                Json::List(
                    tools
                        .iter()
                        .map(|tool| {
                            Json::object([
                                ("type", Json::text("function")),
                                (
                                    "function",
                                    Json::object([
                                        ("name", Json::text(tool.name.clone())),
                                        ("description", Json::text(tool.description.clone())),
                                        ("parameters", tool.schema.clone()),
                                    ]),
                                ),
                            ])
                        })
                        .collect(),
                ),
            );
        }
        body
    }

    fn request(
        &self,
        method: &str,
        suffix: &str,
        payload: Option<&str>,
        (seconds, cancel): (u64, &AtomicBool),
    ) -> Result<String, ConnectError> {
        let config = self.curl_config(method, suffix, payload, seconds, false)?;
        again_after_a_wait(cancel, || run_curl(&config, (seconds, cancel)))
            .map_err(|error| redact_error(error, &self.api_key))
    }

    fn request_streaming(
        &self,
        suffix: &str,
        payload: &str,
        (seconds, cancel): (u64, &AtomicBool),
        on_line: &mut dyn FnMut(&str),
    ) -> Result<String, ConnectError> {
        let config = self.curl_config("POST", suffix, Some(payload), seconds, true)?;
        again_after_a_wait(cancel, || {
            run_curl_streaming(&config, (seconds, cancel), on_line)
        })
        .map_err(|error| redact_error(error, &self.api_key))
    }

    fn curl_config(
        &self,
        method: &str,
        suffix: &str,
        payload: Option<&str>,
        seconds: u64,
        as_it_arrives: bool,
    ) -> Result<String, ConnectError> {
        let url = endpoint(&self.base_url, suffix, self.provider)?;
        let mut headers = vec!["Content-Type: application/json".to_owned()];
        if !self.api_key.is_empty() {
            if self.api_key.contains(['\r', '\n']) {
                return Err(ConnectError::Invalid(
                    "API key contains a line break".to_owned(),
                ));
            }
            if self.provider == Provider::Anthropic {
                headers.push(format!("x-api-key: {}", self.api_key));
            } else {
                headers.push(format!("Authorization: Bearer {}", self.api_key));
            }
        } else if matches!(
            self.provider,
            Provider::OpenAi | Provider::Anthropic | Provider::Gemini
        ) {
            return Err(ConnectError::Invalid(
                "this provider requires an API key".to_owned(),
            ));
        }
        if self.provider == Provider::Anthropic {
            headers.push("anthropic-version: 2023-06-01".to_owned());
        }
        let mut config = String::new();
        config.push_str("url = \"");
        config.push_str(&curl_escape(&url));
        config.push_str("\"\nrequest = \"");
        config.push_str(method);
        config.push_str("\"\nsilent\nshow-error\nfail-with-body\ngloboff\nproto = \"https,http\"\nconnect-timeout = 10\nnoproxy = \"*\"\n");
        config.push_str("max-time = ");
        config.push_str(&seconds.to_string());
        config.push('\n');
        if as_it_arrives {
            config.push_str("no-buffer\n");
        }
        for header in headers {
            config.push_str("header = \"");
            config.push_str(&curl_escape(&header));
            config.push_str("\"\n");
        }
        if let Some(payload) = payload {
            config.push_str("data = \"");
            config.push_str(&curl_escape(payload));
            config.push_str("\"\n");
        }
        Ok(config)
    }
}

fn endpoint(base: &str, suffix: &str, provider: Provider) -> Result<String, ConnectError> {
    if base.is_empty() || base.contains(['\r', '\n', '\t', ' ', '\\', '?', '#']) {
        return Err(ConnectError::Invalid(
            "endpoint is empty or contains whitespace".to_owned(),
        ));
    }
    let local_http = base
        .strip_prefix("http://")
        .and_then(|rest| rest.split('/').next())
        .is_some_and(loopback_authority);
    let secure = base.starts_with("https://");
    let plain_and_local = local_http && base.starts_with("http://");
    if !(secure || plain_and_local) {
        return Err(ConnectError::Invalid(
            "HTTPS is required; HTTP is allowed only for loopback providers".to_owned(),
        ));
    }
    let base = base.trim_end_matches('/');
    let _ = provider;
    Ok(format!("{base}{suffix}"))
}

fn loopback_authority(authority: &str) -> bool {
    if authority.contains('@') {
        return false;
    }
    if let Some(rest) = authority.strip_prefix("[::1]") {
        return rest.is_empty() || valid_port(rest);
    }
    let (host, port) = authority
        .split_once(':')
        .map_or((authority, None), |(host, port)| (host, Some(port)));
    matches!(host, "127.0.0.1" | "localhost")
        && port.is_none_or(|digits| {
            !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn valid_port(port: &str) -> bool {
    port.strip_prefix(':').is_some_and(|digits| {
        !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn curl_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\r', "\\r")
        .replace('\n', "\\n")
}

const MOST_WAIT: Duration = Duration::from_mins(1);

const TRIES: usize = 3;

fn wait_asked_for(body: &str) -> Option<Duration> {
    let asking = body.contains("rate limit")
        || body.contains("Rate limit")
        || body.contains("RESOURCE_EXHAUSTED")
        || body.contains("rate_limit")
        || body.contains("quota")
        || body.contains("Quota");
    if !asking {
        return None;
    }
    let mut rest = body;
    while let Some(at) = rest.find(" in ") {
        rest = &rest[at + 4..];
        let digits: String = rest
            .chars()
            .take_while(|letter| letter.is_ascii_digit() || *letter == '.')
            .collect();
        if digits.is_empty() {
            continue;
        }
        let after = rest[digits.len()..].trim_start();
        if !after.starts_with('s') {
            continue;
        }
        if let Ok(seconds) = digits.parse::<f64>()
            && seconds.is_finite()
            && seconds > 0.0
        {
            return Some(Duration::from_secs_f64(seconds + 1.0));
        }
    }
    None
}

fn wait_watching(how_long: Duration, cancel: &AtomicBool) -> bool {
    let until = Instant::now() + how_long;
    while Instant::now() < until {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        thread::sleep(Duration::from_millis(100).min(until - Instant::now()));
    }
    !cancel.load(Ordering::Relaxed)
}

fn again_after_a_wait<F>(cancel: &AtomicBool, make: F) -> Result<String, ConnectError>
where
    F: FnMut() -> Result<String, ConnectError>,
{
    again_after(cancel, make, OVERLOADED_WAITS)
}

const OVERLOADED_WAITS: [Duration; TRIES] = [
    Duration::from_secs(3),
    Duration::from_secs(8),
    Duration::from_secs(15),
];

fn again_after<F>(
    cancel: &AtomicBool,
    mut make: F,
    overloaded: [Duration; TRIES],
) -> Result<String, ConnectError>
where
    F: FnMut() -> Result<String, ConnectError>,
{
    let mut tried = 0;
    loop {
        let answer = make();
        let Err(ConnectError::Http(said)) = &answer else {
            return answer;
        };
        let wait = wait_asked_for(said)
            .filter(|wait| *wait <= MOST_WAIT)
            .or_else(|| overloaded_for_now(said).then(|| overloaded[tried.min(TRIES - 1)]));
        tried += 1;
        let Some(wait) = wait else {
            return answer;
        };
        if tried > TRIES || !wait_watching(wait, cancel) {
            return answer;
        }
    }
}

fn overloaded_for_now(said: &str) -> bool {
    let said = said.to_ascii_lowercase();
    if said.contains("quota") {
        return false;
    }
    [
        "high demand",
        "overloaded",
        "unavailable",
        "try again later",
        "\"code\": 503",
        "\"code\": 529",
    ]
    .iter()
    .any(|sign| said.contains(sign))
}

fn run_curl(config: &str, (seconds, cancel): (u64, &AtomicBool)) -> Result<String, ConnectError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(ConnectError::Cancelled);
    }
    let mut child = Command::new("curl")
        .arg("-q")
        .arg("--config")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| ConnectError::Curl(e.to_string()))?;
    child
        .stdin
        .take()
        .ok_or_else(|| ConnectError::Curl("curl stdin was unavailable".to_owned()))?
        .write_all(config.as_bytes())
        .map_err(|e| ConnectError::Curl(e.to_string()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ConnectError::Curl("curl stdout was unavailable".to_owned()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ConnectError::Curl("curl stderr was unavailable".to_owned()))?;
    let reader = thread::spawn(|| read_limited(stdout));
    let error_reader = thread::spawn(|| read_limited(stderr));
    let started = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            let _ = error_reader.join();
            return Err(ConnectError::Cancelled);
        }
        if child
            .try_wait()
            .map_err(|e| ConnectError::Curl(e.to_string()))?
            .is_some()
        {
            break;
        }
        if started.elapsed() > Duration::from_secs(seconds + 2) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            let _ = error_reader.join();
            return Err(ConnectError::Curl("request timed out".to_owned()));
        }
        thread::sleep(Duration::from_millis(20));
    }
    let status = child
        .wait()
        .map_err(|e| ConnectError::Curl(e.to_string()))?;
    let (body, too_large) = reader
        .join()
        .map_err(|_| ConnectError::Curl("curl reader stopped".to_owned()))?;
    let (complaint, _) = error_reader
        .join()
        .map_err(|_| ConnectError::Curl("curl error reader stopped".to_owned()))?;
    if too_large {
        return Err(ConnectError::ResponseTooLarge);
    }
    let body = String::from_utf8(body)
        .map_err(|_| ConnectError::Protocol("answer was not UTF-8".to_owned()))?;
    if !status.success() {
        if status.code() == Some(22) {
            return Err(ConnectError::Http(error_text(&body)));
        }
        let said = String::from_utf8_lossy(&complaint);
        let said = said.trim();
        return Err(ConnectError::Curl(if said.is_empty() {
            match status.code() {
                Some(code) => format!("curl stopped with code {code}"),
                None => "curl was stopped".to_owned(),
            }
        } else {
            said.to_owned()
        }));
    }
    Ok(body)
}

fn run_curl_streaming(
    config: &str,
    (seconds, cancel): (u64, &AtomicBool),
    on_line: &mut dyn FnMut(&str),
) -> Result<String, ConnectError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(ConnectError::Cancelled);
    }
    let mut child = Command::new("curl")
        .arg("-q")
        .arg("--config")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| ConnectError::Curl(e.to_string()))?;
    child
        .stdin
        .take()
        .ok_or_else(|| ConnectError::Curl("curl stdin was unavailable".to_owned()))?
        .write_all(config.as_bytes())
        .map_err(|e| ConnectError::Curl(e.to_string()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ConnectError::Curl("curl stdout was unavailable".to_owned()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ConnectError::Curl("curl stderr was unavailable".to_owned()))?;
    let (sender, lines) = std::sync::mpsc::channel();
    let reader = thread::spawn(move || read_by_line(stdout, &sender));
    let error_reader = thread::spawn(|| read_limited(stderr));
    let mut body = String::new();
    let mut trouble: Option<ConnectError> = None;
    let started = Instant::now();
    let stop = |child: &mut std::process::Child| {
        let _ = child.kill();
        let _ = child.wait();
    };
    loop {
        while let Ok(line) = lines.try_recv() {
            take_line(line, &mut body, &mut trouble, on_line);
        }
        if let Some(why) = trouble {
            stop(&mut child);
            let _ = reader.join();
            let _ = error_reader.join();
            return Err(why);
        }
        if cancel.load(Ordering::Relaxed) {
            stop(&mut child);
            let _ = reader.join();
            let _ = error_reader.join();
            return Err(ConnectError::Cancelled);
        }
        if child
            .try_wait()
            .map_err(|e| ConnectError::Curl(e.to_string()))?
            .is_some()
        {
            break;
        }
        if started.elapsed() > Duration::from_secs(seconds + 2) {
            stop(&mut child);
            let _ = reader.join();
            let _ = error_reader.join();
            return Err(ConnectError::Curl("request timed out".to_owned()));
        }
        thread::sleep(Duration::from_millis(20));
    }
    let status = child
        .wait()
        .map_err(|e| ConnectError::Curl(e.to_string()))?;
    reader
        .join()
        .map_err(|_| ConnectError::Curl("curl reader stopped".to_owned()))?;
    while let Ok(line) = lines.try_recv() {
        take_line(line, &mut body, &mut trouble, on_line);
    }
    if let Some(why) = trouble {
        let _ = error_reader.join();
        return Err(why);
    }
    let (complaint, _) = error_reader
        .join()
        .map_err(|_| ConnectError::Curl("curl error reader stopped".to_owned()))?;
    if !status.success() {
        if status.code() == Some(22) {
            return Err(ConnectError::Http(error_text(&body)));
        }
        let said = String::from_utf8_lossy(&complaint);
        let said = said.trim();
        return Err(ConnectError::Curl(if said.is_empty() {
            match status.code() {
                Some(code) => format!("curl stopped with code {code}"),
                None => "curl was stopped".to_owned(),
            }
        } else {
            said.to_owned()
        }));
    }
    Ok(body)
}

fn take_line(
    line: Vec<u8>,
    body: &mut String,
    trouble: &mut Option<ConnectError>,
    on_line: &mut dyn FnMut(&str),
) {
    if trouble.is_some() {
        return;
    }
    let Ok(text) = String::from_utf8(line) else {
        *trouble = Some(ConnectError::Protocol("answer was not UTF-8".to_owned()));
        return;
    };
    if body.len() + text.len() > MAX_RESPONSE {
        *trouble = Some(ConnectError::ResponseTooLarge);
        return;
    }
    body.push_str(&text);
    on_line(text.trim_end_matches(['\n', '\r']));
}

fn read_by_line(mut reader: impl Read, lines: &std::sync::mpsc::Sender<Vec<u8>>) {
    let mut held: Vec<u8> = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(count) => {
                held.extend_from_slice(&chunk[..count]);
                while let Some(at) = held.iter().position(|byte| *byte == b'\n') {
                    let line: Vec<u8> = held.drain(..=at).collect();
                    if lines.send(line).is_err() {
                        return;
                    }
                }
                if held.len() > MAX_RESPONSE {
                    let _ = lines.send(std::mem::take(&mut held));
                    return;
                }
            }
        }
    }
    if !held.is_empty() {
        let _ = lines.send(held);
    }
}

fn read_limited(mut reader: impl Read) -> (Vec<u8>, bool) {
    let mut result = Vec::new();
    let mut chunk = [0_u8; 8192];
    let mut too_large = false;
    loop {
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(count) => {
                if result.len() < MAX_RESPONSE {
                    let keep = count.min(MAX_RESPONSE - result.len());
                    result.extend_from_slice(&chunk[..keep]);
                    too_large |= keep != count;
                } else {
                    too_large = true;
                }
            }
        }
    }
    (result, too_large)
}

fn error_text(body: &str) -> String {
    Json::parse(body)
        .ok()
        .map(|json| match json {
            Json::List(mut list) if list.len() == 1 => list.remove(0),
            other => other,
        })
        .and_then(|json| {
            json.get("error")
                .and_then(|error| error.get("message"))
                .and_then(Json::as_str)
                .or_else(|| json.get("message").and_then(Json::as_str))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| body.chars().take(512).collect())
}

fn redact_error(error: ConnectError, secret: &str) -> ConnectError {
    if secret.is_empty() {
        return error;
    }
    let replace = |text: String| text.replace(secret, "<redacted>");
    match error {
        ConnectError::Curl(text) => ConnectError::Curl(replace(text)),
        ConnectError::Http(text) => ConnectError::Http(replace(text)),
        ConnectError::Protocol(text) => ConnectError::Protocol(replace(text)),
        other => other,
    }
}

fn put(body: &mut Json, key: &str, value: Json) {
    if let Json::Object(members) = body {
        members.insert(key.to_owned(), value);
    }
}

fn messages_person_content(text: &str, attachments: &[Attachment]) -> Json {
    if attachments.is_empty() {
        return Json::text(text.to_owned());
    }
    let mut blocks = vec![Json::object([
        ("type", Json::text("text")),
        ("text", Json::text(text.to_owned())),
    ])];
    for attachment in attachments {
        blocks.push(match &attachment.kind {
            AttachmentKind::Text => Json::object([
                ("type", Json::text("text")),
                (
                    "text",
                    Json::text(attached_words(&attachment.name, &attachment.as_text())),
                ),
            ]),
            AttachmentKind::Image { media_type } => Json::object([
                ("type", Json::text("image")),
                (
                    "source",
                    Json::object([
                        ("type", Json::text("base64")),
                        ("media_type", Json::text(media_type.clone())),
                        ("data", Json::text(crate::json::base64(&attachment.bytes))),
                    ]),
                ),
            ]),
        });
    }
    Json::List(blocks)
}

fn responses_person_content(text: &str, attachments: &[Attachment]) -> Json {
    let mut parts = vec![Json::object([
        ("type", Json::text("input_text")),
        ("text", Json::text(text.to_owned())),
    ])];
    for attachment in attachments {
        parts.push(match &attachment.kind {
            AttachmentKind::Text => Json::object([
                ("type", Json::text("input_text")),
                (
                    "text",
                    Json::text(attached_words(&attachment.name, &attachment.as_text())),
                ),
            ]),
            AttachmentKind::Image { media_type } => Json::object([
                ("type", Json::text("input_image")),
                (
                    "image_url",
                    Json::text(data_url(media_type, &attachment.bytes)),
                ),
            ]),
        });
    }
    Json::List(parts)
}

fn chat_person_content(text: &str, attachments: &[Attachment]) -> Json {
    if attachments.is_empty() {
        return Json::text(text.to_owned());
    }
    let mut parts = vec![Json::object([
        ("type", Json::text("text")),
        ("text", Json::text(text.to_owned())),
    ])];
    for attachment in attachments {
        parts.push(match &attachment.kind {
            AttachmentKind::Text => Json::object([
                ("type", Json::text("text")),
                (
                    "text",
                    Json::text(attached_words(&attachment.name, &attachment.as_text())),
                ),
            ]),
            AttachmentKind::Image { media_type } => Json::object([
                ("type", Json::text("image_url")),
                (
                    "image_url",
                    Json::object([("url", Json::text(data_url(media_type, &attachment.bytes)))]),
                ),
            ]),
        });
    }
    Json::List(parts)
}

fn tool_result_block(result: &ToolResult) -> Json {
    let content = match &result.picture {
        None => Json::text(result.text.clone()),
        Some(picture) => Json::List(vec![
            Json::object([
                ("type", Json::text("text")),
                ("text", Json::text(result.text.clone())),
            ]),
            Json::object([
                ("type", Json::text("image")),
                (
                    "source",
                    Json::object([
                        ("type", Json::text("base64")),
                        ("media_type", Json::text(picture.media_type.clone())),
                        ("data", Json::text(picture.base64.clone())),
                    ]),
                ),
            ]),
        ]),
    };
    Json::object([
        ("type", Json::text("tool_result")),
        ("tool_use_id", Json::text(result.call_id.clone())),
        ("content", content),
        ("is_error", Json::Bool(result.is_error)),
    ])
}

fn answer(provider: Provider, root: &Json) -> Result<Reply, ConnectError> {
    let reply = read_reply(provider, root)?;
    if reply.text.is_empty() && reply.calls.is_empty() {
        return Err(ConnectError::Protocol(
            "answer contains no assistant text".to_owned(),
        ));
    }
    Ok(reply)
}

fn read_reply(provider: Provider, root: &Json) -> Result<Reply, ConnectError> {
    match provider.wire() {
        Wire::Responses => read_responses(provider, root),
        Wire::Messages => read_messages(provider, root),
        Wire::Chat => read_chat(provider, root),
    }
}

fn read_responses(provider: Provider, root: &Json) -> Result<Reply, ConnectError> {
    let status = root.get("status").and_then(Json::as_str);
    let cut_short = status == Some("incomplete");
    if status.is_some_and(|status| status != "completed" && status != "incomplete") {
        return Err(ConnectError::Protocol(
            "response did not complete".to_owned(),
        ));
    }
    let items = root.get("output").and_then(Json::as_list).unwrap_or(&[]);
    let mut text = String::new();
    let mut calls = Vec::new();
    for item in items {
        match item.get("type").and_then(Json::as_str) {
            Some("message") => {
                if let Some(parts) = item.get("content").and_then(Json::as_list) {
                    for part in parts {
                        if part.get("type").and_then(Json::as_str) == Some("output_text")
                            && let Some(value) = part.get("text").and_then(Json::as_str)
                        {
                            text.push_str(value);
                        }
                    }
                }
            }
            Some("function_call") => calls.push(ToolCall {
                id: item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                name: item
                    .get("name")
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                arguments: arguments_of(item.get("arguments"))?,
            }),
            _ => {}
        }
    }
    Ok(Reply {
        text,
        calls,
        raw: raw_of(provider, items),
        cut_short,
    })
}

fn read_messages(provider: Provider, root: &Json) -> Result<Reply, ConnectError> {
    let parts = root.get("content").and_then(Json::as_list).unwrap_or(&[]);
    let mut text = String::new();
    let mut calls = Vec::new();
    for part in parts {
        match part.get("type").and_then(Json::as_str) {
            Some("text") => {
                if let Some(value) = part.get("text").and_then(Json::as_str) {
                    text.push_str(value);
                }
            }
            Some("tool_use") => calls.push(ToolCall {
                id: part
                    .get("id")
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                name: part
                    .get("name")
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                arguments: arguments_of(part.get("input"))?,
            }),
            _ => {}
        }
    }
    Ok(Reply {
        text,
        calls,
        raw: raw_of(provider, parts),
        cut_short: root.get("stop_reason").and_then(Json::as_str) == Some("max_tokens"),
    })
}

fn read_chat(provider: Provider, root: &Json) -> Result<Reply, ConnectError> {
    let choice = root
        .get("choices")
        .and_then(Json::as_list)
        .and_then(<[Json]>::first);
    let message = choice.and_then(|item| item.get("message"));
    let content = message.and_then(|message| message.get("content"));
    let mut text = String::new();
    if let Some(value) = content.and_then(Json::as_str) {
        text.push_str(value);
    } else if let Some(parts) = content.and_then(Json::as_list) {
        for part in parts {
            if part
                .get("type")
                .and_then(Json::as_str)
                .is_some_and(|kind| kind == "text" || kind == "output_text")
                && let Some(value) = part.get("text").and_then(Json::as_str)
            {
                text.push_str(value);
            }
        }
    }
    let mut calls = Vec::new();
    if let Some(items) = message
        .and_then(|message| message.get("tool_calls"))
        .and_then(Json::as_list)
    {
        for item in items {
            let function = item.get("function");
            calls.push(ToolCall {
                id: item
                    .get("id")
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                name: function
                    .and_then(|function| function.get("name"))
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                arguments: arguments_of(function.and_then(|function| function.get("arguments")))?,
            });
        }
    }
    Ok(Reply {
        text,
        calls,
        raw: message.map(|message| Raw {
            provider,
            items: vec![message.clone()],
        }),
        cut_short: choice
            .and_then(|item| item.get("finish_reason"))
            .and_then(Json::as_str)
            == Some("length"),
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Progress<'a> {
    pub said: &'a str,
    pub thinking: &'a str,
}

enum Gathering {
    Responses {
        said: String,
        thinking: String,
        items: Vec<Json>,
        completed: Option<Json>,
    },
    Messages {
        said: String,
        thinking: String,
        blocks: Vec<(Json, String)>,
        stop_reason: Option<String>,
        started: bool,
    },
    Chat {
        said: String,
        reasoning: String,
        calls: Vec<ArrivingCall>,
        kept: BTreeMap<String, Json>,
        finish_reason: Option<String>,
        started: bool,
    },
}

#[derive(Default)]
struct ArrivingCall {
    id: String,
    name: String,
    arguments: String,
    kept: BTreeMap<String, Json>,
}

impl ArrivingCall {
    fn spelled(&self) -> Json {
        let mut spelled = Json::object([
            ("id", Json::text(self.id.clone())),
            ("type", Json::text("function")),
            (
                "function",
                Json::object([
                    ("name", Json::text(self.name.clone())),
                    ("arguments", Json::text(self.arguments.clone())),
                ]),
            ),
        ]);
        for (key, value) in &self.kept {
            put(&mut spelled, key, value.clone());
        }
        spelled
    }
}

impl Gathering {
    fn new(wire: Wire) -> Self {
        match wire {
            Wire::Responses => Self::Responses {
                said: String::new(),
                thinking: String::new(),
                items: Vec::new(),
                completed: None,
            },
            Wire::Messages => Self::Messages {
                said: String::new(),
                thinking: String::new(),
                blocks: Vec::new(),
                stop_reason: None,
                started: false,
            },
            Wire::Chat => Self::Chat {
                said: String::new(),
                reasoning: String::new(),
                calls: Vec::new(),
                kept: BTreeMap::new(),
                finish_reason: None,
                started: false,
            },
        }
    }

    fn said(&self) -> &str {
        match self {
            Self::Responses { said, .. }
            | Self::Messages { said, .. }
            | Self::Chat { said, .. } => said,
        }
    }

    fn thinking(&self) -> &str {
        match self {
            Self::Responses { thinking, .. } | Self::Messages { thinking, .. } => thinking,
            Self::Chat { reasoning, .. } => reasoning,
        }
    }

    fn line(&mut self, line: &str) -> bool {
        let Some(data) = line.trim_end().strip_prefix("data:") else {
            return false;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            return false;
        }
        let Ok(event) = Json::parse(data) else {
            return false;
        };
        let before = (self.said().len(), self.thinking().len());
        self.event(&event);
        (self.said().len(), self.thinking().len()) != before
    }

    fn event(&mut self, event: &Json) {
        let kind = event.get("type").and_then(Json::as_str).unwrap_or_default();
        match self {
            Self::Responses {
                said,
                thinking,
                items,
                completed,
            } => match kind {
                "response.output_text.delta" => {
                    if let Some(piece) = event.get("delta").and_then(Json::as_str) {
                        said.push_str(piece);
                    }
                }
                "response.reasoning_summary_text.delta" => {
                    if let Some(piece) = event.get("delta").and_then(Json::as_str) {
                        thinking.push_str(piece);
                    }
                }
                "response.output_item.done" => {
                    if let Some(item) = event.get("item") {
                        items.push(item.clone());
                    }
                }
                "response.completed" => *completed = event.get("response").cloned(),
                _ => {}
            },
            Self::Messages {
                said,
                thinking,
                blocks,
                stop_reason,
                started,
            } => {
                *started = true;
                messages_event(event, kind, (said, thinking), blocks, stop_reason);
            }
            Self::Chat {
                said,
                reasoning,
                calls,
                kept,
                finish_reason,
                started,
            } => chat_event(
                event,
                (said, reasoning),
                (calls, kept),
                finish_reason,
                started,
            ),
        }
    }

    fn whole(&self) -> Option<Json> {
        match self {
            Self::Responses {
                said,
                items,
                completed,
                ..
            } => {
                if let Some(response) = completed {
                    return Some(response.clone());
                }
                if items.is_empty() && said.is_empty() {
                    return None;
                }
                let mut output = items.clone();
                if output.is_empty() {
                    output.push(Json::object([
                        ("type", Json::text("message")),
                        (
                            "content",
                            Json::List(vec![Json::object([
                                ("type", Json::text("output_text")),
                                ("text", Json::text(said.clone())),
                            ])]),
                        ),
                    ]));
                }
                Some(Json::object([
                    ("status", Json::text("completed")),
                    ("output", Json::List(output)),
                ]))
            }
            Self::Messages {
                blocks,
                stop_reason,
                started,
                ..
            } => {
                if !*started {
                    return None;
                }
                let mut body = Json::object([(
                    "content",
                    Json::List(blocks.iter().map(|(block, _)| block.clone()).collect()),
                )]);
                if let Some(why) = stop_reason {
                    put(&mut body, "stop_reason", Json::text(why.clone()));
                }
                Some(body)
            }
            Self::Chat {
                said,
                reasoning,
                calls,
                kept,
                finish_reason,
                started,
            } => {
                if !*started {
                    return None;
                }
                let mut message = Json::object([
                    ("role", Json::text("assistant")),
                    ("content", Json::text(said.clone())),
                ]);
                for (key, value) in kept {
                    put(&mut message, key, value.clone());
                }
                if !reasoning.is_empty() {
                    put(&mut message, "reasoning", Json::text(reasoning.clone()));
                }
                if !calls.is_empty() {
                    put(
                        &mut message,
                        "tool_calls",
                        Json::List(calls.iter().map(ArrivingCall::spelled).collect()),
                    );
                }
                let mut choice = Json::object([("message", message)]);
                if let Some(why) = finish_reason {
                    put(&mut choice, "finish_reason", Json::text(why.clone()));
                }
                Some(Json::object([("choices", Json::List(vec![choice]))]))
            }
        }
    }
}

fn messages_event(
    event: &Json,
    kind: &str,
    (said, thinking): (&mut String, &mut String),
    blocks: &mut Vec<(Json, String)>,
    stop_reason: &mut Option<String>,
) {
    let at = index_of(event);
    match kind {
        "content_block_start" => {
            let block = event
                .get("content_block")
                .cloned()
                .unwrap_or_else(|| Json::object([]));
            while blocks.len() <= at {
                blocks.push((Json::object([]), String::new()));
            }
            blocks[at] = (block, String::new());
        }
        "content_block_delta" => {
            let Some((block, arguments)) = blocks.get_mut(at) else {
                return;
            };
            let delta = event.get("delta");
            let piece = |name: &str| {
                delta
                    .and_then(|delta| delta.get(name))
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_owned()
            };
            match delta
                .and_then(|delta| delta.get("type"))
                .and_then(Json::as_str)
            {
                Some("text_delta") => {
                    let text = piece("text");
                    said.push_str(&text);
                    append(block, "text", &text);
                }
                Some("input_json_delta") => arguments.push_str(&piece("partial_json")),
                Some("thinking_delta") => {
                    let thought = piece("thinking");
                    thinking.push_str(&thought);
                    append(block, "thinking", &thought);
                }
                Some("signature_delta") => {
                    put(block, "signature", Json::text(piece("signature")));
                }
                _ => {}
            }
        }
        "content_block_stop" => {
            if let Some((block, arguments)) = blocks.get_mut(at)
                && !arguments.is_empty()
                && let Ok(input) = Json::parse(arguments)
            {
                put(block, "input", input);
                arguments.clear();
            }
        }
        "message_delta" => {
            if let Some(why) = event
                .get("delta")
                .and_then(|delta| delta.get("stop_reason"))
                .and_then(Json::as_str)
            {
                *stop_reason = Some(why.to_owned());
            }
        }
        _ => {}
    }
}

fn chat_event(
    event: &Json,
    (said, reasoning): (&mut String, &mut String),
    (calls, kept): (&mut Vec<ArrivingCall>, &mut BTreeMap<String, Json>),
    finish_reason: &mut Option<String>,
    started: &mut bool,
) {
    let Some(choice) = event
        .get("choices")
        .and_then(Json::as_list)
        .and_then(<[Json]>::first)
    else {
        return;
    };
    *started = true;
    if let Some(why) = choice.get("finish_reason").and_then(Json::as_str) {
        *finish_reason = Some(why.to_owned());
    }
    let Some(delta) = choice.get("delta") else {
        return;
    };
    if let Some(piece) = delta.get("content").and_then(Json::as_str) {
        said.push_str(piece);
    }
    if let Some(piece) = delta.get("reasoning").and_then(Json::as_str) {
        reasoning.push_str(piece);
    }
    if let Json::Object(members) = delta {
        for (key, value) in members {
            if !matches!(
                key.as_str(),
                "content" | "reasoning" | "role" | "tool_calls"
            ) {
                kept.insert(key.clone(), value.clone());
            }
        }
    }
    let Some(asked) = delta.get("tool_calls").and_then(Json::as_list) else {
        return;
    };
    for call in asked {
        let at = index_of(call);
        while calls.len() <= at {
            calls.push(ArrivingCall::default());
        }
        let arriving = &mut calls[at];
        if let Some(value) = call.get("id").and_then(Json::as_str) {
            value.clone_into(&mut arriving.id);
        }
        let function = call.get("function");
        if let Some(value) = function
            .and_then(|function| function.get("name"))
            .and_then(Json::as_str)
        {
            value.clone_into(&mut arriving.name);
        }
        if let Some(value) = function
            .and_then(|function| function.get("arguments"))
            .and_then(Json::as_str)
        {
            arriving.arguments.push_str(value);
        }
        if let Json::Object(members) = call {
            for (key, value) in members {
                if !matches!(key.as_str(), "id" | "index" | "type" | "function") {
                    arriving.kept.insert(key.clone(), value.clone());
                }
            }
        }
    }
}

fn index_of(event: &Json) -> usize {
    event
        .get("index")
        .and_then(Json::as_count)
        .filter(|at| *at < 1024)
        .unwrap_or(0)
}

fn append(block: &mut Json, key: &str, piece: &str) {
    let mut grown = block
        .get(key)
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_owned();
    grown.push_str(piece);
    put(block, key, Json::text(grown));
}

fn raw_of(provider: Provider, items: &[Json]) -> Option<Raw> {
    (!items.is_empty()).then(|| Raw {
        provider,
        items: items.to_vec(),
    })
}

fn arguments_of(value: Option<&Json>) -> Result<Json, ConnectError> {
    match value {
        None | Some(Json::Null) => Ok(Json::object([])),
        Some(Json::Text(written)) if written.trim().is_empty() => Ok(Json::object([])),
        Some(Json::Text(written)) => Json::parse(written).map_err(|error| {
            ConnectError::Protocol(format!("a tool call's arguments are not JSON: {error}"))
        }),
        Some(other) => Ok(other.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_newest_picture_is_sent_again() {
        let drawn = |id: &str| ToolResult {
            call_id: id.to_owned(),
            text: "a page".to_owned(),
            is_error: false,
            picture: Some(Picture {
                media_type: "image/png".to_owned(),
                base64: id.to_owned(),
            }),
        };
        let turns = vec![
            Turn::person("look at page 1"),
            Turn::Results {
                results: vec![drawn("one"), ToolResult::said("words", "some words")],
            },
            Turn::model("I see it."),
            Turn::Results {
                results: vec![drawn("two"), drawn("three")],
            },
            Turn::model("and that one too"),
        ];
        let sent = without_the_old_pictures(&turns);
        assert_eq!(sent.len(), turns.len());
        let pictures: Vec<String> = sent
            .iter()
            .filter_map(|turn| match turn {
                Turn::Results { results } => Some(results),
                _ => None,
            })
            .flatten()
            .filter_map(|result| result.picture.as_ref().map(|it| it.base64.clone()))
            .collect();
        assert_eq!(pictures, vec!["three".to_owned()]);
        assert_eq!(sent[1], {
            let Turn::Results { results } = &turns[1] else {
                panic!("a turn of results");
            };
            Turn::Results {
                results: vec![
                    ToolResult {
                        picture: None,
                        ..results[0].clone()
                    },
                    results[1].clone(),
                ],
            }
        });
        assert_eq!(sent[0], turns[0]);
        assert_eq!(sent[2], turns[2]);
        assert_eq!(sent[4], turns[4]);
    }

    #[test]
    fn a_conversation_without_pictures_is_unchanged() {
        let turns = vec![
            Turn::person("hello"),
            Turn::Results {
                results: vec![ToolResult::said("one", "done")],
            },
        ];
        assert_eq!(without_the_old_pictures(&turns), turns);
        assert_eq!(without_the_old_pictures(&[]), Vec::<Turn>::new());
    }

    #[test]
    fn presets_and_transport_rules_are_explicit() {
        assert_eq!(
            Provider::Ollama.default_base_url(),
            "http://127.0.0.1:11434/v1"
        );
        assert!(endpoint("http://example.test/v1", "/models", Provider::Custom).is_err());
        assert!(endpoint("http://127.0.0.1:1/v1", "/models", Provider::Ollama).is_ok());
        assert!(endpoint("http://127.0.0.1.evil/v1", "/models", Provider::Ollama).is_err());
        assert!(endpoint("https://example.test/v1", "/models", Provider::Custom).is_ok());
    }

    #[test]
    fn curl_config_escapes_prompt_like_values() {
        let escaped = curl_escape("a\\b\"c");
        assert_eq!(escaped, "a\\\\b\\\"c");
        assert!(!escaped.contains('\n'));
    }

    #[test]
    fn text_normalization_covers_openai_and_anthropic_shapes() {
        let value = Json::parse(
            r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"a"}]}]}"#,
        )
        .unwrap();
        assert_eq!(answer(Provider::OpenAi, &value).unwrap().text, "a");
        let unrelated = Json::parse(r#"{"output":[{"type":"reasoning","content":[{"type":"summary_text","text":"ignore"}]}]}"#).unwrap();
        assert!(answer(Provider::OpenAi, &unrelated).is_err());
    }

    #[test]
    fn a_conversation_is_spelled_for_each_wire_family() {
        let turns = [
            Turn::person("first"),
            Turn::model("answered"),
            Turn::person("second"),
        ];
        let of = |provider| Connection {
            provider,
            base_url: "http://127.0.0.1:1/v1".into(),
            model: "m".into(),
            api_key: "k".into(),
            effort: Effort::Off,
        };
        let anthropic = of(Provider::Anthropic)
            .payload(
                &of(Provider::Anthropic).spoken(&turns, None).unwrap(),
                &[],
                None,
            )
            .write();
        assert_eq!(anthropic.matches("\"role\"").count(), 3, "{anthropic}");
        assert!(anthropic.contains("\"assistant\""), "{anthropic}");
        assert!(
            anthropic.find("first").unwrap() < anthropic.find("second").unwrap(),
            "{anthropic}"
        );
        let openai = of(Provider::OpenAi)
            .payload(
                &of(Provider::OpenAi).spoken(&turns, None).unwrap(),
                &[],
                None,
            )
            .write();
        assert_eq!(openai.matches("input_text").count(), 2, "{openai}");
        assert_eq!(openai.matches("output_text").count(), 1, "{openai}");
        let local = of(Provider::Ollama)
            .payload(
                &of(Provider::Ollama).spoken(&turns, None).unwrap(),
                &[],
                None,
            )
            .write();
        assert_eq!(local.matches("\"content\"").count(), 3, "{local}");
    }

    #[test]
    fn the_document_is_given_once_at_the_start_of_the_conversation() {
        let connection = Connection {
            provider: Provider::Ollama,
            base_url: "http://127.0.0.1:1/v1".into(),
            model: "m".into(),
            api_key: String::new(),
            effort: Effort::Off,
        };
        let turns = [
            Turn::person("first"),
            Turn::model("answered"),
            Turn::person("second"),
        ];
        let spoken = connection.spoken(&turns, Some("the page")).unwrap();
        let written = connection.payload(&spoken, &[], None).write();
        assert_eq!(written.matches("Document context").count(), 1, "{written}");
        assert!(
            written.find("the page").unwrap() < written.find("second").unwrap(),
            "{written}"
        );
    }

    #[test]
    fn a_conversation_must_end_with_something_the_person_said() {
        let connection = Connection {
            provider: Provider::Ollama,
            base_url: "http://127.0.0.1:1/v1".into(),
            model: "m".into(),
            api_key: String::new(),
            effort: Effort::Off,
        };
        assert!(connection.spoken(&[], None).is_err());
        assert!(connection.spoken(&[Turn::model("hello")], None).is_err());
        assert!(connection.spoken(&[Turn::person("")], None).is_err());
    }

    #[test]
    fn a_request_curl_could_not_make_is_not_a_refusal() {
        use std::net::TcpListener;
        let closed = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = closed.local_addr().unwrap().port();
        drop(closed);
        let connection = Connection {
            provider: Provider::Ollama,
            base_url: format!("http://127.0.0.1:{port}/v1"),
            model: "m".into(),
            api_key: String::new(),
            effort: Effort::Off,
        };
        let error = connection.models(&AtomicBool::new(false)).unwrap_err();
        match error {
            ConnectError::Curl(said) => assert!(!said.is_empty(), "curl says why"),
            other => panic!("a request curl could not make: {other:?}"),
        }
    }

    #[test]
    fn keys_with_lines_are_rejected() {
        let connection = Connection {
            provider: Provider::Ollama,
            base_url: "http://127.0.0.1:1/v1".into(),
            model: "m".into(),
            api_key: "bad\nkey".into(),
            effort: Effort::Off,
        };
        let error = connection.models(&AtomicBool::new(false)).unwrap_err();
        assert!(matches!(error, ConnectError::Invalid(_)));
    }

    #[test]
    fn local_fake_server_proves_models_request_and_json_answer() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let count = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..count]);
            assert!(request.starts_with("GET /v1/models"));
            let body = r#"{"data":[{"id":"local-model"}]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let connection = Connection {
            provider: Provider::Ollama,
            base_url: format!("http://127.0.0.1:{}/v1", address.port()),
            model: "local-model".into(),
            api_key: String::new(),
            effort: Effort::Off,
        };
        let models = connection.models(&AtomicBool::new(false)).unwrap();
        server.join().unwrap();
        assert_eq!(
            models,
            vec![Model {
                id: "local-model".into()
            }]
        );
    }

    #[test]
    fn a_level_of_thinking_is_written_and_read_as_one_word() {
        for level in [Effort::Off, Effort::Low, Effort::Medium, Effort::High] {
            assert_eq!(Effort::parse(level.as_str()), Some(level));
        }
        assert_eq!(Effort::Medium.as_str(), "medium");
        assert_eq!(Effort::parse("Medium"), None);
        assert_eq!(Effort::parse("maximum"), None);
        assert_eq!(Effort::default(), Effort::Off);
    }

    #[test]
    fn an_anthropic_model_is_asked_to_think_in_the_way_its_own_name_takes() {
        for budget in [
            "claude-3-5-sonnet-20241022",
            "claude-3-opus-20240229",
            "claude-sonnet-4-5-20250929",
            "claude-opus-4-1-20250805",
            "claude-opus-4-0",
        ] {
            assert_eq!(
                anthropic_thinking_style(budget),
                ThinkingStyle::Budget,
                "{budget}"
            );
        }
        for adaptive in ["claude-opus-5", "claude-sonnet-5-20260101"] {
            assert_eq!(
                anthropic_thinking_style(adaptive),
                ThinkingStyle::Adaptive,
                "{adaptive}"
            );
        }
    }

    #[test]
    fn thinking_is_asked_for_in_each_familys_own_words() {
        let members = |provider, model, effort| {
            let (members, max_tokens) = reasoning_members(provider, model, effort);
            (
                Json::Object(
                    members
                        .into_iter()
                        .map(|(key, value)| (key.to_owned(), value))
                        .collect(),
                )
                .write(),
                max_tokens,
            )
        };
        for provider in [Provider::OpenAi, Provider::Anthropic, Provider::Ollama] {
            let model = if provider == Provider::Anthropic {
                "claude-sonnet-4-5-20250929"
            } else {
                "m"
            };
            assert_eq!(
                members(provider, model, Effort::Off),
                ("{}".to_owned(), 4096),
                "{provider:?} asked for nothing"
            );
        }
        assert_eq!(
            members(Provider::OpenAi, "gpt-5", Effort::Low).0,
            r#"{"reasoning":{"effort":"low"}}"#
        );
        assert_eq!(
            members(Provider::OpenAi, "gpt-5", Effort::High).0,
            r#"{"reasoning":{"effort":"high"}}"#
        );
        assert_eq!(
            members(Provider::Anthropic, "claude-sonnet-4-5", Effort::Low),
            (
                r#"{"thinking":{"budget_tokens":1024,"type":"enabled"}}"#.to_owned(),
                1024 + 4096
            )
        );
        assert_eq!(
            members(Provider::Anthropic, "claude-3-5-haiku", Effort::Medium),
            (
                r#"{"thinking":{"budget_tokens":4096,"type":"enabled"}}"#.to_owned(),
                4096 + 4096
            )
        );
        assert_eq!(
            members(Provider::Anthropic, "claude-opus-4-1", Effort::High),
            (
                r#"{"thinking":{"budget_tokens":16000,"type":"enabled"}}"#.to_owned(),
                16000 + 4096
            )
        );
        assert_eq!(
            members(Provider::Anthropic, "claude-opus-5", Effort::Medium),
            (
                r#"{"output_config":{"effort":"medium"},"thinking":{"type":"adaptive"}}"#
                    .to_owned(),
                16000
            )
        );
        for provider in [Provider::Ollama, Provider::LmStudio, Provider::Gemini] {
            assert_eq!(
                members(provider, "qwen3.5:latest", Effort::Low).0,
                r#"{"reasoning_effort":"low"}"#,
                "{provider:?}"
            );
        }
    }

    #[test]
    fn what_was_asked_for_is_in_the_body() {
        let connection = Connection {
            provider: Provider::Anthropic,
            base_url: "https://api.anthropic.com/v1".into(),
            model: "claude-sonnet-4-5-20250929".into(),
            api_key: "k".into(),
            effort: Effort::High,
        };
        let spoken = connection.spoken(&[Turn::person("hello")], None).unwrap();
        let written = connection.payload(&spoken, &[], None).write();
        assert!(written.contains(r#""budget_tokens":16000"#), "{written}");
        assert!(written.contains(r#""max_tokens":20096"#), "{written}");
    }

    fn of(provider: Provider) -> Connection {
        Connection {
            provider,
            base_url: "http://127.0.0.1:1/v1".into(),
            model: "m".into(),
            api_key: "k".into(),
            effort: Effort::Off,
        }
    }

    fn one_tool() -> Vec<ToolOffer> {
        vec![ToolOffer {
            name: "read_text".into(),
            description: "Reads a page.".into(),
            schema: Json::parse(r#"{"type":"object","properties":{"page":{"type":"integer"}}}"#)
                .unwrap(),
        }]
    }

    #[test]
    fn with_nothing_offered_the_body_is_what_it_always_was() {
        let turns = [Turn::person("first")];
        let body = |provider| {
            of(provider)
                .payload(&of(provider).spoken(&turns, None).unwrap(), &[], None)
                .write()
        };
        assert_eq!(
            body(Provider::Anthropic),
            r#"{"max_tokens":4096,"messages":[{"content":"first","role":"user"}],"model":"m"}"#
        );
        assert_eq!(
            body(Provider::OpenAi),
            r#"{"input":[{"content":[{"text":"first","type":"input_text"}],"role":"user"}],"model":"m","store":false}"#
        );
        assert_eq!(
            body(Provider::Ollama),
            r#"{"messages":[{"content":"first","role":"user"}],"model":"m"}"#
        );
        for provider in [Provider::Anthropic, Provider::OpenAi, Provider::Ollama] {
            let offered = of(provider)
                .payload(
                    &of(provider).spoken(&turns, None).unwrap(),
                    &one_tool(),
                    None,
                )
                .write();
            assert!(offered.contains(r#""tools""#), "{offered}");
        }
        let attached = [Turn::person_with(
            "first",
            vec![Attachment::image("shot.png", "image/png", vec![1, 2, 3])],
        )];
        for provider in [Provider::Anthropic, Provider::OpenAi, Provider::Ollama] {
            let written = of(provider)
                .payload(&of(provider).spoken(&attached, None).unwrap(), &[], None)
                .write();
            assert!(written.contains("AQID"), "{written}");
        }
    }

    #[test]
    fn an_attachment_is_spelled_in_each_familys_own_words() {
        let turns = [Turn::person_with(
            "look at this",
            vec![
                Attachment::text("report.pdf", "page one says hello"),
                Attachment::image("shot.png", "image/png", vec![1, 2, 3]),
            ],
        )];
        let body = |provider| {
            of(provider)
                .payload(&of(provider).spoken(&turns, None).unwrap(), &[], None)
                .write()
        };

        let anthropic = body(Provider::Anthropic);
        assert!(
            anthropic.contains(
                r#"{"source":{"data":"AQID","media_type":"image/png","type":"base64"},"type":"image"}"#
            ),
            "{anthropic}"
        );
        assert!(
            anthropic.contains(r#"Attached file \"report.pdf\":\npage one says hello"#),
            "{anthropic}"
        );

        let openai = body(Provider::OpenAi);
        assert!(
            openai.contains(r#"{"image_url":"data:image/png;base64,AQID","type":"input_image"}"#),
            "{openai}"
        );
        assert_eq!(openai.matches("input_text").count(), 2, "{openai}");

        let local = body(Provider::Ollama);
        assert!(
            local.contains(
                r#"{"image_url":{"url":"data:image/png;base64,AQID"},"type":"image_url"}"#
            ),
            "{local}"
        );
        assert_eq!(local.matches(r#""type":"text""#).count(), 2, "{local}");

        for written in [anthropic, openai, local] {
            assert_eq!(written.matches("AQID").count(), 1, "{written}");
        }
    }

    #[test]
    fn what_is_attached_has_an_allowance_of_its_own() {
        let connection = of(Provider::Ollama);
        let photograph = |bytes: usize| {
            Turn::person_with(
                "",
                vec![Attachment::image("shot.png", "image/png", vec![7; bytes])],
            )
        };
        assert!(connection.spoken(&[photograph(3)], None).is_ok());
        assert!(
            connection
                .spoken(&[Turn::person("x".repeat(MOST_SPOKEN_BYTES + 1))], None)
                .is_err()
        );
        assert!(
            connection
                .spoken(&[photograph(MOST_SPOKEN_BYTES + 1)], None)
                .is_ok()
        );
        assert!(
            connection
                .spoken(&[photograph(crate::attach::MOST_TOTAL_BYTES + 1024)], None)
                .is_err()
        );
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one spelled-out known answer per wire family"
    )]
    fn a_tool_is_offered_and_answered_in_each_familys_own_words() {
        let turns = [
            Turn::person("read them"),
            Turn::Model {
                text: "Looking.".into(),
                calls: vec![
                    ToolCall {
                        id: "c1".into(),
                        name: "read_text".into(),
                        arguments: Json::parse(r#"{"page":1}"#).unwrap(),
                    },
                    ToolCall {
                        id: "c2".into(),
                        name: "read_text".into(),
                        arguments: Json::parse(r#"{"page":2}"#).unwrap(),
                    },
                ],
                raw: None,
            },
            Turn::Results {
                results: vec![
                    ToolResult::said("c1", "page one"),
                    ToolResult::failed("c2", "no such page"),
                ],
            },
        ];
        let body = |provider| {
            of(provider)
                .payload(
                    &of(provider).spoken(&turns, None).unwrap(),
                    &one_tool(),
                    Some("you are helping"),
                )
                .write()
        };

        let anthropic = body(Provider::Anthropic);
        assert!(
            anthropic.contains(r#""input_schema":{"properties":{"page":{"type":"integer"}}"#),
            "{anthropic}"
        );
        assert!(
            anthropic.contains(r#""system":"you are helping""#),
            "{anthropic}"
        );
        assert!(
            anthropic
                .contains(r#"{"id":"c1","input":{"page":1},"name":"read_text","type":"tool_use"}"#),
            "{anthropic}"
        );
        assert_eq!(
            anthropic.matches(r#""tool_result""#).count(),
            2,
            "{anthropic}"
        );
        assert_eq!(
            anthropic.matches(r#""role":"user""#).count(),
            2,
            "{anthropic}"
        );
        assert!(
            anthropic.contains(r#"{"content":"no such page","is_error":true,"tool_use_id":"c2","type":"tool_result"}"#),
            "{anthropic}"
        );

        let openai = body(Provider::OpenAi);
        assert!(
            openai.contains(r#""instructions":"you are helping""#),
            "{openai}"
        );
        assert!(openai.contains(r#""tool_choice":"auto""#), "{openai}");
        assert!(
            openai.contains(r#""include":["reasoning.encrypted_content"]"#),
            "{openai}"
        );
        assert!(
            openai.contains(r#"{"description":"Reads a page.","name":"read_text","parameters":{"properties":{"page":{"type":"integer"}},"type":"object"},"strict":false,"type":"function"}"#),
            "{openai}"
        );
        assert!(
            openai.contains(r#"{"arguments":"{\"page\":1}","call_id":"c1","name":"read_text","type":"function_call"}"#),
            "{openai}"
        );
        assert_eq!(
            openai.matches(r#""type":"function_call_output""#).count(),
            2,
            "{openai}"
        );

        let local = body(Provider::Ollama);
        assert!(
            local.contains(r#"{"content":"you are helping","role":"system"}"#),
            "{local}"
        );
        assert!(
            local.contains(
                r#""function":{"description":"Reads a page.","name":"read_text","parameters""#
            ),
            "{local}"
        );
        assert!(
            local.contains(r#"{"function":{"arguments":"{\"page\":1}","name":"read_text"},"id":"c1","type":"function"}"#),
            "{local}"
        );
        assert_eq!(local.matches(r#""role":"tool""#).count(), 2, "{local}");
        assert!(
            local.contains(r#"{"content":"page one","role":"tool","tool_call_id":"c1"}"#),
            "{local}"
        );
    }

    #[test]
    fn a_picture_in_a_result_is_spelled_where_it_can_be() {
        let turns = [
            Turn::person("show me"),
            Turn::Model {
                text: String::new(),
                calls: vec![ToolCall {
                    id: "c1".into(),
                    name: "render_page".into(),
                    arguments: Json::object([]),
                }],
                raw: None,
            },
            Turn::Results {
                results: vec![ToolResult {
                    call_id: "c1".into(),
                    text: "page 1".into(),
                    is_error: false,
                    picture: Some(Picture {
                        media_type: "image/png".into(),
                        base64: "AAAA".into(),
                    }),
                }],
            },
        ];
        let anthropic = of(Provider::Anthropic)
            .payload(
                &of(Provider::Anthropic).spoken(&turns, None).unwrap(),
                &one_tool(),
                None,
            )
            .write();
        assert!(
            anthropic.contains(
                r#"{"source":{"data":"AAAA","media_type":"image/png","type":"base64"},"type":"image"}"#
            ),
            "{anthropic}"
        );
        let local = of(Provider::Ollama)
            .payload(
                &of(Provider::Ollama).spoken(&turns, None).unwrap(),
                &one_tool(),
                None,
            )
            .write();
        assert!(
            local.contains(r#"{"content":"page 1","role":"tool""#),
            "{local}"
        );
        assert!(!local.contains("AAAA"), "{local}");
    }

    #[test]
    fn calls_are_read_from_each_familys_answer() {
        let block = |reply: &Reply| {
            reply.calls[0]
                .arguments
                .get("block")
                .and_then(Json::as_str)
                .unwrap_or("")
                .to_owned()
        };

        let openai = read_reply(
            Provider::OpenAi,
            &Json::parse(
                r#"{"status":"completed","output":[
{"type":"reasoning","id":"rs_1","encrypted_content":"xx"},
{"type":"function_call","call_id":"call_1","name":"replace_text","arguments":"{\"block\":\"p2-b3\",\"text\":\"Hello\"}"}]}"#,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(openai.calls.len(), 1);
        assert_eq!(openai.calls[0].id, "call_1");
        assert_eq!(openai.calls[0].name, "replace_text");
        assert_eq!(block(&openai), "p2-b3");
        assert!(openai.text.is_empty());
        assert!(!openai.cut_short);

        let anthropic = read_reply(
            Provider::Anthropic,
            &Json::parse(
                r#"{"stop_reason":"tool_use","content":[
{"type":"thinking","thinking":"hm","signature":"s"},
{"type":"text","text":"Changing it."},
{"type":"tool_use","id":"toolu_1","name":"replace_text","input":{"block":"p2-b3","text":"Hello"}}]}"#,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(anthropic.calls[0].id, "toolu_1");
        assert_eq!(anthropic.text, "Changing it.");
        assert_eq!(block(&anthropic), "p2-b3");

        let local = read_reply(
            Provider::Ollama,
            &Json::parse(
                r#"{"choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":"","reasoning":"thinking","tool_calls":[{"id":"call_1","type":"function","function":{"name":"replace_text","arguments":"{\"block\":\"p2-b3\"}"}}]}}]}"#,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(local.calls[0].id, "call_1");
        assert_eq!(block(&local), "p2-b3");
    }

    #[test]
    fn arguments_are_read_as_an_object_or_as_a_string() {
        let object = read_reply(
            Provider::Ollama,
            &Json::parse(
                r#"{"choices":[{"message":{"tool_calls":[{"id":"call_1","function":{"name":"replace_text","arguments":{"block":"p2-b3"}}}]}}]}"#,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            object.calls[0]
                .arguments
                .get("block")
                .and_then(Json::as_str),
            Some("p2-b3")
        );
        let broken = read_reply(
            Provider::Ollama,
            &Json::parse(
                r#"{"choices":[{"message":{"tool_calls":[{"id":"call_1","function":{"name":"replace_text","arguments":"{not json"}}]}}]}"#,
            )
            .unwrap(),
        );
        assert!(
            matches!(broken, Err(ConnectError::Protocol(_))),
            "{broken:?}"
        );
    }

    #[test]
    fn an_answer_that_ran_out_of_room_is_marked() {
        let openai = read_reply(
            Provider::OpenAi,
            &Json::parse(
                r#"{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"output":[{"type":"message","content":[{"type":"output_text","text":"half"}]}]}"#,
            )
            .unwrap(),
        )
        .unwrap();
        assert!(openai.cut_short && openai.text == "half");
        let anthropic = read_reply(
            Provider::Anthropic,
            &Json::parse(
                r#"{"stop_reason":"max_tokens","content":[{"type":"text","text":"half"}]}"#,
            )
            .unwrap(),
        )
        .unwrap();
        assert!(anthropic.cut_short);
        let local = read_reply(
            Provider::Ollama,
            &Json::parse(
                r#"{"choices":[{"finish_reason":"length","message":{"content":"half"}}]}"#,
            )
            .unwrap(),
        )
        .unwrap();
        assert!(local.cut_short);
        let finished = read_reply(
            Provider::Ollama,
            &Json::parse(r#"{"choices":[{"finish_reason":"stop","message":{"content":"all"}}]}"#)
                .unwrap(),
        )
        .unwrap();
        assert!(!finished.cut_short);
    }

    #[test]
    fn a_models_own_items_are_sent_back_as_they_came() {
        let raw = Raw {
            provider: Provider::Anthropic,
            items: vec![
                Json::parse(r#"{"type":"thinking","thinking":"hm","signature":"sig-1"}"#).unwrap(),
                Json::parse(r#"{"type":"text","text":"Changing it."}"#).unwrap(),
                Json::parse(
                    r#"{"type":"tool_use","id":"toolu_1","name":"replace_text","input":{"block":"p2-b3"}}"#,
                )
                .unwrap(),
            ],
        };
        let turns = [
            Turn::person("change it"),
            Turn::Model {
                text: "Changing it.".into(),
                calls: vec![ToolCall {
                    id: "toolu_1".into(),
                    name: "replace_text".into(),
                    arguments: Json::parse(r#"{"block":"p2-b3"}"#).unwrap(),
                }],
                raw: Some(raw),
            },
            Turn::Results {
                results: vec![ToolResult::said("toolu_1", "done")],
            },
        ];
        let anthropic = of(Provider::Anthropic)
            .payload(
                &of(Provider::Anthropic).spoken(&turns, None).unwrap(),
                &one_tool(),
                None,
            )
            .write();
        assert!(anthropic.contains(r#""signature":"sig-1""#), "{anthropic}");
        let openai = of(Provider::OpenAi)
            .payload(
                &of(Provider::OpenAi).spoken(&turns, None).unwrap(),
                &one_tool(),
                None,
            )
            .write();
        assert!(!openai.contains("sig-1"), "{openai}");
        assert!(openai.contains(r#""type":"function_call""#), "{openai}");
    }

    #[test]
    fn a_conversation_ending_in_results_is_still_a_question() {
        let connection = of(Provider::Ollama);
        let answered = [
            Turn::person("read it"),
            Turn::Model {
                text: String::new(),
                calls: vec![ToolCall {
                    id: "c1".into(),
                    name: "read_text".into(),
                    arguments: Json::object([]),
                }],
                raw: None,
            },
            Turn::Results {
                results: vec![ToolResult::said("c1", "page one")],
            },
        ];
        assert!(connection.spoken(&answered, None).is_ok());
        assert!(connection.spoken(&answered[..2], None).is_err());
        assert!(
            connection
                .spoken(
                    &[Turn::person("x"), Turn::Results { results: vec![] }],
                    None
                )
                .is_err()
        );
    }

    #[test]
    fn calls_nobody_answered_are_settled() {
        let call = |id: &str| ToolCall {
            id: id.into(),
            name: "read_text".into(),
            arguments: Json::object([]),
        };
        let mut half = vec![
            Turn::person("read them"),
            Turn::Model {
                text: String::new(),
                calls: vec![call("c1"), call("c2")],
                raw: None,
            },
            Turn::Results {
                results: vec![ToolResult::said("c1", "page one")],
            },
        ];
        settle_dangling_calls(&mut half);
        assert_eq!(half.len(), 3, "{half:?}");
        let Turn::Results { results } = &half[2] else {
            panic!("the results stay one round: {half:?}");
        };
        assert_eq!(results.len(), 2);
        assert_eq!(results[1].call_id, "c2");
        assert!(results[1].is_error);
        assert!(results[1].text.contains("Do not retry"));

        let mut none = vec![
            Turn::person("read them"),
            Turn::Model {
                text: String::new(),
                calls: vec![call("c1")],
                raw: None,
            },
        ];
        settle_dangling_calls(&mut none);
        assert_eq!(none.len(), 3);
        assert!(matches!(none[2], Turn::Results { .. }));

        let mut whole = vec![
            Turn::person("read them"),
            Turn::Model {
                text: String::new(),
                calls: vec![call("c1")],
                raw: None,
            },
            Turn::Results {
                results: vec![ToolResult::said("c1", "page one")],
            },
        ];
        let before = whole.clone();
        settle_dangling_calls(&mut whole);
        assert_eq!(whole, before);
    }

    #[test]
    fn a_turn_of_results_is_the_persons_side_and_has_no_words() {
        let results = Turn::Results {
            results: vec![ToolResult::said("c1", "page one")],
        };
        assert_eq!(results.said(), Said::Person);
        assert_eq!(results.text(), "");
        assert_eq!(Turn::person("hi").said(), Said::Person);
        assert_eq!(Turn::model("hello").said(), Said::Model);
        assert_eq!(Turn::model("hello").text(), "hello");
        assert!(Turn::model("hello").calls().is_empty());
    }

    #[test]
    fn a_local_server_asking_for_a_tool_is_read_end_to_end() {
        use std::net::TcpListener;
        for arguments in [r#""{\"block\":\"p2-b3\"}""#, r#"{"block":"p2-b3"}"#] {
            let body = format!(
                r#"{{"choices":[{{"finish_reason":"tool_calls","message":{{"role":"assistant","content":"","reasoning":"I should read it.","tool_calls":[{{"id":"call_1","index":0,"type":"function","function":{{"name":"replace_text","arguments":{arguments}}}}}]}}}}]}}"#
            );
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 8192];
                let _ = stream.read(&mut request).unwrap();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            });
            let connection = Connection {
                provider: Provider::Ollama,
                base_url: format!("http://127.0.0.1:{}/v1", address.port()),
                model: "local-model".into(),
                api_key: String::new(),
                effort: Effort::Low,
            };
            let reply = connection
                .converse_with(
                    &[Turn::person("change it")],
                    None,
                    &one_tool(),
                    Some("you are helping"),
                    &AtomicBool::new(false),
                )
                .unwrap();
            server.join().unwrap();
            assert_eq!(reply.calls.len(), 1, "{reply:?}");
            assert_eq!(reply.calls[0].id, "call_1");
            assert_eq!(
                reply.calls[0].arguments.get("block").and_then(Json::as_str),
                Some("p2-b3"),
                "{arguments}"
            );
            assert!(reply.text.is_empty());
            assert!(reply.raw.is_some(), "the model's own message is kept");
        }
    }

    #[test]
    #[ignore = "needs Ollama running on this machine"]
    fn a_real_local_model_asks_for_a_tool() {
        let connection = Connection {
            provider: Provider::Ollama,
            base_url: "http://127.0.0.1:11434/v1".into(),
            model: "qwen3.5:latest".into(),
            api_key: String::new(),
            effort: Effort::Low,
        };
        let tools = crate::tools::offered_to_a_window();
        let reply = connection
            .converse_with(
                &[Turn::person("Read page 1 of the document.")],
                None,
                &tools,
                Some(&crate::tools::window_instructions(
                    &crate::tools::DocumentBrief {
                        file_name: "report.pdf".into(),
                        title: "Report".into(),
                        pages: 3,
                    },
                )),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(
            reply.calls.first().map(|call| call.name.as_str()),
            Some("read_text"),
            "{reply:?}"
        );
    }

    fn streamed(provider: Provider, body: &str) -> (Reply, Vec<String>) {
        let mut gathering = Gathering::new(provider.wire());
        let mut partials = Vec::new();
        for line in body.lines() {
            if gathering.line(line)
                && !gathering.said().is_empty()
                && partials.last().map(String::as_str) != Some(gathering.said())
            {
                partials.push(gathering.said().to_owned());
            }
        }
        let whole = gathering.whole().expect("the events rebuild an answer");
        (answer(provider, &whole).expect("an answer"), partials)
    }

    #[test]
    fn asking_for_no_thinking_is_not_the_same_as_saying_nothing() {
        let (chat, _) = reasoning_members(Provider::Ollama, "qwen3.5:latest", Effort::None);
        assert_eq!(
            chat,
            vec![("reasoning_effort", Json::text("none"))],
            "{chat:?}"
        );
        let (claude, _) = reasoning_members(Provider::Anthropic, "claude-opus-5", Effort::None);
        assert_eq!(
            claude,
            vec![("thinking", Json::object([("type", Json::text("disabled"))]))],
            "{claude:?}"
        );
        let (openai, _) = reasoning_members(Provider::OpenAi, "gpt-5", Effort::None);
        assert_eq!(
            openai,
            vec![(
                "reasoning",
                Json::object([("effort", Json::text("minimal"))])
            )],
            "{openai:?}"
        );
        for provider in [Provider::Ollama, Provider::Anthropic, Provider::OpenAi] {
            let (nothing, _) = reasoning_members(provider, "a-model", Effort::Off);
            assert!(nothing.is_empty(), "{provider:?} {nothing:?}");
        }
        assert_eq!(Effort::parse(Effort::None.as_str()), Some(Effort::None));
    }

    #[test]
    fn a_model_that_is_still_thinking_says_so_and_says_nothing_else() {
        let each = [
            (
                Provider::Anthropic,
                vec![
                    r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
                    r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"first I"}}"#,
                    r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":" read it"}}"#,
                ],
            ),
            (
                Provider::OpenAi,
                vec![
                    r#"data: {"type":"response.reasoning_summary_text.delta","delta":"first I"}"#,
                    r#"data: {"type":"response.reasoning_summary_text.delta","delta":" read it"}"#,
                ],
            ),
            (
                Provider::Ollama,
                vec![
                    r#"data: {"choices":[{"delta":{"content":"","reasoning":"first I"}}]}"#,
                    r#"data: {"choices":[{"delta":{"content":"","reasoning":" read it"}}]}"#,
                ],
            ),
        ];
        for (provider, body) in each {
            let mut gathering = Gathering::new(provider.wire());
            let mut grew = 0;
            for line in &body {
                if gathering.line(line) {
                    grew += 1;
                }
            }
            assert_eq!(gathering.thinking(), "first I read it", "{provider:?}");
            assert!(gathering.said().is_empty(), "{provider:?}");
            assert_eq!(grew, 2, "{provider:?}: every piece of thinking is progress");
        }
    }

    #[test]
    fn a_streamed_answer_is_the_same_answer_as_a_whole_one() {
        let stream = "\
event: message_start
data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"role\":\"assistant\",\"content\":[]}}

event: content_block_start
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}

data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"I should read it.\"}}

data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig-abc\"}}

data: {\"type\":\"content_block_stop\",\"index\":0}

data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}

data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"Reading \"}}

data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"page one.\"}}

data: {\"type\":\"content_block_stop\",\"index\":1}

data: {\"type\":\"content_block_start\",\"index\":2,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"read_text\",\"input\":{}}}

data: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"page\\\":\"}}

data: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"1}\"}}

data: {\"type\":\"content_block_stop\",\"index\":2}

data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"}}

data: {\"type\":\"message_stop\"}
";
        let whole = r#"{"content":[{"type":"thinking","thinking":"I should read it.","signature":"sig-abc"},{"type":"text","text":"Reading page one."},{"type":"tool_use","id":"toolu_1","name":"read_text","input":{"page":1}}],"stop_reason":"tool_use"}"#;
        let (reply, partials) = streamed(Provider::Anthropic, stream);
        let at_once = answer(Provider::Anthropic, &Json::parse(whole).unwrap()).unwrap();
        assert_eq!(reply, at_once, "{reply:?}");
        assert_eq!(partials, vec!["Reading ", "Reading page one."]);
        assert_eq!(reply.calls[0].arguments.write(), r#"{"page":1}"#);

        let items = r#"{"type":"reasoning","id":"rs_1","encrypted_content":"abc"},{"type":"message","content":[{"type":"output_text","text":"Reading page one."}]},{"type":"function_call","call_id":"call_1","name":"read_text","arguments":"{\"page\":1}"}"#;
        let stream = "\
data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"reasoning\",\"id\":\"rs_1\",\"encrypted_content\":\"abc\"}}
data: {\"type\":\"response.output_text.delta\",\"delta\":\"Reading \"}
data: {\"type\":\"response.output_text.delta\",\"delta\":\"page one.\"}
data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"Reading page one.\"}]}}
data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"read_text\",\"arguments\":\"{\\\"page\\\":1}\"}}
";
        let whole = format!(r#"{{"status":"completed","output":[{items}]}}"#);
        let at_once = answer(Provider::OpenAi, &Json::parse(&whole).unwrap()).unwrap();
        let (reply, partials) = streamed(Provider::OpenAi, stream);
        assert_eq!(reply, at_once, "{reply:?}");
        assert_eq!(partials, vec!["Reading ", "Reading page one."]);
        let finished = format!(
            "{stream}data: {{\"type\":\"response.completed\",\"response\":{whole}}}\ndata: [DONE]\n"
        );
        let (reply, _) = streamed(Provider::OpenAi, &finished);
        assert_eq!(reply, at_once, "{reply:?}");

        let stream = "\
data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}]}
data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning\":\"I should read it.\"}}]}
data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Reading \"}}]}
data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"page one.\"}}]}
data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"read_text\",\"arguments\":\"{\\\"page\\\":\"}}]}}]}
data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"1}\"}}]}}]}
data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}
data: [DONE]
";
        let whole = r#"{"choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":"Reading page one.","reasoning":"I should read it.","tool_calls":[{"id":"call_1","type":"function","function":{"name":"read_text","arguments":"{\"page\":1}"}}]}}]}"#;
        let (reply, partials) = streamed(Provider::Ollama, stream);
        let at_once = answer(Provider::Ollama, &Json::parse(whole).unwrap()).unwrap();
        assert_eq!(reply, at_once, "{reply:?}");
        assert_eq!(partials, vec!["Reading ", "Reading page one."]);
        assert_eq!(reply.calls[0].name, "read_text");
    }

    #[test]
    fn a_tool_call_that_arrived_signed_is_sent_back_signed() {
        let stream = "\
data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"extra_content\":{\"google\":{\"thought_signature\":\"sig-xyz\"}},\"function\":{\"name\":\"read_text\",\"arguments\":\"{\\\"first\\\":1,\\\"last\\\":1}\"}}]}}]}
data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}
data: [DONE]
";
        let (reply, _) = streamed(Provider::Gemini, stream);
        let raw = reply.raw.expect("the model's own message");
        let signed = raw.items[0]
            .get("tool_calls")
            .and_then(Json::as_list)
            .and_then(<[Json]>::first)
            .and_then(|call| call.get("extra_content"))
            .map(Json::write);
        assert_eq!(
            signed.as_deref(),
            Some(r#"{"google":{"thought_signature":"sig-xyz"}}"#),
            "the signature Google sent is in the message sent back"
        );
        assert_eq!(reply.calls[0].name, "read_text");
        assert_eq!(reply.calls[0].id, "call_1");

        let stream = "\
data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"ok\"}}]}
data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"extra_content\":{\"google\":{\"thought_signature\":\"sig-plain\"}}},\"finish_reason\":\"stop\"}]}
data: [DONE]
";
        let (reply, _) = streamed(Provider::Gemini, stream);
        let raw = reply.raw.expect("the model's own message");
        assert_eq!(
            raw.items[0]
                .get("extra_content")
                .map(Json::write)
                .as_deref(),
            Some(r#"{"google":{"thought_signature":"sig-plain"}}"#),
            "an answer with no tool call keeps its signature too"
        );
        assert_eq!(reply.text, "ok");
    }

    #[test]
    fn a_provider_that_says_how_long_to_wait_is_read() {
        let google = r#"{"error":{"code":429,"message":"You exceeded your current quota. \n* Quota exceeded for metric: generate_content_free_tier_requests, limit: 20, model: gemini-3.6-flash\nPlease retry in 18.4395459245s.","status":"RESOURCE_EXHAUSTED"}}"#;
        let groq = "Rate limit reached for model `qwen/qwen3.8-27b` in organization `org_1` \
                    service tier `on_demand` on input tokens per minute (ITPM): Limit 7000, \
                    Used 6026, Requested 4769. Please try again in 32.5285714286s.";
        assert_eq!(
            wait_asked_for(google),
            Some(Duration::from_secs_f64(19.439_545_924_5))
        );
        assert_eq!(
            wait_asked_for(groq),
            Some(Duration::from_secs_f64(33.528_571_428_6))
        );
        assert_eq!(
            wait_asked_for(r#"{"error":{"message":"invalid api key"}}"#),
            None
        );
        assert_eq!(
            wait_asked_for("The model refused to answer in 3 sentences"),
            None,
            "a refusal that happens to say \u{201c}in 3 s\u{201d} is still a refusal"
        );
        let hour = "Rate limit reached. Please try again in 3600s.";
        assert!(wait_asked_for(hour).is_some_and(|wait| wait > MOST_WAIT));
    }

    #[test]
    fn an_overloaded_provider_is_asked_again() {
        let cancel = AtomicBool::new(false);
        let quick = [Duration::from_millis(20); TRIES];
        let busy = "This model is currently experiencing high demand. Please try again later.";
        let mut tries = 0;
        let answer = again_after(
            &cancel,
            || {
                tries += 1;
                if tries < 3 {
                    Err(ConnectError::Http(busy.to_owned()))
                } else {
                    Ok("the answer".to_owned())
                }
            },
            quick,
        );
        assert_eq!(answer.unwrap(), "the answer");
        assert_eq!(tries, 3);
        let mut tries = 0;
        let answer = again_after(
            &cancel,
            || {
                tries += 1;
                Err::<String, _>(ConnectError::Http(
                    "You exceeded your current quota. Please try again later.".to_owned(),
                ))
            },
            quick,
        );
        assert!(answer.is_err());
        assert_eq!(tries, 1);
        let mut tries = 0;
        let answer = again_after(
            &cancel,
            || {
                tries += 1;
                Err::<String, _>(ConnectError::Http(busy.to_owned()))
            },
            quick,
        );
        assert!(answer.is_err());
        assert_eq!(tries, TRIES + 1, "asked again three times, then told");
        assert!(!overloaded_for_now("invalid api key"));
        assert_eq!(
            error_text(r#"[{"error": {"code": 503, "message": "busy", "status": "UNAVAILABLE"}}]"#),
            "busy"
        );
    }

    #[test]
    fn a_request_told_to_wait_is_made_again() {
        let cancel = AtomicBool::new(false);
        let mut tries = 0;
        let answer = again_after_a_wait(&cancel, || {
            tries += 1;
            if tries < 3 {
                Err(ConnectError::Http(
                    "Rate limit reached. Please try again in 0.05s.".to_owned(),
                ))
            } else {
                Ok("the answer".to_owned())
            }
        });
        assert_eq!(answer.unwrap(), "the answer");
        assert_eq!(tries, 3);

        let mut tries = 0;
        let answer = again_after_a_wait(&cancel, || {
            tries += 1;
            Err::<String, _>(ConnectError::Http("invalid api key".to_owned()))
        });
        assert!(matches!(answer, Err(ConnectError::Http(_))));
        assert_eq!(tries, 1, "a refusal is not tried again");

        let stopped = AtomicBool::new(true);
        let mut tries = 0;
        let answer = again_after_a_wait(&stopped, || {
            tries += 1;
            Err::<String, _>(ConnectError::Http(
                "Rate limit reached. Please try again in 0.05s.".to_owned(),
            ))
        });
        assert!(matches!(answer, Err(ConnectError::Http(_))));
        assert_eq!(tries, 1);
    }

    #[test]
    fn a_local_server_that_streams_is_seen_as_it_writes() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 8192];
            let count = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..count]);
            assert!(request.contains(r#""stream":true"#), "{request}");
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            stream.flush().unwrap();
            for piece in ["Reading ", "page ", "one."] {
                write!(
                    stream,
                    "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{piece}\"}}}}]}}\n\n"
                )
                .unwrap();
                stream.flush().unwrap();
                thread::sleep(Duration::from_millis(60));
            }
            write!(
                stream,
                "data: {{\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":\"stop\"}}]}}\n\ndata: [DONE]\n\n"
            )
            .unwrap();
            stream.flush().unwrap();
        });
        let connection = Connection {
            provider: Provider::Ollama,
            base_url: format!("http://127.0.0.1:{}/v1", address.port()),
            model: "local-model".into(),
            api_key: String::new(),
            effort: Effort::Off,
        };
        let mut partials: Vec<String> = Vec::new();
        let reply = connection
            .converse_streaming(
                &[Turn::person("read it")],
                None,
                &[],
                None,
                &AtomicBool::new(false),
                &mut |far| partials.push(far.said.to_owned()),
            )
            .unwrap();
        server.join().unwrap();
        assert_eq!(reply.text, "Reading page one.");
        assert!(!reply.cut_short);
        assert_eq!(
            partials,
            vec!["Reading ", "Reading page ", "Reading page one."],
            "the answer is seen growing"
        );
    }

    #[test]
    fn a_server_that_does_not_stream_is_read_as_one_body() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 8192];
            let _ = stream.read(&mut request).unwrap();
            let body = r#"{"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"all at once"}}]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let connection = Connection {
            provider: Provider::Ollama,
            base_url: format!("http://127.0.0.1:{}/v1", address.port()),
            model: "local-model".into(),
            api_key: String::new(),
            effort: Effort::Off,
        };
        let mut partials = 0_usize;
        let reply = connection
            .converse_streaming(
                &[Turn::person("hello")],
                None,
                &[],
                None,
                &AtomicBool::new(false),
                &mut |_| partials += 1,
            )
            .unwrap();
        server.join().unwrap();
        assert_eq!(reply.text, "all at once");
        assert_eq!(partials, 0);
    }

    #[test]
    #[ignore = "needs Ollama running on this machine"]
    fn a_real_local_model_streams_its_answer() {
        let connection = Connection {
            provider: Provider::Ollama,
            base_url: "http://127.0.0.1:11434/v1".into(),
            model: "qwen3.5:latest".into(),
            api_key: String::new(),
            effort: Effort::Low,
        };
        let mut partials: Vec<String> = Vec::new();
        let reply = connection
            .converse_streaming(
                &[Turn::person("Read page 1 of the document.")],
                None,
                &crate::tools::offered_to_a_window(),
                Some(&crate::tools::window_instructions(
                    &crate::tools::DocumentBrief {
                        file_name: "report.pdf".into(),
                        title: "Report".into(),
                        pages: 3,
                    },
                )),
                &AtomicBool::new(false),
                &mut |far| partials.push(far.said.to_owned()),
            )
            .unwrap();
        println!("partials seen: {}", partials.len());
        println!("reply: {reply:?}");
        assert!(!reply.calls.is_empty() || !reply.text.is_empty());
    }
}
