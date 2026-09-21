#![forbid(unsafe_code)]

use std::fmt;

use pdf_bytes::{ByteStore, SourceSpan, SpanError};
use pdf_syntax::{LexError, Lexer, Object, ObjectParser, ParseError, ParseLimits, TokenKind};

mod page;

pub use pdf_font::tounicode::{Code, Confidence, Meaning, ToUnicode};
pub use pdf_font::{Font, FontError, SimpleFont, SourceCode, parse_simple_font};

pub use page::{
    Annotation, AnnotationFlags, Annotations, Appearance, DecodedContentStream, Destination,
    Followed, FormXObject, FunctionData, FunctionObject, IccProfileStream, ImageXObject,
    IndexedLookupStream, Link, LinkAction, LinkResolver, Medium, OptionalContent, PageContentError,
    PageContentErrorKind, PageContentLimits, PageGeometry, PageLinks, PageProgram, PageResources,
    ResourceEntry, ResourceFontError, ShadingPattern, TilingPattern, Type3Font, Type3Procedure,
    UnreadableAnnotation, View, count_pages_recovering, count_pages_strict,
    count_pages_with_password, load_page_program_for, load_page_program_recovering,
    load_page_program_recovering_for, load_page_program_strict,
    load_page_program_tolerating_damage, load_page_program_tolerating_damage_for,
    load_page_program_with_password, open_link_resolver, open_link_resolver_recovering,
    open_link_resolver_tolerating_damage, page_geometries_recovering,
    page_geometries_with_password, page_references_recovering, page_references_with_password,
};
pub use pdf_font::cff::CffFont;
pub use pdf_font::decipher;
pub use pdf_font::glyph::{GlyphPath, GlyphProgram, GlyphSegment};
pub use pdf_font::legacy_cjk::{LegacyByte, LegacyEncoding, decode_legacy_bytes};
pub use pdf_font::outline_match;
pub use pdf_font::sha256::hex as sha256_hex;
pub use pdf_font::standard14::Standard14;
pub use pdf_font::subset::{
    GLYPHLESS_ADVANCE, Subset, SubsetError, glyphless, subset_cff, subset_truetype,
};
pub use pdf_font::substitute::{
    DecipherScope, FaceIdentity, FontFlags, FontProvider, FontRequest, FontStyle, FontSubstitution,
    GenericFamily, MappingCensus, MappingRoute, ProgramEvidence, SUBSTITUTION_POLICY,
    SubstitutedFace, SubstitutionReason, UnresolvedReason, character_for_glyph_name,
    is_combining_mark, is_ignorable, standard_encoding_name,
};
pub use pdf_font::system_fonts::{PackagedFace, SystemFontProvider};
pub use pdf_font::truetype::{GlyphOutline, OutlinePoint, TrueTypeError, TrueTypeFont};
pub use pdf_security::PrintAllowance;
pub use pdf_syntax::ImageCodec;
pub use pdf_syntax::{RecoverLimits, Recovered, Repair, RepairKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContentLimits {
    pub objects: ParseLimits,
    pub max_operations: usize,
    pub max_operands_per_operation: usize,
    pub max_total_operands: usize,
}

impl Default for ContentLimits {
    fn default() -> Self {
        Self {
            objects: ParseLimits::default(),
            max_operations: 1_000_000,
            max_operands_per_operation: 256,
            max_total_operands: 4_000_000,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InlineImage {
    pub entries: Vec<(Object, Object)>,
    pub data: SourceSpan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Operation {
    operands: Vec<Object>,
    operator: SourceSpan,
    span: SourceSpan,
    inline_image: Option<Box<InlineImage>>,
}

impl Operation {
    #[must_use]
    pub fn operands(&self) -> &[Object] {
        &self.operands
    }

    #[must_use]
    pub const fn operator_span(&self) -> SourceSpan {
        self.operator
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }

    #[must_use]
    pub fn inline_image(&self) -> Option<&InlineImage> {
        self.inline_image.as_deref()
    }

    pub fn operator_bytes<'source>(
        &self,
        source: &'source ByteStore,
    ) -> Result<&'source [u8], SpanError> {
        source.resolve(self.operator)
    }
}

pub fn parse_operations_strict(
    source: &ByteStore,
    limits: ContentLimits,
) -> Result<Vec<Operation>, ContentError> {
    let (operations, carried) = parse_operations_carrying(source, limits, Vec::new())?;
    if !carried.is_empty() {
        return Err(ContentError::new(
            carried[0].span().start(),
            ContentErrorKind::DanglingOperands {
                count: carried.len(),
            },
        ));
    }
    Ok(operations)
}

pub fn parse_operation_sequence_strict(
    sources: &[&ByteStore],
    limits: ContentLimits,
) -> Result<Vec<Vec<Operation>>, ContentError> {
    let mut carried: Vec<Object> = Vec::new();
    let mut per_stream = Vec::with_capacity(sources.len());
    for source in sources {
        let (operations, leftover) = parse_operations_carrying(source, limits, carried)?;
        per_stream.push(operations);
        carried = leftover;
    }
    if !carried.is_empty() {
        return Err(ContentError::new(
            carried[0].span().start(),
            ContentErrorKind::DanglingOperands {
                count: carried.len(),
            },
        ));
    }
    Ok(per_stream)
}

fn parse_operations_carrying(
    source: &ByteStore,
    limits: ContentLimits,
    carried: Vec<Object>,
) -> Result<(Vec<Operation>, Vec<Object>), ContentError> {
    let mut cursor = 0_usize;
    let mut operands: Vec<Object> = carried;
    let mut operations = Vec::new();
    let mut total_operands = operands.len();

    loop {
        let mut lexer = Lexer::new(source, cursor, limits.objects.lex);
        let token = lexer
            .next_token()
            .map_err(|error| ContentError::lexical(error.offset(), error))?;
        let Some(token) = token else {
            return Ok((operations, operands));
        };

        if token.kind() == TokenKind::Keyword {
            let operator = source.resolve(token.span()).map_err(|_| {
                ContentError::new(token.span().start(), ContentErrorKind::SourceSpanFailure)
            })?;
            if operator == b"BI" {
                let (image, end) = parse_inline_image(source, token.span().end(), limits)?;
                let span = source.span(token.span().start()..end).map_err(|_| {
                    ContentError::new(token.span().start(), ContentErrorKind::SourceSpanFailure)
                })?;
                let operator = source.span(end - b"EI".len()..end).map_err(|_| {
                    ContentError::new(token.span().start(), ContentErrorKind::SourceSpanFailure)
                })?;
                operations.push(Operation {
                    operands: std::mem::take(&mut operands),
                    operator,
                    span,
                    inline_image: Some(Box::new(image)),
                });
                cursor = end;
                continue;
            }
            if operations.len() >= limits.max_operations {
                return Err(ContentError::new(
                    token.span().start(),
                    ContentErrorKind::OperationLimit,
                ));
            }
            let start = operands
                .iter()
                .find(|operand| source.resolve(operand.span()).is_ok())
                .map_or(token.span().start(), |operand| operand.span().start());
            let span = source.span(start..token.span().end()).map_err(|_| {
                ContentError::new(token.span().start(), ContentErrorKind::SourceSpanFailure)
            })?;
            operations.push(Operation {
                operands: std::mem::take(&mut operands),
                operator: token.span(),
                span,
                inline_image: None,
            });
            cursor = token.span().end();
            continue;
        }

        if operands.len() >= limits.max_operands_per_operation {
            return Err(ContentError::new(
                token.span().start(),
                ContentErrorKind::OperandLimit,
            ));
        }
        if total_operands >= limits.max_total_operands {
            return Err(ContentError::new(
                token.span().start(),
                ContentErrorKind::TotalOperandLimit,
            ));
        }
        let mut parser = ObjectParser::new(source, cursor, limits.objects);
        let operand = parser
            .parse_next()
            .map_err(|error| ContentError::object(error.offset(), error))?
            .ok_or_else(|| {
                ContentError::new(token.span().start(), ContentErrorKind::MissingOperand)
            })?;
        cursor = parser.offset();
        operands.push(operand);
        total_operands += 1;
    }
}

fn parse_inline_image(
    source: &ByteStore,
    after_bi: usize,
    limits: ContentLimits,
) -> Result<(InlineImage, usize), ContentError> {
    let mut entries = Vec::new();
    let mut cursor = after_bi;
    let data_start = loop {
        let mut lexer = Lexer::new(source, cursor, limits.objects.lex);
        let token = lexer
            .next_token()
            .map_err(|error| ContentError::lexical(error.offset(), error))?
            .ok_or_else(|| ContentError::new(cursor, ContentErrorKind::InlineImageUnterminated))?;
        if token.kind() == TokenKind::Keyword
            && source
                .resolve(token.span())
                .is_ok_and(|bytes| bytes == b"ID")
        {
            break token.span().end() + 1;
        }
        if entries.len() >= limits.max_operands_per_operation {
            return Err(ContentError::new(
                token.span().start(),
                ContentErrorKind::OperandLimit,
            ));
        }
        let mut parser = ObjectParser::new(source, cursor, limits.objects);
        let key = parser
            .parse_next()
            .map_err(|error| ContentError::object(error.offset(), error))?
            .ok_or_else(|| ContentError::new(cursor, ContentErrorKind::InlineImageUnterminated))?;
        let value = parser
            .parse_next()
            .map_err(|error| ContentError::object(error.offset(), error))?
            .ok_or_else(|| ContentError::new(cursor, ContentErrorKind::InlineImageUnterminated))?;
        cursor = parser.offset();
        entries.push((key, value));
    };

    let bytes = source.as_bytes();
    if data_start > bytes.len() {
        return Err(ContentError::new(
            after_bi,
            ContentErrorKind::InlineImageUnterminated,
        ));
    }
    let search_from = is_dct(source, &entries)
        .then(|| jpeg_end(bytes, data_start))
        .flatten()
        .unwrap_or(data_start);
    let end_at = delimited_ei(bytes, search_from)
        .ok_or_else(|| ContentError::new(data_start, ContentErrorKind::InlineImageUnterminated))?;
    let data = source
        .span(data_start..end_at)
        .map_err(|_| ContentError::new(data_start, ContentErrorKind::SourceSpanFailure))?;
    let after = bytes[end_at..]
        .iter()
        .position(|byte| !is_pdf_whitespace(*byte))
        .map_or(bytes.len(), |offset| end_at + offset + b"EI".len());
    Ok((InlineImage { entries, data }, after))
}

fn is_dct(source: &ByteStore, entries: &[(Object, Object)]) -> bool {
    entries.iter().any(|(key, value)| {
        let key = source.resolve(key.span()).unwrap_or_default();
        if key != b"/F" && key != b"/Filter" {
            return false;
        }
        let value = source.resolve(value.span()).unwrap_or_default();
        value.windows(4).any(|window| window == b"/DCT")
    })
}

fn jpeg_end(bytes: &[u8], from: usize) -> Option<usize> {
    let tail = bytes.get(from..)?;
    let at = tail.windows(2).position(|pair| pair == [0xff, 0xd9])?;
    Some(from + at + 2)
}

fn delimited_ei(bytes: &[u8], from: usize) -> Option<usize> {
    let mut at = from;
    while at + 2 <= bytes.len() {
        let found = at + bytes[at..].windows(2).position(|pair| pair == b"EI")?;
        let before = found.checked_sub(1).map(|index| bytes[index]);
        let after = bytes.get(found + 2).copied();
        let delimited = before.is_some_and(is_pdf_whitespace)
            && after.is_none_or(|byte| is_pdf_whitespace(byte) || is_pdf_delimiter(byte));
        if delimited {
            return Some(found - 1);
        }
        at = found + 2;
    }
    None
}

const fn is_pdf_whitespace(byte: u8) -> bool {
    matches!(byte, b'\0' | b'\t' | b'\n' | 0x0c | b'\r' | b' ')
}

const fn is_pdf_delimiter(byte: u8) -> bool {
    matches!(
        byte,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContentError {
    offset: usize,
    kind: ContentErrorKind,
}

impl ContentError {
    const fn new(offset: usize, kind: ContentErrorKind) -> Self {
        Self { offset, kind }
    }

    const fn lexical(offset: usize, error: LexError) -> Self {
        Self::new(offset, ContentErrorKind::Lexical(error))
    }

    const fn object(offset: usize, error: ParseError) -> Self {
        Self::new(offset, ContentErrorKind::Object(error))
    }

    #[must_use]
    pub const fn offset(self) -> usize {
        self.offset
    }

    #[must_use]
    pub const fn kind(self) -> ContentErrorKind {
        self.kind
    }
}

impl fmt::Display for ContentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} at byte {}", self.kind, self.offset)
    }
}

impl std::error::Error for ContentError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContentErrorKind {
    Lexical(LexError),
    Object(ParseError),
    MissingOperand,
    DanglingOperands { count: usize },
    InlineImageUnsupported,
    InlineImageUnterminated,
    OperationLimit,
    OperandLimit,
    TotalOperandLimit,
    SourceSpanFailure,
}

impl fmt::Display for ContentErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lexical(error) => write!(formatter, "content-stream lexical error: {error}"),
            Self::Object(error) => write!(formatter, "invalid content operand: {error}"),
            Self::MissingOperand => formatter.write_str("content operand is missing"),
            Self::DanglingOperands { count } => {
                write!(
                    formatter,
                    "content stream ends with {count} dangling operands"
                )
            }
            Self::InlineImageUnsupported => {
                formatter.write_str("inline image parsing is not implemented")
            }
            Self::InlineImageUnterminated => {
                formatter.write_str("inline image has no ID or no delimited EI")
            }
            Self::OperationLimit => formatter.write_str("content operation limit exceeded"),
            Self::OperandLimit => {
                formatter.write_str("content operand-per-operation limit exceeded")
            }
            Self::TotalOperandLimit => formatter.write_str("content total operand limit exceeded"),
            Self::SourceSpanFailure => {
                formatter.write_str("content source span cannot be resolved")
            }
        }
    }
}

#[cfg(test)]
mod sequence_tests {
    use std::sync::Arc;

    use pdf_bytes::{ByteStore, SourceId};

    use super::{ContentErrorKind, ContentLimits, parse_operation_sequence_strict};

    fn store(id: u64, bytes: &'static [u8]) -> ByteStore {
        ByteStore::new(SourceId::new(id), Arc::<[u8]>::from(bytes))
    }

    #[test]
    fn an_operation_may_have_its_operands_in_one_stream_and_its_operator_in_the_next() {
        let first = store(1, b"1 0 0 1 10 20 cm /F1");
        let second = store(2, b" 12 Tf (hi) Tj");
        let streams = [&first, &second];
        let parsed = parse_operation_sequence_strict(&streams, ContentLimits::default())
            .expect("an operation may span a stream boundary");

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].len(), 1);
        assert_eq!(parsed[0][0].operator_bytes(&first).expect("cm"), b"cm");
        assert_eq!(parsed[1].len(), 2);
        let split = &parsed[1][0];
        assert_eq!(split.operator_bytes(&second).expect("Tf"), b"Tf");
        assert_eq!(split.operands().len(), 2);
        assert_eq!(
            first
                .resolve(split.operands()[0].span())
                .expect("carried operand"),
            b"/F1"
        );
        assert!(
            second.resolve(split.operands()[0].span()).is_err(),
            "a span names the store it came from, so it does not resolve against another"
        );
        assert_eq!(
            second
                .resolve(split.operands()[1].span())
                .expect("own operand"),
            b"12"
        );
        assert_eq!(second.resolve(split.span()).expect("extent"), b"12 Tf");
    }

    #[test]
    fn operands_left_at_the_end_of_the_last_stream_are_still_dangling() {
        let first = store(3, b"q /F1");
        let second = store(4, b" 12 Tf /Im1");
        let streams = [&first, &second];
        let error = parse_operation_sequence_strict(&streams, ContentLimits::default())
            .expect_err("the last stream may not end mid-operation");
        assert_eq!(
            error.kind(),
            ContentErrorKind::DanglingOperands { count: 1 }
        );
    }

    #[test]
    fn a_single_stream_sequence_behaves_exactly_like_parsing_it_alone() {
        let only = store(5, b"q 1 0 0 1 5 5 cm Q");
        let sequence = parse_operation_sequence_strict(&[&only], ContentLimits::default())
            .expect("one stream is still a sequence");
        let alone = super::parse_operations_strict(&only, ContentLimits::default())
            .expect("one stream on its own");
        assert_eq!(sequence.len(), 1);
        assert_eq!(sequence[0], alone);
    }
}

#[cfg(test)]
mod tests {
    use pdf_bytes::{ByteStore, SourceId};
    use pdf_syntax::{ObjectKind, ParseLimits};

    use super::{ContentErrorKind, ContentLimits, parse_operations_strict};

    fn source(bytes: &'static [u8]) -> ByteStore {
        ByteStore::new(SourceId::derived(SourceId::new(7), 4), bytes)
    }

    #[test]
    fn separates_operands_from_operators_and_preserves_exact_spans() {
        let source = source(b"% lead\nq 1 0 0 1 72 24 cm /F1 12 Tf (Hi) Tj Q");
        let operations =
            parse_operations_strict(&source, ContentLimits::default()).expect("valid content");

        assert_eq!(operations.len(), 5);
        assert_eq!(operations[0].operator_bytes(&source), Ok(&b"q"[..]));
        assert!(operations[0].operands().is_empty());
        assert_eq!(operations[1].operator_bytes(&source), Ok(&b"cm"[..]));
        assert_eq!(operations[1].operands().len(), 6);
        assert_eq!(
            source.resolve(operations[1].span()),
            Ok(&b"1 0 0 1 72 24 cm"[..])
        );
        assert_eq!(operations[3].operator_bytes(&source), Ok(&b"Tj"[..]));
        assert!(matches!(
            operations[3].operands()[0].kind(),
            ObjectKind::LiteralString
        ));
    }

    #[test]
    fn retains_nested_array_operands_as_one_object() {
        let source = source(b"[(A) -25 (B)] TJ");
        let operations =
            parse_operations_strict(&source, ContentLimits::default()).expect("valid TJ");

        assert_eq!(operations.len(), 1);
        assert_eq!(operations[0].operands().len(), 1);
        assert!(matches!(
            operations[0].operands()[0].kind(),
            ObjectKind::Array(values) if values.len() == 3
        ));
    }

    #[test]
    fn rejects_dangling_operands() {
        let dangling = parse_operations_strict(&source(b"1 2"), ContentLimits::default())
            .expect_err("missing operator");
        assert_eq!(
            dangling.kind(),
            ContentErrorKind::DanglingOperands { count: 2 }
        );
    }

    #[test]
    fn an_inline_image_carries_its_dictionary_and_the_bytes_that_may_spell_its_end() {
        let stream = source(b"q BI /W 4 /H 1 /BPC 8 /CS /G ID AEIB EI Q");
        let operations =
            parse_operations_strict(&stream, ContentLimits::default()).expect("the stream parses");
        assert_eq!(operations.len(), 3, "q, the image, and Q");
        assert_eq!(operations[0].operator_bytes(&stream), Ok(&b"q"[..]));
        assert_eq!(operations[2].operator_bytes(&stream), Ok(&b"Q"[..]));

        let image = operations[1]
            .inline_image()
            .expect("the middle operation is the image");
        assert_eq!(operations[1].operator_bytes(&stream), Ok(&b"EI"[..]));
        let keys: Vec<&[u8]> = image
            .entries
            .iter()
            .map(|(key, _)| stream.resolve(key.span()).expect("in this source"))
            .collect();
        assert_eq!(keys, vec![&b"/W"[..], b"/H", b"/BPC", b"/CS"]);
        assert_eq!(
            stream.resolve(image.data),
            Ok(&b"AEIB"[..]),
            "four bytes, including the two that spell the keyword"
        );
        assert_eq!(
            stream.resolve(operations[1].span()),
            Ok(&b"BI /W 4 /H 1 /BPC 8 /CS /G ID AEIB EI"[..])
        );
    }

    #[test]
    fn an_inline_image_with_no_delimited_ei_is_refused() {
        for stream in [
            &b"BI /W 1 /H 1 ID x"[..],
            b"BI /W 1 /H 1 ID xEIx",
            b"BI /W 1 /H 1",
        ] {
            let error = parse_operations_strict(&source(stream), ContentLimits::default())
                .expect_err("an unterminated inline image");
            assert_eq!(
                error.kind(),
                ContentErrorKind::InlineImageUnterminated,
                "{:?}",
                String::from_utf8_lossy(stream)
            );
        }
    }

    #[test]
    fn enforces_operation_operand_and_nested_object_limits() {
        let operation_limit = ContentLimits {
            max_operations: 1,
            ..ContentLimits::default()
        };
        assert_eq!(
            parse_operations_strict(&source(b"q Q"), operation_limit)
                .expect_err("two operations")
                .kind(),
            ContentErrorKind::OperationLimit
        );

        let operand_limit = ContentLimits {
            max_operands_per_operation: 1,
            ..ContentLimits::default()
        };
        assert_eq!(
            parse_operations_strict(&source(b"1 2 m"), operand_limit)
                .expect_err("two operands")
                .kind(),
            ContentErrorKind::OperandLimit
        );

        let object_limit = ContentLimits {
            objects: ParseLimits {
                max_depth: 1,
                ..ParseLimits::default()
            },
            ..ContentLimits::default()
        };
        assert!(matches!(
            parse_operations_strict(&source(b"[[1]] op"), object_limit)
                .expect_err("nested object limit")
                .kind(),
            ContentErrorKind::Object(_)
        ));
    }
}

pub use pdf_font::shaping::{ShapedGlyph, shape_cluster};
