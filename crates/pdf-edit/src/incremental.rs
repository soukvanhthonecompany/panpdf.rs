use std::fmt;
use std::sync::Arc;

use pdf_bytes::ByteStore;
use pdf_security::{SecurityErrorKind, authenticate_standard_password};
use pdf_syntax::{
    ObjectKind, Reference, ResolveError, ResolveLimits, RevisionIndex, XrefEntryKind, XrefError,
    XrefLimits, parse_revision_chain_strict,
};

#[derive(Clone, Copy, Debug)]
pub struct StreamReplacement<'bytes> {
    pub reference: Reference,
    pub decoded: &'bytes [u8],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtectionPolicy<'credential> {
    RefuseProtected,
    Preserve {
        credential: &'credential [u8],
        restrictions: Restrictions,
    },
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Restrictions {
    #[default]
    Respect,
    SetAside,
}

const MAX_CLASSIC_XREF_OFFSET: u64 = 9_999_999_999;

pub fn append_stream_replacement(
    source: &ByteStore,
    replacement: StreamReplacement<'_>,
    policy: ProtectionPolicy<'_>,
) -> Result<Arc<[u8]>, IncrementalWriteError> {
    append_object_writes(
        source,
        &[ObjectWrite {
            reference: replacement.reference,
            body: ObjectBody::ReplacedStream {
                decoded: replacement.decoded,
            },
        }],
        policy,
    )
}

#[derive(Clone, Copy, Debug)]
pub struct ObjectWrite<'bytes> {
    pub reference: Reference,
    pub body: ObjectBody<'bytes>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TrailerExtras {
    pub info: Option<Reference>,
}

#[derive(Clone, Copy, Debug)]
pub enum ObjectBody<'bytes> {
    ReplacedStream {
        decoded: &'bytes [u8],
    },
    NewStream {
        dictionary: &'bytes [u8],
        decoded: &'bytes [u8],
    },
    Direct {
        body: &'bytes [u8],
    },
}

pub fn append_object_writes(
    source: &ByteStore,
    writes: &[ObjectWrite<'_>],
    policy: ProtectionPolicy<'_>,
) -> Result<Arc<[u8]>, IncrementalWriteError> {
    append_object_writes_bounded(
        source,
        writes,
        policy,
        TrailerExtras::default(),
        XrefLimits::default(),
    )
    .map(Arc::<[u8]>::from)
}

pub(crate) fn append_object_writes_bounded(
    source: &ByteStore,
    writes: &[ObjectWrite<'_>],
    policy: ProtectionPolicy<'_>,
    extras: TrailerExtras,
    limits: XrefLimits,
) -> Result<Vec<u8>, IncrementalWriteError> {
    if writes.is_empty() {
        return Err(IncrementalWriteError::NothingToWrite);
    }
    let chain =
        parse_revision_chain_strict(source, limits).map_err(IncrementalWriteError::Revisions)?;
    if chain.revisions().len() >= limits.max_revisions {
        return Err(IncrementalWriteError::RevisionCapacity);
    }
    let index = RevisionIndex::from_chain(&chain).map_err(IncrementalWriteError::Index)?;

    let root = effective_trailer_value(source, &chain, b"/Root")
        .ok_or(IncrementalWriteError::MissingRoot)?;
    let size = effective_trailer_value(source, &chain, b"/Size")
        .ok_or(IncrementalWriteError::MissingSize)?;
    let info = extras.info.map_or_else(
        || effective_trailer_value(source, &chain, b"/Info"),
        |reference| {
            Some(format!("{} {} R", reference.object_number(), reference.generation()).into_bytes())
        },
    );
    let id = effective_trailer_value(source, &chain, b"/ID");
    let encrypt = effective_trailer_value(source, &chain, b"/Encrypt");

    let security = write_security(source, &chain, &index, encrypt.is_some(), policy)?;

    let mut out = Vec::with_capacity(source.len() + source.len() / 16 + 4096);
    out.extend_from_slice(source.as_bytes());
    if !out.ends_with(b"\n") {
        out.push(b'\n');
    }
    let mut placed: Vec<(Reference, usize)> = Vec::with_capacity(writes.len());
    let mut highest = 0_u32;
    for write in writes {
        let offset = out.len();
        if offset as u64 > MAX_CLASSIC_XREF_OFFSET {
            return Err(IncrementalWriteError::ClassicXrefOffsetTooLarge);
        }
        highest = highest.max(write.reference.object_number());
        out.extend_from_slice(
            format!(
                "{} {} obj\n",
                write.reference.object_number(),
                write.reference.generation()
            )
            .as_bytes(),
        );
        write_object_body(&mut out, source, &index, security.as_ref(), *write)?;
        out.extend_from_slice(b"\nendobj\n");
        placed.push((write.reference, offset));
    }

    let size = match std::str::from_utf8(&size)
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
    {
        Some(declared) if declared <= highest => (highest + 1).to_string().into_bytes(),
        _ => size,
    };

    let xref_offset = out.len();
    out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n");
    for (reference, offset) in &placed {
        out.extend_from_slice(format!("{} 1\n", reference.object_number()).as_bytes());
        out.extend_from_slice(
            format!("{offset:010} {:05} n \n", reference.generation()).as_bytes(),
        );
    }
    out.extend_from_slice(b"trailer\n<< /Size ");
    out.extend_from_slice(&size);
    out.extend_from_slice(b" /Root ");
    out.extend_from_slice(&root);
    if let Some(info) = info {
        out.extend_from_slice(b" /Info ");
        out.extend_from_slice(&info);
    }
    if let Some(id) = id {
        out.extend_from_slice(b" /ID ");
        out.extend_from_slice(&id);
    }
    if let Some(encrypt) = encrypt {
        out.extend_from_slice(b" /Encrypt ");
        out.extend_from_slice(&encrypt);
    }
    out.extend_from_slice(
        format!(
            " /Prev {} >>\nstartxref\n{xref_offset}\n%%EOF\n",
            chain.startxref()
        )
        .as_bytes(),
    );
    Ok(out)
}

fn write_security(
    source: &ByteStore,
    chain: &pdf_syntax::RevisionChain,
    index: &RevisionIndex,
    encrypted: bool,
    policy: ProtectionPolicy<'_>,
) -> Result<Option<pdf_security::AuthenticatedSecurity>, IncrementalWriteError> {
    let security = match (encrypted, policy) {
        (false, _) => None,
        (true, ProtectionPolicy::RefuseProtected) => {
            return Err(IncrementalWriteError::ProtectedDocument);
        }
        (true, ProtectionPolicy::Preserve { credential, .. }) => Some(
            authenticate_standard_password(
                source,
                chain,
                index,
                credential,
                ResolveLimits::default(),
            )
            .map_err(|error| IncrementalWriteError::Authenticate(error.kind()))?,
        ),
    };
    let respected = matches!(
        policy,
        ProtectionPolicy::Preserve {
            restrictions: Restrictions::Respect,
            ..
        }
    );
    if respected
        && security
            .as_ref()
            .is_some_and(|security| !security.may_modify_content())
    {
        return Err(IncrementalWriteError::PermissionDenied);
    }

    Ok(security)
}

pub(crate) fn session_write_limits() -> XrefLimits {
    let mut limits = XrefLimits::default();
    limits.max_revisions += 1;
    limits
}

pub(crate) fn compact_session(
    original: &ByteStore,
    current: &ByteStore,
) -> Result<ByteStore, IncrementalWriteError> {
    if !current.as_bytes().starts_with(original.as_bytes()) {
        return Err(IncrementalWriteError::InvalidSourceSpan);
    }
    let chain = parse_revision_chain_strict(current, session_write_limits())
        .map_err(IncrementalWriteError::Revisions)?;
    let original_chain = parse_revision_chain_strict(original, XrefLimits::default())
        .map_err(IncrementalWriteError::Revisions)?;
    let index = RevisionIndex::from_chain(&chain).map_err(IncrementalWriteError::Index)?;
    let mut changed: Vec<_> = index
        .selected_entries()
        .filter_map(|selected| {
            let entry = selected.entry();
            match entry.kind() {
                XrefEntryKind::InUse { byte_offset } if byte_offset >= original.len() as u64 => {
                    Some(Reference::new(entry.object_number(), entry.generation()))
                }
                _ => None,
            }
        })
        .collect();
    changed.sort_by_key(|reference| (reference.object_number(), reference.generation()));
    let mut out = Vec::with_capacity(current.len() + 4096);
    out.extend_from_slice(original.as_bytes());
    out.push(b'\n');
    let mut placed = Vec::new();
    for reference in changed {
        let resolved = index
            .resolve_object(current, reference, ResolveLimits::default())
            .map_err(IncrementalWriteError::Target)?;
        let object = resolved
            .indirect_object()
            .ok_or(IncrementalWriteError::InvalidSourceSpan)?;
        let raw = current
            .resolve(object.span())
            .map_err(|_| IncrementalWriteError::InvalidSourceSpan)?;
        placed.push((reference, out.len()));
        out.extend_from_slice(raw);
        out.push(b'\n');
    }
    let xref = out.len();
    out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n");
    for (reference, offset) in placed {
        if offset as u64 > MAX_CLASSIC_XREF_OFFSET {
            return Err(IncrementalWriteError::ClassicXrefOffsetTooLarge);
        }
        out.extend_from_slice(
            format!(
                "{} 1\n{offset:010} {:05} n \n",
                reference.object_number(),
                reference.generation()
            )
            .as_bytes(),
        );
    }
    out.extend_from_slice(b"trailer\n<<");
    for key in [b"/Size".as_slice(), b"/Root", b"/Info", b"/ID", b"/Encrypt"] {
        if let Some(value) = effective_trailer_value(current, &chain, key) {
            out.push(b' ');
            out.extend_from_slice(key);
            out.push(b' ');
            out.extend_from_slice(&value);
        }
    }
    out.extend_from_slice(
        format!(
            " /Prev {} >>\nstartxref\n{xref}\n%%EOF\n",
            original_chain.startxref()
        )
        .as_bytes(),
    );
    let compact = ByteStore::owning(current.id(), out);
    parse_revision_chain_strict(&compact, XrefLimits::default())
        .map_err(IncrementalWriteError::Revisions)?;
    Ok(compact)
}

fn write_object_body(
    out: &mut Vec<u8>,
    source: &ByteStore,
    index: &RevisionIndex,
    security: Option<&pdf_security::AuthenticatedSecurity>,
    write: ObjectWrite<'_>,
) -> Result<(), IncrementalWriteError> {
    let (ObjectBody::ReplacedStream { decoded } | ObjectBody::NewStream { decoded, .. }) =
        write.body
    else {
        let ObjectBody::Direct { body } = write.body else {
            unreachable!("the stream arms are matched above")
        };
        match security {
            Some(security) if strings_are_plain(index, write.reference) => {
                out.extend_from_slice(&with_strings_encrypted(security, write.reference, body)?);
            }
            _ => out.extend_from_slice(body),
        }
        return Ok(());
    };
    let packed = match write.body {
        ObjectBody::ReplacedStream { .. } => {
            Some(pdf_syntax::deflate_zlib(decoded)).filter(|packed| packed.len() < decoded.len())
        }
        _ => None,
    };
    let payload = packed.as_deref().unwrap_or(decoded);
    let stored = match security {
        None => payload.to_vec(),
        Some(security) => security
            .encrypt_stream(write.reference, payload)
            .map_err(|error| IncrementalWriteError::Encrypt(error.kind()))?,
    };
    let dictionary = match write.body {
        ObjectBody::NewStream { dictionary, .. } => {
            let mut written = format!("<< /Length {}", stored.len()).into_bytes();
            written.push(b' ');
            written.extend_from_slice(dictionary);
            written.extend_from_slice(b" >>");
            written
        }
        ObjectBody::ReplacedStream { .. } | ObjectBody::Direct { .. } => {
            let resolved = index
                .resolve_object(source, write.reference, ResolveLimits::default())
                .map_err(IncrementalWriteError::Target)?;
            if resolved.stream().is_none() {
                return Err(IncrementalWriteError::TargetNotStream);
            }
            replacement_stream_dictionary(
                source,
                resolved.value(),
                (stored.len(), decoded.len()),
                packed.is_some(),
            )?
        }
    };
    out.extend_from_slice(&dictionary);
    out.extend_from_slice(b"\nstream\n");
    out.extend_from_slice(&stored);
    out.extend_from_slice(b"\nendstream");
    Ok(())
}

pub(crate) fn strings_are_plain(index: &RevisionIndex, reference: Reference) -> bool {
    index
        .selected_for_number(reference.object_number())
        .is_none_or(|selected| {
            matches!(
                selected.entry().kind(),
                XrefEntryKind::Free { .. } | XrefEntryKind::Compressed { .. }
            )
        })
}

pub(crate) fn with_strings_encrypted(
    security: &pdf_security::AuthenticatedSecurity,
    reference: Reference,
    body: &[u8],
) -> Result<Vec<u8>, IncrementalWriteError> {
    let unreadable = || IncrementalWriteError::UnreadableObjectBody;
    let store = ByteStore::new(pdf_bytes::SourceId::new(0), Arc::<[u8]>::from(body));
    let mut lexer = pdf_syntax::Lexer::new(&store, 0, pdf_syntax::LexLimits::default());
    let mut out = Vec::with_capacity(body.len());
    let mut copied = 0;
    while let Some(token) = lexer.next_token().map_err(|_| unreadable())? {
        if !matches!(
            token.kind(),
            pdf_syntax::TokenKind::LiteralString | pdf_syntax::TokenKind::HexString
        ) {
            continue;
        }
        let span = token.span();
        let object =
            pdf_syntax::ObjectParser::new(&store, span.start(), pdf_syntax::ParseLimits::default())
                .parse_next()
                .map_err(|_| unreadable())?
                .ok_or_else(unreadable)?;
        let plain =
            pdf_syntax::decode_string(&store, &object, body.len()).map_err(|_| unreadable())?;
        let encrypted = security
            .encrypt_string(reference, &plain)
            .map_err(|error| IncrementalWriteError::Encrypt(error.kind()))?;
        out.extend_from_slice(&body[copied..span.start()]);
        out.push(b'<');
        for byte in encrypted {
            out.extend_from_slice(format!("{byte:02x}").as_bytes());
        }
        out.push(b'>');
        copied = span.end();
    }
    out.extend_from_slice(&body[copied..]);
    Ok(out)
}

fn replacement_stream_dictionary(
    source: &ByteStore,
    value: &pdf_syntax::Object,
    (stored_length, decoded_length): (usize, usize),
    deflated: bool,
) -> Result<Vec<u8>, IncrementalWriteError> {
    let ObjectKind::Dictionary(entries) = value.kind() else {
        return Err(IncrementalWriteError::TargetNotStream);
    };
    if entries.iter().any(|entry| {
        entry.key_equals(source, b"/F")
            || entry.key_equals(source, b"/FFilter")
            || entry.key_equals(source, b"/FDecodeParms")
    }) {
        return Err(IncrementalWriteError::ExternalFileStream);
    }

    let mut dictionary = format!("<< /Length {stored_length}").into_bytes();
    if deflated {
        dictionary.extend_from_slice(b" /Filter /FlateDecode");
    }
    let mut had_decoded_length = false;
    for entry in entries {
        if entry.key_equals(source, b"/Length")
            || entry.key_equals(source, b"/Filter")
            || entry.key_equals(source, b"/DecodeParms")
        {
            continue;
        }
        if entry.key_equals(source, b"/DL") {
            had_decoded_length = true;
            continue;
        }
        let key = source
            .resolve(entry.key().span())
            .map_err(|_| IncrementalWriteError::InvalidSourceSpan)?;
        let value = source
            .resolve(entry.value().span())
            .map_err(|_| IncrementalWriteError::InvalidSourceSpan)?;
        dictionary.push(b' ');
        dictionary.extend_from_slice(key);
        dictionary.push(b' ');
        dictionary.extend_from_slice(value);
    }
    if had_decoded_length {
        dictionary.extend_from_slice(format!(" /DL {decoded_length}").as_bytes());
    }
    dictionary.extend_from_slice(b" >>");
    Ok(dictionary)
}

fn effective_trailer_value(
    source: &ByteStore,
    chain: &pdf_syntax::RevisionChain,
    key: &[u8],
) -> Option<Vec<u8>> {
    chain.revisions().iter().find_map(|revision| {
        let ObjectKind::Dictionary(entries) = revision.trailer().kind() else {
            return None;
        };
        let value = entries
            .iter()
            .find(|entry| entry.key_equals(source, key))?
            .value();
        source.resolve(value.span()).ok().map(<[u8]>::to_vec)
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IncrementalWriteError {
    RevisionCapacity,
    PermissionDenied,
    Revisions(XrefError),
    Index(ResolveError),
    Target(ResolveError),
    TargetNotStream,
    MissingRoot,
    MissingSize,
    ProtectedDocument,
    NothingToWrite,
    Authenticate(SecurityErrorKind),
    Encrypt(SecurityErrorKind),
    ExternalFileStream,
    InvalidSourceSpan,
    ClassicXrefOffsetTooLarge,
    UnreadableObjectBody,
}

impl fmt::Display for IncrementalWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RevisionCapacity => {
                formatter.write_str("another update would exceed the PDF revision limit")
            }
            Self::PermissionDenied => {
                formatter.write_str("this password does not permit changing page content")
            }
            Self::Revisions(error) => write!(formatter, "revision chain: {error}"),
            Self::Index(error) => write!(formatter, "active object index: {error}"),
            Self::Target(error) => write!(formatter, "replacement target: {error}"),
            Self::TargetNotStream => formatter.write_str("replacement target is not a stream"),
            Self::MissingRoot => formatter.write_str("effective trailer has no /Root"),
            Self::MissingSize => formatter.write_str("effective trailer has no /Size"),
            Self::NothingToWrite => {
                formatter.write_str("a revision must contain at least one object")
            }
            Self::ProtectedDocument => formatter.write_str(
                "the document is encrypted, and writing into it needs an encryption policy",
            ),
            Self::Authenticate(error) => {
                write!(formatter, "preserving this document's protection: {error}")
            }
            Self::Encrypt(error) => {
                write!(formatter, "re-encrypting the written stream: {error}")
            }
            Self::ExternalFileStream => {
                formatter.write_str("replacement target stores its bytes in an external file")
            }
            Self::InvalidSourceSpan => {
                formatter.write_str("replacement stream dictionary has an invalid source span")
            }
            Self::ClassicXrefOffsetTooLarge => {
                formatter.write_str("incremental object offset does not fit a classic xref entry")
            }
            Self::UnreadableObjectBody => formatter
                .write_str("a new object's body cannot be read to encrypt the strings in it"),
        }
    }
}

impl std::error::Error for IncrementalWriteError {}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Arc;

    use pdf_bytes::{ByteStore, SourceId};
    use pdf_syntax::{
        ObjectKind, Reference, ResolveLimits, RevisionIndex, XrefLimits,
        parse_revision_chain_strict,
    };

    use super::ProtectionPolicy;

    use super::{
        IncrementalWriteError, ObjectBody, ObjectWrite, StreamReplacement, append_object_writes,
        append_stream_replacement, effective_trailer_value,
    };
    use pdf_security::authenticate_standard_password;

    fn store(bytes: Vec<u8>) -> ByteStore {
        ByteStore::new(SourceId::new(301), Arc::<[u8]>::from(bytes))
    }

    fn stream_pdf(generation: u16, protected: bool, external_file: bool) -> ByteStore {
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let catalog = bytes.len();
        bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog >>\nendobj\n");
        let stream = bytes.len();
        bytes.extend_from_slice(
            format!(
                "2 {generation} obj\n<< /Length 3 /DL 3 /Intent /View /Filter /FlateDecode /DecodeParms <<>>{} >>\nstream\nold\nendstream\nendobj\n",
                if external_file { " /F (payload.bin)" } else { "" }
            )
            .as_bytes(),
        );
        let info = bytes.len();
        bytes.extend_from_slice(b"3 0 obj\n<< /Producer (known) >>\nendobj\n");
        let encrypt = if protected {
            let offset = bytes.len();
            bytes.extend_from_slice(b"4 0 obj\n<< /Filter /Standard >>\nendobj\n");
            Some(offset)
        } else {
            None
        };
        let size = if protected { 5 } else { 4 };
        let xref = bytes.len();
        bytes.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
        bytes.extend_from_slice(format!("{catalog:010} 00000 n \n").as_bytes());
        bytes.extend_from_slice(format!("{stream:010} {generation:05} n \n").as_bytes());
        bytes.extend_from_slice(format!("{info:010} 00000 n \n").as_bytes());
        if let Some(encrypt) = encrypt {
            bytes.extend_from_slice(format!("{encrypt:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!(
                "trailer\n<< /Size {size} /Root 1 0 R /Info 3 0 R /ID [<0102> <0304>]{} >>\nstartxref\n{xref}\n%%EOF\n",
                if protected { " /Encrypt 4 0 R" } else { "" }
            )
            .as_bytes(),
        );
        store(bytes)
    }

    fn with_minimal_update(source: &ByteStore, size: usize) -> ByteStore {
        let chain = parse_revision_chain_strict(source, XrefLimits::default())
            .expect("base revision chain");
        let mut bytes = source.as_bytes().to_vec();
        let xref = bytes.len();
        bytes.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n");
        bytes.extend_from_slice(
            format!(
                "trailer\n<< /Size {size} /Prev {} >>\nstartxref\n{xref}\n%%EOF\n",
                chain.startxref()
            )
            .as_bytes(),
        );
        store(bytes)
    }

    #[test]
    fn replacement_preserves_prefix_generation_and_effective_trailer_state() {
        let source = with_minimal_update(&stream_pdf(7, false, false), 4);
        let bytes = append_stream_replacement(
            &source,
            StreamReplacement {
                reference: Reference::new(2, 7),
                decoded: b"new stream bytes",
            },
            ProtectionPolicy::RefuseProtected,
        )
        .expect("valid replacement");
        assert_eq!(&bytes[..source.len()], source.as_bytes());

        let edited = ByteStore::new(SourceId::new(302), bytes);
        let chain = parse_revision_chain_strict(&edited, XrefLimits::default())
            .expect("strict incremental chain");
        assert_eq!(chain.revisions().len(), 3);
        let index = RevisionIndex::from_chain(&chain).expect("active index");
        let resolved = index
            .resolve_object(&edited, Reference::new(2, 7), ResolveLimits::default())
            .expect("replacement generation remains active");
        let stream = resolved.stream().expect("replacement stays a stream");
        assert_eq!(
            edited.resolve(stream.data_span()).expect("stream bytes"),
            b"new stream bytes"
        );
        let ObjectKind::Dictionary(entries) = resolved.value().kind() else {
            panic!("stream value is a dictionary");
        };
        let value = |key: &[u8]| {
            let value = entries
                .iter()
                .find(|entry| entry.key_equals(&edited, key))?
                .value();
            edited.resolve(value.span()).ok().map(<[u8]>::to_vec)
        };
        assert_eq!(value(b"/Length"), Some(b"16".to_vec()));
        assert_eq!(value(b"/DL"), Some(b"16".to_vec()));
        assert_eq!(value(b"/Intent"), Some(b"/View".to_vec()));
        assert_eq!(value(b"/Filter"), None);
        assert_eq!(value(b"/DecodeParms"), None);
        assert_eq!(
            effective_trailer_value(&edited, &chain, b"/Info"),
            Some(b"3 0 R".to_vec())
        );
        assert_eq!(
            effective_trailer_value(&edited, &chain, b"/ID"),
            Some(b"[<0102> <0304>]".to_vec())
        );
    }

    #[test]
    fn a_replaced_stream_is_stored_deflated_when_that_is_smaller() {
        let source = stream_pdf(0, false, false);
        let page: Vec<u8> = b"BT /F1 12 Tf 72 700 Td (a line of a page) Tj ET\n".repeat(200);
        let bytes = append_stream_replacement(
            &source,
            StreamReplacement {
                reference: Reference::new(2, 0),
                decoded: &page,
            },
            ProtectionPolicy::RefuseProtected,
        )
        .expect("valid replacement");
        let edited = ByteStore::new(SourceId::new(303), bytes);
        let chain = parse_revision_chain_strict(&edited, XrefLimits::default()).unwrap();
        let index = RevisionIndex::from_chain(&chain).unwrap();
        let resolved = index
            .resolve_object(&edited, Reference::new(2, 0), ResolveLimits::default())
            .unwrap();
        let stored = edited
            .resolve(resolved.stream().unwrap().data_span())
            .unwrap()
            .to_vec();
        assert!(
            stored.len() * 10 < page.len(),
            "{} of {}",
            stored.len(),
            page.len()
        );
        assert_eq!(
            pdf_syntax::inflate_zlib(&stored, page.len()).expect("it inflates"),
            page
        );
        let ObjectKind::Dictionary(entries) = resolved.value().kind() else {
            panic!("stream value is a dictionary");
        };
        let value = |key: &[u8]| {
            let value = entries
                .iter()
                .find(|entry| entry.key_equals(&edited, key))?
                .value();
            edited.resolve(value.span()).ok().map(<[u8]>::to_vec)
        };
        assert_eq!(value(b"/Filter"), Some(b"/FlateDecode".to_vec()));
        assert_eq!(
            value(b"/Length"),
            Some(stored.len().to_string().into_bytes())
        );
        assert_eq!(value(b"/DL"), Some(page.len().to_string().into_bytes()));
        assert_eq!(value(b"/DecodeParms"), None, "no predictor is written");
    }

    #[test]
    fn the_writer_refuses_before_it_creates_an_unreadable_revision() {
        let mut source = stream_pdf(0, false, false);
        for _ in 1..XrefLimits::default().max_revisions {
            source = with_minimal_update(&source, 4);
        }
        let before = source.as_bytes().to_vec();
        assert_eq!(
            parse_revision_chain_strict(&source, XrefLimits::default())
                .unwrap()
                .revisions()
                .len(),
            1_024
        );
        assert!(matches!(
            append_stream_replacement(
                &source,
                StreamReplacement {
                    reference: Reference::new(2, 0),
                    decoded: b"new"
                },
                ProtectionPolicy::RefuseProtected
            ),
            Err(IncrementalWriteError::RevisionCapacity)
        ));
        assert_eq!(source.as_bytes(), before);
    }

    #[test]
    fn wrong_generation_and_non_stream_targets_fail_before_writing() {
        let source = stream_pdf(7, false, false);
        assert!(matches!(
            append_stream_replacement(
                &source,
                StreamReplacement {
                    reference: Reference::new(2, 0),
                    decoded: b"new"
                },
                ProtectionPolicy::RefuseProtected,
            ),
            Err(IncrementalWriteError::Target(_))
        ));
        assert_eq!(
            append_stream_replacement(
                &source,
                StreamReplacement {
                    reference: Reference::new(1, 0),
                    decoded: b"new"
                },
                ProtectionPolicy::RefuseProtected,
            ),
            Err(IncrementalWriteError::TargetNotStream)
        );
    }

    #[test]
    fn external_file_streams_fail_closed() {
        let source = stream_pdf(0, false, true);
        assert_eq!(
            append_stream_replacement(
                &source,
                StreamReplacement {
                    reference: Reference::new(2, 0),
                    decoded: b"embedded"
                },
                ProtectionPolicy::RefuseProtected,
            ),
            Err(IncrementalWriteError::ExternalFileStream)
        );
    }

    pub(crate) fn protected_pdf() -> ByteStore {
        const OWNER: &str = "2055c756c72e1ad702608e8196acad447ad32d17cff583235f6dd15fed7dab67";
        const USER: &str = "c8230d20f998233377bea57fc7895a7fd5eb1ce3557417a098cee032b973feab";
        const CIPHERTEXT: [u8; 3] = [0x3f, 0xc9, 0x71];

        let mut bytes = b"%PDF-1.7\n".to_vec();
        let catalog = bytes.len();
        bytes.extend_from_slice(b"1 0 obj\n<< /Type /Catalog >>\nendobj\n");
        let stream = bytes.len();
        bytes.extend_from_slice(b"2 0 obj\n<< /Length 3 /Intent /View >>\nstream\n");
        bytes.extend_from_slice(&CIPHERTEXT);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        let encrypt = bytes.len();
        bytes.extend_from_slice(
            format!(
                "3 0 obj\n<< /Filter /Standard /V 1 /R 2 /Length 40 /P -4 /O <{OWNER}> /U <{USER}> >>\nendobj\n"
            )
            .as_bytes(),
        );
        let xref = bytes.len();
        bytes.extend_from_slice(b"xref\n0 4\n0000000000 65535 f \n");
        bytes.extend_from_slice(format!("{catalog:010} 00000 n \n").as_bytes());
        bytes.extend_from_slice(format!("{stream:010} 00000 n \n").as_bytes());
        bytes.extend_from_slice(format!("{encrypt:010} 00000 n \n").as_bytes());
        bytes.extend_from_slice(
            format!(
                "trailer\n<< /Size 4 /Root 1 0 R /Encrypt 3 0 R /ID [<0102030405060708090a0b0c0d0e0f10> <0304>] >>\nstartxref\n{xref}\n%%EOF\n"
            )
            .as_bytes(),
        );
        store(bytes)
    }

    #[test]
    fn preserving_protection_writes_ciphertext_and_keeps_the_document_openable() {
        const EXPECTED: [u8; 16] = [
            0x3e, 0xc0, 0x62, 0x11, 0xe8, 0x04, 0x7e, 0xbd, 0x15, 0x9d, 0x69, 0x96, 0x25, 0x2c,
            0x52, 0xbd,
        ];

        let source = protected_pdf();
        let bytes = append_stream_replacement(
            &source,
            StreamReplacement {
                reference: Reference::new(2, 0),
                decoded: b"new stream bytes",
            },
            ProtectionPolicy::Preserve {
                credential: b"",
                restrictions: crate::incremental::Restrictions::Respect,
            },
        )
        .expect("the empty user password opens this document");
        assert_eq!(&bytes[..source.len()], source.as_bytes());
        let edited = ByteStore::new(SourceId::new(311), bytes);

        let written = &edited.as_bytes()[source.len()..];
        assert!(
            written.windows(EXPECTED.len()).any(|w| w == EXPECTED),
            "the new revision must carry the independently computed ciphertext"
        );
        assert!(
            !written.windows(16).any(|w| w == b"new stream bytes"),
            "the plaintext must not appear in a protected document"
        );

        let chain = parse_revision_chain_strict(&edited, XrefLimits::default())
            .expect("strict incremental chain");
        assert_eq!(
            effective_trailer_value(&edited, &chain, b"/Encrypt"),
            Some(b"3 0 R".to_vec())
        );
        assert_eq!(
            effective_trailer_value(&edited, &chain, b"/ID"),
            Some(b"[<0102030405060708090a0b0c0d0e0f10> <0304>]".to_vec())
        );
        let index = RevisionIndex::from_chain(&chain).expect("active index");
        let resolved = index
            .resolve_object(&edited, Reference::new(2, 0), ResolveLimits::default())
            .expect("the replacement is the active object");
        let ObjectKind::Dictionary(entries) = resolved.value().kind() else {
            panic!("stream value is a dictionary");
        };
        let length = entries
            .iter()
            .find(|entry| entry.key_equals(&edited, b"/Length"))
            .map(|entry| edited.resolve(entry.value().span()).expect("length span"));
        assert_eq!(length, Some(&b"16"[..]));
    }

    #[test]
    fn a_new_objects_strings_are_encrypted_and_an_existing_objects_are_left_as_written() {
        let source = protected_pdf();
        let catalog = b"<< /Type /Catalog /Lang (en) >>";
        let bytes = append_object_writes(
            &source,
            &[
                ObjectWrite {
                    reference: Reference::new(1, 0),
                    body: ObjectBody::Direct { body: catalog },
                },
                ObjectWrite {
                    reference: Reference::new(4, 0),
                    body: ObjectBody::Direct {
                        body: b"<< /Registry (Adobe) /Ordering <4964656e74697479> >>",
                    },
                },
            ],
            ProtectionPolicy::Preserve {
                credential: b"",
                restrictions: crate::incremental::Restrictions::Respect,
            },
        )
        .expect("the empty user password opens this document");
        let edited = ByteStore::new(SourceId::new(312), bytes);
        let written = &edited.as_bytes()[source.len()..];
        assert!(
            written.windows(catalog.len()).any(|w| w == catalog),
            "an existing object's strings are already encrypted in the file"
        );
        assert!(
            !written.windows(7).any(|w| w == b"(Adobe)"),
            "a new object's plaintext string must not appear"
        );

        let chain = parse_revision_chain_strict(&edited, XrefLimits::default())
            .expect("strict incremental chain");
        let index = RevisionIndex::from_chain(&chain).expect("active index");
        let security =
            authenticate_standard_password(&edited, &chain, &index, b"", ResolveLimits::default())
                .expect("still opens");
        let resolved = index
            .resolve_object(&edited, Reference::new(4, 0), ResolveLimits::default())
            .expect("the new object");
        let ObjectKind::Dictionary(entries) = resolved.value().kind() else {
            panic!("a dictionary");
        };
        let read = |key: &[u8]| {
            let value = entries
                .iter()
                .find(|entry| entry.key_equals(&edited, key))
                .expect("the entry")
                .value();
            assert_eq!(value.kind(), &ObjectKind::HexString);
            let encrypted = pdf_syntax::decode_string(&edited, value, 64).expect("a string");
            security
                .decrypt_string(Reference::new(4, 0), &encrypted)
                .expect("decrypts")
        };
        assert_eq!(read(b"/Registry"), b"Adobe");
        assert_eq!(read(b"/Ordering"), b"Identity");
    }

    #[test]
    fn a_credential_that_does_not_open_the_document_is_refused() {
        assert!(matches!(
            append_stream_replacement(
                &protected_pdf(),
                StreamReplacement {
                    reference: Reference::new(2, 0),
                    decoded: b"new stream bytes",
                },
                ProtectionPolicy::Preserve {
                    credential: b"not the password",
                    restrictions: super::Restrictions::Respect,
                },
            ),
            Err(IncrementalWriteError::Authenticate(_))
        ));
    }

    #[test]
    fn protected_documents_fail_before_any_plaintext_revision_is_created() {
        let source = stream_pdf(0, true, false);
        assert_eq!(
            append_stream_replacement(
                &source,
                StreamReplacement {
                    reference: Reference::new(2, 0),
                    decoded: b"plaintext"
                },
                ProtectionPolicy::RefuseProtected,
            ),
            Err(IncrementalWriteError::ProtectedDocument)
        );
    }
}
