#![forbid(unsafe_code)]

use std::fmt;

use pdf_bytes::{ByteStore, SourceSpan};

mod filter;
mod indirect;
mod object_stream;
mod recover;
mod resolve;
mod string;
mod token;
mod value;
mod xref;
mod xref_stream;

pub use filter::{
    DecodedStream, ImageCodec, ImageStreamData, SelfDelimitingFilter, StreamDecodeError,
    StreamDecodeErrorKind, StreamRepair, decode_image_stream_bytes,
    decode_image_stream_bytes_recovering, decode_stream_bytes, decode_stream_bytes_recovering,
    deflate_zlib, inflate_zlib,
};
pub use indirect::{
    IndirectObject, IndirectObjectError, IndirectObjectErrorKind, IndirectRepair,
    ResolvedStreamLength, StreamObject, parse_indirect_object_recovering,
    parse_indirect_object_strict, parse_indirect_object_with_resolved_length_strict,
};
pub use object_stream::{ObjectStreamError, ObjectStreamErrorKind};
pub use recover::{
    DocumentObjects, RecoverError, RecoverLimits, Recovered, Repair, RepairKind, ScannedEntry,
    ScannedLocation, ScannedObjects, open_objects_recovering, parse_header_recovering,
    scan_indirect_objects,
};
pub use resolve::{
    Damage, DecryptionRefused, ResolveError, ResolveErrorKind, ResolveLimits, ResolvedObject,
    RevisionIndex, SelectedXrefEntry, StreamDecryptor,
};
pub use string::{StringDecodeError, StringDecodeErrorKind, decode_string};

pub use token::{LexError, LexErrorKind, LexLimits, Lexer, NumberKind, Token, TokenKind};
pub use value::{
    DictionaryEntry, NameDecodeError, Object, ObjectKind, ObjectParser, ParseError, ParseErrorKind,
    ParseLimits, Reference, decode_name,
};
pub use xref::{
    ClassicXrefSection, HybridXrefSection, RevisionChain, XrefEntry, XrefEntryKind, XrefError,
    XrefErrorKind, XrefLimits, XrefSection, find_startxref_strict,
    parse_classic_revision_chain_strict, parse_revision_chain_strict,
};
pub use xref_stream::XrefStreamSection;

const HEADER_PREFIX: &[u8] = b"%PDF-";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PdfVersion {
    major: u8,
    minor: u8,
}

impl PdfVersion {
    #[must_use]
    pub const fn new(major: u8, minor: u8) -> Self {
        Self { major, minor }
    }

    #[must_use]
    pub const fn major(self) -> u8 {
        self.major
    }

    #[must_use]
    pub const fn minor(self) -> u8 {
        self.minor
    }
}

impl fmt::Display for PdfVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}", self.major, self.minor)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PdfHeader {
    version: PdfVersion,
    span: SourceSpan,
}

impl PdfHeader {
    #[must_use]
    pub const fn version(self) -> PdfVersion {
        self.version
    }

    #[must_use]
    pub const fn span(self) -> SourceSpan {
        self.span
    }
}

pub fn parse_header_strict(source: &ByteStore) -> Result<PdfHeader, HeaderError> {
    if !source
        .ahead(0, HEADER_PREFIX.len())
        .starts_with(HEADER_PREFIX)
    {
        return Err(HeaderError::MissingAtByteZero);
    }
    parse_header_at(source, 0)
}

pub(crate) fn parse_header_at(source: &ByteStore, offset: usize) -> Result<PdfHeader, HeaderError> {
    parse_header_at_ending(source, offset, LineEnding::Required)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LineEnding {
    Required,
    Optional,
}

pub(crate) fn parse_header_at_ending(
    source: &ByteStore,
    offset: usize,
    ending: LineEnding,
) -> Result<PdfHeader, HeaderError> {
    if offset > source.len() {
        return Err(HeaderError::MissingAtByteZero);
    }
    let tail = source.ahead(offset, HEADER_PREFIX.len() + 4);
    if !tail.starts_with(HEADER_PREFIX) {
        return Err(HeaderError::MissingAtByteZero);
    }

    let major = tail
        .get(HEADER_PREFIX.len())
        .copied()
        .filter(u8::is_ascii_digit)
        .ok_or(HeaderError::MalformedVersion)?
        - b'0';
    let separator = tail
        .get(HEADER_PREFIX.len() + 1)
        .copied()
        .ok_or(HeaderError::MalformedVersion)?;
    let minor = tail
        .get(HEADER_PREFIX.len() + 2)
        .copied()
        .filter(u8::is_ascii_digit)
        .ok_or(HeaderError::MalformedVersion)?
        - b'0';

    if separator != b'.' {
        return Err(HeaderError::MalformedVersion);
    }

    let header_end = HEADER_PREFIX.len() + 3;
    match tail.get(header_end) {
        Some(b'\r' | b'\n') => {}
        _ if ending == LineEnding::Optional => {}
        _ => return Err(HeaderError::MissingLineEnding),
    }

    let span = source
        .span(offset..offset + header_end)
        .map_err(|_| HeaderError::MalformedVersion)?;

    Ok(PdfHeader {
        version: PdfVersion::new(major, minor),
        span,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeaderError {
    MissingAtByteZero,
    MalformedVersion,
    MissingLineEnding,
}

impl fmt::Display for HeaderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingAtByteZero => {
                formatter.write_str("PDF header does not begin at byte zero")
            }
            Self::MalformedVersion => formatter.write_str("PDF header has a malformed version"),
            Self::MissingLineEnding => {
                formatter.write_str("PDF header is not terminated by a line ending")
            }
        }
    }
}

impl std::error::Error for HeaderError {}

#[cfg(test)]
mod tests {
    use pdf_bytes::{ByteStore, SourceId};

    use super::{HeaderError, PdfVersion, parse_header_strict};

    fn source(bytes: &'static [u8]) -> ByteStore {
        ByteStore::new(SourceId::new(1), bytes)
    }

    #[test]
    fn parses_pdf_1_7_and_preserves_the_exact_header_span() {
        let source = source(b"%PDF-1.7\r\n%binary\n");
        let header = parse_header_strict(&source).expect("valid PDF header");

        assert_eq!(header.version(), PdfVersion::new(1, 7));
        assert_eq!(source.resolve(header.span()), Ok(&b"%PDF-1.7"[..]));
    }

    #[test]
    fn parses_pdf_2_0() {
        let header = parse_header_strict(&source(b"%PDF-2.0\n")).expect("valid PDF header");

        assert_eq!(header.version(), PdfVersion::new(2, 0));
    }

    #[test]
    fn strict_mode_rejects_a_displaced_header() {
        assert_eq!(
            parse_header_strict(&source(b"junk%PDF-1.7\n")),
            Err(HeaderError::MissingAtByteZero)
        );
    }

    #[test]
    fn rejects_malformed_or_unterminated_versions() {
        assert_eq!(
            parse_header_strict(&source(b"%PDF-17\n")),
            Err(HeaderError::MalformedVersion)
        );
        assert_eq!(
            parse_header_strict(&source(b"%PDF-1.7x")),
            Err(HeaderError::MissingLineEnding)
        );
    }
}
