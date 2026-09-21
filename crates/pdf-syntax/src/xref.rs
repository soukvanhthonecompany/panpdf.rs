use std::collections::{HashMap, HashSet};
use std::fmt;

use pdf_bytes::{ByteStore, SourceSpan};

use crate::value::parse_unsigned;
use crate::xref_stream::{
    BootstrapStreamLength, XrefStreamSection, parse_xref_stream_section_strict,
};
use crate::{
    IndirectObjectError, IndirectObjectErrorKind, LexError, LexErrorKind, Lexer, NumberKind,
    Object, ObjectKind, ObjectParser, ParseError, ParseErrorKind, ParseLimits, ResolveLimits,
    RevisionIndex, StreamDecodeError, StreamDecodeErrorKind, Token, TokenKind,
};

const STARTXREF: &[u8] = b"startxref";
const END_OF_FILE: &[u8] = b"%%EOF";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XrefLimits {
    pub objects: ParseLimits,
    pub max_tail_scan_bytes: usize,
    pub max_entries: usize,
    pub max_revisions: usize,
    pub max_decoded_stream_bytes: usize,
}

impl Default for XrefLimits {
    fn default() -> Self {
        Self {
            objects: ParseLimits::default(),
            max_tail_scan_bytes: 1_048_576,
            max_entries: 10_000_000,
            max_revisions: 1_024,
            max_decoded_stream_bytes: 256 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XrefEntryKind {
    Free {
        next_free_object: u32,
    },
    InUse {
        byte_offset: u64,
    },
    Compressed {
        object_stream_number: u32,
        index: u32,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XrefEntry {
    object_number: u32,
    generation: u16,
    kind: XrefEntryKind,
    span: SourceSpan,
    decoded_byte_offset: Option<usize>,
}

impl XrefEntry {
    #[must_use]
    pub const fn rebuilt(
        object_number: u32,
        generation: u16,
        kind: XrefEntryKind,
        span: SourceSpan,
    ) -> Self {
        Self {
            object_number,
            generation,
            kind,
            span,
            decoded_byte_offset: None,
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

    #[must_use]
    pub const fn kind(self) -> XrefEntryKind {
        self.kind
    }

    #[must_use]
    pub const fn span(self) -> SourceSpan {
        self.span
    }

    #[must_use]
    pub const fn decoded_byte_offset(self) -> Option<usize> {
        self.decoded_byte_offset
    }

    pub(crate) const fn from_stream(
        object_number: u32,
        generation: u16,
        kind: XrefEntryKind,
        encoded_span: SourceSpan,
        decoded_byte_offset: usize,
    ) -> Self {
        Self {
            object_number,
            generation,
            kind,
            span: encoded_span,
            decoded_byte_offset: Some(decoded_byte_offset),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum XrefSection {
    Classic(ClassicXrefSection),
    Stream(XrefStreamSection),
    Hybrid(Box<HybridXrefSection>),
}

impl XrefSection {
    #[must_use]
    pub const fn byte_offset(&self) -> usize {
        match self {
            Self::Classic(section) => section.byte_offset(),
            Self::Stream(section) => section.byte_offset(),
            Self::Hybrid(section) => section.byte_offset(),
        }
    }

    #[must_use]
    pub fn entries(&self) -> &[XrefEntry] {
        match self {
            Self::Classic(section) => section.entries(),
            Self::Stream(section) => section.entries(),
            Self::Hybrid(section) => section.entries(),
        }
    }

    #[must_use]
    pub const fn trailer(&self) -> &Object {
        match self {
            Self::Classic(section) => section.trailer(),
            Self::Stream(section) => section.trailer(),
            Self::Hybrid(section) => section.trailer(),
        }
    }

    #[must_use]
    pub const fn previous_byte_offset(&self) -> Option<usize> {
        match self {
            Self::Classic(section) => section.previous_byte_offset(),
            Self::Stream(section) => section.previous_byte_offset(),
            Self::Hybrid(section) => section.previous_byte_offset(),
        }
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        match self {
            Self::Classic(section) => section.span(),
            Self::Stream(section) => section.span(),
            Self::Hybrid(section) => section.span(),
        }
    }

    const fn bootstrap_length(&self) -> Option<BootstrapStreamLength> {
        match self {
            Self::Classic(_) => None,
            Self::Stream(section) => section.bootstrap_length(),
            Self::Hybrid(section) => section.supplemental().bootstrap_length(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassicXrefSection {
    byte_offset: usize,
    entries: Vec<XrefEntry>,
    trailer: Object,
    previous_byte_offset: Option<usize>,
    xref_stream_byte_offset: Option<usize>,
    span: SourceSpan,
}

impl ClassicXrefSection {
    #[must_use]
    pub const fn byte_offset(&self) -> usize {
        self.byte_offset
    }

    #[must_use]
    pub fn entries(&self) -> &[XrefEntry] {
        &self.entries
    }

    #[must_use]
    pub const fn trailer(&self) -> &Object {
        &self.trailer
    }

    #[must_use]
    pub const fn previous_byte_offset(&self) -> Option<usize> {
        self.previous_byte_offset
    }

    #[must_use]
    pub const fn xref_stream_byte_offset(&self) -> Option<usize> {
        self.xref_stream_byte_offset
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HybridXrefSection {
    classic: ClassicXrefSection,
    supplemental: XrefStreamSection,
    entries: Vec<XrefEntry>,
    span: SourceSpan,
}

impl HybridXrefSection {
    fn new(
        source: &ByteStore,
        classic: ClassicXrefSection,
        supplemental: XrefStreamSection,
    ) -> Result<Self, XrefError> {
        let mut entries = classic.entries.clone();
        let mut classic_by_number = HashMap::with_capacity(entries.len());
        for &entry in &entries {
            classic_by_number
                .entry(entry.object_number())
                .or_insert(entry);
        }
        for &entry in supplemental.entries() {
            if let Some(classic_entry) = classic_by_number.get(&entry.object_number()) {
                if classic_entry.generation() != entry.generation()
                    || classic_entry.kind() != entry.kind()
                {
                    return Err(XrefError::new(
                        entry.span().start(),
                        XrefErrorKind::HybridEntryConflict {
                            object_number: entry.object_number(),
                        },
                    ));
                }
            } else {
                entries.push(entry);
            }
        }

        let span_start = classic.span().start().min(supplemental.span().start());
        let span_end = classic.span().end().max(supplemental.span().end());
        let span = source
            .span(span_start..span_end)
            .map_err(|_| XrefError::new(span_start, XrefErrorKind::SourceSpanFailure))?;
        Ok(Self {
            classic,
            supplemental,
            entries,
            span,
        })
    }

    #[must_use]
    pub const fn byte_offset(&self) -> usize {
        self.classic.byte_offset()
    }

    #[must_use]
    pub fn entries(&self) -> &[XrefEntry] {
        &self.entries
    }

    #[must_use]
    pub const fn trailer(&self) -> &Object {
        self.classic.trailer()
    }

    #[must_use]
    pub const fn previous_byte_offset(&self) -> Option<usize> {
        self.classic.previous_byte_offset()
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }

    #[must_use]
    pub const fn classic(&self) -> &ClassicXrefSection {
        &self.classic
    }

    #[must_use]
    pub const fn supplemental(&self) -> &XrefStreamSection {
        &self.supplemental
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RevisionChain {
    startxref: usize,
    revisions: Vec<XrefSection>,
}

impl RevisionChain {
    #[must_use]
    pub const fn startxref(&self) -> usize {
        self.startxref
    }

    #[must_use]
    pub fn revisions(&self) -> &[XrefSection] {
        &self.revisions
    }
}

pub fn find_startxref_strict(source: &ByteStore, limits: XrefLimits) -> Result<usize, XrefError> {
    let bytes = source.as_bytes();
    let tail_start = bytes.len().saturating_sub(limits.max_tail_scan_bytes);
    let tail = &bytes[tail_start..];
    let eof_relative = rfind(tail, END_OF_FILE)
        .ok_or_else(|| XrefError::new(tail_start, XrefErrorKind::MissingEofMarker))?;
    let eof = tail_start + eof_relative;

    if !bytes[eof + END_OF_FILE.len()..]
        .iter()
        .all(|byte| is_whitespace(*byte))
    {
        return Err(XrefError::new(
            eof + END_OF_FILE.len(),
            XrefErrorKind::TrailingDataAfterEof,
        ));
    }

    let start_relative = rfind(&bytes[tail_start..eof], STARTXREF)
        .ok_or_else(|| XrefError::new(tail_start, XrefErrorKind::MissingStartXref))?;
    let start = tail_start + start_relative;
    let mut lexer = Lexer::new(source, start, limits.objects.lex);
    let marker = required_lex_token(&mut lexer)?;
    if !token_equals(source, marker, STARTXREF) {
        return Err(XrefError::new(start, XrefErrorKind::MissingStartXref));
    }
    let offset_token = required_lex_token(&mut lexer)?;
    let raw_offset = parse_integer_token(source, offset_token).ok_or_else(|| {
        XrefError::new(offset_token.span().start(), XrefErrorKind::InvalidStartXref)
    })?;
    let offset = usize::try_from(raw_offset).map_err(|_| {
        XrefError::new(
            offset_token.span().start(),
            XrefErrorKind::StartXrefOutOfBounds,
        )
    })?;
    if offset >= source.len() {
        return Err(XrefError::new(
            offset_token.span().start(),
            XrefErrorKind::StartXrefOutOfBounds,
        ));
    }
    if !bytes[offset_token.span().end()..eof]
        .iter()
        .all(|byte| is_whitespace(*byte))
    {
        return Err(XrefError::new(
            offset_token.span().end(),
            XrefErrorKind::DataBetweenStartXrefAndEof,
        ));
    }

    Ok(offset)
}

pub fn parse_classic_revision_chain_strict(
    source: &ByteStore,
    limits: XrefLimits,
) -> Result<RevisionChain, XrefError> {
    let startxref = find_startxref_strict(source, limits)?;
    let mut next = Some(startxref);
    let mut seen = HashSet::new();
    let mut revisions = Vec::new();

    while let Some(offset) = next {
        if revisions.len() >= limits.max_revisions {
            return Err(XrefError::new(offset, XrefErrorKind::RevisionLimit));
        }
        if !seen.insert(offset) {
            return Err(XrefError::new(offset, XrefErrorKind::PreviousRevisionLoop));
        }
        let section = parse_classic_section(source, offset, limits)?;
        if section.xref_stream_byte_offset().is_some() {
            return Err(XrefError::new(
                section.trailer().span().start(),
                XrefErrorKind::HybridXrefUnsupported,
            ));
        }
        next = section.previous_byte_offset;
        revisions.push(XrefSection::Classic(section));
    }

    Ok(RevisionChain {
        startxref,
        revisions,
    })
}

pub fn parse_revision_chain_strict(
    source: &ByteStore,
    limits: XrefLimits,
) -> Result<RevisionChain, XrefError> {
    let startxref = find_startxref_strict(source, limits)?;
    let mut next = Some(startxref);
    let mut seen = HashSet::new();
    let mut revisions = Vec::new();

    while let Some(offset) = next {
        if revisions.len() >= limits.max_revisions {
            return Err(XrefError::new(offset, XrefErrorKind::RevisionLimit));
        }
        if !seen.insert(offset) {
            return Err(XrefError::new(offset, XrefErrorKind::PreviousRevisionLoop));
        }
        let section = if source.as_bytes()[offset..].starts_with(b"xref") {
            let classic = parse_classic_section(source, offset, limits)?;
            if let Some(stream_offset) = classic.xref_stream_byte_offset() {
                let supplemental = parse_xref_stream_section_strict(source, stream_offset, limits)?;
                XrefSection::Hybrid(Box::new(HybridXrefSection::new(
                    source,
                    classic,
                    supplemental,
                )?))
            } else {
                XrefSection::Classic(classic)
            }
        } else {
            XrefSection::Stream(parse_xref_stream_section_strict(source, offset, limits)?)
        };
        next = section.previous_byte_offset();
        revisions.push(section);
    }

    let chain = RevisionChain {
        startxref,
        revisions,
    };
    validate_bootstrap_lengths(source, &chain, limits)?;
    Ok(chain)
}

fn validate_bootstrap_lengths(
    source: &ByteStore,
    chain: &RevisionChain,
    limits: XrefLimits,
) -> Result<(), XrefError> {
    let resolve_limits = ResolveLimits {
        objects: limits.objects,
        max_decoded_stream_bytes: limits.max_decoded_stream_bytes,
        ..ResolveLimits::default()
    };
    for revision_index in 0..chain.revisions.len() {
        let Some(bootstrap) = chain.revisions[revision_index].bootstrap_length() else {
            continue;
        };
        let revision_chain = RevisionChain {
            startxref: chain.revisions[revision_index].byte_offset(),
            revisions: chain.revisions[revision_index..].to_vec(),
        };
        let index = RevisionIndex::from_chain(&revision_chain).map_err(|_| {
            XrefError::new(
                bootstrap.source_offset(),
                XrefErrorKind::IndirectXrefLengthUnresolvable,
            )
        })?;
        let resolved = index
            .resolve_unsigned_integer(source, bootstrap.reference(), resolve_limits)
            .map_err(|_| {
                XrefError::new(
                    bootstrap.source_offset(),
                    XrefErrorKind::IndirectXrefLengthUnresolvable,
                )
            })?;
        if resolved != bootstrap.derived_value() {
            return Err(XrefError::new(
                bootstrap.source_offset(),
                XrefErrorKind::IndirectXrefLengthMismatch,
            ));
        }
    }
    Ok(())
}

fn parse_classic_section(
    source: &ByteStore,
    offset: usize,
    limits: XrefLimits,
) -> Result<ClassicXrefSection, XrefError> {
    let mut lexer = Lexer::new(source, offset, limits.objects.lex);
    let xref = required_lex_token(&mut lexer)?;
    if !token_equals(source, xref, b"xref") {
        return Err(XrefError::new(offset, XrefErrorKind::ExpectedClassicXref));
    }

    let (entries, trailer_marker) = parse_entries_until_trailer(source, &mut lexer, limits)?;
    let (trailer, previous_byte_offset, xref_stream_byte_offset) =
        parse_trailer(source, trailer_marker, limits.objects)?;
    let span = source
        .span(xref.span().start()..trailer.span().end())
        .map_err(|_| XrefError::new(offset, XrefErrorKind::SourceSpanFailure))?;

    Ok(ClassicXrefSection {
        byte_offset: offset,
        entries,
        trailer,
        previous_byte_offset,
        xref_stream_byte_offset,
        span,
    })
}

fn parse_entries_until_trailer(
    source: &ByteStore,
    lexer: &mut Lexer<'_>,
    limits: XrefLimits,
) -> Result<(Vec<XrefEntry>, Token), XrefError> {
    let mut entries = Vec::new();
    loop {
        let first = required_lex_token(lexer)?;
        if token_equals(source, first, b"trailer") {
            return Ok((entries, first));
        }

        let first_object = parse_integer_token(source, first)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| {
                XrefError::new(first.span().start(), XrefErrorKind::InvalidSubsectionHeader)
            })?;
        let count_token = required_lex_token(lexer)?;
        let count = parse_integer_token(source, count_token)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| {
                XrefError::new(
                    count_token.span().start(),
                    XrefErrorKind::InvalidSubsectionHeader,
                )
            })?;

        if entries
            .len()
            .checked_add(count)
            .is_none_or(|total| total > limits.max_entries)
        {
            return Err(XrefError::new(
                count_token.span().start(),
                XrefErrorKind::EntryLimit,
            ));
        }

        for index in 0..count {
            entries.push(parse_entry(source, lexer, first, first_object, index)?);
        }
    }
}

fn parse_entry(
    source: &ByteStore,
    lexer: &mut Lexer<'_>,
    subsection_start: Token,
    first_object: u32,
    index: usize,
) -> Result<XrefEntry, XrefError> {
    let entry_offset = required_lex_token(lexer)?;
    let generation = required_lex_token(lexer)?;
    let flag = required_lex_token(lexer)?;
    validate_entry_layout(source, entry_offset, generation, flag)?;

    let object_number = first_object
        .checked_add(u32::try_from(index).map_err(|_| {
            XrefError::new(
                subsection_start.span().start(),
                XrefErrorKind::ObjectNumberOverflow,
            )
        })?)
        .ok_or_else(|| {
            XrefError::new(
                subsection_start.span().start(),
                XrefErrorKind::ObjectNumberOverflow,
            )
        })?;
    let first_field = parse_integer_token(source, entry_offset)
        .ok_or_else(|| XrefError::new(entry_offset.span().start(), XrefErrorKind::InvalidEntry))?;
    let generation_value = parse_integer_token(source, generation)
        .and_then(|value| u16::try_from(value).ok())
        .ok_or_else(|| XrefError::new(generation.span().start(), XrefErrorKind::InvalidEntry))?;
    let kind = parse_entry_kind(source, entry_offset, flag, first_field)?;
    let span = source
        .span(entry_offset.span().start()..flag.span().end())
        .map_err(|_| {
            XrefError::new(
                entry_offset.span().start(),
                XrefErrorKind::SourceSpanFailure,
            )
        })?;
    Ok(XrefEntry {
        object_number,
        generation: generation_value,
        kind,
        span,
        decoded_byte_offset: None,
    })
}

fn parse_entry_kind(
    source: &ByteStore,
    entry_offset: Token,
    flag: Token,
    first_field: u64,
) -> Result<XrefEntryKind, XrefError> {
    if token_equals(source, flag, b"n") {
        return Ok(XrefEntryKind::InUse {
            byte_offset: first_field,
        });
    }
    if token_equals(source, flag, b"f") {
        return Ok(XrefEntryKind::Free {
            next_free_object: u32::try_from(first_field).map_err(|_| {
                XrefError::new(entry_offset.span().start(), XrefErrorKind::InvalidEntry)
            })?,
        });
    }
    Err(XrefError::new(
        flag.span().start(),
        XrefErrorKind::InvalidEntryFlag,
    ))
}

fn parse_trailer(
    source: &ByteStore,
    trailer_marker: Token,
    limits: ParseLimits,
) -> Result<(Object, Option<usize>, Option<usize>), XrefError> {
    let mut trailer_parser = ObjectParser::new(source, trailer_marker.span().end(), limits);

    let trailer = trailer_parser
        .parse_next()
        .map_err(XrefError::from)?
        .ok_or_else(|| {
            XrefError::new(
                trailer_marker.span().end(),
                XrefErrorKind::MissingTrailerDictionary,
            )
        })?;
    let ObjectKind::Dictionary(trailer_entries) = trailer.kind() else {
        return Err(XrefError::new(
            trailer.span().start(),
            XrefErrorKind::TrailerNotDictionary,
        ));
    };

    let previous_byte_offset = unique_trailer_offset(
        source,
        trailer_entries,
        b"/Prev",
        XrefErrorKind::DuplicatePreviousOffset,
        XrefErrorKind::InvalidPreviousOffset,
    )?;
    if previous_byte_offset.is_some_and(|previous| previous >= source.len()) {
        return Err(XrefError::new(
            trailer.span().start(),
            XrefErrorKind::PreviousOffsetOutOfBounds,
        ));
    }
    let xref_stream_byte_offset = unique_trailer_offset(
        source,
        trailer_entries,
        b"/XRefStm",
        XrefErrorKind::DuplicateXrefStreamOffset,
        XrefErrorKind::InvalidXrefStreamOffset,
    )?;
    if xref_stream_byte_offset.is_some_and(|stream| stream >= source.len()) {
        return Err(XrefError::new(
            trailer.span().start(),
            XrefErrorKind::XrefStreamOffsetOutOfBounds,
        ));
    }
    Ok((trailer, previous_byte_offset, xref_stream_byte_offset))
}

fn unique_trailer_offset(
    source: &ByteStore,
    entries: &[crate::DictionaryEntry],
    name: &[u8],
    duplicate: XrefErrorKind,
    invalid: XrefErrorKind,
) -> Result<Option<usize>, XrefError> {
    let mut matches = entries
        .iter()
        .filter(|entry| entry.key_equals(source, name));
    let Some(entry) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() {
        return Err(XrefError::new(entry.key().span().start(), duplicate));
    }
    let ObjectKind::Number(NumberKind::Integer) = entry.value().kind() else {
        return Err(XrefError::new(entry.value().span().start(), invalid));
    };
    let raw = source.resolve(entry.value().span()).map_err(|_| {
        XrefError::new(
            entry.value().span().start(),
            XrefErrorKind::SourceSpanFailure,
        )
    })?;
    let offset = parse_unsigned(raw)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| XrefError::new(entry.value().span().start(), invalid))?;
    Ok(Some(offset))
}

fn validate_entry_layout(
    source: &ByteStore,
    offset: Token,
    generation: Token,
    flag: Token,
) -> Result<(), XrefError> {
    let offset_bytes = source
        .resolve(offset.span())
        .map_err(|_| XrefError::new(offset.span().start(), XrefErrorKind::SourceSpanFailure))?;
    let generation_bytes = source
        .resolve(generation.span())
        .map_err(|_| XrefError::new(generation.span().start(), XrefErrorKind::SourceSpanFailure))?;
    if offset_bytes.len() != 10
        || generation_bytes.len() != 5
        || &source.as_bytes()[offset.span().end()..generation.span().start()] != b" "
        || &source.as_bytes()[generation.span().end()..flag.span().start()] != b" "
    {
        return Err(XrefError::new(
            offset.span().start(),
            XrefErrorKind::InvalidEntryLayout,
        ));
    }

    let after_flag = &source.as_bytes()[flag.span().end()..];
    if !(after_flag.starts_with(b"\n")
        || after_flag.starts_with(b"\r")
        || after_flag.starts_with(b" \n")
        || after_flag.starts_with(b" \r"))
    {
        return Err(XrefError::new(
            flag.span().end(),
            XrefErrorKind::InvalidEntryLayout,
        ));
    }
    Ok(())
}

fn parse_integer_token(source: &ByteStore, token: Token) -> Option<u64> {
    if token.kind() != TokenKind::Number(NumberKind::Integer) {
        return None;
    }
    source.resolve(token.span()).ok().and_then(parse_unsigned)
}

fn token_equals(source: &ByteStore, token: Token, expected: &[u8]) -> bool {
    source.resolve(token.span()) == Ok(expected)
}

fn required_lex_token(lexer: &mut Lexer<'_>) -> Result<Token, XrefError> {
    let offset = lexer.offset();
    lexer
        .next_token()
        .map_err(XrefError::from)?
        .ok_or_else(|| XrefError::new(offset, XrefErrorKind::UnexpectedEndOfInput))
}

fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}

const fn is_whitespace(byte: u8) -> bool {
    matches!(byte, 0x00 | b'\t' | b'\n' | 0x0c | b'\r' | b' ')
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XrefError {
    offset: usize,
    kind: XrefErrorKind,
}

impl XrefError {
    pub(crate) const fn new(offset: usize, kind: XrefErrorKind) -> Self {
        Self { offset, kind }
    }

    #[must_use]
    pub const fn offset(self) -> usize {
        self.offset
    }

    #[must_use]
    pub const fn kind(self) -> XrefErrorKind {
        self.kind
    }
}

impl From<IndirectObjectError> for XrefError {
    fn from(error: IndirectObjectError) -> Self {
        Self {
            offset: error.offset(),
            kind: XrefErrorKind::IndirectObject(error.kind()),
        }
    }
}

impl From<StreamDecodeError> for XrefError {
    fn from(error: StreamDecodeError) -> Self {
        Self {
            offset: error.offset(),
            kind: XrefErrorKind::StreamDecode(error.kind()),
        }
    }
}

impl From<LexError> for XrefError {
    fn from(error: LexError) -> Self {
        Self {
            offset: error.offset(),
            kind: XrefErrorKind::Lexical(error.kind()),
        }
    }
}

impl From<ParseError> for XrefError {
    fn from(error: ParseError) -> Self {
        Self {
            offset: error.offset(),
            kind: XrefErrorKind::Object(error.kind()),
        }
    }
}

impl fmt::Display for XrefError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} at byte {}", self.kind, self.offset)
    }
}

impl std::error::Error for XrefError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XrefErrorKind {
    Lexical(LexErrorKind),
    Object(ParseErrorKind),
    IndirectObject(IndirectObjectErrorKind),
    StreamDecode(StreamDecodeErrorKind),
    MissingEofMarker,
    TrailingDataAfterEof,
    MissingStartXref,
    InvalidStartXref,
    StartXrefOutOfBounds,
    DataBetweenStartXrefAndEof,
    ExpectedClassicXref,
    UnexpectedEndOfInput,
    InvalidSubsectionHeader,
    EntryLimit,
    ObjectNumberOverflow,
    InvalidEntry,
    InvalidEntryLayout,
    InvalidEntryFlag,
    MissingTrailerDictionary,
    TrailerNotDictionary,
    HybridXrefUnsupported,
    DuplicateXrefStreamOffset,
    InvalidXrefStreamOffset,
    XrefStreamOffsetOutOfBounds,
    HybridEntryConflict { object_number: u32 },
    XrefStreamNotDictionary,
    XrefStreamMissingData,
    XrefStreamWrongType,
    MissingXrefSize,
    InvalidXrefSize,
    MissingXrefWidths,
    InvalidXrefWidths,
    InvalidXrefIndex,
    DuplicateXrefParameter,
    DecodedXrefLengthMismatch,
    IndirectXrefLengthUnresolvable,
    IndirectXrefLengthMismatch,
    UnsupportedXrefEntryType,
    DuplicatePreviousOffset,
    InvalidPreviousOffset,
    PreviousOffsetOutOfBounds,
    PreviousRevisionLoop,
    RevisionLimit,
    SourceSpanFailure,
}

impl fmt::Display for XrefErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lexical(kind) => write!(formatter, "invalid xref token: {kind}"),
            Self::Object(kind) => write!(formatter, "invalid trailer object: {kind}"),
            Self::IndirectObject(kind) => write!(formatter, "invalid xref stream object: {kind}"),
            Self::StreamDecode(kind) => write!(formatter, "cannot decode xref stream: {kind}"),
            Self::HybridEntryConflict { object_number } => write!(
                formatter,
                "classic and supplemental xref entries conflict for object {object_number}"
            ),
            _ => formatter.write_str(match self {
                Self::MissingEofMarker => "missing final %%EOF marker",
                Self::TrailingDataAfterEof => "non-whitespace data follows final %%EOF",
                Self::MissingStartXref => "missing final startxref marker",
                Self::InvalidStartXref => "startxref is not a non-negative integer",
                Self::StartXrefOutOfBounds => "startxref points outside the source",
                Self::DataBetweenStartXrefAndEof => {
                    "unexpected data lies between startxref and %%EOF"
                }
                Self::ExpectedClassicXref => "xref offset does not point to a classic xref table",
                Self::UnexpectedEndOfInput => "unexpected end of xref data",
                Self::InvalidSubsectionHeader => "invalid xref subsection header",
                Self::EntryLimit => "xref entry limit exceeded",
                Self::ObjectNumberOverflow => "xref object number overflow",
                Self::InvalidEntry => "invalid xref entry value",
                Self::InvalidEntryLayout => "xref entry does not use the strict fixed-width layout",
                Self::InvalidEntryFlag => "xref entry flag is neither n nor f",
                Self::MissingTrailerDictionary => "xref table has no trailer dictionary",
                Self::TrailerNotDictionary => "xref trailer value is not a dictionary",
                Self::HybridXrefUnsupported => "hybrid /XRefStm references are not implemented yet",
                Self::DuplicateXrefStreamOffset => "trailer contains duplicate /XRefStm keys",
                Self::InvalidXrefStreamOffset => {
                    "trailer /XRefStm is not a non-negative direct integer"
                }
                Self::XrefStreamOffsetOutOfBounds => "trailer /XRefStm points outside the source",
                Self::XrefStreamNotDictionary => "xref stream value is not a dictionary",
                Self::XrefStreamMissingData => "xref stream object has no stream data",
                Self::XrefStreamWrongType => "xref stream dictionary Type is not XRef",
                Self::MissingXrefSize => "xref stream dictionary has no Size",
                Self::InvalidXrefSize => "xref stream Size is invalid",
                Self::MissingXrefWidths => "xref stream dictionary has no W array",
                Self::InvalidXrefWidths => "xref stream W array is invalid",
                Self::InvalidXrefIndex => "xref stream Index array is invalid",
                Self::DuplicateXrefParameter => "xref stream dictionary repeats a required key",
                Self::DecodedXrefLengthMismatch => {
                    "decoded xref bytes do not exactly match W and Index"
                }
                Self::IndirectXrefLengthUnresolvable => {
                    "xref stream Length reference cannot be resolved in its revision"
                }
                Self::IndirectXrefLengthMismatch => {
                    "xref stream Length reference disagrees with its encoded data"
                }
                Self::UnsupportedXrefEntryType => "xref stream entry type is unsupported",
                Self::DuplicatePreviousOffset => "trailer contains duplicate /Prev keys",
                Self::InvalidPreviousOffset => "trailer /Prev is not a non-negative direct integer",
                Self::PreviousOffsetOutOfBounds => "trailer /Prev points outside the source",
                Self::PreviousRevisionLoop => "trailer /Prev chain contains a loop",
                Self::RevisionLimit => "PDF revision limit exceeded",
                Self::SourceSpanFailure => "xref span exceeds its source",
                Self::Lexical(_)
                | Self::Object(_)
                | Self::IndirectObject(_)
                | Self::StreamDecode(_)
                | Self::HybridEntryConflict { .. } => unreachable!(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdf_bytes::{ByteStore, SourceId};

    use super::{
        XrefEntryKind, XrefErrorKind, XrefLimits, find_startxref_strict,
        parse_classic_revision_chain_strict, parse_revision_chain_strict,
    };

    fn store(bytes: Vec<u8>) -> ByteStore {
        ByteStore::new(SourceId::new(21), Arc::<[u8]>::from(bytes))
    }

    fn base_pdf() -> (Vec<u8>, usize) {
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let object_offset = bytes.len();
        bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog >>\nendobj\n");
        let xref_offset = bytes.len();
        bytes.extend_from_slice(b"xref\n0 2\n0000000000 65535 f \n");
        bytes.extend_from_slice(format!("{object_offset:010} 00000 n \n").as_bytes());
        bytes.extend_from_slice(b"trailer\n<< /Size 2 /Root 1 0 R >>\nstartxref\n");
        bytes.extend_from_slice(xref_offset.to_string().as_bytes());
        bytes.extend_from_slice(b"\n%%EOF\n");
        (bytes, xref_offset)
    }

    #[test]
    fn finds_and_parses_a_classic_table_with_exact_entries() {
        let (bytes, expected_xref) = base_pdf();
        let source = store(bytes);

        assert_eq!(
            find_startxref_strict(&source, XrefLimits::default()),
            Ok(expected_xref)
        );
        let chain = parse_classic_revision_chain_strict(&source, XrefLimits::default())
            .expect("valid revision chain");
        assert_eq!(chain.revisions().len(), 1);
        let entries = chain.revisions()[0].entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0].kind(),
            XrefEntryKind::Free {
                next_free_object: 0
            }
        );
        assert!(matches!(entries[1].kind(), XrefEntryKind::InUse { .. }));
    }

    #[test]
    fn follows_incremental_prev_offsets_newest_first() {
        let (mut bytes, first_xref) = base_pdf();
        let second_object = bytes.len();
        bytes.extend_from_slice(b"2 0 obj\n(added)\nendobj\n");
        let second_xref = bytes.len();
        bytes.extend_from_slice(b"xref\n2 1\n");
        bytes.extend_from_slice(format!("{second_object:010} 00000 n \n").as_bytes());
        bytes.extend_from_slice(
            format!(
                "trailer\n<< /Size 3 /Root 1 0 R /Prev {first_xref} >>\nstartxref\n{second_xref}\n%%EOF\n"
            )
            .as_bytes(),
        );
        let source = store(bytes);
        let chain = parse_classic_revision_chain_strict(&source, XrefLimits::default())
            .expect("valid incremental chain");

        assert_eq!(chain.startxref(), second_xref);
        assert_eq!(chain.revisions().len(), 2);
        assert_eq!(chain.revisions()[0].byte_offset(), second_xref);
        assert_eq!(chain.revisions()[1].byte_offset(), first_xref);
    }

    #[test]
    fn rejects_prev_loops_and_hybrid_references_explicitly() {
        let mut looping = b"%PDF-1.7\n".to_vec();
        let xref = looping.len();
        looping.extend_from_slice(
            format!(
                "xref\n0 1\n0000000000 65535 f \ntrailer\n<< /Size 1 /Prev {xref} >>\nstartxref\n{xref}\n%%EOF\n"
            )
            .as_bytes(),
        );
        let error = parse_classic_revision_chain_strict(&store(looping), XrefLimits::default())
            .expect_err("Prev loop must fail");
        assert_eq!(error.kind(), XrefErrorKind::PreviousRevisionLoop);

        let (bytes, _) = base_pdf();
        let text = String::from_utf8(bytes).expect("synthetic PDF is ASCII");
        let hybrid = text.replace("/Root 1 0 R", "/Root 1 0 R /XRefStm 9");
        let error =
            parse_classic_revision_chain_strict(&store(hybrid.into_bytes()), XrefLimits::default())
                .expect_err("hybrid reference must be explicit");
        assert_eq!(error.kind(), XrefErrorKind::HybridXrefUnsupported);
    }

    #[test]
    fn strict_tail_rejects_trailing_data_and_wrong_startxref() {
        let (mut bytes, _) = base_pdf();
        bytes.extend_from_slice(b"junk");
        let error = find_startxref_strict(&store(bytes), XrefLimits::default())
            .expect_err("trailing data must fail");
        assert_eq!(error.kind(), XrefErrorKind::TrailingDataAfterEof);

        let (bytes, xref) = base_pdf();
        let text = String::from_utf8(bytes).expect("synthetic PDF is ASCII");
        let wrong = text.replace(&format!("startxref\n{xref}"), "startxref\n999999");
        let error = find_startxref_strict(&store(wrong.into_bytes()), XrefLimits::default())
            .expect_err("out-of-bounds startxref must fail");
        assert_eq!(error.kind(), XrefErrorKind::StartXrefOutOfBounds);
    }

    #[test]
    fn rejects_non_fixed_width_entries_and_entry_limit_overflow() {
        let (bytes, _) = base_pdf();
        let text = String::from_utf8(bytes).expect("synthetic PDF is ASCII");
        let malformed = text.replace("0000000000 65535 f ", "0 65535 f ");
        let error = parse_classic_revision_chain_strict(
            &store(malformed.into_bytes()),
            XrefLimits::default(),
        )
        .expect_err("non-fixed entry must fail");
        assert_eq!(error.kind(), XrefErrorKind::InvalidEntryLayout);

        let (bytes, _) = base_pdf();
        let error = parse_classic_revision_chain_strict(
            &store(bytes),
            XrefLimits {
                max_entries: 1,
                ..XrefLimits::default()
            },
        )
        .expect_err("entry limit must fail");
        assert_eq!(error.kind(), XrefErrorKind::EntryLimit);
    }

    #[test]
    fn rejects_invalid_and_duplicate_xrefstm_offsets() {
        let (bytes, _) = base_pdf();
        let text = String::from_utf8(bytes).expect("synthetic PDF is ASCII");
        let out_of_bounds = text.replace("/Root 1 0 R", "/Root 1 0 R /XRefStm 999999");
        let error =
            parse_revision_chain_strict(&store(out_of_bounds.into_bytes()), XrefLimits::default())
                .expect_err("supplemental offset must lie inside the source");
        assert_eq!(error.kind(), XrefErrorKind::XrefStreamOffsetOutOfBounds);

        let duplicate = text.replace("/Root 1 0 R", "/Root 1 0 R /XRefStm 9 /XRefStm 9");
        let error =
            parse_revision_chain_strict(&store(duplicate.into_bytes()), XrefLimits::default())
                .expect_err("duplicate supplemental pointers are ambiguous");
        assert_eq!(error.kind(), XrefErrorKind::DuplicateXrefStreamOffset);
    }
}
