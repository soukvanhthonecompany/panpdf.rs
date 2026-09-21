use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

use pdf_bytes::ByteStore;

use crate::object_stream::{ObjectStreamLimits, parse_compressed_object_strict};
use crate::value::parse_unsigned;
use crate::{
    IndirectObject, IndirectObjectError, IndirectObjectErrorKind, NumberKind, ObjectKind,
    ObjectStreamError, ParseLimits, Reference, ResolvedStreamLength, RevisionChain, StreamObject,
    XrefEntry, XrefEntryKind, parse_indirect_object_with_resolved_length_strict,
};

pub trait StreamDecryptor: fmt::Debug + Send + Sync {
    fn decrypt_stream(
        &self,
        reference: Reference,
        encrypted: &[u8],
    ) -> Result<Vec<u8>, DecryptionRefused>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecryptionRefused {
    reason: &'static str,
}

impl DecryptionRefused {
    #[must_use]
    pub const fn new(reason: &'static str) -> Self {
        Self { reason }
    }

    #[must_use]
    pub const fn reason(self) -> &'static str {
        self.reason
    }
}

impl fmt::Display for DecryptionRefused {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.reason)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SelectedXrefEntry {
    revision_index: usize,
    section_byte_offset: usize,
    entry: XrefEntry,
}

impl SelectedXrefEntry {
    #[must_use]
    pub const fn revision_index(self) -> usize {
        self.revision_index
    }

    #[must_use]
    pub const fn section_byte_offset(self) -> usize {
        self.section_byte_offset
    }

    #[must_use]
    pub const fn entry(self) -> XrefEntry {
        self.entry
    }
}

#[derive(Clone, Debug)]
pub struct RevisionIndex {
    selected: HashMap<u32, SelectedXrefEntry>,
    decryptor: Option<Arc<dyn StreamDecryptor>>,
    damage: Damage,
    undefined: Undefined,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Damage {
    #[default]
    Refuse,
    Repair,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Undefined {
    #[default]
    Refuse,
    Null,
}

impl PartialEq for RevisionIndex {
    fn eq(&self, other: &Self) -> bool {
        self.selected == other.selected
            && self.damage == other.damage
            && self.undefined == other.undefined
            && match (self.decryptor.as_ref(), other.decryptor.as_ref()) {
                (None, None) => true,
                (Some(left), Some(right)) => Arc::ptr_eq(left, right),
                (None, Some(_)) | (Some(_), None) => false,
            }
    }
}

impl Eq for RevisionIndex {}

impl RevisionIndex {
    pub fn from_chain(chain: &RevisionChain) -> Result<Self, ResolveError> {
        let mut selected = HashMap::new();

        for (revision_index, revision) in chain.revisions().iter().enumerate() {
            let mut seen_in_revision = HashSet::new();
            for &entry in revision.entries() {
                let object_number = entry.object_number();
                if !seen_in_revision.insert(object_number) {
                    return Err(ResolveError::new(
                        Some(Reference::new(object_number, entry.generation())),
                        ResolveErrorKind::DuplicateEntryInRevision { revision_index },
                    ));
                }
                if object_number == 0 && !matches!(entry.kind(), XrefEntryKind::Free { .. }) {
                    return Err(ResolveError::new(
                        Some(Reference::new(0, entry.generation())),
                        ResolveErrorKind::ObjectZeroInUse,
                    ));
                }

                selected.entry(object_number).or_insert(SelectedXrefEntry {
                    revision_index,
                    section_byte_offset: revision.byte_offset(),
                    entry,
                });
            }
        }

        Ok(Self {
            selected,
            decryptor: None,
            damage: Damage::default(),
            undefined: Undefined::default(),
        })
    }

    #[must_use]
    pub fn from_scanned(scanned: &crate::recover::ScannedObjects) -> Self {
        let mut selected = HashMap::new();
        for entry in scanned.entries() {
            let reference = entry.reference();
            let kind = match entry.location() {
                crate::recover::ScannedLocation::Direct { byte_offset } => XrefEntryKind::InUse {
                    byte_offset: byte_offset as u64,
                },
                crate::recover::ScannedLocation::InObjectStream {
                    object_stream_number,
                    index,
                } => XrefEntryKind::Compressed {
                    object_stream_number,
                    index,
                },
            };
            selected.insert(
                reference.object_number(),
                SelectedXrefEntry {
                    revision_index: 0,
                    section_byte_offset: 0,
                    entry: XrefEntry::rebuilt(
                        reference.object_number(),
                        reference.generation(),
                        kind,
                        entry.span(),
                    ),
                },
            );
        }
        Self {
            selected,
            decryptor: None,
            damage: Damage::default(),
            undefined: Undefined::default(),
        }
    }

    #[must_use]
    pub fn with_stream_decryptor(mut self, decryptor: Arc<dyn StreamDecryptor>) -> Self {
        self.decryptor = Some(decryptor);
        self
    }

    #[must_use]
    pub fn tolerating_damage(mut self) -> Self {
        self.damage = Damage::Repair;
        self
    }

    #[must_use]
    pub fn resolving_undefined_as_null(mut self) -> Self {
        self.undefined = Undefined::Null;
        self
    }

    #[must_use]
    pub const fn undefined(&self) -> Undefined {
        self.undefined
    }

    #[must_use]
    pub const fn damage(&self) -> Damage {
        self.damage
    }

    pub fn from_classic_chain(chain: &RevisionChain) -> Result<Self, ResolveError> {
        Self::from_chain(chain)
    }

    #[must_use]
    pub fn selected_for_number(&self, object_number: u32) -> Option<SelectedXrefEntry> {
        self.selected.get(&object_number).copied()
    }

    pub fn selected_entries(&self) -> impl Iterator<Item = SelectedXrefEntry> + '_ {
        self.selected.values().copied()
    }

    pub fn lookup(&self, reference: Reference) -> Result<SelectedXrefEntry, ResolveError> {
        let selected = self
            .selected_for_number(reference.object_number())
            .ok_or_else(|| ResolveError::new(Some(reference), ResolveErrorKind::MissingObject))?;
        if matches!(selected.entry.kind(), XrefEntryKind::Free { .. }) {
            return Err(ResolveError::new(
                Some(reference),
                ResolveErrorKind::FreedObject {
                    revision_index: selected.revision_index,
                },
            ));
        }
        if selected.entry.generation() != reference.generation() {
            return Err(ResolveError::new(
                Some(reference),
                ResolveErrorKind::GenerationMismatch {
                    active_generation: selected.entry.generation(),
                },
            ));
        }
        Ok(selected)
    }

    pub fn resolve_object(
        &self,
        source: &ByteStore,
        reference: Reference,
        limits: ResolveLimits,
    ) -> Result<ResolvedObject, ResolveError> {
        self.resolve_object_inner(source, reference, limits, &mut Vec::new())
    }

    pub(crate) fn resolve_unsigned_integer(
        &self,
        source: &ByteStore,
        reference: Reference,
        limits: ResolveLimits,
    ) -> Result<usize, ResolveError> {
        self.resolve_integer_reference(source, reference, limits, &mut Vec::new())
    }

    fn resolve_object_inner(
        &self,
        source: &ByteStore,
        reference: Reference,
        limits: ResolveLimits,
        resolving: &mut Vec<Reference>,
    ) -> Result<ResolvedObject, ResolveError> {
        if resolving.len() >= limits.max_reference_depth {
            return Err(ResolveError::new(
                Some(reference),
                ResolveErrorKind::ReferenceDepthLimit,
            ));
        }
        if resolving.contains(&reference) {
            return Err(ResolveError::new(
                Some(reference),
                ResolveErrorKind::ReferenceCycle,
            ));
        }
        resolving.push(reference);
        let result = self.resolve_selected_object(source, reference, limits, resolving);
        resolving.pop();
        result
    }

    fn resolve_selected_object(
        &self,
        source: &ByteStore,
        reference: Reference,
        limits: ResolveLimits,
        resolving: &mut Vec<Reference>,
    ) -> Result<ResolvedObject, ResolveError> {
        let selected = match self.lookup(reference) {
            Ok(selected) => selected,
            Err(error)
                if self.undefined == Undefined::Null
                    && matches!(
                        error.kind(),
                        ResolveErrorKind::MissingObject
                            | ResolveErrorKind::FreedObject { .. }
                            | ResolveErrorKind::ObjectOffsetOutOfBounds
                    ) =>
            {
                return Ok(ResolvedObject {
                    selected: None,
                    source: source.clone(),
                    kind: ResolvedObjectKind::Compressed(crate::Object::null_at(
                        crate::SourceSpan::empty(source.id()),
                    )),
                    repairs: vec![crate::IndirectRepair::UndefinedObject {
                        object_number: reference.object_number(),
                        generation: reference.generation(),
                    }],
                });
            }
            Err(error) => return Err(error),
        };
        match selected.entry.kind() {
            XrefEntryKind::InUse { .. } => {
                let (object, repairs) =
                    self.resolve_direct_object(source, reference, selected, limits, resolving)?;
                Ok(ResolvedObject {
                    selected: Some(selected),
                    source: source.clone(),
                    kind: ResolvedObjectKind::Indirect(object),
                    repairs,
                })
            }
            XrefEntryKind::Compressed {
                object_stream_number,
                index,
            } => {
                let object_stream_reference = Reference::new(object_stream_number, 0);
                let object_stream_selection = self.lookup(object_stream_reference)?;
                if !matches!(
                    object_stream_selection.entry.kind(),
                    XrefEntryKind::InUse { .. }
                ) {
                    return Err(ResolveError::new(
                        Some(reference),
                        ResolveErrorKind::ObjectStreamNotDirect,
                    ));
                }
                let resolved_stream =
                    self.resolve_object_inner(source, object_stream_reference, limits, resolving)?;
                let object_stream = resolved_stream.indirect_object().ok_or_else(|| {
                    ResolveError::new(Some(reference), ResolveErrorKind::ObjectStreamNotDirect)
                })?;
                let compressed = parse_compressed_object_strict(
                    source,
                    object_stream,
                    object_stream_selection.revision_index,
                    reference,
                    index,
                    ObjectStreamLimits {
                        objects: limits.objects,
                        max_objects: limits.max_object_stream_objects,
                        max_decoded_bytes: limits.max_decoded_stream_bytes,
                    },
                    self.decryptor.as_deref(),
                )
                .map_err(|error| ResolveError::object_stream(reference, error))?;
                Ok(ResolvedObject {
                    selected: Some(selected),
                    source: compressed.source().clone(),
                    kind: ResolvedObjectKind::Compressed(compressed.value().clone()),
                    repairs: Vec::new(),
                })
            }
            XrefEntryKind::Free { .. } => unreachable!("lookup rejects free entries"),
        }
    }

    fn resolve_direct_object(
        &self,
        source: &ByteStore,
        reference: Reference,
        selected: SelectedXrefEntry,
        limits: ResolveLimits,
        resolving: &mut Vec<Reference>,
    ) -> Result<(IndirectObject, Vec<crate::IndirectRepair>), ResolveError> {
        let offset = direct_offset(selected, source, reference)?;
        let read = |length: Option<ResolvedStreamLength>| match self.damage {
            Damage::Refuse => parse_indirect_object_with_resolved_length_strict(
                source,
                offset,
                limits.objects,
                length,
            )
            .map(|object| (object, Vec::new())),
            Damage::Repair => {
                crate::parse_indirect_object_recovering(source, offset, limits.objects, length)
            }
        };
        let (object, repairs) = match read(None) {
            Ok(read) => read,
            Err(error) => match error.kind() {
                IndirectObjectErrorKind::UnresolvedStreamLength(length_reference) => {
                    let length = self.resolve_integer_reference(
                        source,
                        length_reference,
                        limits,
                        resolving,
                    )?;
                    read(Some(ResolvedStreamLength::new(length_reference, length)))
                        .map_err(|error| ResolveError::indirect(reference, error))?
                }
                _ => return Err(ResolveError::indirect(reference, error)),
            },
        };
        verify_header(reference, &object)?;
        Ok((object, repairs))
    }

    fn resolve_integer_reference(
        &self,
        source: &ByteStore,
        reference: Reference,
        limits: ResolveLimits,
        resolving: &mut Vec<Reference>,
    ) -> Result<usize, ResolveError> {
        if resolving.len() >= limits.max_reference_depth {
            return Err(ResolveError::new(
                Some(reference),
                ResolveErrorKind::ReferenceDepthLimit,
            ));
        }
        if resolving.contains(&reference) {
            return Err(ResolveError::new(
                Some(reference),
                ResolveErrorKind::ReferenceCycle,
            ));
        }
        resolving.push(reference);
        let result = self.resolve_integer_selected(source, reference, limits, resolving);
        resolving.pop();
        result
    }

    fn resolve_integer_selected(
        &self,
        source: &ByteStore,
        reference: Reference,
        limits: ResolveLimits,
        resolving: &mut Vec<Reference>,
    ) -> Result<usize, ResolveError> {
        let object = self.resolve_selected_object(source, reference, limits, resolving)?;
        if object.stream().is_some() {
            return Err(ResolveError::new(
                Some(reference),
                ResolveErrorKind::StreamLengthNotInteger,
            ));
        }

        match object.value().kind() {
            ObjectKind::Number(NumberKind::Integer) => {
                let raw = object
                    .source()
                    .resolve(object.value().span())
                    .map_err(|_| {
                        ResolveError::new(Some(reference), ResolveErrorKind::SourceSpanFailure)
                    })?;
                parse_unsigned(raw)
                    .and_then(|value| usize::try_from(value).ok())
                    .ok_or_else(|| {
                        ResolveError::new(Some(reference), ResolveErrorKind::StreamLengthOutOfRange)
                    })
            }
            ObjectKind::Reference(next) => {
                self.resolve_integer_reference(source, *next, limits, resolving)
            }
            _ => Err(ResolveError::new(
                Some(reference),
                ResolveErrorKind::StreamLengthNotInteger,
            )),
        }
    }
}

fn direct_offset(
    selected: SelectedXrefEntry,
    source: &ByteStore,
    reference: Reference,
) -> Result<usize, ResolveError> {
    let byte_offset = match selected.entry.kind() {
        XrefEntryKind::InUse { byte_offset } => byte_offset,
        XrefEntryKind::Compressed { .. } => {
            return Err(ResolveError::new(
                Some(reference),
                ResolveErrorKind::ExpectedDirectObject,
            ));
        }
        XrefEntryKind::Free { .. } => unreachable!("lookup rejects free entries"),
    };
    let offset = usize::try_from(byte_offset).map_err(|_| {
        ResolveError::new(Some(reference), ResolveErrorKind::ObjectOffsetOutOfBounds)
    })?;
    if offset >= source.len() {
        return Err(ResolveError::new(
            Some(reference),
            ResolveErrorKind::ObjectOffsetOutOfBounds,
        ));
    }
    Ok(offset)
}

fn verify_header(reference: Reference, object: &IndirectObject) -> Result<(), ResolveError> {
    if object.reference() != reference {
        return Err(ResolveError::new(
            Some(reference),
            ResolveErrorKind::ObjectHeaderMismatch {
                actual: object.reference(),
            },
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolveLimits {
    pub objects: ParseLimits,
    pub max_reference_depth: usize,
    pub max_object_stream_objects: usize,
    pub max_decoded_stream_bytes: usize,
}

impl Default for ResolveLimits {
    fn default() -> Self {
        Self {
            objects: ParseLimits::default(),
            max_reference_depth: 128,
            max_object_stream_objects: 1_000_000,
            max_decoded_stream_bytes: 256 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ResolvedObject {
    selected: Option<SelectedXrefEntry>,
    source: ByteStore,
    kind: ResolvedObjectKind,
    repairs: Vec<crate::IndirectRepair>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ResolvedObjectKind {
    Indirect(IndirectObject),
    Compressed(crate::Object),
}

impl ResolvedObject {
    #[must_use]
    pub const fn selected_entry(&self) -> Option<SelectedXrefEntry> {
        self.selected
    }

    #[must_use]
    pub fn repairs(&self) -> &[crate::IndirectRepair] {
        &self.repairs
    }

    #[must_use]
    pub const fn source(&self) -> &ByteStore {
        &self.source
    }

    #[must_use]
    pub const fn value(&self) -> &crate::Object {
        match &self.kind {
            ResolvedObjectKind::Indirect(object) => object.value(),
            ResolvedObjectKind::Compressed(value) => value,
        }
    }

    #[must_use]
    pub const fn stream(&self) -> Option<&StreamObject> {
        match &self.kind {
            ResolvedObjectKind::Indirect(object) => object.stream(),
            ResolvedObjectKind::Compressed(_) => None,
        }
    }

    #[must_use]
    pub const fn indirect_object(&self) -> Option<&IndirectObject> {
        match &self.kind {
            ResolvedObjectKind::Indirect(object) => Some(object),
            ResolvedObjectKind::Compressed(_) => None,
        }
    }

    #[must_use]
    pub const fn is_compressed(&self) -> bool {
        matches!(self.kind, ResolvedObjectKind::Compressed(_))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolveError {
    reference: Option<Reference>,
    kind: ResolveErrorKind,
}

impl ResolveError {
    const fn new(reference: Option<Reference>, kind: ResolveErrorKind) -> Self {
        Self { reference, kind }
    }

    const fn indirect(reference: Reference, error: IndirectObjectError) -> Self {
        Self::new(Some(reference), ResolveErrorKind::IndirectObject(error))
    }

    const fn object_stream(reference: Reference, error: ObjectStreamError) -> Self {
        Self::new(Some(reference), ResolveErrorKind::ObjectStream(error))
    }

    #[must_use]
    pub const fn reference(self) -> Option<Reference> {
        self.reference
    }

    #[must_use]
    pub const fn kind(self) -> ResolveErrorKind {
        self.kind
    }
}

impl fmt::Display for ResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(reference) = self.reference {
            write!(
                formatter,
                "{} while resolving {} {} R",
                self.kind,
                reference.object_number(),
                reference.generation()
            )
        } else {
            self.kind.fmt(formatter)
        }
    }
}

impl std::error::Error for ResolveError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolveErrorKind {
    DuplicateEntryInRevision { revision_index: usize },
    ObjectZeroInUse,
    MissingObject,
    FreedObject { revision_index: usize },
    GenerationMismatch { active_generation: u16 },
    ObjectOffsetOutOfBounds,
    ExpectedDirectObject,
    ObjectStreamNotDirect,
    ObjectHeaderMismatch { actual: Reference },
    IndirectObject(IndirectObjectError),
    ObjectStream(ObjectStreamError),
    ReferenceCycle,
    ReferenceDepthLimit,
    StreamLengthNotInteger,
    StreamLengthOutOfRange,
    SourceSpanFailure,
}

impl fmt::Display for ResolveErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateEntryInRevision { revision_index } => {
                write!(
                    formatter,
                    "duplicate xref entry in revision {revision_index}"
                )
            }
            Self::FreedObject { revision_index } => {
                write!(formatter, "object was freed by revision {revision_index}")
            }
            Self::GenerationMismatch { active_generation } => {
                write!(formatter, "active object generation is {active_generation}")
            }
            Self::ObjectHeaderMismatch { actual } => write!(
                formatter,
                "xref target header declares {} {} R",
                actual.object_number(),
                actual.generation()
            ),
            Self::IndirectObject(error) => error.fmt(formatter),
            Self::ObjectStream(error) => error.fmt(formatter),
            _ => formatter.write_str(match self {
                Self::ObjectZeroInUse => "xref marks reserved object zero as in use",
                Self::MissingObject => "object number is absent from the revision chain",
                Self::ObjectOffsetOutOfBounds => "xref object offset lies outside the source",
                Self::ExpectedDirectObject => "operation requires an uncompressed indirect object",
                Self::ObjectStreamNotDirect => "an object stream is itself marked as compressed",
                Self::ReferenceCycle => "indirect reference chain contains a cycle",
                Self::ReferenceDepthLimit => "indirect reference depth limit exceeded",
                Self::StreamLengthNotInteger => {
                    "indirect stream Length does not resolve to an integer"
                }
                Self::StreamLengthOutOfRange => {
                    "indirect stream Length is negative or exceeds addressable memory"
                }
                Self::SourceSpanFailure => "resolved object span does not belong to the source",
                Self::DuplicateEntryInRevision { .. }
                | Self::FreedObject { .. }
                | Self::GenerationMismatch { .. }
                | Self::ObjectHeaderMismatch { .. }
                | Self::IndirectObject(_)
                | Self::ObjectStream(_) => unreachable!(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdf_bytes::{ByteStore, SourceId};

    use super::{
        DecryptionRefused, ResolveErrorKind, ResolveLimits, RevisionIndex, StreamDecryptor,
    };
    use crate::{
        ObjectKind, Reference, XrefErrorKind, XrefLimits, XrefSection,
        parse_classic_revision_chain_strict, parse_revision_chain_strict,
    };

    fn store(bytes: Vec<u8>) -> ByteStore {
        ByteStore::new(SourceId::new(31), Arc::<[u8]>::from(bytes))
    }

    fn append_xref(
        bytes: &mut Vec<u8>,
        entries: &[(u32, usize, u16, u8)],
        previous: Option<usize>,
    ) -> usize {
        let xref = bytes.len();
        bytes.extend_from_slice(b"xref\n");
        for &(number, offset, generation, flag) in entries {
            bytes.extend_from_slice(format!("{number} 1\n").as_bytes());
            bytes.extend_from_slice(
                format!("{offset:010} {generation:05} {} \n", char::from(flag)).as_bytes(),
            );
        }
        bytes.extend_from_slice(b"trailer\n<< /Size 20");
        if let Some(previous) = previous {
            bytes.extend_from_slice(format!(" /Prev {previous}").as_bytes());
        }
        bytes.extend_from_slice(b" >>\nstartxref\n");
        bytes.extend_from_slice(xref.to_string().as_bytes());
        bytes.extend_from_slice(b"\n%%EOF\n");
        xref
    }

    fn encode_stream_entry(kind: u8, second: u32, third: u16) -> [u8; 7] {
        let second = second.to_be_bytes();
        let third = third.to_be_bytes();
        [
            kind, second[0], second[1], second[2], second[3], third[0], third[1],
        ]
    }

    fn hybrid_pdf(conflicting_classic_entry: bool) -> Vec<u8> {
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let catalog = bytes.len();
        bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog >>\nendobj\n");
        let main_xref = bytes.len();
        bytes.extend_from_slice(b"xref\n0 4\n0000000000 65535 f \n");
        bytes.extend_from_slice(format!("{catalog:010} 00000 n \n").as_bytes());
        bytes.extend_from_slice(b"0000000000 65535 f \n0000000000 65535 f \n");
        bytes.extend_from_slice(b"trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n");
        bytes.extend_from_slice(main_xref.to_string().as_bytes());
        bytes.extend_from_slice(b"\n%%EOF\n");

        let object_stream_offset = bytes.len();
        let object_stream_data = b"3 0 << /Hybrid true >>";
        bytes.extend_from_slice(
            format!(
                "4 0 obj\n<< /Type /ObjStm /N 1 /First 4 /Length {} >>\nstream\n",
                object_stream_data.len()
            )
            .as_bytes(),
        );
        bytes.extend_from_slice(object_stream_data);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");

        let supplemental_offset = bytes.len();
        let mut xref_data = Vec::new();
        xref_data.extend_from_slice(&encode_stream_entry(2, 4, 0));
        xref_data.extend_from_slice(&encode_stream_entry(
            1,
            u32::try_from(object_stream_offset).expect("fixture offset"),
            0,
        ));
        xref_data.extend_from_slice(&encode_stream_entry(
            1,
            u32::try_from(supplemental_offset).expect("fixture offset"),
            0,
        ));
        bytes.extend_from_slice(
            format!(
                "5 0 obj\n<< /Type /XRef /Size 6 /Index [3 3] /W [1 4 2] /Prev {main_xref} /Length {} >>\nstream\n",
                xref_data.len()
            )
            .as_bytes(),
        );
        bytes.extend_from_slice(&xref_data);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");

        let update_xref = bytes.len();
        bytes.extend_from_slice(b"xref\n");
        if conflicting_classic_entry {
            bytes.extend_from_slice(b"3 1\n");
            bytes.extend_from_slice(format!("{object_stream_offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!(
                "trailer\n<< /Size 6 /Root 1 0 R /Prev {main_xref} /XRefStm {supplemental_offset} >>\nstartxref\n{update_xref}\n%%EOF\n"
            )
            .as_bytes(),
        );
        bytes
    }

    fn scrambled_object_stream_pdf() -> Vec<u8> {
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let catalog = bytes.len();
        bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 3 0 R >>\nendobj\n");

        let object_stream_offset = bytes.len();
        let plaintext = b"3 0 << /Type /Pages /Count 0 >>";
        let scrambled: Vec<u8> = plaintext.iter().map(|byte| byte ^ SCRAMBLE_KEY).collect();
        bytes.extend_from_slice(
            format!(
                "4 0 obj\n<< /Type /ObjStm /N 1 /First 4 /Length {} >>\nstream\n",
                scrambled.len()
            )
            .as_bytes(),
        );
        bytes.extend_from_slice(&scrambled);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");

        let xref_offset = bytes.len();
        let mut xref_data = Vec::new();
        xref_data.extend_from_slice(&encode_stream_entry(0, 0, 0xffff));
        xref_data.extend_from_slice(&encode_stream_entry(
            1,
            u32::try_from(catalog).expect("fixture offset"),
            0,
        ));
        xref_data.extend_from_slice(&encode_stream_entry(0, 0, 0xffff));
        xref_data.extend_from_slice(&encode_stream_entry(2, 4, 0));
        xref_data.extend_from_slice(&encode_stream_entry(
            1,
            u32::try_from(object_stream_offset).expect("fixture offset"),
            0,
        ));
        xref_data.extend_from_slice(&encode_stream_entry(
            1,
            u32::try_from(xref_offset).expect("fixture offset"),
            0,
        ));
        bytes.extend_from_slice(
            format!(
                "5 0 obj\n<< /Type /XRef /Size 6 /Index [0 6] /W [1 4 2] /Root 1 0 R /Length {} >>\nstream\n",
                xref_data.len()
            )
            .as_bytes(),
        );
        bytes.extend_from_slice(&xref_data);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        bytes.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF\n").as_bytes());
        bytes
    }

    const SCRAMBLE_KEY: u8 = 0x5a;

    #[derive(Debug)]
    struct Unscrambler;

    impl StreamDecryptor for Unscrambler {
        fn decrypt_stream(
            &self,
            reference: Reference,
            encrypted: &[u8],
        ) -> Result<Vec<u8>, DecryptionRefused> {
            if reference != Reference::new(4, 0) {
                return Err(DecryptionRefused::new("unexpected object"));
            }
            Ok(encrypted.iter().map(|byte| byte ^ SCRAMBLE_KEY).collect())
        }
    }

    #[test]
    fn a_compressed_object_is_decrypted_before_its_container_is_decoded() {
        let bytes = scrambled_object_stream_pdf();
        let source = ByteStore::new(SourceId::new(41), Arc::<[u8]>::from(bytes));
        let chain = parse_revision_chain_strict(&source, XrefLimits::default())
            .expect("scrambled object stream fixture");
        let plain = RevisionIndex::from_chain(&chain).expect("index");

        let error = plain
            .resolve_object(&source, Reference::new(3, 0), ResolveLimits::default())
            .expect_err("ciphertext must not decode");
        assert!(matches!(error.kind(), ResolveErrorKind::ObjectStream(_)));

        let index = plain.with_stream_decryptor(Arc::new(Unscrambler));
        let resolved = index
            .resolve_object(&source, Reference::new(3, 0), ResolveLimits::default())
            .expect("a decrypted object stream resolves its objects");
        let ObjectKind::Dictionary(entries) = resolved.value().kind() else {
            panic!("expected the compressed object to be a dictionary")
        };
        assert!(
            entries
                .iter()
                .any(|entry| entry.key_equals(resolved.source(), b"/Count"))
        );
    }

    fn compressed_length_pdf(object_stream_length: &[u8]) -> Vec<u8> {
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let stream_offset = bytes.len();
        bytes
            .extend_from_slice(b"1 0 obj\n<< /Length 3 0 R >>\nstream\nabcde\nendstream\nendobj\n");

        let object_stream_offset = bytes.len();
        bytes.extend_from_slice(b"4 0 obj\n<< /Type /ObjStm /N 1 /First 4 /Length ");
        bytes.extend_from_slice(object_stream_length);
        bytes.extend_from_slice(b" >>\nstream\n3 0 5\nendstream\nendobj\n");

        let xref_offset = bytes.len();
        let mut xref_data = Vec::new();
        xref_data.extend_from_slice(&encode_stream_entry(0, 0, u16::MAX));
        xref_data.extend_from_slice(&encode_stream_entry(
            1,
            u32::try_from(stream_offset).expect("fixture offset"),
            0,
        ));
        xref_data.extend_from_slice(&encode_stream_entry(0, 0, 0));
        xref_data.extend_from_slice(&encode_stream_entry(2, 4, 0));
        xref_data.extend_from_slice(&encode_stream_entry(
            1,
            u32::try_from(object_stream_offset).expect("fixture offset"),
            0,
        ));
        xref_data.extend_from_slice(&encode_stream_entry(
            1,
            u32::try_from(xref_offset).expect("fixture offset"),
            0,
        ));
        bytes.extend_from_slice(
            format!(
                "5 0 obj\n<< /Type /XRef /Size 6 /W [1 4 2] /Length {} >>\nstream\n",
                xref_data.len()
            )
            .as_bytes(),
        );
        bytes.extend_from_slice(&xref_data);
        bytes.extend_from_slice(b"\nendstream\nendobj\nstartxref\n");
        bytes.extend_from_slice(xref_offset.to_string().as_bytes());
        bytes.extend_from_slice(b"\n%%EOF\n");
        bytes
    }

    #[test]
    fn newest_entry_wins_and_records_its_revision() {
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let old = bytes.len();
        bytes.extend_from_slice(b"1 0 obj\n(old)\nendobj\n");
        let first_xref = append_xref(&mut bytes, &[(1, old, 0, b'n')], None);
        let new = bytes.len();
        bytes.extend_from_slice(b"1 0 obj\n(new)\nendobj\n");
        append_xref(&mut bytes, &[(1, new, 0, b'n')], Some(first_xref));
        let source = store(bytes);
        let chain = parse_classic_revision_chain_strict(&source, XrefLimits::default())
            .expect("valid revisions");
        let index = RevisionIndex::from_classic_chain(&chain).expect("unambiguous xref");

        let object = index
            .resolve_object(&source, Reference::new(1, 0), ResolveLimits::default())
            .expect("active object");
        assert_eq!(
            object
                .selected_entry()
                .expect("an object the file defines")
                .revision_index(),
            0
        );
        assert_eq!(
            object.source().resolve(object.value().span()),
            Ok(&b"(new)"[..])
        );
    }

    #[test]
    fn a_free_entry_masks_an_older_object() {
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let old = bytes.len();
        bytes.extend_from_slice(b"1 0 obj\n(old)\nendobj\n");
        let first_xref = append_xref(&mut bytes, &[(1, old, 0, b'n')], None);
        append_xref(&mut bytes, &[(1, 0, 1, b'f')], Some(first_xref));
        let source = store(bytes);
        let chain = parse_classic_revision_chain_strict(&source, XrefLimits::default())
            .expect("valid revisions");
        let index = RevisionIndex::from_classic_chain(&chain).expect("unambiguous xref");

        let error = index
            .lookup(Reference::new(1, 0))
            .expect_err("freed object must not resurrect");
        assert_eq!(
            error.kind(),
            ResolveErrorKind::FreedObject { revision_index: 0 }
        );
    }

    #[test]
    fn a_reference_into_nothing_is_the_null_object_only_when_asked_for() {
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let one = bytes.len();
        bytes.extend_from_slice(b"1 0 obj\n(here)\nendobj\n");
        append_xref(&mut bytes, &[(1, one, 0, b'n'), (3, 0, 1, b'f')], None);
        let source = store(bytes);
        let chain = parse_classic_revision_chain_strict(&source, XrefLimits::default())
            .expect("valid revisions");
        let strict = RevisionIndex::from_classic_chain(&chain).expect("unambiguous xref");

        for (reference, expected) in [
            (Reference::new(2, 0), ResolveErrorKind::MissingObject),
            (
                Reference::new(3, 0),
                ResolveErrorKind::FreedObject { revision_index: 0 },
            ),
        ] {
            assert_eq!(
                strict
                    .resolve_object(&source, reference, ResolveLimits::default())
                    .expect_err("strict refuses a reference into nothing")
                    .kind(),
                expected
            );
        }

        let lenient = strict.clone().resolving_undefined_as_null();
        for reference in [Reference::new(2, 0), Reference::new(3, 0)] {
            let object = lenient
                .resolve_object(&source, reference, ResolveLimits::default())
                .expect("7.3.10 makes this the null object");
            assert!(matches!(object.value().kind(), crate::ObjectKind::Null));
            assert_eq!(
                object.repairs(),
                [crate::IndirectRepair::UndefinedObject {
                    object_number: reference.object_number(),
                    generation: reference.generation(),
                }]
            );
            assert!(object.selected_entry().is_none());
        }

        assert_eq!(
            lenient
                .resolve_object(&source, Reference::new(1, 0), ResolveLimits::default())
                .expect("a defined object")
                .repairs(),
            []
        );
        assert_eq!(
            lenient
                .resolve_object(&source, Reference::new(1, 7), ResolveLimits::default())
                .expect_err("a different generation is not an absence")
                .kind(),
            ResolveErrorKind::GenerationMismatch {
                active_generation: 0
            }
        );
    }

    #[test]
    fn resolves_indirect_stream_length_through_the_active_index() {
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let stream = bytes.len();
        bytes
            .extend_from_slice(b"1 0 obj\n<< /Length 2 0 R >>\nstream\nabcde\nendstream\nendobj\n");
        let length = bytes.len();
        bytes.extend_from_slice(b"2 0 obj\n5\nendobj\n");
        append_xref(
            &mut bytes,
            &[(1, stream, 0, b'n'), (2, length, 0, b'n')],
            None,
        );
        let source = store(bytes);
        let chain = parse_classic_revision_chain_strict(&source, XrefLimits::default())
            .expect("valid revisions");
        let index = RevisionIndex::from_classic_chain(&chain).expect("unambiguous xref");

        let object = index
            .resolve_object(&source, Reference::new(1, 0), ResolveLimits::default())
            .expect("resolved stream");
        let stream = object.stream().expect("stream metadata");
        assert_eq!(source.resolve(stream.data_span()), Ok(&b"abcde"[..]));
    }

    #[test]
    fn detects_length_reference_cycles() {
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let stream = bytes.len();
        bytes.extend_from_slice(b"1 0 obj\n<< /Length 2 0 R >>\nstream\nx\nendstream\nendobj\n");
        let second = bytes.len();
        bytes.extend_from_slice(b"2 0 obj\n3 0 R\nendobj\n");
        let third = bytes.len();
        bytes.extend_from_slice(b"3 0 obj\n2 0 R\nendobj\n");
        append_xref(
            &mut bytes,
            &[
                (1, stream, 0, b'n'),
                (2, second, 0, b'n'),
                (3, third, 0, b'n'),
            ],
            None,
        );
        let source = store(bytes);
        let chain = parse_classic_revision_chain_strict(&source, XrefLimits::default())
            .expect("valid revisions");
        let index = RevisionIndex::from_classic_chain(&chain).expect("unambiguous xref");

        let error = index
            .resolve_object(&source, Reference::new(1, 0), ResolveLimits::default())
            .expect_err("reference loop must fail");
        assert_eq!(error.kind(), ResolveErrorKind::ReferenceCycle);
    }

    #[test]
    fn resolves_stream_length_from_a_compressed_object() {
        let source = store(compressed_length_pdf(b"5"));
        let chain = parse_revision_chain_strict(&source, XrefLimits::default())
            .expect("xref stream with compressed length object");
        let index = RevisionIndex::from_chain(&chain).expect("unambiguous active xref");

        let object = index
            .resolve_object(&source, Reference::new(1, 0), ResolveLimits::default())
            .expect("stream whose length is compressed");
        let stream = object.stream().expect("stream metadata");
        assert_eq!(source.resolve(stream.data_span()), Ok(&b"abcde"[..]));
    }

    #[test]
    fn detects_an_object_stream_length_bootstrap_cycle() {
        let source = store(compressed_length_pdf(b"3 0 R"));
        let chain = parse_revision_chain_strict(&source, XrefLimits::default())
            .expect("xref stream with cyclic object-stream length");
        let index = RevisionIndex::from_chain(&chain).expect("unambiguous active xref");

        let error = index
            .resolve_object(&source, Reference::new(3, 0), ResolveLimits::default())
            .expect_err("object stream cannot obtain its length from its own member");
        assert_eq!(error.kind(), ResolveErrorKind::ReferenceCycle);
    }

    #[test]
    fn rejects_duplicate_entries_and_header_disagreement() {
        let mut duplicate = b"%PDF-1.7\n".to_vec();
        let object = duplicate.len();
        duplicate.extend_from_slice(b"1 0 obj\nnull\nendobj\n");
        append_xref(
            &mut duplicate,
            &[(1, object, 0, b'n'), (1, object, 0, b'n')],
            None,
        );
        let source = store(duplicate);
        let chain = parse_classic_revision_chain_strict(&source, XrefLimits::default())
            .expect("syntactically valid xref");
        let error = RevisionIndex::from_classic_chain(&chain)
            .expect_err("duplicate definition is ambiguous");
        assert_eq!(
            error.kind(),
            ResolveErrorKind::DuplicateEntryInRevision { revision_index: 0 }
        );

        let mut mismatch = b"%PDF-1.7\n".to_vec();
        let object = mismatch.len();
        mismatch.extend_from_slice(b"2 0 obj\nnull\nendobj\n");
        append_xref(&mut mismatch, &[(1, object, 0, b'n')], None);
        let source = store(mismatch);
        let chain = parse_classic_revision_chain_strict(&source, XrefLimits::default())
            .expect("syntactically valid xref");
        let index = RevisionIndex::from_classic_chain(&chain).expect("one xref entry");
        let error = index
            .resolve_object(&source, Reference::new(1, 0), ResolveLimits::default())
            .expect_err("xref and object header must agree");
        assert_eq!(
            error.kind(),
            ResolveErrorKind::ObjectHeaderMismatch {
                actual: Reference::new(2, 0)
            }
        );

        assert!(matches!(
            index
                .selected_for_number(1)
                .map(|selected| selected.entry().kind()),
            Some(crate::XrefEntryKind::InUse { .. })
        ));
        assert!(matches!(
            index
                .resolve_object(&source, Reference::new(1, 0), ResolveLimits::default())
                .expect_err("same mismatch")
                .kind(),
            ResolveErrorKind::ObjectHeaderMismatch { .. }
        ));
    }

    #[test]
    fn resolves_a_compressed_object_into_a_distinct_byte_source() {
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let object_stream_offset = bytes.len();
        let object_stream_data = b"3 0 << /Answer 42 >>";
        bytes.extend_from_slice(
            format!(
                "4 0 obj\n<< /Type /ObjStm /N 1 /First 4 /Length {} >>\nstream\n",
                object_stream_data.len()
            )
            .as_bytes(),
        );
        bytes.extend_from_slice(object_stream_data);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");

        let xref_offset = bytes.len();
        let mut xref_data = Vec::new();
        xref_data.extend_from_slice(&encode_stream_entry(0, 0, u16::MAX));
        xref_data.extend_from_slice(&encode_stream_entry(0, 0, 0));
        xref_data.extend_from_slice(&encode_stream_entry(0, 0, 0));
        xref_data.extend_from_slice(&encode_stream_entry(2, 4, 0));
        xref_data.extend_from_slice(&encode_stream_entry(
            1,
            u32::try_from(object_stream_offset).expect("fixture offset"),
            0,
        ));
        xref_data.extend_from_slice(&encode_stream_entry(
            1,
            u32::try_from(xref_offset).expect("fixture offset"),
            0,
        ));
        bytes.extend_from_slice(
            format!(
                "5 0 obj\n<< /Type /XRef /Size 6 /W [1 4 2] /Length {} >>\nstream\n",
                xref_data.len()
            )
            .as_bytes(),
        );
        bytes.extend_from_slice(&xref_data);
        bytes.extend_from_slice(b"\nendstream\nendobj\nstartxref\n");
        bytes.extend_from_slice(xref_offset.to_string().as_bytes());
        bytes.extend_from_slice(b"\n%%EOF\n");

        let source = store(bytes);
        let chain = parse_revision_chain_strict(&source, XrefLimits::default())
            .expect("xref stream with compressed object");
        let index = RevisionIndex::from_chain(&chain).expect("unambiguous active xref");
        let object = index
            .resolve_object(&source, Reference::new(3, 0), ResolveLimits::default())
            .expect("compressed object");

        assert!(object.is_compressed());
        assert_ne!(object.source().id(), source.id());
        assert_ne!(object.source().id().derivation(), 0);
        assert_eq!(
            object.source().resolve(object.value().span()),
            Ok(&b"<< /Answer 42 >>"[..])
        );
        assert!(matches!(object.value().kind(), ObjectKind::Dictionary(_)));
    }

    #[test]
    fn hybrid_revision_searches_supplemental_before_previous_revision() {
        let source = store(hybrid_pdf(false));
        let chain = parse_revision_chain_strict(&source, XrefLimits::default())
            .expect("valid hybrid revision");
        assert_eq!(chain.revisions().len(), 2);
        let XrefSection::Hybrid(hybrid) = &chain.revisions()[0] else {
            panic!("newest revision must preserve both hybrid sources");
        };
        assert!(hybrid.classic().entries().is_empty());
        assert_eq!(hybrid.supplemental().entries().len(), 3);
        assert!(matches!(chain.revisions()[1], XrefSection::Classic(_)));

        let index = RevisionIndex::from_chain(&chain).expect("unambiguous hybrid entries");
        let object = index
            .resolve_object(&source, Reference::new(3, 0), ResolveLimits::default())
            .expect("supplemental compressed object masks older free entry");
        assert!(object.is_compressed());
        assert_eq!(
            object.source().resolve(object.value().span()),
            Ok(&b"<< /Hybrid true >>"[..])
        );
    }

    #[test]
    fn hybrid_conflict_is_not_resolved_by_silent_precedence() {
        let error = parse_revision_chain_strict(&store(hybrid_pdf(true)), XrefLimits::default())
            .expect_err("same-revision sources disagree about object 3");
        assert_eq!(
            error.kind(),
            XrefErrorKind::HybridEntryConflict { object_number: 3 }
        );
    }
}
