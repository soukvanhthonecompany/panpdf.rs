use std::collections::BTreeMap;
use std::fmt::Write as _;

pub const MOST_BYTES: usize = 16 * 1024 * 1024;

pub const MOST_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(f64),
    Text(String),
    List(Vec<Json>),
    Object(BTreeMap<String, Json>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JsonError {
    pub at: usize,
    pub reason: &'static str,
}

impl std::fmt::Display for JsonError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} at byte {}", self.reason, self.at)
    }
}

impl Json {
    pub fn parse(text: &str) -> Result<Self, JsonError> {
        if text.len() > MOST_BYTES {
            return Err(JsonError {
                at: MOST_BYTES,
                reason: "message too long",
            });
        }
        let mut reader = Reader {
            bytes: text.as_bytes(),
            at: 0,
        };
        let value = reader.value(0)?;
        reader.space();
        if reader.at != reader.bytes.len() {
            return Err(reader.fail("more after the value"));
        }
        Ok(value)
    }

    #[must_use]
    pub fn object<const N: usize>(pairs: [(&str, Self); N]) -> Self {
        Self::Object(
            pairs
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value))
                .collect(),
        )
    }

    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(text.into())
    }

    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        reason = "counts and page numbers are far below 2^53"
    )]
    pub const fn count(number: usize) -> Self {
        Self::Number(number as f64)
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Object(members) => members.get(key),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Number(number) => Some(*number),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_list(&self) -> Option<&[Self]> {
        match self {
            Self::List(items) => Some(items),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_count(&self) -> Option<usize> {
        let number = self.as_f64()?;
        if number < 0.0 || number.fract() != 0.0 || number > 9_007_199_254_740_991.0 {
            return None;
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "checked whole, not negative and within 2^53 above"
        )]
        usize::try_from(number as u64).ok()
    }

    #[must_use]
    pub fn write(&self) -> String {
        let mut out = String::new();
        self.write_into(&mut out);
        out
    }

    fn write_into(&self, out: &mut String) {
        match self {
            Self::Null => out.push_str("null"),
            Self::Bool(true) => out.push_str("true"),
            Self::Bool(false) => out.push_str("false"),
            Self::Number(number) => write_number(*number, out),
            Self::Text(text) => write_text(text, out),
            Self::List(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    item.write_into(out);
                }
                out.push(']');
            }
            Self::Object(members) => {
                out.push('{');
                for (index, (key, value)) in members.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    write_text(key, out);
                    out.push(':');
                    value.write_into(out);
                }
                out.push('}');
            }
        }
    }
}

fn write_number(number: f64, out: &mut String) {
    if !number.is_finite() {
        out.push_str("null");
    } else if number.fract() == 0.0 && number.abs() < 9_007_199_254_740_992.0 {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "whole and within 2^53, checked above"
        )]
        let whole = number as i64;
        let _ = write!(out, "{whole}");
    } else {
        let _ = write!(out, "{number}");
    }
}

fn write_text(text: &str, out: &mut String) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{2028}' | '\u{2029}' => {
                let _ = write!(out, "\\u{:04x}", u32::from(character));
            }
            control if u32::from(control) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(control));
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    const fn fail(&self, reason: &'static str) -> JsonError {
        JsonError {
            at: self.at,
            reason,
        }
    }

    fn space(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.bytes.get(self.at) {
            self.at += 1;
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, JsonError> {
        if depth > MOST_DEPTH {
            return Err(self.fail("nested too deeply"));
        }
        self.space();
        match self.bytes.get(self.at) {
            None => Err(self.fail("a value was expected")),
            Some(b'{') => self.object(depth),
            Some(b'[') => self.list(depth),
            Some(b'"') => self.string().map(Json::Text),
            Some(b't') => self.word("true", Json::Bool(true)),
            Some(b'f') => self.word("false", Json::Bool(false)),
            Some(b'n') => self.word("null", Json::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.fail("not the start of a value")),
        }
    }

    fn word(&mut self, word: &str, value: Json) -> Result<Json, JsonError> {
        if self.bytes[self.at..].starts_with(word.as_bytes()) {
            self.at += word.len();
            Ok(value)
        } else {
            Err(self.fail("not a value"))
        }
    }

    fn object(&mut self, depth: usize) -> Result<Json, JsonError> {
        self.at += 1;
        let mut members = BTreeMap::new();
        self.space();
        if self.bytes.get(self.at) == Some(&b'}') {
            self.at += 1;
            return Ok(Json::Object(members));
        }
        loop {
            self.space();
            if self.bytes.get(self.at) != Some(&b'"') {
                return Err(self.fail("a member name was expected"));
            }
            let key = self.string()?;
            self.space();
            if self.bytes.get(self.at) != Some(&b':') {
                return Err(self.fail("':' was expected"));
            }
            self.at += 1;
            let value = self.value(depth + 1)?;
            members.insert(key, value);
            self.space();
            match self.bytes.get(self.at) {
                Some(b',') => self.at += 1,
                Some(b'}') => {
                    self.at += 1;
                    return Ok(Json::Object(members));
                }
                _ => return Err(self.fail("',' or '}' was expected")),
            }
        }
    }

    fn list(&mut self, depth: usize) -> Result<Json, JsonError> {
        self.at += 1;
        let mut items = Vec::new();
        self.space();
        if self.bytes.get(self.at) == Some(&b']') {
            self.at += 1;
            return Ok(Json::List(items));
        }
        loop {
            items.push(self.value(depth + 1)?);
            self.space();
            match self.bytes.get(self.at) {
                Some(b',') => self.at += 1,
                Some(b']') => {
                    self.at += 1;
                    return Ok(Json::List(items));
                }
                _ => return Err(self.fail("',' or ']' was expected")),
            }
        }
    }

    fn number(&mut self) -> Result<Json, JsonError> {
        let start = self.at;
        let digits = |reader: &mut Self| {
            let from = reader.at;
            while reader.bytes.get(reader.at).is_some_and(u8::is_ascii_digit) {
                reader.at += 1;
            }
            reader.at > from
        };
        if self.bytes.get(self.at) == Some(&b'-') {
            self.at += 1;
        }
        if self.bytes.get(self.at) == Some(&b'0') {
            self.at += 1;
            if self.bytes.get(self.at).is_some_and(u8::is_ascii_digit) {
                return Err(self.fail("a number with a leading zero"));
            }
        } else if !digits(self) {
            return Err(self.fail("digits were expected"));
        }
        if self.bytes.get(self.at) == Some(&b'.') {
            self.at += 1;
            if !digits(self) {
                return Err(self.fail("digits were expected after '.'"));
            }
        }
        if let Some(b'e' | b'E') = self.bytes.get(self.at) {
            self.at += 1;
            if let Some(b'+' | b'-') = self.bytes.get(self.at) {
                self.at += 1;
            }
            if !digits(self) {
                return Err(self.fail("digits were expected in the exponent"));
            }
        }
        let text = std::str::from_utf8(&self.bytes[start..self.at])
            .map_err(|_| self.fail("not a number"))?;
        let number: f64 = text.parse().map_err(|_| self.fail("not a number"))?;
        if number.is_finite() {
            Ok(Json::Number(number))
        } else {
            Err(self.fail("a number too large to hold"))
        }
    }

    fn string(&mut self) -> Result<String, JsonError> {
        self.at += 1;
        let mut out = String::new();
        loop {
            let from = self.at;
            while let Some(&byte) = self.bytes.get(self.at) {
                if byte == b'"' || byte == b'\\' || byte < 0x20 {
                    break;
                }
                self.at += 1;
            }
            out.push_str(
                std::str::from_utf8(&self.bytes[from..self.at])
                    .map_err(|_| self.fail("not text"))?,
            );
            match self.bytes.get(self.at) {
                None => return Err(self.fail("the string does not end")),
                Some(b'"') => {
                    self.at += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.at += 1;
                    let escaped = match self.bytes.get(self.at) {
                        Some(b'"') => '"',
                        Some(b'\\') => '\\',
                        Some(b'/') => '/',
                        Some(b'b') => '\u{8}',
                        Some(b'f') => '\u{c}',
                        Some(b'n') => '\n',
                        Some(b'r') => '\r',
                        Some(b't') => '\t',
                        Some(b'u') => {
                            self.at += 1;
                            let first = self.four()?;
                            let code = if (0xD800..0xDC00).contains(&first) {
                                if !self.bytes[self.at..].starts_with(b"\\u") {
                                    return Err(self.fail("half of a surrogate pair"));
                                }
                                self.at += 2;
                                let second = self.four()?;
                                if !(0xDC00..0xE000).contains(&second) {
                                    return Err(self.fail("half of a surrogate pair"));
                                }
                                0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00)
                            } else if (0xDC00..0xE000).contains(&first) {
                                return Err(self.fail("half of a surrogate pair"));
                            } else {
                                first
                            };
                            out.push(
                                char::from_u32(code).ok_or_else(|| self.fail("not a character"))?,
                            );
                            continue;
                        }
                        _ => return Err(self.fail("not an escape")),
                    };
                    out.push(escaped);
                    self.at += 1;
                }
                Some(_) => return Err(self.fail("a control character inside a string")),
            }
        }
    }

    fn four(&mut self) -> Result<u32, JsonError> {
        let hex = self
            .bytes
            .get(self.at..self.at + 4)
            .ok_or_else(|| self.fail("four hex digits were expected"))?;
        let text = std::str::from_utf8(hex).map_err(|_| self.fail("not hex"))?;
        let code = u32::from_str_radix(text, 16).map_err(|_| self.fail("not hex"))?;
        if !hex.iter().all(u8::is_ascii_hexdigit) {
            return Err(self.fail("not hex"));
        }
        self.at += 4;
        Ok(code)
    }
}

#[must_use]
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let joined = chunk.iter().enumerate().fold(0_u32, |held, (index, byte)| {
            held | (u32::from(*byte) << (16 - 8 * index))
        });
        for place in 0..4 {
            if place <= chunk.len() {
                let six = (joined >> (18 - 6 * place)) & 0x3F;
                out.push(char::from(ALPHABET[six as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;
