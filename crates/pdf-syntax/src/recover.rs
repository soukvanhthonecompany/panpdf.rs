use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt;

use pdf_bytes::{ByteStore, SourceSpan};

use crate::object_stream::{
    ObjectStreamLimits, object_stream_contents, parse_compressed_object_strict,
};
use crate::token::{is_delimiter, is_whitespace};
use crate::value::name_object_equals;
use crate::{
    HeaderError, IndirectObject, IndirectObjectError, IndirectObjectErrorKind, Object, ObjectKind,
    ObjectStreamError, ParseLimits, PdfHeader, Reference, ResolveError, ResolvedStreamLength,
    RevisionChain, RevisionIndex, StreamObject, XrefError, XrefLimits,
    parse_indirect_object_strict, parse_indirect_object_with_resolved_length_strict,
    parse_revision_chain_strict,
};

const HEADER_PREFIX: &[u8] = b"%PDF-";
const OBJ_KEYWORD: &[u8] = b"obj";
const MAX_NUMBER_DIGITS: usize = 10;

#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use = "a recovered value carries repairs that the caller must account for"]
pub struct Recovered<T> {
    value: T,
    repairs: Vec<Repair>,
}

impl<T> Recovered<T> {
    pub const fn new(value: T, repairs: Vec<Repair>) -> Self {
        Self { value, repairs }
    }

    #[must_use]
    pub fn repairs(&self) -> &[Repair] {
        &self.repairs
    }

    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.repairs.is_empty()
    }

    #[must_use]
    pub fn into_parts(self) -> (T, Vec<Repair>) {
        (self.value, self.repairs)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Repair {
    byte_offset: usize,
    kind: RepairKind,
}

impl Repair {
    pub(crate) const fn new(byte_offset: usize, kind: RepairKind) -> Self {
        Self { byte_offset, kind }
    }

    #[must_use]
    pub const fn tolerated_structural_damage(byte_offset: usize) -> Self {
        Self {
            byte_offset,
            kind: RepairKind::ToleratedStructuralDamage,
        }
    }

    #[must_use]
    pub const fn stream_decoding(byte_offset: usize, repair: crate::StreamRepair) -> Self {
        Self {
            byte_offset,
            kind: RepairKind::StreamDecoding { repair },
        }
    }

    #[must_use]
    pub const fn read_damaged_object(
        byte_offset: usize,
        object_number: u32,
        generation: u16,
        repair: crate::IndirectRepair,
    ) -> Self {
        Self {
            byte_offset,
            kind: RepairKind::ReadDamagedObject {
                object_number,
                generation,
                repair,
            },
        }
    }

    #[must_use]
    pub const fn rebuilt_catalog(byte_offset: usize, object_number: u32, generation: u16) -> Self {
        Self {
            byte_offset,
            kind: RepairKind::RebuiltCatalog {
                object_number,
                generation,
            },
        }
    }

    #[must_use]
    pub const fn byte_offset(self) -> usize {
        self.byte_offset
    }

    #[must_use]
    pub const fn kind(self) -> RepairKind {
        self.kind
    }
}

impl fmt::Display for Repair {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "at byte {}: {}", self.byte_offset, self.kind)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepairKind {
    DisplacedHeader {
        byte_offset: usize,
    },
    HeaderLineEndingMissing {
        byte_offset: usize,
    },
    RebuiltObjectMapAfterChainFailure {
        error: XrefError,
    },
    RebuiltObjectMapAfterIndexFailure {
        error: ResolveError,
    },
    SupersededDefinition {
        object_number: u32,
        generation: u16,
        superseded_byte_offset: usize,
    },
    ExpandedObjectStream {
        object_stream_number: u32,
        objects_found: usize,
    },
    UnreadableObjectStream {
        object_stream_number: u32,
        error: ObjectStreamError,
    },
    DirectDefinitionShadowsObjectStream {
        object_number: u32,
        object_stream_number: u32,
    },
    RebuiltCatalog {
        object_number: u32,
        generation: u16,
    },
    ToleratedStructuralDamage,
    ReadDamagedObject {
        object_number: u32,
        generation: u16,
        repair: crate::IndirectRepair,
    },
    StreamDecoding {
        repair: crate::StreamRepair,
    },
}

impl fmt::Display for RepairKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DisplacedHeader { byte_offset } => {
                write!(
                    formatter,
                    "header found at byte {byte_offset}, not byte zero"
                )
            }
            Self::HeaderLineEndingMissing { byte_offset } => {
                write!(
                    formatter,
                    "header version at byte {byte_offset} is not followed by a line ending"
                )
            }
            Self::StreamDecoding { repair } => repair.fmt(formatter),
            Self::RebuiltObjectMapAfterChainFailure { error } => write!(
                formatter,
                "object map rebuilt by scanning after revision chain failed: {error}"
            ),
            Self::RebuiltObjectMapAfterIndexFailure { error } => write!(
                formatter,
                "object map rebuilt by scanning after active index failed: {error}"
            ),
            Self::SupersededDefinition {
                object_number,
                generation,
                superseded_byte_offset,
            } => write!(
                formatter,
                "{object_number} {generation} R redefined; \
                 the definition at byte {superseded_byte_offset} was superseded"
            ),
            Self::ExpandedObjectStream {
                object_stream_number,
                objects_found,
            } => write!(
                formatter,
                "object stream {object_stream_number} expanded, {objects_found} objects found"
            ),
            Self::UnreadableObjectStream {
                object_stream_number,
                error,
            } => write!(
                formatter,
                "object stream {object_stream_number} is unreadable and its objects \
                 remain unavailable: {error}"
            ),
            Self::DirectDefinitionShadowsObjectStream {
                object_number,
                object_stream_number,
            } => write!(
                formatter,
                "object {object_number} is defined directly and in object stream \
                 {object_stream_number}; the direct definition was kept"
            ),
            Self::RebuiltCatalog {
                object_number,
                generation,
            } => write!(
                formatter,
                "no trailer named /Root, so object {object_number} {generation} was used as the \
                 catalog because it declares itself one"
            ),
            Self::ToleratedStructuralDamage => formatter.write_str(
                "this document was read with structural damage repaired rather than refused",
            ),
            Self::ReadDamagedObject {
                object_number,
                generation,
                repair,
            } => write!(formatter, "object {object_number} {generation}: {repair}"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecoverLimits {
    pub xref: XrefLimits,
    pub objects: ParseLimits,
    pub max_header_scan_bytes: usize,
    pub max_scanned_objects: usize,
    pub max_object_streams: usize,
    pub max_object_stream_objects: usize,
    pub max_decoded_stream_bytes: usize,
    pub max_reference_depth: usize,
}

impl Default for RecoverLimits {
    fn default() -> Self {
        let xref = XrefLimits::default();
        Self {
            xref,
            objects: xref.objects,
            max_header_scan_bytes: 1024,
            max_scanned_objects: 500_000,
            max_object_streams: 50_000,
            max_object_stream_objects: 100_000,
            max_decoded_stream_bytes: xref.max_decoded_stream_bytes,
            max_reference_depth: 64,
        }
    }
}

impl RecoverLimits {
    const fn object_stream(self) -> ObjectStreamLimits {
        ObjectStreamLimits {
            objects: self.objects,
            max_objects: self.max_object_stream_objects,
            max_decoded_bytes: self.max_decoded_stream_bytes,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScannedLocation {
    Direct {
        byte_offset: usize,
    },
    InObjectStream {
        object_stream_number: u32,
        index: u32,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScannedEntry {
    reference: Reference,
    location: ScannedLocation,
    span: SourceSpan,
}

impl ScannedEntry {
    #[must_use]
    pub const fn reference(self) -> Reference {
        self.reference
    }

    #[must_use]
    pub const fn location(self) -> ScannedLocation {
        self.location
    }

    #[must_use]
    pub const fn span(self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScannedObjects {
    entries: HashMap<u32, ScannedEntry>,
}

impl ScannedObjects {
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[must_use]
    pub fn entry_for_number(&self, object_number: u32) -> Option<ScannedEntry> {
        self.entries.get(&object_number).copied()
    }

    pub fn entries(&self) -> impl Iterator<Item = ScannedEntry> + '_ {
        self.entries.values().copied()
    }

    pub fn resolve_object(
        &self,
        source: &ByteStore,
        reference: Reference,
        limits: RecoverLimits,
    ) -> Result<ScannedResolution, RecoverError> {
        self.resolve_inner(source, reference, limits, &mut Vec::new())
    }

    fn resolve_inner(
        &self,
        source: &ByteStore,
        reference: Reference,
        limits: RecoverLimits,
        resolving: &mut Vec<Reference>,
    ) -> Result<ScannedResolution, RecoverError> {
        if resolving.len() >= limits.max_reference_depth {
            return Err(RecoverError::ReferenceDepthLimit { reference });
        }
        if resolving.contains(&reference) {
            return Err(RecoverError::ReferenceCycle { reference });
        }
        resolving.push(reference);
        let result = self.resolve_located(source, reference, limits, resolving);
        resolving.pop();
        result
    }

    fn resolve_located(
        &self,
        source: &ByteStore,
        reference: Reference,
        limits: RecoverLimits,
        resolving: &mut Vec<Reference>,
    ) -> Result<ScannedResolution, RecoverError> {
        let entry = self
            .entry_for_number(reference.object_number())
            .ok_or(RecoverError::MissingObject { reference })?;
        if entry.reference.generation() != reference.generation() {
            return Err(RecoverError::GenerationMismatch {
                reference,
                located_generation: entry.reference.generation(),
            });
        }

        match entry.location {
            ScannedLocation::Direct { byte_offset } => {
                let object =
                    self.parse_direct(source, reference, byte_offset, limits, resolving)?;
                Ok(ScannedResolution {
                    entry,
                    source: source.clone(),
                    kind: ScannedResolutionKind::Direct(object),
                })
            }
            ScannedLocation::InObjectStream {
                object_stream_number,
                index,
            } => {
                let stream_reference = Reference::new(object_stream_number, 0);
                let resolved_stream =
                    self.resolve_inner(source, stream_reference, limits, resolving)?;
                let ScannedResolutionKind::Direct(object_stream) = &resolved_stream.kind else {
                    return Err(RecoverError::ObjectStreamNotDirect {
                        reference,
                        object_stream_number,
                    });
                };
                let compressed = parse_compressed_object_strict(
                    source,
                    object_stream,
                    0,
                    reference,
                    index,
                    limits.object_stream(),
                    None,
                )
                .map_err(|error| RecoverError::ObjectStream { reference, error })?;
                Ok(ScannedResolution {
                    entry,
                    source: compressed.source().clone(),
                    kind: ScannedResolutionKind::Compressed(compressed.value().clone()),
                })
            }
        }
    }

    fn parse_direct(
        &self,
        source: &ByteStore,
        reference: Reference,
        byte_offset: usize,
        limits: RecoverLimits,
        resolving: &mut Vec<Reference>,
    ) -> Result<IndirectObject, RecoverError> {
        let object = match parse_indirect_object_strict(source, byte_offset, limits.objects) {
            Ok(object) => object,
            Err(error) => match error.kind() {
                IndirectObjectErrorKind::UnresolvedStreamLength(length_reference) => {
                    let length =
                        self.resolve_length(source, length_reference, limits, resolving)?;
                    parse_indirect_object_with_resolved_length_strict(
                        source,
                        byte_offset,
                        limits.objects,
                        Some(ResolvedStreamLength::new(length_reference, length)),
                    )
                    .map_err(|error| RecoverError::Indirect { reference, error })?
                }
                _ => return Err(RecoverError::Indirect { reference, error }),
            },
        };
        if object.reference() != reference {
            return Err(RecoverError::HeaderDisagreesWithScan {
                expected: reference,
                found: object.reference(),
            });
        }
        Ok(object)
    }

    fn resolve_length(
        &self,
        source: &ByteStore,
        reference: Reference,
        limits: RecoverLimits,
        resolving: &mut Vec<Reference>,
    ) -> Result<usize, RecoverError> {
        let resolved = self.resolve_inner(source, reference, limits, resolving)?;
        let ObjectKind::Number(kind) = resolved.value().kind() else {
            return Err(RecoverError::StreamLengthNotInteger { reference });
        };
        if !matches!(kind, crate::NumberKind::Integer) {
            return Err(RecoverError::StreamLengthNotInteger { reference });
        }
        let bytes = resolved
            .source
            .resolve(resolved.value().span())
            .map_err(|_| RecoverError::StreamLengthNotInteger { reference })?;
        crate::value::parse_unsigned(bytes)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or(RecoverError::StreamLengthNotInteger { reference })
    }
}

#[derive(Clone, Debug)]
pub struct ScannedResolution {
    entry: ScannedEntry,
    source: ByteStore,
    kind: ScannedResolutionKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ScannedResolutionKind {
    Direct(IndirectObject),
    Compressed(Object),
}

impl ScannedResolution {
    #[must_use]
    pub const fn entry(&self) -> ScannedEntry {
        self.entry
    }

    #[must_use]
    pub const fn source(&self) -> &ByteStore {
        &self.source
    }

    #[must_use]
    pub const fn value(&self) -> &Object {
        match &self.kind {
            ScannedResolutionKind::Direct(object) => object.value(),
            ScannedResolutionKind::Compressed(value) => value,
        }
    }

    #[must_use]
    pub const fn stream(&self) -> Option<&StreamObject> {
        match &self.kind {
            ScannedResolutionKind::Direct(object) => object.stream(),
            ScannedResolutionKind::Compressed(_) => None,
        }
    }

    #[must_use]
    pub const fn indirect_object(&self) -> Option<&IndirectObject> {
        match &self.kind {
            ScannedResolutionKind::Direct(object) => Some(object),
            ScannedResolutionKind::Compressed(_) => None,
        }
    }

    #[must_use]
    pub const fn is_compressed(&self) -> bool {
        matches!(self.kind, ScannedResolutionKind::Compressed(_))
    }
}

#[derive(Clone, Debug)]
pub enum DocumentObjects {
    Revisions {
        chain: Box<RevisionChain>,
        index: RevisionIndex,
    },
    Scanned(ScannedObjects),
}

pub fn parse_header_recovering(
    source: &ByteStore,
    limits: RecoverLimits,
) -> Result<Recovered<PdfHeader>, HeaderError> {
    fn header_at(
        source: &ByteStore,
        byte_offset: usize,
        repairs: &mut Vec<Repair>,
    ) -> Result<PdfHeader, HeaderError> {
        match crate::parse_header_at(source, byte_offset) {
            Err(HeaderError::MissingLineEnding) => {
                let header = crate::parse_header_at_ending(
                    source,
                    byte_offset,
                    crate::LineEnding::Optional,
                )?;
                repairs.push(Repair::new(
                    byte_offset,
                    RepairKind::HeaderLineEndingMissing { byte_offset },
                ));
                Ok(header)
            }
            other => other,
        }
    }

    let mut repairs = Vec::new();
    if source
        .ahead(0, HEADER_PREFIX.len())
        .starts_with(HEADER_PREFIX)
    {
        let header = header_at(source, 0, &mut repairs)?;
        return Ok(Recovered::new(header, repairs));
    }

    let window_end = limits.max_header_scan_bytes.min(source.len());
    let window = &source.ahead(0, window_end)[..window_end];
    let byte_offset = find_subslice(window, HEADER_PREFIX).ok_or(HeaderError::MissingAtByteZero)?;
    repairs.push(Repair::new(
        byte_offset,
        RepairKind::DisplacedHeader { byte_offset },
    ));
    let header = header_at(source, byte_offset, &mut repairs)?;
    Ok(Recovered::new(header, repairs))
}

pub fn scan_indirect_objects(
    source: &ByteStore,
    limits: RecoverLimits,
) -> Result<Recovered<ScannedObjects>, RecoverError> {
    let mut repairs = Vec::new();
    let mut entries: HashMap<u32, ScannedEntry> = HashMap::new();

    for (reference, byte_offset, span) in scan_object_headers(source, limits)? {
        let entry = ScannedEntry {
            reference,
            location: ScannedLocation::Direct { byte_offset },
            span,
        };
        if let Some(previous) = entries.insert(reference.object_number(), entry) {
            let ScannedLocation::Direct {
                byte_offset: superseded_byte_offset,
            } = previous.location
            else {
                continue;
            };
            repairs.push(Repair::new(
                byte_offset,
                RepairKind::SupersededDefinition {
                    object_number: reference.object_number(),
                    generation: reference.generation(),
                    superseded_byte_offset,
                },
            ));
        }
    }

    if entries.is_empty() {
        return Err(RecoverError::NoIndirectObjectsFound);
    }

    expand_object_streams(source, &mut entries, &mut repairs, limits);
    Ok(Recovered::new(ScannedObjects { entries }, repairs))
}

pub fn open_objects_recovering(
    source: &ByteStore,
    limits: RecoverLimits,
) -> Result<Recovered<DocumentObjects>, RecoverError> {
    let chain_result = parse_revision_chain_strict(source, limits.xref);
    let failure = match chain_result {
        Ok(chain) => match RevisionIndex::from_chain(&chain) {
            Ok(index) => {
                return Ok(Recovered::new(
                    DocumentObjects::Revisions {
                        chain: Box::new(chain),
                        index,
                    },
                    Vec::new(),
                ));
            }
            Err(error) => Repair::new(
                chain.startxref(),
                RepairKind::RebuiltObjectMapAfterIndexFailure { error },
            ),
        },
        Err(error) => Repair::new(
            error.offset(),
            RepairKind::RebuiltObjectMapAfterChainFailure { error },
        ),
    };

    let (scanned, mut repairs) = scan_indirect_objects(source, limits)?.into_parts();
    repairs.insert(0, failure);
    Ok(Recovered::new(DocumentObjects::Scanned(scanned), repairs))
}

fn expand_object_streams(
    source: &ByteStore,
    entries: &mut HashMap<u32, ScannedEntry>,
    repairs: &mut Vec<Repair>,
    limits: RecoverLimits,
) {
    let direct: ScannedObjects = ScannedObjects {
        entries: entries.clone(),
    };
    let mut candidates: Vec<ScannedEntry> = direct.entries().collect();
    candidates.sort_by_key(|entry| entry.reference.object_number());

    let mut expanded = 0usize;
    for candidate in candidates {
        if expanded >= limits.max_object_streams {
            break;
        }
        let Ok(resolved) = direct.resolve_object(source, candidate.reference, limits) else {
            continue;
        };
        let Some(object_stream) = resolved.indirect_object() else {
            continue;
        };
        if !is_object_stream(source, object_stream) {
            continue;
        }
        expanded += 1;

        let object_stream_number = candidate.reference.object_number();
        let contents =
            match object_stream_contents(source, object_stream, 0, limits.object_stream(), None) {
                Ok(contents) => contents,
                Err(error) => {
                    repairs.push(Repair::new(
                        candidate.span.start(),
                        RepairKind::UnreadableObjectStream {
                            object_stream_number,
                            error,
                        },
                    ));
                    continue;
                }
            };

        let mut added = 0usize;
        for (object_number, index) in contents {
            match entries.entry(object_number) {
                Entry::Occupied(_) => repairs.push(Repair::new(
                    candidate.span.start(),
                    RepairKind::DirectDefinitionShadowsObjectStream {
                        object_number,
                        object_stream_number,
                    },
                )),
                Entry::Vacant(vacant) => {
                    vacant.insert(ScannedEntry {
                        reference: Reference::new(object_number, 0),
                        location: ScannedLocation::InObjectStream {
                            object_stream_number,
                            index,
                        },
                        span: candidate.span,
                    });
                    added += 1;
                }
            }
        }
        repairs.push(Repair::new(
            candidate.span.start(),
            RepairKind::ExpandedObjectStream {
                object_stream_number,
                objects_found: added,
            },
        ));
    }
}

fn is_object_stream(source: &ByteStore, object: &IndirectObject) -> bool {
    let ObjectKind::Dictionary(entries) = object.value().kind() else {
        return false;
    };
    object.stream().is_some()
        && entries.iter().any(|entry| {
            entry.key_equals(source, b"/Type")
                && name_object_equals(source, entry.value(), b"/ObjStm")
        })
}

fn scan_object_headers(
    source: &ByteStore,
    limits: RecoverLimits,
) -> Result<Vec<(Reference, usize, SourceSpan)>, RecoverError> {
    let bytes = source.as_bytes();
    let mut found = Vec::new();
    let mut cursor = 0usize;

    while cursor < bytes.len() {
        let Some(relative) = find_subslice(&bytes[cursor..], OBJ_KEYWORD) else {
            break;
        };
        let keyword = cursor + relative;
        cursor = keyword + OBJ_KEYWORD.len();

        if let Some(next) = bytes.get(keyword + OBJ_KEYWORD.len())
            && !is_whitespace(*next)
            && !is_delimiter(*next)
        {
            continue;
        }
        let Some((reference, start)) = read_object_header_before(bytes, keyword) else {
            continue;
        };
        if found.len() >= limits.max_scanned_objects {
            return Err(RecoverError::ScannedObjectLimit);
        }
        let span = source
            .span(start..keyword + OBJ_KEYWORD.len())
            .map_err(|_| RecoverError::SourceSpanFailure)?;
        found.push((reference, start, span));
    }

    Ok(found)
}

fn read_object_header_before(bytes: &[u8], keyword: usize) -> Option<(Reference, usize)> {
    let after_generation = skip_whitespace_back(bytes, keyword)?;
    let (generation, after_number) = read_digits_back(bytes, after_generation)?;
    let generation = u16::try_from(generation).ok()?;
    let number_end = skip_whitespace_back(bytes, after_number)?;
    let (object_number, start) = read_digits_back(bytes, number_end)?;
    let object_number = u32::try_from(object_number)
        .ok()
        .filter(|value| *value != 0)?;

    if start > 0 {
        let previous = bytes[start - 1];
        if !is_whitespace(previous) && !is_delimiter(previous) {
            return None;
        }
    }

    Some((Reference::new(object_number, generation), start))
}

fn skip_whitespace_back(bytes: &[u8], end: usize) -> Option<usize> {
    let mut index = end;
    let mut skipped = 0usize;
    while index > 0 && is_whitespace(bytes[index - 1]) {
        index -= 1;
        skipped += 1;
    }
    (skipped > 0).then_some(index)
}

fn read_digits_back(bytes: &[u8], end: usize) -> Option<(u64, usize)> {
    let mut index = end;
    while index > 0 && bytes[index - 1].is_ascii_digit() && end - index < MAX_NUMBER_DIGITS {
        index -= 1;
    }
    if index == end {
        return None;
    }
    let mut value = 0u64;
    for &byte in &bytes[index..end] {
        value = value.checked_mul(10)?.checked_add(u64::from(byte - b'0'))?;
    }
    Some((value, index))
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoverError {
    NoIndirectObjectsFound,
    ScannedObjectLimit,
    SourceSpanFailure,
    MissingObject {
        reference: Reference,
    },
    GenerationMismatch {
        reference: Reference,
        located_generation: u16,
    },
    HeaderDisagreesWithScan {
        expected: Reference,
        found: Reference,
    },
    ReferenceCycle {
        reference: Reference,
    },
    ReferenceDepthLimit {
        reference: Reference,
    },
    StreamLengthNotInteger {
        reference: Reference,
    },
    ObjectStreamNotDirect {
        reference: Reference,
        object_stream_number: u32,
    },
    Indirect {
        reference: Reference,
        error: IndirectObjectError,
    },
    ObjectStream {
        reference: Reference,
        error: ObjectStreamError,
    },
}

fn reference_text(reference: Reference) -> String {
    format!("{} {} R", reference.object_number(), reference.generation())
}

impl fmt::Display for RecoverError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoIndirectObjectsFound => {
                formatter.write_str("no indirect object was found anywhere in the file")
            }
            Self::ScannedObjectLimit => {
                formatter.write_str("the scan reached its configured object limit")
            }
            Self::SourceSpanFailure => {
                formatter.write_str("a scanned object header produced an invalid source span")
            }
            Self::MissingObject { reference } => write!(
                formatter,
                "{} was not located by the scan",
                reference_text(*reference)
            ),
            Self::GenerationMismatch {
                reference,
                located_generation,
            } => write!(
                formatter,
                "{} was located with generation {located_generation}",
                reference_text(*reference)
            ),
            Self::HeaderDisagreesWithScan { expected, found } => write!(
                formatter,
                "the object at the scanned offset is {}, not {}",
                reference_text(*found),
                reference_text(*expected)
            ),
            Self::ReferenceCycle { reference } => write!(
                formatter,
                "{} takes part in a reference cycle",
                reference_text(*reference)
            ),
            Self::ReferenceDepthLimit { reference } => write!(
                formatter,
                "resolving {} reached the reference depth limit",
                reference_text(*reference)
            ),
            Self::StreamLengthNotInteger { reference } => write!(
                formatter,
                "the stream length object {} is not an integer",
                reference_text(*reference)
            ),
            Self::ObjectStreamNotDirect {
                reference,
                object_stream_number,
            } => write!(
                formatter,
                "{} needs object stream {object_stream_number}, which is not a direct object",
                reference_text(*reference)
            ),
            Self::Indirect { reference, error } => {
                write!(formatter, "{}: {error}", reference_text(*reference))
            }
            Self::ObjectStream { reference, error } => {
                write!(formatter, "{}: {error}", reference_text(*reference))
            }
        }
    }
}

impl std::error::Error for RecoverError {}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdf_bytes::{ByteStore, SourceId};

    use super::{
        DocumentObjects, RecoverError, RecoverLimits, RepairKind, ScannedLocation,
        open_objects_recovering, parse_header_recovering, scan_indirect_objects,
    };
    use crate::{ObjectKind, Reference, XrefLimits};

    #[test]
    fn a_header_with_no_line_ending_is_read_by_a_recovering_parse_and_said_out_loud() {
        let bytes = b"%PDF-1.2\x001 0 obj\n<< /Type /Catalog >>\nendobj\n".to_vec();
        let source = ByteStore::new(SourceId::new(77), Arc::<[u8]>::from(bytes));
        assert_eq!(
            crate::parse_header_strict(&source).expect_err("strict refuses"),
            crate::HeaderError::MissingLineEnding
        );

        let (header, repairs) = parse_header_recovering(&source, RecoverLimits::default())
            .expect("a recovering read takes the version")
            .into_parts();
        assert_eq!(header.version(), crate::PdfVersion::new(1, 2));
        assert!(
            repairs.iter().any(|repair| matches!(
                repair.kind(),
                RepairKind::HeaderLineEndingMissing { byte_offset: 0 }
            )),
            "the assumption is recorded: {repairs:?}"
        );
    }

    fn healthy_pdf() -> Vec<u8> {
        let mut bytes = b"%PDF-1.7\n%\x80\x81\x9e\x9f\n".to_vec();
        let catalog = bytes.len();
        bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        let pages = bytes.len();
        bytes.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Count 0 /Kids [] >>\nendobj\n");
        let xref = bytes.len();
        bytes.extend_from_slice(b"xref\n0 3\n0000000000 65535 f \n");
        bytes.extend_from_slice(format!("{catalog:010} 00000 n \n").as_bytes());
        bytes.extend_from_slice(format!("{pages:010} 00000 n \n").as_bytes());
        bytes.extend_from_slice(b"trailer\n<< /Size 3 /Root 1 0 R >>\nstartxref\n");
        bytes.extend_from_slice(xref.to_string().as_bytes());
        bytes.extend_from_slice(b"\n%%EOF\n");
        bytes
    }

    fn broken_startxref(mut bytes: Vec<u8>) -> Vec<u8> {
        let keyword = b"startxref\n";
        let position = bytes
            .windows(keyword.len())
            .rposition(|window| window == keyword)
            .expect("the control document has a startxref");
        let digits_start = position + keyword.len();
        let digits_end = digits_start
            + bytes[digits_start..]
                .iter()
                .position(|byte| *byte == b'\n')
                .expect("the startxref value ends with a newline");
        bytes.splice(digits_start..digits_end, b"999999".iter().copied());
        bytes
    }

    fn object_stream_pdf() -> Vec<u8> {
        let catalog = b"<< /Type /Catalog /Pages 2 0 R >>";
        let pages = b"<< /Type /Pages /Count 0 /Kids [] >>";
        let header = format!("1 0 2 {} ", catalog.len() + 1);
        let first = header.len();
        let mut payload = header.into_bytes();
        payload.extend_from_slice(catalog);
        payload.push(b' ');
        payload.extend_from_slice(pages);

        let mut bytes = b"%PDF-1.7\n%\x80\x81\x9e\x9f\n".to_vec();
        bytes.extend_from_slice(
            format!(
                "3 0 obj\n<< /Type /ObjStm /N 2 /First {first} /Length {} >>\nstream\n",
                payload.len()
            )
            .as_bytes(),
        );
        bytes.extend_from_slice(&payload);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        bytes.extend_from_slice(b"startxref\n999999\n%%EOF\n");
        bytes
    }

    fn store(bytes: Vec<u8>) -> ByteStore {
        ByteStore::new(SourceId::new(77), Arc::<[u8]>::from(bytes))
    }

    #[test]
    fn recovery_reports_no_repairs_for_a_healthy_document() {
        let source = store(healthy_pdf());
        let recovered = open_objects_recovering(&source, RecoverLimits::default())
            .expect("the control document is valid");
        assert!(recovered.is_clean());
        let (objects, repairs) = recovered.into_parts();
        assert!(repairs.is_empty());
        assert!(matches!(objects, DocumentObjects::Revisions { .. }));
    }

    #[test]
    fn a_header_at_byte_zero_is_not_reported_as_displaced() {
        let source = store(healthy_pdf());
        let recovered = parse_header_recovering(&source, RecoverLimits::default())
            .expect("the control document is valid");
        assert!(recovered.is_clean());
    }

    #[test]
    fn a_displaced_header_is_found_and_reported() {
        let mut bytes = b"\n\n<!-- leading junk -->\n".to_vec();
        let offset = bytes.len();
        bytes.extend_from_slice(&healthy_pdf());
        let source = store(bytes);

        assert!(crate::parse_header_strict(&source).is_err());

        let (header, repairs) = parse_header_recovering(&source, RecoverLimits::default())
            .expect("the header is present, only displaced")
            .into_parts();
        assert_eq!(header.version().major(), 1);
        assert_eq!(header.version().minor(), 7);
        assert_eq!(repairs.len(), 1);
        assert_eq!(
            repairs[0].kind(),
            RepairKind::DisplacedHeader {
                byte_offset: offset
            }
        );
    }

    #[test]
    fn a_header_beyond_the_scan_window_is_not_found() {
        let mut bytes = vec![b' '; 4096];
        bytes.extend_from_slice(&healthy_pdf());
        let source = store(bytes);
        assert!(parse_header_recovering(&source, RecoverLimits::default()).is_err());
    }

    #[test]
    fn a_destroyed_xref_falls_back_to_a_scan_that_still_resolves_the_catalog() {
        let source = store(broken_startxref(healthy_pdf()));

        assert!(crate::parse_revision_chain_strict(&source, XrefLimits::default()).is_err());

        let (objects, repairs) = open_objects_recovering(&source, RecoverLimits::default())
            .expect("the objects are still present in the file")
            .into_parts();
        assert!(matches!(
            repairs[0].kind(),
            RepairKind::RebuiltObjectMapAfterChainFailure { .. }
        ));
        let DocumentObjects::Scanned(scanned) = objects else {
            panic!("a broken chain must produce a scanned map");
        };
        assert_eq!(scanned.len(), 2);

        let catalog = scanned
            .resolve_object(&source, Reference::new(1, 0), RecoverLimits::default())
            .expect("the catalog bytes are intact");
        assert!(matches!(catalog.value().kind(), ObjectKind::Dictionary(_)));
        assert!(matches!(
            catalog.entry().location(),
            ScannedLocation::Direct { .. }
        ));
    }

    #[test]
    fn a_redefined_object_keeps_the_later_definition_and_reports_the_earlier_one() {
        let mut bytes = healthy_pdf();
        let first = bytes
            .windows(7)
            .position(|window| window == b"1 0 obj")
            .expect("control defines object 1");
        bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Replaced true >>\nendobj\n");
        let source = store(bytes);

        let (scanned, repairs) = scan_indirect_objects(&source, RecoverLimits::default())
            .expect("objects are present")
            .into_parts();
        let superseded: Vec<_> = repairs
            .iter()
            .filter(|repair| {
                matches!(
                    repair.kind(),
                    RepairKind::SupersededDefinition {
                        object_number: 1,
                        superseded_byte_offset,
                        ..
                    } if superseded_byte_offset == first
                )
            })
            .collect();
        assert_eq!(superseded.len(), 1);

        let resolved = scanned
            .resolve_object(&source, Reference::new(1, 0), RecoverLimits::default())
            .expect("the later definition parses");
        let ObjectKind::Dictionary(entries) = resolved.value().kind() else {
            panic!("the catalog is a dictionary");
        };
        assert!(
            entries
                .iter()
                .any(|entry| entry.key_equals(&source, b"/Replaced")),
            "the definition later in the file must win"
        );
    }

    #[test]
    fn endobj_is_not_mistaken_for_an_object_header() {
        let source = store(healthy_pdf());
        let (scanned, _) = scan_indirect_objects(&source, RecoverLimits::default())
            .expect("objects are present")
            .into_parts();
        assert_eq!(scanned.len(), 2);
        assert!(scanned.entry_for_number(1).is_some());
        assert!(scanned.entry_for_number(2).is_some());
    }

    #[test]
    fn a_scan_expands_object_streams_so_compressed_objects_are_not_lost() {
        let source = store(object_stream_pdf());
        let (scanned, repairs) = scan_indirect_objects(&source, RecoverLimits::default())
            .expect("the object stream is present")
            .into_parts();

        assert_eq!(scanned.len(), 3);
        assert!(repairs.iter().any(|repair| repair.kind()
            == RepairKind::ExpandedObjectStream {
                object_stream_number: 3,
                objects_found: 2,
            }));

        let catalog = scanned
            .resolve_object(&source, Reference::new(1, 0), RecoverLimits::default())
            .expect("the catalog lives inside the object stream");
        assert!(catalog.is_compressed());
        assert!(matches!(
            catalog.entry().location(),
            ScannedLocation::InObjectStream {
                object_stream_number: 3,
                index: 0,
            }
        ));
        let ObjectKind::Dictionary(entries) = catalog.value().kind() else {
            panic!("the catalog is a dictionary");
        };
        assert!(
            entries
                .iter()
                .any(|entry| entry.key_equals(catalog.source(), b"/Pages")),
            "the compressed catalog must be read from the decoded stream"
        );
    }

    #[test]
    fn an_object_stream_document_recovers_through_the_public_entry_point() {
        let source = store(object_stream_pdf());
        let (objects, repairs) = open_objects_recovering(&source, RecoverLimits::default())
            .expect("the objects are recoverable")
            .into_parts();
        assert!(matches!(
            repairs[0].kind(),
            RepairKind::RebuiltObjectMapAfterChainFailure { .. }
        ));
        let DocumentObjects::Scanned(scanned) = objects else {
            panic!("a broken chain must produce a scanned map");
        };
        assert!(scanned.entry_for_number(1).is_some());
        assert!(scanned.entry_for_number(2).is_some());
    }

    #[test]
    fn a_file_with_no_indirect_objects_fails_rather_than_returning_an_empty_map() {
        let source = store(b"<html><body>not a pdf at all</body></html>".to_vec());
        assert_eq!(
            scan_indirect_objects(&source, RecoverLimits::default()),
            Err(RecoverError::NoIndirectObjectsFound)
        );
        assert_eq!(
            open_objects_recovering(&source, RecoverLimits::default()).err(),
            Some(RecoverError::NoIndirectObjectsFound)
        );
    }
}
