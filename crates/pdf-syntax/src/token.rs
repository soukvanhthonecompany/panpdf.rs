use std::fmt;

use pdf_bytes::{ByteStore, SourceSpan};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LexLimits {
    pub max_literal_string_depth: usize,
    pub max_token_bytes: usize,
}

impl Default for LexLimits {
    fn default() -> Self {
        Self {
            max_literal_string_depth: 64,
            max_token_bytes: 16 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NumberKind {
    Integer,
    Real,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TokenKind {
    Null,
    Boolean(bool),
    Number(NumberKind),
    Name,
    LiteralString,
    HexString,
    ArrayStart,
    ArrayEnd,
    DictionaryStart,
    DictionaryEnd,
    Keyword,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Token {
    kind: TokenKind,
    span: SourceSpan,
}

impl Token {
    #[must_use]
    pub const fn kind(self) -> TokenKind {
        self.kind
    }

    #[must_use]
    pub const fn span(self) -> SourceSpan {
        self.span
    }
}

pub struct Lexer<'a> {
    source: &'a ByteStore,
    bytes: &'a [u8],
    base: usize,
    offset: usize,
    limits: LexLimits,
}

const LOOKAHEAD: usize = 2;

impl<'a> Lexer<'a> {
    #[must_use]
    pub fn new(source: &'a ByteStore, offset: usize, limits: LexLimits) -> Self {
        let (base, bytes) = source.run_at(offset);
        Self {
            source,
            bytes,
            base,
            offset: offset - base,
            limits,
        }
    }

    #[must_use]
    pub const fn offset(&self) -> usize {
        self.base + self.offset
    }

    pub fn next_token(&mut self) -> Result<Option<Token>, LexError> {
        let from = self.offset();
        let read = self.read_token();
        let ends_here = self.base + self.bytes.len() >= self.source.len();
        if ends_here || self.offset + LOOKAHEAD < self.bytes.len() {
            return read;
        }
        self.bytes = self.source.as_bytes();
        self.base = 0;
        self.offset = from;
        self.read_token()
    }

    fn read_token(&mut self) -> Result<Option<Token>, LexError> {
        let found = self.scan_token();
        let base = self.base;
        found.map_err(|error| Self::error(base + error.offset, error.kind))
    }

    fn scan_token(&mut self) -> Result<Option<Token>, LexError> {
        let bytes = self.bytes;
        if self.offset > bytes.len() {
            return Err(Self::error(self.offset, LexErrorKind::OffsetOutOfBounds));
        }

        self.skip_trivia();
        let bytes = self.bytes;
        if self.offset == bytes.len() {
            return Ok(None);
        }

        let start = self.offset;
        let kind = match bytes[start] {
            b'[' => {
                self.offset += 1;
                TokenKind::ArrayStart
            }
            b']' => {
                self.offset += 1;
                TokenKind::ArrayEnd
            }
            b'<' if bytes.get(start + 1) == Some(&b'<') => {
                self.offset += 2;
                TokenKind::DictionaryStart
            }
            b'>' if bytes.get(start + 1) == Some(&b'>') => {
                self.offset += 2;
                TokenKind::DictionaryEnd
            }
            b'<' => self.scan_hex_string(start)?,
            b'(' => self.scan_literal_string(start)?,
            b'/' => self.scan_name(start)?,
            b'+' | b'-' | b'.' | b'0'..=b'9' => self.scan_number(start)?,
            b'>' | b'{' | b'}' | b')' => {
                return Err(Self::error(start, LexErrorKind::UnexpectedDelimiter));
            }
            _ => self.scan_keyword(start)?,
        };

        let len = self.offset - start;
        if len > self.limits.max_token_bytes {
            return Err(Self::error(start, LexErrorKind::TokenTooLong));
        }

        let span = self
            .source
            .span(self.base + start..self.base + self.offset)
            .map_err(|_| Self::error(start, LexErrorKind::OffsetOutOfBounds))?;
        Ok(Some(Token { kind, span }))
    }

    fn skip_trivia(&mut self) {
        let mut in_comment = false;
        loop {
            let bytes = self.bytes;
            while let Some(&byte) = bytes.get(self.offset) {
                if in_comment {
                    if matches!(byte, b'\r' | b'\n') {
                        in_comment = false;
                    } else {
                        self.offset += 1;
                    }
                } else if is_whitespace(byte) {
                    self.offset += 1;
                } else if byte == b'%' {
                    in_comment = true;
                    self.offset += 1;
                } else {
                    return;
                }
            }
            let end = self.base + bytes.len();
            if self.offset != bytes.len() || end >= self.source.len() {
                return;
            }
            let (base, next) = self.source.run_at(end);
            self.base = base;
            self.bytes = next;
            self.offset = end - base;
        }
    }

    fn scan_name(&mut self, start: usize) -> Result<TokenKind, LexError> {
        let bytes = self.bytes;
        self.offset += 1;
        while let Some(byte) = bytes.get(self.offset).copied() {
            if is_whitespace(byte) || is_delimiter(byte) {
                break;
            }
            if byte == b'#' {
                let first = bytes.get(self.offset + 1).copied();
                let second = bytes.get(self.offset + 2).copied();
                if !first.is_some_and(is_hex) || !second.is_some_and(is_hex) {
                    return Err(Self::error(self.offset, LexErrorKind::MalformedNameEscape));
                }
                self.offset += 3;
            } else {
                self.offset += 1;
            }
        }
        self.check_token_length(start)?;
        Ok(TokenKind::Name)
    }

    fn scan_literal_string(&mut self, start: usize) -> Result<TokenKind, LexError> {
        let bytes = self.bytes;
        let mut depth = 1_usize;
        self.offset += 1;

        while let Some(byte) = bytes.get(self.offset).copied() {
            match byte {
                b'\\' => {
                    self.offset += 1;
                    if bytes.get(self.offset) == Some(&b'\r') {
                        self.offset += 1;
                        if bytes.get(self.offset) == Some(&b'\n') {
                            self.offset += 1;
                        }
                    } else if self.offset < bytes.len() {
                        self.offset += 1;
                    }
                }
                b'(' => {
                    depth += 1;
                    if depth > self.limits.max_literal_string_depth {
                        return Err(Self::error(self.offset, LexErrorKind::StringNestingLimit));
                    }
                    self.offset += 1;
                }
                b')' => {
                    self.offset += 1;
                    depth -= 1;
                    if depth == 0 {
                        self.check_token_length(start)?;
                        return Ok(TokenKind::LiteralString);
                    }
                }
                _ => self.offset += 1,
            }
            self.check_token_length(start)?;
        }

        Err(Self::error(start, LexErrorKind::UnterminatedLiteralString))
    }

    fn scan_hex_string(&mut self, start: usize) -> Result<TokenKind, LexError> {
        let bytes = self.bytes;
        self.offset += 1;
        while let Some(byte) = bytes.get(self.offset).copied() {
            if byte == b'>' {
                self.offset += 1;
                self.check_token_length(start)?;
                return Ok(TokenKind::HexString);
            }
            if !is_whitespace(byte) && !is_hex(byte) {
                return Err(Self::error(self.offset, LexErrorKind::InvalidHexDigit));
            }
            self.offset += 1;
            self.check_token_length(start)?;
        }
        Err(Self::error(start, LexErrorKind::UnterminatedHexString))
    }

    fn scan_number(&mut self, start: usize) -> Result<TokenKind, LexError> {
        let bytes = self.bytes;
        if matches!(bytes.get(self.offset), Some(b'+' | b'-')) {
            self.offset += 1;
        }

        let mut digits = 0_usize;
        let mut points = 0_usize;
        while let Some(byte) = bytes.get(self.offset).copied() {
            match byte {
                b'0'..=b'9' => {
                    digits += 1;
                    self.offset += 1;
                }
                b'.' => {
                    points += 1;
                    self.offset += 1;
                }
                _ if is_whitespace(byte) || is_delimiter(byte) => break,
                _ => return Err(Self::error(self.offset, LexErrorKind::MalformedNumber)),
            }
            self.check_token_length(start)?;
        }

        if digits == 0 || points > 1 {
            return Err(Self::error(start, LexErrorKind::MalformedNumber));
        }
        Ok(TokenKind::Number(if points == 0 {
            NumberKind::Integer
        } else {
            NumberKind::Real
        }))
    }

    fn scan_keyword(&mut self, start: usize) -> Result<TokenKind, LexError> {
        let bytes = self.bytes;
        while let Some(byte) = bytes.get(self.offset).copied() {
            if is_whitespace(byte) || is_delimiter(byte) {
                break;
            }
            self.offset += 1;
            self.check_token_length(start)?;
        }
        if self.offset == start {
            return Err(Self::error(start, LexErrorKind::UnexpectedDelimiter));
        }

        Ok(match &bytes[start..self.offset] {
            b"null" => TokenKind::Null,
            b"true" => TokenKind::Boolean(true),
            b"false" => TokenKind::Boolean(false),
            _ => TokenKind::Keyword,
        })
    }

    fn check_token_length(&self, start: usize) -> Result<(), LexError> {
        if self.offset - start > self.limits.max_token_bytes {
            return Err(Self::error(start, LexErrorKind::TokenTooLong));
        }
        Ok(())
    }

    const fn error(offset: usize, kind: LexErrorKind) -> LexError {
        LexError { offset, kind }
    }
}

pub(crate) const fn is_whitespace(byte: u8) -> bool {
    matches!(byte, 0x00 | b'\t' | b'\n' | 0x0c | b'\r' | b' ')
}

pub(crate) const fn is_delimiter(byte: u8) -> bool {
    matches!(
        byte,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

const fn is_hex(byte: u8) -> bool {
    byte.is_ascii_hexdigit()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LexError {
    offset: usize,
    kind: LexErrorKind,
}

impl LexError {
    #[must_use]
    pub const fn offset(self) -> usize {
        self.offset
    }

    #[must_use]
    pub const fn kind(self) -> LexErrorKind {
        self.kind
    }
}

impl fmt::Display for LexError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} at byte {}", self.kind, self.offset)
    }
}

impl std::error::Error for LexError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LexErrorKind {
    OffsetOutOfBounds,
    UnexpectedDelimiter,
    MalformedNameEscape,
    MalformedNumber,
    InvalidHexDigit,
    UnterminatedHexString,
    UnterminatedLiteralString,
    StringNestingLimit,
    TokenTooLong,
}

impl fmt::Display for LexErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::OffsetOutOfBounds => "lexer offset is outside the source",
            Self::UnexpectedDelimiter => "unexpected PDF delimiter",
            Self::MalformedNameEscape => "malformed hexadecimal escape in PDF name",
            Self::MalformedNumber => "malformed PDF number",
            Self::InvalidHexDigit => "invalid digit in hexadecimal string",
            Self::UnterminatedHexString => "unterminated hexadecimal string",
            Self::UnterminatedLiteralString => "unterminated literal string",
            Self::StringNestingLimit => "literal string nesting limit exceeded",
            Self::TokenTooLong => "PDF token exceeds the configured byte limit",
        };
        formatter.write_str(message)
    }
}

#[cfg(test)]
mod tests {
    use pdf_bytes::{ByteStore, SourceId};

    use super::{LexErrorKind, LexLimits, Lexer, NumberKind, TokenKind};

    fn source(bytes: &'static [u8]) -> ByteStore {
        ByteStore::new(SourceId::new(9), bytes)
    }

    fn tokens(source: &ByteStore) -> Vec<(TokenKind, Vec<u8>)> {
        let mut lexer = Lexer::new(source, 0, LexLimits::default());
        let mut found = Vec::new();
        while let Some(token) = lexer.next_token().expect("valid token stream") {
            found.push((
                token.kind(),
                source
                    .resolve(token.span())
                    .expect("token source span")
                    .to_vec(),
            ));
        }
        found
    }

    #[test]
    fn a_source_in_pieces_lexes_as_the_same_bytes_in_one() {
        let bytes: &[u8] =
            b"1 0 obj\n<< /Name#20x [ (a (nested) string) <4142> -1.5 ] >> % note\r\nendobj\n%%EOF";
        let whole = ByteStore::new(SourceId::new(9), bytes);
        let expected = lexed(&whole);
        for cut in 1..bytes.len() {
            let pieces = ByteStore::new(SourceId::new(9), &bytes[..cut])
                .followed_by(SourceId::new(9), bytes[cut..].to_vec());
            assert_eq!(lexed(&pieces), expected, "cut at {cut}");
        }
        let broken: &[u8] = b"(unterminated % and /Na#";
        for cut in 1..broken.len() {
            let pieces = ByteStore::new(SourceId::new(9), &broken[..cut])
                .followed_by(SourceId::new(9), broken[cut..].to_vec());
            assert_eq!(
                lexed(&pieces),
                lexed(&ByteStore::new(SourceId::new(9), broken)),
                "cut at {cut}"
            );
        }
    }

    type Lexed = (TokenKind, usize, Vec<u8>);

    fn lexed(source: &ByteStore) -> Vec<Result<Lexed, super::LexError>> {
        let mut lexer = Lexer::new(source, 0, LexLimits::default());
        let mut found = Vec::new();
        loop {
            match lexer.next_token() {
                Ok(Some(token)) => found.push(Ok((
                    token.kind(),
                    lexer.offset(),
                    source.resolve(token.span()).expect("its span").to_vec(),
                ))),
                Ok(None) => return found,
                Err(error) => {
                    found.push(Err(error));
                    return found;
                }
            }
        }
    }

    #[test]
    fn skips_all_pdf_whitespace_and_comments() {
        let source = source(b"\0\t\n\x0c\r  % a comment\r\ntrue%tail");

        assert_eq!(
            tokens(&source),
            vec![(TokenKind::Boolean(true), b"true".to_vec())]
        );
    }

    #[test]
    fn classifies_objects_without_normalizing_their_spelling() {
        let source = source(b"null true false +12 -0 3. .5 -.25 /A#20B [] <<>> obj");

        assert_eq!(
            tokens(&source),
            vec![
                (TokenKind::Null, b"null".to_vec()),
                (TokenKind::Boolean(true), b"true".to_vec()),
                (TokenKind::Boolean(false), b"false".to_vec()),
                (TokenKind::Number(NumberKind::Integer), b"+12".to_vec()),
                (TokenKind::Number(NumberKind::Integer), b"-0".to_vec()),
                (TokenKind::Number(NumberKind::Real), b"3.".to_vec()),
                (TokenKind::Number(NumberKind::Real), b".5".to_vec()),
                (TokenKind::Number(NumberKind::Real), b"-.25".to_vec()),
                (TokenKind::Name, b"/A#20B".to_vec()),
                (TokenKind::ArrayStart, b"[".to_vec()),
                (TokenKind::ArrayEnd, b"]".to_vec()),
                (TokenKind::DictionaryStart, b"<<".to_vec()),
                (TokenKind::DictionaryEnd, b">>".to_vec()),
                (TokenKind::Keyword, b"obj".to_vec()),
            ]
        );
    }

    #[test]
    fn literal_strings_support_nesting_escapes_and_line_continuation() {
        let source = source(b"(outer (inner\\)) \\\r\ncontinued)");

        assert_eq!(
            tokens(&source),
            vec![(TokenKind::LiteralString, source.as_bytes().to_vec())]
        );
    }

    #[test]
    fn hex_strings_allow_whitespace_and_an_odd_final_nibble() {
        let source = source(b"<48 65 6c 6c 6f 2>");

        assert_eq!(
            tokens(&source),
            vec![(TokenKind::HexString, source.as_bytes().to_vec())]
        );
    }

    #[test]
    fn reports_malformed_inputs_at_the_causal_byte() {
        let cases: &[(&[u8], LexErrorKind, usize)] = &[
            (b"/bad#x0", LexErrorKind::MalformedNameEscape, 4),
            (b"1.2.3", LexErrorKind::MalformedNumber, 0),
            (b"<0g>", LexErrorKind::InvalidHexDigit, 2),
            (b"<abc", LexErrorKind::UnterminatedHexString, 0),
            (b"(abc", LexErrorKind::UnterminatedLiteralString, 0),
            (b")", LexErrorKind::UnexpectedDelimiter, 0),
        ];

        for &(bytes, kind, offset) in cases {
            let source = ByteStore::new(SourceId::new(1), bytes);
            let error = Lexer::new(&source, 0, LexLimits::default())
                .next_token()
                .expect_err("malformed token must fail");
            assert_eq!((error.kind(), error.offset()), (kind, offset));
        }
    }

    #[test]
    fn enforces_string_depth_and_token_size_limits() {
        let deep = source(b"(((x)))");
        let depth_error = Lexer::new(
            &deep,
            0,
            LexLimits {
                max_literal_string_depth: 2,
                max_token_bytes: 100,
            },
        )
        .next_token()
        .expect_err("depth limit must fail");
        assert_eq!(depth_error.kind(), LexErrorKind::StringNestingLimit);

        let long = source(b"/abcdef");
        let length_error = Lexer::new(
            &long,
            0,
            LexLimits {
                max_literal_string_depth: 2,
                max_token_bytes: 3,
            },
        )
        .next_token()
        .expect_err("token length limit must fail");
        assert_eq!(length_error.kind(), LexErrorKind::TokenTooLong);
    }

    #[test]
    fn rejects_an_initial_offset_beyond_the_source() {
        let source = source(b"null");
        let error = Lexer::new(&source, 5, LexLimits::default())
            .next_token()
            .expect_err("offset outside source must fail");

        assert_eq!(error.kind(), LexErrorKind::OffsetOutOfBounds);
    }
}
