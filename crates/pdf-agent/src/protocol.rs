use crate::desk::Desk;
use crate::json::Json;

pub const MODERN: &str = "2026-07-28";

pub const LEGACY: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const VERSION_KEY: &str = "io.modelcontextprotocol/protocolVersion";

#[derive(Default)]
pub struct Server {
    desk: Desk,
}

impl Server {
    #[must_use]
    pub const fn with_desk(desk: Desk) -> Self {
        Self { desk }
    }

    #[must_use]
    pub fn answer_line(&mut self, line: &str) -> Option<String> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        match Json::parse(line) {
            Ok(message) => self.answer(&message).map(|reply| reply.write()),
            Err(error) => Some(failure(&Json::Null, -32700, &format!("not JSON: {error}")).write()),
        }
    }

    pub fn answer(&mut self, message: &Json) -> Option<Json> {
        let Json::Object(members) = message else {
            return Some(failure(&Json::Null, -32600, "a message is one JSON object"));
        };
        let id = members.get("id").cloned();
        let Some(method) = members.get("method").and_then(Json::as_str) else {
            return id.map(|id| failure(&id, -32600, "a request needs a method"));
        };
        let id = id?;
        if !matches!(id, Json::Text(_) | Json::Number(_)) {
            return Some(failure(
                &Json::Null,
                -32600,
                "an id is a string or a number",
            ));
        }
        let params = members.get("params").cloned().unwrap_or(Json::Null);
        let asked = params
            .get("_meta")
            .and_then(|meta| meta.get(VERSION_KEY))
            .and_then(Json::as_str);
        let modern = asked.is_some();
        if let Some(asked) = asked
            && asked != MODERN
            && !LEGACY.contains(&asked)
        {
            return Some(Json::object([
                ("jsonrpc", Json::text("2.0")),
                ("id", id),
                (
                    "error",
                    Json::object([
                        ("code", Json::Number(-32022.0)),
                        ("message", Json::text("Unsupported protocol version")),
                        (
                            "data",
                            Json::object([
                                ("supported", supported()),
                                ("requested", Json::text(asked)),
                            ]),
                        ),
                    ]),
                ),
            ]));
        }
        let result = match method {
            "initialize" => Ok(initialized(&params)),
            "ping" => Ok(Json::object([])),
            "server/discover" => Ok(discovered()),
            "tools/list" => Ok(Json::object([("tools", crate::tools::listed())])),
            "tools/call" => self.call(&params),
            other => Err((-32601, format!("no method {other}"))),
        };
        Some(match result {
            Ok(mut result) => {
                if modern && let Json::Object(members) = &mut result {
                    members.insert("resultType".to_owned(), Json::text("complete"));
                }
                Json::object([
                    ("jsonrpc", Json::text("2.0")),
                    ("id", id),
                    ("result", result),
                ])
            }
            Err((code, message)) => failure(&id, code, &message),
        })
    }

    fn call(&mut self, params: &Json) -> Result<Json, (i32, String)> {
        let name = params
            .get("name")
            .and_then(Json::as_str)
            .ok_or((-32602, "tools/call needs a tool name".to_owned()))?;
        if !crate::tools::exists(name) {
            return Err((-32602, format!("Unknown tool: {name}")));
        }
        let arguments = params.get("arguments").cloned().unwrap_or(Json::Null);
        if !matches!(arguments, Json::Object(_) | Json::Null) {
            return Err((-32602, "arguments are an object".to_owned()));
        }
        Ok(match crate::tools::call(&mut self.desk, name, &arguments) {
            Ok(answer) => {
                let mut content = vec![Json::object([
                    ("type", Json::text("text")),
                    ("text", Json::text(answer.text)),
                ])];
                if let Some(png) = answer.picture {
                    content.push(Json::object([
                        ("type", Json::text("image")),
                        ("data", Json::text(crate::json::base64(&png))),
                        ("mimeType", Json::text("image/png")),
                    ]));
                }
                let mut result = vec![
                    ("content", Json::List(content)),
                    ("isError", Json::Bool(false)),
                ];
                match answer.data {
                    Json::Null => {}
                    data @ Json::Object(_) => result.push(("structuredContent", data)),
                    data => result.push(("structuredContent", Json::object([("result", data)]))),
                }
                Json::Object(
                    result
                        .into_iter()
                        .map(|(key, value)| (key.to_owned(), value))
                        .collect(),
                )
            }
            Err(why) => Json::object([
                (
                    "content",
                    Json::List(vec![Json::object([
                        ("type", Json::text("text")),
                        ("text", Json::text(why)),
                    ])]),
                ),
                ("isError", Json::Bool(true)),
            ]),
        })
    }
}

fn supported() -> Json {
    Json::List(
        std::iter::once(MODERN)
            .chain(LEGACY)
            .map(Json::text)
            .collect(),
    )
}

fn identity() -> Json {
    Json::object([
        ("name", Json::text("panpdf")),
        ("title", Json::text("PanPDF")),
        ("version", Json::text(env!("CARGO_PKG_VERSION"))),
    ])
}

fn capabilities() -> Json {
    Json::object([("tools", Json::object([("listChanged", Json::Bool(false))]))])
}

fn initialized(params: &Json) -> Json {
    let asked = params.get("protocolVersion").and_then(Json::as_str);
    let version = asked
        .filter(|asked| LEGACY.contains(asked))
        .unwrap_or(LEGACY[0]);
    Json::object([
        ("protocolVersion", Json::text(version)),
        ("capabilities", capabilities()),
        ("serverInfo", identity()),
        ("instructions", Json::text(crate::tools::INSTRUCTIONS)),
    ])
}

fn discovered() -> Json {
    Json::object([
        ("supportedVersions", supported()),
        ("capabilities", capabilities()),
        (
            "_meta",
            Json::object([("io.modelcontextprotocol/serverInfo", identity())]),
        ),
        ("instructions", Json::text(crate::tools::INSTRUCTIONS)),
    ])
}

fn failure(id: &Json, code: i32, message: &str) -> Json {
    Json::object([
        ("jsonrpc", Json::text("2.0")),
        ("id", id.clone()),
        (
            "error",
            Json::object([
                ("code", Json::Number(f64::from(code))),
                ("message", Json::text(message)),
            ]),
        ),
    ])
}

#[cfg(test)]
mod tests;
