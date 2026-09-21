use std::fmt;

use pdf_bytes::{ByteStore, SourceSpan};

use crate::value::parse_unsigned;
use crate::{
    Object, ObjectKind, ObjectParser, ParseError, ParseLimits, Reference, Token, TokenKind,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamObject {
    keyword: SourceSpan,
    data: SourceSpan,
    end_keyword: SourceSpan,
}

impl StreamObject {
    #[must_use]
    pub const fn keyword_span(&self) -> SourceSpan {
        self.keyword
    }

    #[must_use]
    pub const fn data_span(&self) -> SourceSpan {
        self.data
    }

    #[must_use]
    pub const fn end_keyword_span(&self) -> SourceSpan {
        self.end_keyword
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndirectRepair {
    MissingEndObj {
        byte_offset: usize,
    },
    StreamLengthFromEndStream {
        byte_offset: usize,
        length: usize,
    },
    StreamLengthDisagrees {
        byte_offset: usize,
        declared: usize,
        found: usize,
    },
    SkippedDictionaryKey {
        byte_offset: usize,
    },
    UndefinedObject {
        object_number: u32,
        generation: u16,
    },
}

impl fmt::Display for IndirectRepair {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SkippedDictionaryKey { byte_offset } => {
                write!(
                    formatter,
                    "dictionary key at byte {byte_offset} is not a name; it was stepped over"
                )
            }
            Self::UndefinedObject {
                object_number,
                generation,
            } => {
                write!(
                    formatter,
                    "{object_number} {generation} R names no object; \
                     PDF 32000-1 7.3.10 makes that the null object"
                )
            }
            Self::MissingEndObj { byte_offset } => {
                write!(
                    formatter,
                    "no endobj keyword at byte {byte_offset}; the object ends where its value does"
                )
            }
            Self::StreamLengthFromEndStream {
                byte_offset,
                length,
            } => write!(
                formatter,
                "stream dictionary at byte {byte_offset} states no /Length; endstream puts {length} bytes in it"
            ),
            Self::StreamLengthDisagrees {
                byte_offset,
                declared,
                found,
            } => write!(
                formatter,
                "/Length at byte {byte_offset} says {declared} bytes and endstream says {found}; the keyword was taken"
            ),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndirectObject {
    reference: Reference,
    value: Object,
    stream: Option<StreamObject>,
    span: SourceSpan,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolvedStreamLength {
    reference: Reference,
    value: usize,
}

impl ResolvedStreamLength {
    #[must_use]
    pub const fn new(reference: Reference, value: usize) -> Self {
        Self { reference, value }
    }

    #[must_use]
    pub const fn reference(self) -> Reference {
        self.reference
    }

    #[must_use]
    pub const fn value(self) -> usize {
        self.value
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct IndirectStreamPrefix {
    reference: Reference,
    value: Object,
    data_start: usize,
    length_reference: Reference,
    length_offset: usize,
}

impl IndirectStreamPrefix {
    pub(crate) const fn reference(&self) -> Reference {
        self.reference
    }

    pub(crate) const fn value(&self) -> &Object {
        &self.value
    }

    pub(crate) const fn data_start(&self) -> usize {
        self.data_start
    }

    pub(crate) const fn length_reference(&self) -> Reference {
        self.length_reference
    }

    pub(crate) const fn length_offset(&self) -> usize {
        self.length_offset
    }
}

impl IndirectObject {
    #[must_use]
    pub const fn reference(&self) -> Reference {
        self.reference
    }

    #[must_use]
    pub const fn value(&self) -> &Object {
        &self.value
    }

    #[must_use]
    pub const fn stream(&self) -> Option<&StreamObject> {
        self.stream.as_ref()
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

pub fn parse_indirect_object_strict(
    source: &ByteStore,
    offset: usize,
    limits: ParseLimits,
) -> Result<IndirectObject, IndirectObjectError> {
    parse_indirect_object_with_resolved_length_strict(source, offset, limits, None)
}

pub(crate) fn parse_indirect_stream_prefix_strict(
    source: &ByteStore,
    offset: usize,
    limits: ParseLimits,
) -> Result<IndirectStreamPrefix, IndirectObjectError> {
    let mut parser = ObjectParser::new(source, offset, limits);
    let object_number = required_token(&mut parser, offset)?;
    let generation_offset = parser.offset();
    let generation = required_token(&mut parser, generation_offset)?;
    let keyword_offset = parser.offset();
    let obj_keyword = required_token(&mut parser, keyword_offset)?;

    let object_number_value = parse_token_u32(source, object_number)
        .filter(|value| *value != 0)
        .ok_or_else(|| {
            IndirectObjectError::new(
                object_number.span().start(),
                IndirectObjectErrorKind::InvalidObjectNumber,
            )
        })?;
    let generation_value = parse_token_u16(source, generation).ok_or_else(|| {
        IndirectObjectError::new(
            generation.span().start(),
            IndirectObjectErrorKind::InvalidGeneration,
        )
    })?;
    if !parser.token_equals(obj_keyword, b"obj") {
        return Err(IndirectObjectError::new(
            obj_keyword.span().start(),
            IndirectObjectErrorKind::MissingObjKeyword,
        ));
    }

    let value = parser
        .parse_next()
        .map_err(IndirectObjectError::from)?
        .ok_or_else(|| {
            IndirectObjectError::new(parser.offset(), IndirectObjectErrorKind::MissingValue)
        })?;
    let stream_keyword_offset = parser.offset();
    let stream_keyword = required_token(&mut parser, stream_keyword_offset)?;
    if !parser.token_equals(stream_keyword, b"stream") {
        return Err(IndirectObjectError::new(
            stream_keyword.span().start(),
            IndirectObjectErrorKind::MissingEndObjKeyword,
        ));
    }

    let ObjectKind::Dictionary(entries) = value.kind() else {
        return Err(IndirectObjectError::new(
            stream_keyword.span().start(),
            IndirectObjectErrorKind::StreamRequiresDictionary,
        ));
    };
    let mut lengths = entries
        .iter()
        .filter(|entry| entry.key_equals(source, b"/Length"));
    let length = lengths.next().ok_or_else(|| {
        IndirectObjectError::new(
            value.span().start(),
            IndirectObjectErrorKind::MissingStreamLength,
        )
    })?;
    if lengths.next().is_some() {
        return Err(IndirectObjectError::new(
            value.span().start(),
            IndirectObjectErrorKind::DuplicateStreamLength,
        ));
    }
    let ObjectKind::Reference(length_reference) = length.value().kind() else {
        return Err(IndirectObjectError::new(
            length.value().span().start(),
            IndirectObjectErrorKind::InvalidStreamLength,
        ));
    };
    let length_reference = *length_reference;
    let length_offset = length.value().span().start();
    let data_start = consume_required_eol(source.as_bytes(), stream_keyword.span().end())
        .ok_or_else(|| {
            IndirectObjectError::new(
                stream_keyword.span().end(),
                IndirectObjectErrorKind::MissingStreamLineEnding,
            )
        })?;

    Ok(IndirectStreamPrefix {
        reference: Reference::new(object_number_value, generation_value),
        value,
        data_start,
        length_reference,
        length_offset,
    })
}

pub fn parse_indirect_object_with_resolved_length_strict(
    source: &ByteStore,
    offset: usize,
    limits: ParseLimits,
    resolved_length: Option<ResolvedStreamLength>,
) -> Result<IndirectObject, IndirectObjectError> {
    read_indirect_object(source, offset, limits, resolved_length, None)
}

pub fn parse_indirect_object_recovering(
    source: &ByteStore,
    offset: usize,
    limits: ParseLimits,
    resolved_length: Option<ResolvedStreamLength>,
) -> Result<(IndirectObject, Vec<IndirectRepair>), IndirectObjectError> {
    let mut repairs = Vec::new();
    let object = read_indirect_object(source, offset, limits, resolved_length, Some(&mut repairs))?;
    Ok((object, repairs))
}

fn read_indirect_object(
    source: &ByteStore,
    offset: usize,
    limits: ParseLimits,
    resolved_length: Option<ResolvedStreamLength>,
    mut repairs: Option<&mut Vec<IndirectRepair>>,
) -> Result<IndirectObject, IndirectObjectError> {
    let mut parser = ObjectParser::new(source, offset, limits);
    if repairs.is_some() {
        parser = parser.tolerating_non_name_keys();
    }
    let object_number = required_token(&mut parser, offset)?;
    let generation_offset = parser.offset();
    let generation = required_token(&mut parser, generation_offset)?;
    let keyword_offset = parser.offset();
    let obj_keyword = required_token(&mut parser, keyword_offset)?;

    let object_number_value = parse_token_u32(source, object_number)
        .filter(|value| *value != 0)
        .ok_or_else(|| {
            IndirectObjectError::new(
                object_number.span().start(),
                IndirectObjectErrorKind::InvalidObjectNumber,
            )
        })?;
    let generation_value = parse_token_u16(source, generation).ok_or_else(|| {
        IndirectObjectError::new(
            generation.span().start(),
            IndirectObjectErrorKind::InvalidGeneration,
        )
    })?;
    if !parser.token_equals(obj_keyword, b"obj") {
        return Err(IndirectObjectError::new(
            obj_keyword.span().start(),
            IndirectObjectErrorKind::MissingObjKeyword,
        ));
    }

    let value = parser
        .parse_next()
        .map_err(IndirectObjectError::from)?
        .ok_or_else(|| {
            IndirectObjectError::new(parser.offset(), IndirectObjectErrorKind::MissingValue)
        })?;
    if let Some(repairs) = repairs.as_deref_mut() {
        repairs.extend(parser.take_repairs());
    }
    let next_offset = parser.offset();
    let next = required_token(&mut parser, next_offset)?;

    let (stream, ends_at) = if parser.token_equals(next, b"stream") {
        let stream = parse_stream(
            source,
            &value,
            next,
            limits,
            resolved_length,
            repairs.as_deref_mut(),
        )?;
        let after_stream = stream.end_keyword_span().end();
        let mut ending_parser = ObjectParser::new(source, after_stream, limits);
        let endobj = required_token(&mut ending_parser, after_stream)?;
        let ends_at = if ending_parser.token_equals(endobj, b"endobj") {
            endobj.span().end()
        } else {
            record(
                &mut repairs,
                IndirectRepair::MissingEndObj {
                    byte_offset: endobj.span().start(),
                },
                IndirectObjectError::new(
                    endobj.span().start(),
                    IndirectObjectErrorKind::MissingEndObjKeyword,
                ),
            )?;
            after_stream
        };
        (Some(stream), ends_at)
    } else if parser.token_equals(next, b"endobj") {
        (None, next.span().end())
    } else {
        record(
            &mut repairs,
            IndirectRepair::MissingEndObj {
                byte_offset: next.span().start(),
            },
            IndirectObjectError::new(
                next.span().start(),
                IndirectObjectErrorKind::MissingEndObjKeyword,
            ),
        )?;
        (None, value.span().end())
    };

    let span = source
        .span(object_number.span().start()..ends_at)
        .map_err(|_| {
            IndirectObjectError::new(offset, IndirectObjectErrorKind::SourceSpanFailure)
        })?;

    Ok(IndirectObject {
        reference: Reference::new(object_number_value, generation_value),
        value,
        stream,
        span,
    })
}

fn record(
    repairs: &mut Option<&mut Vec<IndirectRepair>>,
    repair: IndirectRepair,
    refusal: IndirectObjectError,
) -> Result<(), IndirectObjectError> {
    match repairs {
        Some(taken) => {
            taken.push(repair);
            Ok(())
        }
        None => Err(refusal),
    }
}

fn endstream_after(bytes: &[u8], from: usize) -> Option<(usize, usize)> {
    const KEYWORD: &[u8] = b"endstream";
    let at = from
        + bytes
            .get(from..)?
            .windows(KEYWORD.len())
            .position(|window| window == KEYWORD)?;
    let mut end = at;
    if end > from && bytes.get(end - 1) == Some(&b'\n') {
        end -= 1;
    }
    if end > from && bytes.get(end - 1) == Some(&b'\r') {
        end -= 1;
    }
    Some((end, at))
}

fn parse_stream(
    source: &ByteStore,
    dictionary: &Object,
    stream_keyword: Token,
    limits: ParseLimits,
    resolved_length: Option<ResolvedStreamLength>,
    mut repairs: Option<&mut Vec<IndirectRepair>>,
) -> Result<StreamObject, IndirectObjectError> {
    let ObjectKind::Dictionary(entries) = dictionary.kind() else {
        return Err(IndirectObjectError::new(
            stream_keyword.span().start(),
            IndirectObjectErrorKind::StreamRequiresDictionary,
        ));
    };

    let data_start = consume_required_eol(source.as_bytes(), stream_keyword.span().end())
        .ok_or_else(|| {
            IndirectObjectError::new(
                stream_keyword.span().end(),
                IndirectObjectErrorKind::MissingStreamLineEnding,
            )
        })?;

    let mut lengths = entries
        .iter()
        .filter(|entry| entry.key_equals(source, b"/Length"));
    let length = lengths.next();
    if length.is_some() && lengths.next().is_some() {
        return Err(IndirectObjectError::new(
            dictionary.span().start(),
            IndirectObjectErrorKind::DuplicateStreamLength,
        ));
    }

    let declared = declared_length(
        source,
        dictionary,
        data_start,
        length,
        resolved_length,
        &mut repairs,
    )?;

    let (data_end, end_keyword) =
        stream_boundary(source, data_start, declared, limits, &mut repairs)?;

    let data_span = source.span(data_start..data_end).map_err(|_| {
        IndirectObjectError::new(data_start, IndirectObjectErrorKind::StreamOutOfBounds)
    })?;

    Ok(StreamObject {
        keyword: stream_keyword.span(),
        data: data_span,
        end_keyword,
    })
}

fn declared_length(
    source: &ByteStore,
    dictionary: &Object,
    data_start: usize,
    length: Option<&crate::DictionaryEntry>,
    resolved_length: Option<ResolvedStreamLength>,
    repairs: &mut Option<&mut Vec<IndirectRepair>>,
) -> Result<Option<(usize, usize)>, IndirectObjectError> {
    Ok(match length {
        None => {
            let (data_end, keyword_at) = endstream_after(source.as_bytes(), data_start)
                .ok_or_else(|| {
                    IndirectObjectError::new(
                        dictionary.span().start(),
                        IndirectObjectErrorKind::MissingStreamLength,
                    )
                })?;
            record(
                repairs,
                IndirectRepair::StreamLengthFromEndStream {
                    byte_offset: dictionary.span().start(),
                    length: data_end - data_start,
                },
                IndirectObjectError::new(
                    dictionary.span().start(),
                    IndirectObjectErrorKind::MissingStreamLength,
                ),
            )?;
            let _ = keyword_at;
            None
        }
        Some(length) => Some(match length.value().kind() {
            ObjectKind::Number(crate::NumberKind::Integer) => {
                let bytes = source.resolve(length.value().span()).map_err(|_| {
                    IndirectObjectError::new(
                        length.value().span().start(),
                        IndirectObjectErrorKind::InvalidStreamLength,
                    )
                })?;
                (
                    parse_unsigned(bytes)
                        .and_then(|value| usize::try_from(value).ok())
                        .ok_or_else(|| {
                            IndirectObjectError::new(
                                length.value().span().start(),
                                IndirectObjectErrorKind::InvalidStreamLength,
                            )
                        })?,
                    length.value().span().start(),
                )
            }
            ObjectKind::Reference(reference) => {
                let Some(resolved) = resolved_length else {
                    return Err(IndirectObjectError::new(
                        length.value().span().start(),
                        IndirectObjectErrorKind::UnresolvedStreamLength(*reference),
                    ));
                };
                if resolved.reference() != *reference {
                    return Err(IndirectObjectError::new(
                        length.value().span().start(),
                        IndirectObjectErrorKind::ResolvedLengthReferenceMismatch {
                            expected: *reference,
                            actual: resolved.reference(),
                        },
                    ));
                }
                (resolved.value(), length.value().span().start())
            }
            _ => {
                return Err(IndirectObjectError::new(
                    length.value().span().start(),
                    IndirectObjectErrorKind::InvalidStreamLength,
                ));
            }
        }),
    })
}

fn stream_boundary(
    source: &ByteStore,
    data_start: usize,
    declared: Option<(usize, usize)>,
    limits: ParseLimits,
    repairs: &mut Option<&mut Vec<IndirectRepair>>,
) -> Result<(usize, SourceSpan), IndirectObjectError> {
    let searched = |repairs: &mut Option<&mut Vec<IndirectRepair>>| {
        let (found_end, _) = endstream_after(source.as_bytes(), data_start).ok_or_else(|| {
            IndirectObjectError::new(data_start, IndirectObjectErrorKind::MissingEndStreamKeyword)
        })?;
        let keyword = keyword_at(source, found_end, limits).ok_or_else(|| {
            IndirectObjectError::new(found_end, IndirectObjectErrorKind::MissingEndStreamKeyword)
        })?;
        let _ = repairs;
        Ok::<_, IndirectObjectError>((found_end, keyword))
    };
    let Some((declared, at)) = declared else {
        return searched(repairs);
    };
    let data_end = data_start.checked_add(declared).ok_or_else(|| {
        IndirectObjectError::new(data_start, IndirectObjectErrorKind::StreamOutOfBounds)
    })?;
    if let Some(keyword) = keyword_at(source, data_end, limits) {
        return Ok((data_end, keyword));
    }
    let (found_end, keyword) = searched(repairs)?;
    record(
        repairs,
        IndirectRepair::StreamLengthDisagrees {
            byte_offset: at,
            declared,
            found: found_end - data_start,
        },
        IndirectObjectError::new(data_end, IndirectObjectErrorKind::MissingEndStreamKeyword),
    )?;
    Ok((found_end, keyword))
}

fn keyword_at(source: &ByteStore, data_end: usize, limits: ParseLimits) -> Option<SourceSpan> {
    let start = consume_required_eol(source.as_bytes(), data_end).unwrap_or(data_end);
    let mut parser = ObjectParser::new(source, start, limits);
    let token = parser.take_token().ok()??;
    parser
        .token_equals(token, b"endstream")
        .then(|| token.span())
}

fn consume_required_eol(bytes: &[u8], offset: usize) -> Option<usize> {
    match bytes.get(offset) {
        Some(b'\r') if bytes.get(offset + 1) == Some(&b'\n') => Some(offset + 2),
        Some(b'\n' | b'\r') => Some(offset + 1),
        _ => None,
    }
}

fn required_token(
    parser: &mut ObjectParser<'_>,
    offset: usize,
) -> Result<Token, IndirectObjectError> {
    parser
        .take_token()
        .map_err(IndirectObjectError::from)?
        .ok_or_else(|| {
            IndirectObjectError::new(offset, IndirectObjectErrorKind::UnexpectedEndOfInput)
        })
}

fn parse_token_u32(source: &ByteStore, token: Token) -> Option<u32> {
    if token.kind() != TokenKind::Number(crate::NumberKind::Integer) {
        return None;
    }
    source
        .resolve(token.span())
        .ok()
        .and_then(parse_unsigned)
        .and_then(|value| u32::try_from(value).ok())
}

fn parse_token_u16(source: &ByteStore, token: Token) -> Option<u16> {
    parse_token_u32(source, token).and_then(|value| u16::try_from(value).ok())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndirectObjectError {
    offset: usize,
    kind: IndirectObjectErrorKind,
}

impl IndirectObjectError {
    const fn new(offset: usize, kind: IndirectObjectErrorKind) -> Self {
        Self { offset, kind }
    }

    #[must_use]
    pub const fn offset(self) -> usize {
        self.offset
    }

    #[must_use]
    pub const fn kind(self) -> IndirectObjectErrorKind {
        self.kind
    }
}

impl From<ParseError> for IndirectObjectError {
    fn from(error: ParseError) -> Self {
        Self {
            offset: error.offset(),
            kind: IndirectObjectErrorKind::Object(error.kind()),
        }
    }
}

impl fmt::Display for IndirectObjectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} at byte {}", self.kind, self.offset)
    }
}

impl std::error::Error for IndirectObjectError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndirectObjectErrorKind {
    Object(crate::ParseErrorKind),
    UnexpectedEndOfInput,
    InvalidObjectNumber,
    InvalidGeneration,
    MissingObjKeyword,
    MissingValue,
    MissingEndObjKeyword,
    StreamRequiresDictionary,
    MissingStreamLength,
    DuplicateStreamLength,
    InvalidStreamLength,
    UnresolvedStreamLength(Reference),
    ResolvedLengthReferenceMismatch {
        expected: Reference,
        actual: Reference,
    },
    MissingStreamLineEnding,
    StreamOutOfBounds,
    MissingEndStreamKeyword,
    SourceSpanFailure,
}

impl fmt::Display for IndirectObjectErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Object(kind) => write!(formatter, "invalid direct object: {kind}"),
            Self::UnresolvedStreamLength(reference) => write!(
                formatter,
                "stream length is the unresolved reference {} {} R",
                reference.object_number(),
                reference.generation()
            ),
            Self::ResolvedLengthReferenceMismatch { expected, actual } => write!(
                formatter,
                "resolved stream length belongs to {} {} R, expected {} {} R",
                actual.object_number(),
                actual.generation(),
                expected.object_number(),
                expected.generation()
            ),
            _ => formatter.write_str(match self {
                Self::UnexpectedEndOfInput => "unexpected end of indirect object",
                Self::InvalidObjectNumber => "invalid indirect object number",
                Self::InvalidGeneration => "invalid indirect object generation",
                Self::MissingObjKeyword => "missing obj keyword",
                Self::MissingValue => "missing indirect object value",
                Self::MissingEndObjKeyword => "missing endobj keyword",
                Self::StreamRequiresDictionary => "stream value is not a dictionary",
                Self::MissingStreamLength => "stream dictionary has no Length",
                Self::DuplicateStreamLength => "stream dictionary has duplicate Length keys",
                Self::InvalidStreamLength => "stream Length is not a non-negative integer",
                Self::MissingStreamLineEnding => "stream keyword has no required line ending",
                Self::StreamOutOfBounds => "stream data exceeds the source",
                Self::MissingEndStreamKeyword => "missing endstream keyword at Length boundary",
                Self::SourceSpanFailure => "indirect object span exceeds its source",
                Self::Object(_)
                | Self::UnresolvedStreamLength(_)
                | Self::ResolvedLengthReferenceMismatch { .. } => unreachable!(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use pdf_bytes::{ByteStore, SourceId};

    use super::{
        IndirectObjectErrorKind, IndirectRepair, ParseLimits, Reference,
        parse_indirect_object_recovering, parse_indirect_object_strict,
    };

    fn source(bytes: &'static [u8]) -> ByteStore {
        ByteStore::new(SourceId::new(12), bytes)
    }

    #[test]
    fn a_token_where_a_dictionary_key_belongs_is_stepped_over_only_by_a_recovering_read() {
        let bytes = source(b"1 0 obj << /Type /Catalog 42 /Pages 2 0 R >>\nendobj\n");
        assert_eq!(
            parse_indirect_object_strict(&bytes, 0, ParseLimits::default())
                .expect_err("strict refuses a key that is not a name")
                .kind(),
            IndirectObjectErrorKind::Object(crate::ParseErrorKind::DictionaryKeyNotName)
        );

        let (object, repairs) =
            parse_indirect_object_recovering(&bytes, 0, ParseLimits::default(), None)
                .expect("a recovering read steps over it");
        let crate::ObjectKind::Dictionary(entries) = object.value().kind() else {
            panic!("the object is a dictionary");
        };
        assert_eq!(entries.len(), 2);
        assert!(entries[0].key_equals(&bytes, b"/Type"));
        assert!(entries[1].key_equals(&bytes, b"/Pages"));
        assert!(
            repairs
                .iter()
                .any(|repair| matches!(repair, IndirectRepair::SkippedDictionaryKey { .. })),
            "the step is recorded, not silent: {repairs:?}"
        );
    }

    #[test]
    fn a_recovering_read_repairs_three_kinds_of_damage_and_names_each_one() {
        let missing_endobj = source(b"1 0 obj <<\n  /Type /Catalog\n>>\n2 0 obj <<\n>>\nendobj\n");
        assert_eq!(
            parse_indirect_object_strict(&missing_endobj, 0, ParseLimits::default())
                .expect_err("strict refuses")
                .kind(),
            IndirectObjectErrorKind::MissingEndObjKeyword
        );
        let (object, repairs) =
            parse_indirect_object_recovering(&missing_endobj, 0, ParseLimits::default(), None)
                .expect("the value is complete");
        assert_eq!(object.reference(), Reference::new(1, 0));
        assert_eq!(
            missing_endobj.resolve(object.span()),
            Ok(&b"1 0 obj <<\n  /Type /Catalog\n>>"[..]),
            "the object ends where its value does"
        );
        assert!(matches!(
            repairs.as_slice(),
            [IndirectRepair::MissingEndObj { .. }]
        ));

        let no_length = source(b"6 0 obj <<\n>>\nstream\nBT ET\nendstream\nendobj\n");
        assert_eq!(
            parse_indirect_object_strict(&no_length, 0, ParseLimits::default())
                .expect_err("strict refuses")
                .kind(),
            IndirectObjectErrorKind::MissingStreamLength
        );
        let (object, repairs) =
            parse_indirect_object_recovering(&no_length, 0, ParseLimits::default(), None)
                .expect("endstream bounds it");
        assert_eq!(
            no_length.resolve(object.stream().expect("a stream").data_span()),
            Ok(&b"BT ET"[..])
        );
        assert_eq!(
            repairs,
            vec![IndirectRepair::StreamLengthFromEndStream {
                byte_offset: 8,
                length: 5
            }]
        );

        let wrong_length = source(b"4 0 obj << /Length 2 >>\nstream\nBT ET\nendstream\nendobj\n");
        assert_eq!(
            parse_indirect_object_strict(&wrong_length, 0, ParseLimits::default())
                .expect_err("strict refuses")
                .kind(),
            IndirectObjectErrorKind::MissingEndStreamKeyword
        );
        let (object, repairs) =
            parse_indirect_object_recovering(&wrong_length, 0, ParseLimits::default(), None)
                .expect("the keyword decides");
        assert_eq!(
            wrong_length.resolve(object.stream().expect("a stream").data_span()),
            Ok(&b"BT ET"[..])
        );
        assert_eq!(
            repairs,
            vec![IndirectRepair::StreamLengthDisagrees {
                byte_offset: 19,
                declared: 2,
                found: 5
            }]
        );
    }

    #[test]
    fn a_well_formed_stream_holding_the_endstream_bytes_is_not_repaired() {
        let source =
            source(b"7 0 obj\n<< /Length 17 >>\nstream\nabcendstreamxyz!!\nendstream\nendobj");
        let strict = parse_indirect_object_strict(&source, 0, ParseLimits::default())
            .expect("valid stream object");
        let (recovered, repairs) =
            parse_indirect_object_recovering(&source, 0, ParseLimits::default(), None)
                .expect("valid stream object");
        assert!(repairs.is_empty(), "nothing to repair: {repairs:?}");
        assert_eq!(strict, recovered);
        assert_eq!(
            source.resolve(recovered.stream().expect("a stream").data_span()),
            Ok(&b"abcendstreamxyz!!"[..])
        );
    }

    #[test]
    fn a_stream_with_no_endstream_at_all_is_refused_by_both_reads() {
        let truncated = source(b"9 0 obj << /Length 4 >>\nstream\nBT ET");
        assert!(parse_indirect_object_strict(&truncated, 0, ParseLimits::default()).is_err());
        assert!(
            parse_indirect_object_recovering(&truncated, 0, ParseLimits::default(), None).is_err()
        );

        let not_an_object = source(b"1 0 xyz << >>\nendobj\n");
        assert!(
            parse_indirect_object_recovering(&not_an_object, 0, ParseLimits::default(), None)
                .is_err(),
            "a header that is not N G obj is not damage this reads around"
        );
    }

    #[test]
    fn parses_a_non_stream_indirect_object_and_its_exact_span() {
        let source = source(b"garbage 12 3 obj\n[1 0 R /Name]\nendobj trailing");
        let object = parse_indirect_object_strict(&source, 8, ParseLimits::default())
            .expect("valid indirect object");

        assert_eq!(object.reference(), Reference::new(12, 3));
        assert_eq!(
            source.resolve(object.span()),
            Ok(&b"12 3 obj\n[1 0 R /Name]\nendobj"[..])
        );
        assert!(object.stream().is_none());
    }

    #[test]
    fn stream_length_not_endstream_text_decides_the_binary_boundary() {
        let source =
            source(b"7 0 obj\n<< /Length 17 >>\nstream\nabcendstreamxyz!!\nendstream\nendobj");
        let object = parse_indirect_object_strict(&source, 0, ParseLimits::default())
            .expect("valid stream object");
        let stream = object.stream().expect("stream metadata");

        assert_eq!(
            source.resolve(stream.data_span()),
            Ok(&b"abcendstreamxyz!!"[..])
        );
    }

    #[test]
    fn reports_an_indirect_stream_length_instead_of_scanning() {
        let source = source(b"7 0 obj\n<< /Length 9 0 R >>\nstream\nabc\nendstream\nendobj");
        let error = parse_indirect_object_strict(&source, 0, ParseLimits::default())
            .expect_err("unresolved length must be explicit");

        assert_eq!(
            error.kind(),
            IndirectObjectErrorKind::UnresolvedStreamLength(Reference::new(9, 0))
        );
    }

    #[test]
    fn dictionary_name_escapes_have_their_pdf_meaning() {
        let source = source(b"7 0 obj\n<< /Len#67th 3 >>\nstream\nabc\nendstream\nendobj");
        let object = parse_indirect_object_strict(&source, 0, ParseLimits::default())
            .expect("escaped Length key");

        assert_eq!(
            source.resolve(object.stream().expect("stream").data_span()),
            Ok(&b"abc"[..])
        );
    }

    #[test]
    fn accepts_a_final_line_ending_included_in_stream_length() {
        let source = source(b"1 0 obj << /Length 2 >> stream\nx\nendstream endobj");
        let object = parse_indirect_object_strict(&source, 0, ParseLimits::default())
            .expect("the recommended extra EOL before endstream is optional");

        assert_eq!(
            source.resolve(object.stream().expect("stream").data_span()),
            Ok(&b"x\n"[..])
        );
    }

    #[test]
    fn rejects_missing_or_duplicate_lengths_and_wrong_boundaries() {
        let cases: &[(&[u8], IndirectObjectErrorKind)] = &[
            (
                b"1 0 obj <<>> stream\nx\nendstream endobj",
                IndirectObjectErrorKind::MissingStreamLength,
            ),
            (
                b"1 0 obj << /Length 1 /Length 1 >> stream\nx\nendstream endobj",
                IndirectObjectErrorKind::DuplicateStreamLength,
            ),
            (
                b"1 0 obj << /Length 1 >> stream x\nendstream endobj",
                IndirectObjectErrorKind::MissingStreamLineEnding,
            ),
            (
                b"1 0 obj << /Length 1 >> stream\nxy\nendstream endobj",
                IndirectObjectErrorKind::MissingEndStreamKeyword,
            ),
        ];

        for &(bytes, expected) in cases {
            let source = ByteStore::new(SourceId::new(1), bytes);
            let error = parse_indirect_object_strict(&source, 0, ParseLimits::default())
                .expect_err("malformed stream must fail");
            assert_eq!(error.kind(), expected);
        }
    }

    #[test]
    fn object_zero_and_out_of_range_generations_are_rejected() {
        let cases: &[(&[u8], IndirectObjectErrorKind)] = &[
            (
                b"0 0 obj null endobj",
                IndirectObjectErrorKind::InvalidObjectNumber,
            ),
            (
                b"1 65536 obj null endobj",
                IndirectObjectErrorKind::InvalidGeneration,
            ),
        ];
        for &(bytes, expected) in cases {
            let source = ByteStore::new(SourceId::new(1), bytes);
            let error = parse_indirect_object_strict(&source, 0, ParseLimits::default())
                .expect_err("invalid header must fail");
            assert_eq!(error.kind(), expected);
        }
    }
}
