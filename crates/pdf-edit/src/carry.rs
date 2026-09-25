use std::collections::{HashMap, HashSet, VecDeque};

use std::sync::Arc;

use pdf_bytes::ByteStore;
use pdf_security::AuthenticatedSecurity;
use pdf_syntax::{Object, ObjectKind, Reference, ResolveLimits, RevisionIndex};

use crate::page_tree::refused;
use crate::plan::{PlannedBody, PlannedWrite};
use crate::spike_move_text::SpikeError;

pub(crate) const MOST_OBJECTS: usize = 200_000;

pub(crate) struct Copier<'a> {
    pub(crate) other: &'a ByteStore,
    pub(crate) index: &'a RevisionIndex,
    pub(crate) tree: HashSet<Reference>,
    pub(crate) numbers: HashMap<Reference, Reference>,
    pub(crate) queue: VecDeque<Reference>,
    pub(crate) next: u32,
    pub(crate) writes: Vec<PlannedWrite>,
    pub(crate) security: Option<Arc<AuthenticatedSecurity>>,
    pub(crate) into: Option<Arc<AuthenticatedSecurity>>,
}

pub(crate) type Within = Option<Reference>;

impl Copier<'_> {
    pub(crate) fn take_number(&mut self) -> Result<u32, SpikeError> {
        let number = self.next;
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| refused("this document has no object numbers left"))?;
        Ok(number)
    }

    pub(crate) fn number_of(&mut self, reference: Reference) -> Result<Reference, SpikeError> {
        if let Some(copy) = self.numbers.get(&reference) {
            return Ok(*copy);
        }
        if self.numbers.len() >= MOST_OBJECTS {
            return Err(refused("the pages reach more objects than can be copied"));
        }
        let copy = Reference::new(self.take_number()?, 0);
        self.numbers.insert(reference, copy);
        self.queue.push_back(reference);
        Ok(copy)
    }

    pub(crate) fn copy_reached(&mut self) -> Result<(), SpikeError> {
        while let Some(reference) = self.queue.pop_front() {
            let copy = self.numbers[&reference];
            let resolved = self
                .index
                .resolve_object(self.other, reference, ResolveLimits::default())
                .map_err(|_| refused("an object a copied page uses cannot be read"))?;
            let source = resolved.source().clone();
            let value = resolved.value().clone();
            let within = self.within(reference, resolved.is_compressed());
            if let Some(stream) = resolved.stream() {
                let ObjectKind::Dictionary(entries) = value.kind() else {
                    return Err(refused("a stream a copied page uses has no dictionary"));
                };
                let mut dictionary = Vec::new();
                for entry in entries {
                    if entry.key_equals(&source, b"/Length") {
                        continue;
                    }
                    dictionary.push(b' ');
                    dictionary.extend_from_slice(raw(&source, entry.key().span())?);
                    dictionary.push(b' ');
                    self.value((&source, within), entry.value(), &mut dictionary)?;
                }
                if let Some(into) = &self.into {
                    dictionary =
                        crate::incremental::with_strings_encrypted(into, copy, &dictionary)
                            .map_err(|_| {
                                refused("a stream a copied page uses cannot be written")
                            })?;
                }
                let data = raw(&source, stream.data_span())?;
                let data = match (&self.security, within) {
                    (Some(security), Some(_)) if encrypted_stream(&source, entries, security)? => {
                        security.decrypt_stream(reference, data).map_err(|_| {
                            refused("a stream a copied page uses cannot be deciphered")
                        })?
                    }
                    _ => data.to_vec(),
                };
                self.writes.push(PlannedWrite {
                    reference: copy,
                    body: PlannedBody::NewStream {
                        dictionary,
                        decoded: data,
                    },
                });
            } else {
                let mut body = Vec::new();
                self.value((&source, within), &value, &mut body)?;
                self.writes.push(PlannedWrite {
                    reference: copy,
                    body: PlannedBody::Direct { body },
                });
            }
        }
        Ok(())
    }

    pub(crate) fn within(&self, reference: Reference, compressed: bool) -> Within {
        (self.security.is_some() && !compressed).then_some(reference)
    }

    pub(crate) fn value(
        &mut self,
        (source, within): (&ByteStore, Within),
        value: &Object,
        out: &mut Vec<u8>,
    ) -> Result<(), SpikeError> {
        match value.kind() {
            ObjectKind::Reference(reference) => {
                if self.tree.contains(reference) {
                    out.extend_from_slice(b"null");
                } else {
                    let copy = self.number_of(*reference)?;
                    out.extend_from_slice(
                        format!("{} {} R", copy.object_number(), copy.generation()).as_bytes(),
                    );
                }
            }
            ObjectKind::Array(items) => {
                out.push(b'[');
                for (at, item) in items.iter().enumerate() {
                    if at > 0 {
                        out.push(b' ');
                    }
                    self.value((source, within), item, out)?;
                }
                out.push(b']');
            }
            ObjectKind::Dictionary(entries) => {
                out.extend_from_slice(b"<<");
                for entry in entries {
                    out.push(b' ');
                    out.extend_from_slice(raw(source, entry.key().span())?);
                    out.push(b' ');
                    self.value((source, within), entry.value(), out)?;
                }
                out.extend_from_slice(b" >>");
            }
            ObjectKind::LiteralString | ObjectKind::HexString => match (&self.security, within) {
                (Some(security), Some(object)) => {
                    let unreadable = || refused("a string a copied page uses cannot be read");
                    let encrypted = pdf_syntax::decode_string(source, value, MOST_STRING_BYTES)
                        .map_err(|_| unreadable())?;
                    let plain = security
                        .decrypt_string(object, &encrypted)
                        .map_err(|_| unreadable())?;
                    out.push(b'<');
                    for byte in plain {
                        out.extend_from_slice(format!("{byte:02x}").as_bytes());
                    }
                    out.push(b'>');
                }
                _ => out.extend_from_slice(raw(source, value.span())?),
            },
            _ => out.extend_from_slice(raw(source, value.span())?),
        }
        Ok(())
    }
}

const MOST_STRING_BYTES: usize = 16 * 1024 * 1024;

fn encrypted_stream(
    source: &ByteStore,
    entries: &[pdf_syntax::DictionaryEntry],
    security: &AuthenticatedSecurity,
) -> Result<bool, SpikeError> {
    let says = |key: &[u8], name: &[u8]| {
        entries.iter().any(|entry| {
            entry.key_equals(source, key)
                && match entry.value().kind() {
                    ObjectKind::Name => raw(source, entry.value().span()).ok() == Some(name),
                    ObjectKind::Array(items) => items
                        .iter()
                        .any(|item| raw(source, item.span()).ok() == Some(name)),
                    _ => false,
                }
        })
    };
    if says(b"/Filter", b"/Crypt") {
        return Err(refused(
            "a stream a copied page uses names its own crypt filter",
        ));
    }
    Ok(!says(b"/Type", b"/Metadata") || security.encrypt_metadata())
}

pub(crate) fn raw(source: &ByteStore, span: pdf_bytes::SourceSpan) -> Result<&[u8], SpikeError> {
    source
        .resolve(span)
        .map_err(|_| refused("an object a copied page uses cannot be read"))
}
