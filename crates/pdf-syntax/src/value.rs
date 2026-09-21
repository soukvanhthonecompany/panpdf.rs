use std::collections::VecDeque;
use std::fmt;

use pdf_bytes::{ByteStore, SourceSpan};

use crate::{LexError, LexErrorKind, LexLimits, Lexer, NumberKind, Token, TokenKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParseLimits {
    pub lex: LexLimits,
    pub max_depth: usize,
    pub max_collection_entries: usize,
    pub max_objects: usize,
}

impl Default for ParseLimits {
    fn default() -> Self {
        Self {
            lex: LexLimits::default(),
            max_depth: 128,
            max_collection_entries: 1_000_000,
            max_objects: 10_000_000,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Reference {
    object_number: u32,
    generation: u16,
}

impl Reference {
    #[must_use]
    pub const fn new(object_number: u32, generation: u16) -> Self {
        Self {
            object_number,
            generation,
        }
    }

    #[must_use]
    pub const fn object_number(self) -> u32 {
        self.object_number
    }

    #[must_use]
    pub const fn generation(self) -> u16 {
        self.generation
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DictionaryEntry {
    key: Token,
    value: Object,
}

impl DictionaryEntry {
    #[must_use]
    pub const fn key(&self) -> Token {
        self.key
    }

    #[must_use]
    pub const fn value(&self) -> &Object {
        &self.value
    }

    #[must_use]
    pub fn key_equals(&self, source: &ByteStore, expected: &[u8]) -> bool {
        name_token_equals(source, self.key, expected)
    }

    pub fn decoded_key(&self, source: &ByteStore) -> Result<Vec<u8>, NameDecodeError> {
        decode_name_span(source, self.key.span())
    }
}

pub fn decode_name(source: &ByteStore, object: &Object) -> Result<Vec<u8>, NameDecodeError> {
    if !matches!(object.kind(), ObjectKind::Name) {
        return Err(NameDecodeError::NotName);
    }
    decode_name_span(source, object.span())
}

fn decode_name_span(source: &ByteStore, span: SourceSpan) -> Result<Vec<u8>, NameDecodeError> {
    let raw = source
        .resolve(span)
        .map_err(|_| NameDecodeError::SourceSpanFailure)?;
    let mut decoded = Vec::with_capacity(raw.len());
    let mut cursor = 0_usize;
    while cursor < raw.len() {
        if raw[cursor] == b'#' {
            let first = raw
                .get(cursor + 1)
                .and_then(|byte| hex_value(*byte))
                .ok_or(NameDecodeError::InvalidEscape)?;
            let second = raw
                .get(cursor + 2)
                .and_then(|byte| hex_value(*byte))
                .ok_or(NameDecodeError::InvalidEscape)?;
            decoded.push(first * 16 + second);
            cursor += 3;
        } else {
            decoded.push(raw[cursor]);
            cursor += 1;
        }
    }
    Ok(decoded)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NameDecodeError {
    NotName,
    SourceSpanFailure,
    InvalidEscape,
}

impl fmt::Display for NameDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NotName => "object is not a PDF name",
            Self::SourceSpanFailure => "name source span cannot be resolved",
            Self::InvalidEscape => "PDF name has an invalid hexadecimal escape",
        })
    }
}

impl std::error::Error for NameDecodeError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Object {
    kind: ObjectKind,
    span: SourceSpan,
}

impl Object {
    #[must_use]
    pub const fn kind(&self) -> &ObjectKind {
        &self.kind
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }

    #[must_use]
    pub(crate) const fn null_at(span: SourceSpan) -> Self {
        Self {
            kind: ObjectKind::Null,
            span,
        }
    }

    #[must_use]
    pub fn name_equals(&self, source: &ByteStore, expected: &[u8]) -> bool {
        name_object_equals(source, self, expected)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ObjectKind {
    Null,
    Boolean(bool),
    Number(NumberKind),
    Name,
    LiteralString,
    HexString,
    Reference(Reference),
    Array(Vec<Object>),
    Dictionary(Vec<DictionaryEntry>),
}

pub struct ObjectParser<'a> {
    source: &'a ByteStore,
    lexer: Lexer<'a>,
    lookahead: VecDeque<Token>,
    limits: ParseLimits,
    object_count: usize,
    tolerate_non_name_keys: bool,
    repairs: Vec<crate::IndirectRepair>,
}

impl<'a> ObjectParser<'a> {
    #[must_use]
    pub fn new(source: &'a ByteStore, offset: usize, limits: ParseLimits) -> Self {
        Self {
            source,
            lexer: Lexer::new(source, offset, limits.lex),
            lookahead: VecDeque::with_capacity(3),
            limits,
            object_count: 0,
            tolerate_non_name_keys: false,
            repairs: Vec::new(),
        }
    }

    #[must_use]
    pub(crate) fn tolerating_non_name_keys(mut self) -> Self {
        self.tolerate_non_name_keys = true;
        self
    }

    pub(crate) fn take_repairs(&mut self) -> Vec<crate::IndirectRepair> {
        std::mem::take(&mut self.repairs)
    }

    pub fn parse_next(&mut self) -> Result<Option<Object>, ParseError> {
        if self.peek_token(0)?.is_none() {
            return Ok(None);
        }
        self.parse_object(0).map(Some)
    }

    #[must_use]
    pub fn offset(&self) -> usize {
        self.lookahead
            .front()
            .map_or_else(|| self.lexer.offset(), |token| token.span().start())
    }

    fn parse_object(&mut self, depth: usize) -> Result<Object, ParseError> {
        if depth > self.limits.max_depth {
            return Err(Self::error(self.offset(), ParseErrorKind::NestingLimit));
        }
        if self.object_count >= self.limits.max_objects {
            return Err(Self::error(self.offset(), ParseErrorKind::ObjectCountLimit));
        }
        self.object_count += 1;

        let token = self
            .take_token()?
            .ok_or_else(|| Self::error(self.offset(), ParseErrorKind::UnexpectedEndOfInput))?;

        match token.kind() {
            TokenKind::Null => Ok(Self::scalar(token, ObjectKind::Null)),
            TokenKind::Boolean(value) => Ok(Self::scalar(token, ObjectKind::Boolean(value))),
            TokenKind::Number(kind) => self.parse_number_or_reference(token, kind),
            TokenKind::Name => Ok(Self::scalar(token, ObjectKind::Name)),
            TokenKind::LiteralString => Ok(Self::scalar(token, ObjectKind::LiteralString)),
            TokenKind::HexString => Ok(Self::scalar(token, ObjectKind::HexString)),
            TokenKind::ArrayStart => self.parse_array(token, depth + 1),
            TokenKind::DictionaryStart => self.parse_dictionary(token, depth + 1),
            TokenKind::ArrayEnd | TokenKind::DictionaryEnd => Err(Self::error(
                token.span().start(),
                ParseErrorKind::UnexpectedClosingDelimiter,
            )),
            TokenKind::Keyword => Err(Self::error(
                token.span().start(),
                ParseErrorKind::UnexpectedKeyword,
            )),
        }
    }

    fn parse_number_or_reference(
        &mut self,
        first: Token,
        number_kind: NumberKind,
    ) -> Result<Object, ParseError> {
        if number_kind == NumberKind::Integer
            && self
                .peek_token(0)?
                .is_some_and(|token| token.kind() == TokenKind::Number(NumberKind::Integer))
            && self
                .peek_token(1)?
                .is_some_and(|token| self.token_equals(token, b"R"))
        {
            let second = self
                .take_token()?
                .ok_or_else(|| Self::error(self.offset(), ParseErrorKind::UnexpectedEndOfInput))?;
            let marker = self
                .take_token()?
                .ok_or_else(|| Self::error(self.offset(), ParseErrorKind::UnexpectedEndOfInput))?;
            let object_number = self.parse_u32(first)?;
            let generation = self.parse_u16(second)?;
            let span = self.cover(first.span(), marker.span())?;
            return Ok(Object {
                kind: ObjectKind::Reference(Reference::new(object_number, generation)),
                span,
            });
        }

        Ok(Self::scalar(first, ObjectKind::Number(number_kind)))
    }

    fn parse_array(&mut self, opening: Token, depth: usize) -> Result<Object, ParseError> {
        let mut values = Vec::new();
        loop {
            let next = self.peek_token(0)?.ok_or_else(|| {
                Self::error(opening.span().start(), ParseErrorKind::UnterminatedArray)
            })?;
            if next.kind() == TokenKind::ArrayEnd {
                let closing = self
                    .take_token()?
                    .expect("peeked array delimiter must remain buffered");
                let span = self.cover(opening.span(), closing.span())?;
                return Ok(Object {
                    kind: ObjectKind::Array(values),
                    span,
                });
            }
            if values.len() >= self.limits.max_collection_entries {
                return Err(Self::error(
                    next.span().start(),
                    ParseErrorKind::CollectionLimit,
                ));
            }
            values.push(self.parse_object(depth)?);
        }
    }

    fn parse_dictionary(&mut self, opening: Token, depth: usize) -> Result<Object, ParseError> {
        let mut entries = Vec::new();
        loop {
            let key = self.peek_token(0)?.ok_or_else(|| {
                Self::error(
                    opening.span().start(),
                    ParseErrorKind::UnterminatedDictionary,
                )
            })?;
            if key.kind() == TokenKind::DictionaryEnd {
                let closing = self
                    .take_token()?
                    .expect("peeked dictionary delimiter must remain buffered");
                let span = self.cover(opening.span(), closing.span())?;
                return Ok(Object {
                    kind: ObjectKind::Dictionary(entries),
                    span,
                });
            }
            if key.kind() != TokenKind::Name {
                if !self.tolerate_non_name_keys {
                    return Err(Self::error(
                        key.span().start(),
                        ParseErrorKind::DictionaryKeyNotName,
                    ));
                }
                let skipped = self
                    .take_token()?
                    .expect("peeked dictionary key must remain buffered");
                if self.repairs.len() < self.limits.max_collection_entries {
                    self.repairs
                        .push(crate::IndirectRepair::SkippedDictionaryKey {
                            byte_offset: skipped.span().start(),
                        });
                }
                continue;
            }
            if entries.len() >= self.limits.max_collection_entries {
                return Err(Self::error(
                    key.span().start(),
                    ParseErrorKind::CollectionLimit,
                ));
            }
            let key = self
                .take_token()?
                .expect("peeked dictionary key must remain buffered");
            let value = self.parse_object(depth)?;
            entries.push(DictionaryEntry { key, value });
        }
    }

    fn scalar(token: Token, kind: ObjectKind) -> Object {
        Object {
            kind,
            span: token.span(),
        }
    }

    pub(crate) fn peek_token(&mut self, index: usize) -> Result<Option<Token>, ParseError> {
        while self.lookahead.len() <= index {
            let Some(token) = self.lexer.next_token().map_err(ParseError::from)? else {
                return Ok(None);
            };
            self.lookahead.push_back(token);
        }
        Ok(self.lookahead.get(index).copied())
    }

    pub(crate) fn take_token(&mut self) -> Result<Option<Token>, ParseError> {
        if let Some(token) = self.lookahead.pop_front() {
            return Ok(Some(token));
        }
        self.lexer.next_token().map_err(ParseError::from)
    }

    pub(crate) fn token_equals(&self, token: Token, expected: &[u8]) -> bool {
        self.source.resolve(token.span()) == Ok(expected)
    }

    fn parse_u32(&self, token: Token) -> Result<u32, ParseError> {
        let bytes = self
            .source
            .resolve(token.span())
            .map_err(|_| Self::error(token.span().start(), ParseErrorKind::InvalidReference))?;
        parse_unsigned(bytes)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| Self::error(token.span().start(), ParseErrorKind::InvalidReference))
    }

    fn parse_u16(&self, token: Token) -> Result<u16, ParseError> {
        let bytes = self
            .source
            .resolve(token.span())
            .map_err(|_| Self::error(token.span().start(), ParseErrorKind::InvalidReference))?;
        parse_unsigned(bytes)
            .and_then(|value| u16::try_from(value).ok())
            .ok_or_else(|| Self::error(token.span().start(), ParseErrorKind::InvalidReference))
    }

    fn cover(&self, first: SourceSpan, last: SourceSpan) -> Result<SourceSpan, ParseError> {
        self.source
            .span(first.start()..last.end())
            .map_err(|_| Self::error(first.start(), ParseErrorKind::MismatchedSource))
    }

    const fn error(offset: usize, kind: ParseErrorKind) -> ParseError {
        ParseError { offset, kind }
    }
}

pub(crate) fn parse_unsigned(bytes: &[u8]) -> Option<u64> {
    if bytes.is_empty() || bytes[0] == b'-' {
        return None;
    }
    let digits = bytes.strip_prefix(b"+").unwrap_or(bytes);
    if digits.is_empty() {
        return None;
    }
    digits.iter().try_fold(0_u64, |value, byte| {
        let digit = byte.checked_sub(b'0')?;
        if digit > 9 {
            return None;
        }
        value.checked_mul(10)?.checked_add(u64::from(digit))
    })
}

pub(crate) fn name_object_equals(source: &ByteStore, object: &Object, expected: &[u8]) -> bool {
    if !matches!(object.kind(), ObjectKind::Name) {
        return false;
    }
    source
        .resolve(object.span())
        .is_ok_and(|raw| decoded_name_equals(raw, expected))
}

fn name_token_equals(source: &ByteStore, token: Token, expected: &[u8]) -> bool {
    token.kind() == TokenKind::Name
        && source
            .resolve(token.span())
            .is_ok_and(|raw| decoded_name_equals(raw, expected))
}

fn decoded_name_equals(raw: &[u8], expected: &[u8]) -> bool {
    let mut raw_index = 0;
    let mut expected_index = 0;
    while raw_index < raw.len() && expected_index < expected.len() {
        let actual = if raw[raw_index] == b'#' {
            let Some(first) = raw.get(raw_index + 1).and_then(|byte| hex_value(*byte)) else {
                return false;
            };
            let Some(second) = raw.get(raw_index + 2).and_then(|byte| hex_value(*byte)) else {
                return false;
            };
            raw_index += 3;
            first * 16 + second
        } else {
            let byte = raw[raw_index];
            raw_index += 1;
            byte
        };
        if actual != expected[expected_index] {
            return false;
        }
        expected_index += 1;
    }
    raw_index == raw.len() && expected_index == expected.len()
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParseError {
    offset: usize,
    kind: ParseErrorKind,
}

impl ParseError {
    #[must_use]
    pub const fn offset(self) -> usize {
        self.offset
    }

    #[must_use]
    pub const fn kind(self) -> ParseErrorKind {
        self.kind
    }
}

impl From<LexError> for ParseError {
    fn from(error: LexError) -> Self {
        Self {
            offset: error.offset(),
            kind: ParseErrorKind::Lexical(error.kind()),
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} at byte {}", self.kind, self.offset)
    }
}

impl std::error::Error for ParseError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParseErrorKind {
    Lexical(LexErrorKind),
    UnexpectedEndOfInput,
    UnexpectedClosingDelimiter,
    UnexpectedKeyword,
    UnterminatedArray,
    UnterminatedDictionary,
    DictionaryKeyNotName,
    InvalidReference,
    NestingLimit,
    CollectionLimit,
    ObjectCountLimit,
    MismatchedSource,
}

impl fmt::Display for ParseErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Lexical(kind) => return write!(formatter, "invalid PDF token: {kind}"),
            Self::UnexpectedEndOfInput => "expected a PDF object before end of input",
            Self::UnexpectedClosingDelimiter => "unexpected closing delimiter",
            Self::UnexpectedKeyword => "keyword is not a direct PDF object",
            Self::UnterminatedArray => "unterminated PDF array",
            Self::UnterminatedDictionary => "unterminated PDF dictionary",
            Self::DictionaryKeyNotName => "PDF dictionary key is not a name",
            Self::InvalidReference => "invalid indirect reference",
            Self::NestingLimit => "PDF object nesting limit exceeded",
            Self::CollectionLimit => "PDF collection entry limit exceeded",
            Self::ObjectCountLimit => "PDF object count limit exceeded",
            Self::MismatchedSource => "object spans do not belong to one source",
        };
        formatter.write_str(message)
    }
}

#[cfg(test)]
mod tests {
    use pdf_bytes::{ByteStore, SourceId};

    use crate::LexErrorKind;

    use super::{ObjectKind, ObjectParser, ParseErrorKind, ParseLimits, Reference, decode_name};

    fn source(bytes: &'static [u8]) -> ByteStore {
        ByteStore::new(SourceId::new(4), bytes)
    }

    #[test]
    fn parses_nested_arrays_dictionaries_and_preserves_container_span() {
        let source = source(b"<< /Type /Page /Kids [4 0 R (label)] /Type /Duplicate >>");
        let object = ObjectParser::new(&source, 0, ParseLimits::default())
            .parse_next()
            .expect("valid direct object")
            .expect("one direct object");

        assert_eq!(source.resolve(object.span()), Ok(source.as_bytes()));
        let ObjectKind::Dictionary(entries) = object.kind() else {
            panic!("expected dictionary");
        };
        assert_eq!(entries.len(), 3, "duplicate keys must remain present");
        let ObjectKind::Array(kids) = entries[1].value().kind() else {
            panic!("expected Kids array");
        };
        assert_eq!(kids[0].kind(), &ObjectKind::Reference(Reference::new(4, 0)));
        assert_eq!(source.resolve(kids[0].span()), Ok(&b"4 0 R"[..]));
    }

    #[test]
    fn parses_multiple_top_level_objects_without_losing_lookahead() {
        let source = source(b"1 2 /Name");
        let mut parser = ObjectParser::new(&source, 0, ParseLimits::default());

        let first = parser.parse_next().expect("first object").expect("number");
        let second = parser.parse_next().expect("second object").expect("number");
        let third = parser.parse_next().expect("third object").expect("name");

        assert_eq!(source.resolve(first.span()), Ok(&b"1"[..]));
        assert_eq!(source.resolve(second.span()), Ok(&b"2"[..]));
        assert_eq!(source.resolve(third.span()), Ok(&b"/Name"[..]));
        assert_eq!(parser.parse_next(), Ok(None));
    }

    #[test]
    fn decodes_name_objects_and_dictionary_keys_without_losing_source() {
        let bytes = source(b"/A#20B << /Ext#47State null >>");
        let mut parser = ObjectParser::new(&bytes, 0, ParseLimits::default());
        let name = parser.parse_next().unwrap().unwrap();
        let dictionary = parser.parse_next().unwrap().unwrap();
        assert_eq!(decode_name(&bytes, &name), Ok(b"/A B".to_vec()));
        let ObjectKind::Dictionary(entries) = dictionary.kind() else {
            panic!("dictionary");
        };
        assert_eq!(entries[0].decoded_key(&bytes), Ok(b"/ExtGState".to_vec()));
        let wrong = ByteStore::new(SourceId::new(5), &b"/A#20B"[..]);
        assert!(decode_name(&wrong, &name).is_err());
    }

    #[test]
    fn rejects_invalid_reference_ranges() {
        let cases: &[&[u8]] = &[b"-1 0 R", b"1 65536 R", b"4294967296 0 R"];

        for &bytes in cases {
            let source = ByteStore::new(SourceId::new(1), bytes);
            let error = ObjectParser::new(&source, 0, ParseLimits::default())
                .parse_next()
                .expect_err("invalid reference must fail");
            assert_eq!(error.kind(), ParseErrorKind::InvalidReference);
        }
    }

    #[test]
    fn rejects_non_name_dictionary_keys_and_unterminated_containers() {
        let cases: &[(&[u8], ParseErrorKind)] = &[
            (b"<< 1 true >>", ParseErrorKind::DictionaryKeyNotName),
            (b"[1 2", ParseErrorKind::UnterminatedArray),
            (b"<< /A 1", ParseErrorKind::UnterminatedDictionary),
        ];

        for &(bytes, kind) in cases {
            let source = ByteStore::new(SourceId::new(1), bytes);
            let error = ObjectParser::new(&source, 0, ParseLimits::default())
                .parse_next()
                .expect_err("malformed container must fail");
            assert_eq!(error.kind(), kind);
        }
    }

    #[test]
    fn preserves_the_specific_lexical_diagnostic() {
        let source = source(b"[/bad#x0]");
        let error = ObjectParser::new(&source, 0, ParseLimits::default())
            .parse_next()
            .expect_err("malformed name must fail");

        assert_eq!(
            error.kind(),
            ParseErrorKind::Lexical(LexErrorKind::MalformedNameEscape)
        );
        assert_eq!(error.offset(), 5);
    }

    #[test]
    fn enforces_depth_collection_and_object_limits() {
        let cases = [
            (
                b"[[null]]".as_slice(),
                ParseLimits {
                    max_depth: 1,
                    ..ParseLimits::default()
                },
                ParseErrorKind::NestingLimit,
            ),
            (
                b"[null null]".as_slice(),
                ParseLimits {
                    max_collection_entries: 1,
                    ..ParseLimits::default()
                },
                ParseErrorKind::CollectionLimit,
            ),
            (
                b"[null]".as_slice(),
                ParseLimits {
                    max_objects: 1,
                    ..ParseLimits::default()
                },
                ParseErrorKind::ObjectCountLimit,
            ),
        ];

        for (bytes, limits, kind) in cases {
            let source = ByteStore::new(SourceId::new(1), bytes);
            let error = ObjectParser::new(&source, 0, limits)
                .parse_next()
                .expect_err("resource limit must fail");
            assert_eq!(error.kind(), kind);
        }
    }
}
