use std::fmt;

use pdf_bytes::{ByteStore, SourceId};

use crate::filter::{decode_stream, decode_stream_bytes};
use crate::resolve::{DecryptionRefused, StreamDecryptor};
use crate::value::{name_object_equals, parse_unsigned};
use crate::{
    DictionaryEntry, IndirectObject, LexError, Lexer, NumberKind, Object, ObjectKind, ObjectParser,
    ParseError, ParseLimits, Reference, StreamDecodeError, TokenKind,
};

#[derive(Clone, Debug)]
pub(crate) struct CompressedObject {
    source: ByteStore,
    value: Object,
}

impl CompressedObject {
    pub(crate) const fn source(&self) -> &ByteStore {
        &self.source
    }

    pub(crate) const fn value(&self) -> &Object {
        &self.value
    }
}

fn decode_object_stream(
    source: &ByteStore,
    dictionary: &[DictionaryEntry],
    stream: &crate::StreamObject,
    reference: Reference,
    max_decoded_bytes: usize,
    decryptor: Option<&dyn StreamDecryptor>,
) -> Result<Vec<u8>, ObjectStreamError> {
    let Some(decryptor) = decryptor else {
        return Ok(decode_stream(
            source,
            dictionary,
            stream,
            max_decoded_bytes,
        )?);
    };
    let data_start = stream.data_span().start();
    let encrypted = source.resolve(stream.data_span()).map_err(|_| {
        ObjectStreamError::new(data_start, ObjectStreamErrorKind::SourceSpanFailure)
    })?;
    let plaintext = decryptor
        .decrypt_stream(reference, encrypted)
        .map_err(|refused| {
            ObjectStreamError::new(data_start, ObjectStreamErrorKind::Decryption(refused))
        })?;
    Ok(decode_stream_bytes(
        source,
        dictionary,
        &plaintext,
        data_start,
        max_decoded_bytes,
    )?)
}

pub(crate) fn parse_compressed_object_strict(
    source: &ByteStore,
    object_stream: &IndirectObject,
    object_stream_revision: usize,
    expected: Reference,
    index: u32,
    limits: ObjectStreamLimits,
    decryptor: Option<&dyn StreamDecryptor>,
) -> Result<CompressedObject, ObjectStreamError> {
    let (dictionary, stream, count, first) =
        stream_metadata(source, object_stream, limits.max_objects)?;
    let requested_index = usize::try_from(index).map_err(|_| {
        ObjectStreamError::new(
            object_stream.value().span().start(),
            ObjectStreamErrorKind::IndexOutOfBounds,
        )
    })?;
    if requested_index >= count {
        return Err(ObjectStreamError::new(
            object_stream.value().span().start(),
            ObjectStreamErrorKind::IndexOutOfBounds,
        ));
    }

    let decoded = decode_object_stream(
        source,
        dictionary,
        stream,
        object_stream.reference(),
        limits.max_decoded_bytes,
        decryptor,
    )?;
    if first > decoded.len() {
        return Err(ObjectStreamError::new(
            object_stream.value().span().start(),
            ObjectStreamErrorKind::InvalidFirst,
        ));
    }
    let derivation = derived_discriminator(object_stream_revision, object_stream.reference())?;
    let decoded_source = ByteStore::new(SourceId::derived(source.id(), derivation), decoded);
    let header = parse_header(&decoded_source, count, first, limits.objects)?;
    let (actual_number, relative_offset) = header[requested_index];
    if actual_number != expected.object_number() {
        return Err(ObjectStreamError::new(
            first,
            ObjectStreamErrorKind::ObjectNumberMismatch {
                actual: actual_number,
            },
        ));
    }

    let start = first.checked_add(relative_offset).ok_or_else(|| {
        ObjectStreamError::new(first, ObjectStreamErrorKind::ObjectOffsetOutOfBounds)
    })?;
    let end = header
        .get(requested_index + 1)
        .and_then(|(_, next)| first.checked_add(*next))
        .unwrap_or(decoded_source.len());
    if start >= end || end > decoded_source.len() {
        return Err(ObjectStreamError::new(
            start,
            ObjectStreamErrorKind::ObjectOffsetOutOfBounds,
        ));
    }

    let mut parser = ObjectParser::new(&decoded_source, start, limits.objects);
    let value = parser
        .parse_next()?
        .ok_or_else(|| ObjectStreamError::new(start, ObjectStreamErrorKind::MissingObject))?;
    let parsed_end = parser.offset();
    if parsed_end > end || !only_trivia(&decoded_source.as_bytes()[parsed_end..end]) {
        return Err(ObjectStreamError::new(
            parsed_end,
            ObjectStreamErrorKind::ObjectCrossesBoundary,
        ));
    }

    Ok(CompressedObject {
        source: decoded_source,
        value,
    })
}

pub(crate) fn object_stream_contents(
    source: &ByteStore,
    object_stream: &IndirectObject,
    object_stream_revision: usize,
    limits: ObjectStreamLimits,
    decryptor: Option<&dyn StreamDecryptor>,
) -> Result<Vec<(u32, u32)>, ObjectStreamError> {
    let (dictionary, stream, count, first) =
        stream_metadata(source, object_stream, limits.max_objects)?;
    let decoded = decode_object_stream(
        source,
        dictionary,
        stream,
        object_stream.reference(),
        limits.max_decoded_bytes,
        decryptor,
    )?;
    if first > decoded.len() {
        return Err(ObjectStreamError::new(
            object_stream.value().span().start(),
            ObjectStreamErrorKind::InvalidFirst,
        ));
    }
    let derivation = derived_discriminator(object_stream_revision, object_stream.reference())?;
    let decoded_source = ByteStore::new(SourceId::derived(source.id(), derivation), decoded);
    let header = parse_header(&decoded_source, count, first, limits.objects)?;

    let mut contents = Vec::with_capacity(header.len());
    for (index, (object_number, _)) in header.iter().enumerate() {
        let index = u32::try_from(index)
            .map_err(|_| ObjectStreamError::new(first, ObjectStreamErrorKind::IndexOutOfBounds))?;
        contents.push((*object_number, index));
    }
    Ok(contents)
}

fn stream_metadata<'a>(
    source: &ByteStore,
    object_stream: &'a IndirectObject,
    max_objects: usize,
) -> Result<(&'a [DictionaryEntry], &'a crate::StreamObject, usize, usize), ObjectStreamError> {
    let ObjectKind::Dictionary(dictionary) = object_stream.value().kind() else {
        return Err(ObjectStreamError::new(
            object_stream.value().span().start(),
            ObjectStreamErrorKind::NotDictionary,
        ));
    };
    let stream = object_stream.stream().ok_or_else(|| {
        ObjectStreamError::new(
            object_stream.value().span().end(),
            ObjectStreamErrorKind::MissingStream,
        )
    })?;
    let type_value = required_unique(
        source,
        dictionary,
        b"/Type",
        ObjectStreamErrorKind::WrongType,
    )?;
    if !name_object_equals(source, type_value, b"/ObjStm") {
        return Err(ObjectStreamError::new(
            type_value.span().start(),
            ObjectStreamErrorKind::WrongType,
        ));
    }
    let count_value = required_unique(
        source,
        dictionary,
        b"/N",
        ObjectStreamErrorKind::MissingCount,
    )?;
    let count = integer_usize(source, count_value)
        .filter(|count| *count <= max_objects)
        .ok_or_else(|| {
            ObjectStreamError::new(
                count_value.span().start(),
                ObjectStreamErrorKind::InvalidCount,
            )
        })?;
    let first_value = required_unique(
        source,
        dictionary,
        b"/First",
        ObjectStreamErrorKind::MissingFirst,
    )?;
    let first = integer_usize(source, first_value).ok_or_else(|| {
        ObjectStreamError::new(
            first_value.span().start(),
            ObjectStreamErrorKind::InvalidFirst,
        )
    })?;
    Ok((dictionary, stream, count, first))
}

fn derived_discriminator(
    revision: usize,
    object_stream: Reference,
) -> Result<u64, ObjectStreamError> {
    let revision = u64::try_from(revision)
        .ok()
        .and_then(|revision| revision.checked_add(1))
        .filter(|revision| u32::try_from(*revision).is_ok())
        .ok_or_else(|| ObjectStreamError::new(0, ObjectStreamErrorKind::SourceIdentityOverflow))?;
    Ok((revision << 32) | u64::from(object_stream.object_number()))
}

fn parse_header(
    source: &ByteStore,
    count: usize,
    first: usize,
    limits: ParseLimits,
) -> Result<Vec<(u32, usize)>, ObjectStreamError> {
    let mut lexer = Lexer::new(source, 0, limits.lex);
    let mut entries = Vec::with_capacity(count);
    let mut previous_offset = None;
    for _ in 0..count {
        let number_token = lexer
            .next_token()?
            .ok_or_else(|| ObjectStreamError::new(0, ObjectStreamErrorKind::MalformedHeader))?;
        let offset_token = lexer.next_token()?.ok_or_else(|| {
            ObjectStreamError::new(
                number_token.span().end(),
                ObjectStreamErrorKind::MalformedHeader,
            )
        })?;
        let object_number = integer_token(source, number_token)
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| *value != 0)
            .ok_or_else(|| {
                ObjectStreamError::new(
                    number_token.span().start(),
                    ObjectStreamErrorKind::MalformedHeader,
                )
            })?;
        let relative_offset = integer_token(source, offset_token)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| {
                ObjectStreamError::new(
                    offset_token.span().start(),
                    ObjectStreamErrorKind::MalformedHeader,
                )
            })?;
        if previous_offset.is_some_and(|previous| relative_offset <= previous) {
            return Err(ObjectStreamError::new(
                offset_token.span().start(),
                ObjectStreamErrorKind::NonIncreasingOffsets,
            ));
        }
        previous_offset = Some(relative_offset);
        entries.push((object_number, relative_offset));
    }
    if lexer.offset() > first || !only_trivia(&source.as_bytes()[lexer.offset()..first]) {
        return Err(ObjectStreamError::new(
            lexer.offset(),
            ObjectStreamErrorKind::InvalidFirst,
        ));
    }
    Ok(entries)
}

fn required_unique<'a>(
    source: &ByteStore,
    dictionary: &'a [DictionaryEntry],
    key: &[u8],
    missing: ObjectStreamErrorKind,
) -> Result<&'a Object, ObjectStreamError> {
    let mut matches = dictionary
        .iter()
        .filter(|entry| entry.key_equals(source, key));
    let Some(first) = matches.next() else {
        return Err(ObjectStreamError::new(
            dictionary
                .first()
                .map_or(0, |entry| entry.key().span().start()),
            missing,
        ));
    };
    if matches.next().is_some() {
        return Err(ObjectStreamError::new(
            first.key().span().start(),
            ObjectStreamErrorKind::DuplicateParameter,
        ));
    }
    Ok(first.value())
}

fn integer_usize(source: &ByteStore, object: &Object) -> Option<usize> {
    if !matches!(object.kind(), ObjectKind::Number(NumberKind::Integer)) {
        return None;
    }
    source
        .resolve(object.span())
        .ok()
        .and_then(parse_unsigned)
        .and_then(|value| usize::try_from(value).ok())
}

fn integer_token(source: &ByteStore, token: crate::Token) -> Option<u64> {
    if token.kind() != TokenKind::Number(NumberKind::Integer) {
        return None;
    }
    source.resolve(token.span()).ok().and_then(parse_unsigned)
}

fn only_trivia(bytes: &[u8]) -> bool {
    let mut cursor = 0;
    while cursor < bytes.len() {
        if is_whitespace(bytes[cursor]) {
            cursor += 1;
        } else if bytes[cursor] == b'%' {
            cursor += 1;
            while cursor < bytes.len() && !matches!(bytes[cursor], b'\r' | b'\n') {
                cursor += 1;
            }
        } else {
            return false;
        }
    }
    true
}

const fn is_whitespace(byte: u8) -> bool {
    matches!(byte, 0x00 | b'\t' | b'\n' | 0x0c | b'\r' | b' ')
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ObjectStreamLimits {
    pub objects: ParseLimits,
    pub max_objects: usize,
    pub max_decoded_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObjectStreamError {
    offset: usize,
    kind: ObjectStreamErrorKind,
}

impl ObjectStreamError {
    const fn new(offset: usize, kind: ObjectStreamErrorKind) -> Self {
        Self { offset, kind }
    }

    #[must_use]
    pub const fn offset(self) -> usize {
        self.offset
    }

    #[must_use]
    pub const fn kind(self) -> ObjectStreamErrorKind {
        self.kind
    }
}

impl From<LexError> for ObjectStreamError {
    fn from(error: LexError) -> Self {
        Self::new(error.offset(), ObjectStreamErrorKind::Lexical(error.kind()))
    }
}

impl From<ParseError> for ObjectStreamError {
    fn from(error: ParseError) -> Self {
        Self::new(error.offset(), ObjectStreamErrorKind::Object(error.kind()))
    }
}

impl From<StreamDecodeError> for ObjectStreamError {
    fn from(error: StreamDecodeError) -> Self {
        Self::new(error.offset(), ObjectStreamErrorKind::Decode(error.kind()))
    }
}

impl fmt::Display for ObjectStreamError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} at byte {}", self.kind, self.offset)
    }
}

impl std::error::Error for ObjectStreamError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectStreamErrorKind {
    Lexical(crate::LexErrorKind),
    Object(crate::ParseErrorKind),
    Decode(crate::StreamDecodeErrorKind),
    NotDictionary,
    MissingStream,
    WrongType,
    MissingCount,
    InvalidCount,
    MissingFirst,
    InvalidFirst,
    DuplicateParameter,
    IndexOutOfBounds,
    MalformedHeader,
    NonIncreasingOffsets,
    ObjectNumberMismatch { actual: u32 },
    ObjectOffsetOutOfBounds,
    MissingObject,
    ObjectCrossesBoundary,
    SourceIdentityOverflow,
    SourceSpanFailure,
    Decryption(DecryptionRefused),
}

impl fmt::Display for ObjectStreamErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lexical(kind) => write!(formatter, "invalid object-stream header token: {kind}"),
            Self::Object(kind) => write!(formatter, "invalid compressed object: {kind}"),
            Self::Decode(kind) => write!(formatter, "cannot decode object stream: {kind}"),
            Self::Decryption(refused) => {
                write!(formatter, "cannot decrypt object stream: {refused}")
            }
            Self::ObjectNumberMismatch { actual } => {
                write!(formatter, "object-stream header declares object {actual}")
            }
            _ => formatter.write_str(match self {
                Self::NotDictionary => "object stream value is not a dictionary",
                Self::MissingStream => "object stream has no stream data",
                Self::WrongType => "object stream Type is not ObjStm",
                Self::MissingCount => "object stream dictionary has no N",
                Self::InvalidCount => "object stream N is invalid or exceeds its limit",
                Self::MissingFirst => "object stream dictionary has no First",
                Self::InvalidFirst => "object stream First is invalid",
                Self::DuplicateParameter => "object stream dictionary repeats a required key",
                Self::IndexOutOfBounds => "compressed-object index lies outside object stream N",
                Self::MalformedHeader => "object stream header is malformed",
                Self::NonIncreasingOffsets => "object stream offsets are not strictly increasing",
                Self::ObjectOffsetOutOfBounds => "compressed-object offset lies outside its stream",
                Self::SourceSpanFailure => "object stream span does not belong to the source",
                Self::MissingObject => "object stream entry has no direct object",
                Self::ObjectCrossesBoundary => "compressed object crosses its declared boundary",
                Self::SourceIdentityOverflow => "cannot allocate a derived source identity",
                Self::Lexical(_)
                | Self::Object(_)
                | Self::Decode(_)
                | Self::Decryption(_)
                | Self::ObjectNumberMismatch { .. } => unreachable!(),
            }),
        }
    }
}
