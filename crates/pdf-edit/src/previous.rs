use pdf_bytes::ByteStore;
use pdf_security::authenticate_standard_password;
use pdf_syntax::{
    ObjectKind, Reference, ResolveLimits, RevisionIndex, XrefLimits, decode_stream_bytes,
    parse_revision_chain_strict,
};

use crate::spike_move_text::SpikeError;

pub(crate) fn defined(source: &ByteStore, reference: Reference) -> Result<bool, SpikeError> {
    let chain = parse_revision_chain_strict(source, XrefLimits::default())
        .map_err(|_| SpikeError::PreviousObjectUnreadable)?;
    Ok(chain.revisions().iter().any(|revision| {
        revision.entries().iter().any(|entry| {
            entry.object_number() == reference.object_number()
                && !matches!(entry.kind(), pdf_syntax::XrefEntryKind::Free { .. })
        })
    }))
}

pub(crate) fn direct_body(
    source: &ByteStore,
    reference: Reference,
    credential: &[u8],
) -> Result<Vec<u8>, SpikeError> {
    let (resolved, _) = resolve(source, reference, credential)?;
    let span = resolved.value().span();
    Ok(resolved
        .source()
        .resolve(span)
        .map_err(|_| SpikeError::PreviousObjectUnreadable)?
        .to_vec())
}

pub(crate) fn decoded_stream(
    source: &ByteStore,
    reference: Reference,
    credential: &[u8],
) -> Result<Vec<u8>, SpikeError> {
    let (resolved, protected) = resolve(source, reference, credential)?;
    let stream = resolved
        .stream()
        .ok_or(SpikeError::PreviousObjectUnreadable)?;
    let ObjectKind::Dictionary(entries) = resolved.value().kind() else {
        return Err(SpikeError::PreviousObjectUnreadable);
    };
    let encoded = resolved
        .source()
        .resolve(stream.data_span())
        .map_err(|_| SpikeError::PreviousObjectUnreadable)?;
    let plaintext = if protected {
        let chain = parse_revision_chain_strict(source, XrefLimits::default())
            .map_err(|_| SpikeError::PreviousObjectUnreadable)?;
        let index =
            RevisionIndex::from_chain(&chain).map_err(|_| SpikeError::PreviousObjectUnreadable)?;
        let security = authenticate_standard_password(
            source,
            &chain,
            &index,
            credential,
            ResolveLimits::default(),
        )
        .map_err(|_| SpikeError::PreviousObjectUnreadable)?;
        security
            .decrypt_stream(reference, encoded)
            .map_err(|_| SpikeError::PreviousObjectUnreadable)?
    } else {
        encoded.to_vec()
    };
    decode_stream_bytes(
        resolved.source(),
        entries,
        &plaintext,
        stream.data_span().start(),
        MAX_PREVIOUS_STREAM_BYTES,
    )
    .map_err(|_| SpikeError::PreviousObjectUnreadable)
}

const MAX_PREVIOUS_STREAM_BYTES: usize = 256 * 1024 * 1024;

fn resolve(
    source: &ByteStore,
    reference: Reference,
    credential: &[u8],
) -> Result<(pdf_syntax::ResolvedObject, bool), SpikeError> {
    let (index, security) =
        readable_index(source, credential).ok_or(SpikeError::PreviousObjectUnreadable)?;
    let resolved = index
        .resolve_object(source, reference, ResolveLimits::default())
        .map_err(|_| SpikeError::PreviousObjectUnreadable)?;
    Ok((resolved, security.is_some()))
}

pub(crate) fn protected(source: &ByteStore, chain: &pdf_syntax::RevisionChain) -> bool {
    chain.revisions().iter().any(|revision| {
        let ObjectKind::Dictionary(entries) = revision.trailer().kind() else {
            return false;
        };
        entries
            .iter()
            .any(|entry| entry.key_equals(source, b"/Encrypt"))
    })
}

pub(crate) fn readable_index(
    source: &ByteStore,
    credential: &[u8],
) -> Option<(
    RevisionIndex,
    Option<std::sync::Arc<pdf_security::AuthenticatedSecurity>>,
)> {
    let chain = parse_revision_chain_strict(source, XrefLimits::default()).ok()?;
    let index = RevisionIndex::from_chain(&chain).ok()?;
    if !protected(source, &chain) {
        return Some((index, None));
    }
    let security = std::sync::Arc::new(
        authenticate_standard_password(
            source,
            &chain,
            &index,
            credential,
            ResolveLimits::default(),
        )
        .ok()?,
    );
    Some((
        index.with_stream_decryptor(std::sync::Arc::new(ObjectStreams(std::sync::Arc::clone(
            &security,
        )))),
        Some(security),
    ))
}

#[derive(Debug)]
struct ObjectStreams(std::sync::Arc<pdf_security::AuthenticatedSecurity>);

impl pdf_syntax::StreamDecryptor for ObjectStreams {
    fn decrypt_stream(
        &self,
        reference: Reference,
        encrypted: &[u8],
    ) -> Result<Vec<u8>, pdf_syntax::DecryptionRefused> {
        self.0.decrypt_stream(reference, encrypted).map_err(|_| {
            pdf_syntax::DecryptionRefused::new("the security handler produced no plaintext")
        })
    }
}
