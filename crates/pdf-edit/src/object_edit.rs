use pdf_bytes::ByteStore;
use pdf_syntax::{Object, ObjectKind, Reference};

use crate::new_font::{Body, resolve};
use crate::plan::{PlannedBody, PlannedWrite};
use crate::spike_move_text::SpikeError;

fn malformed() -> SpikeError {
    SpikeError::RetypeUnsupported("this dictionary cannot be rewritten")
}

pub(crate) struct ObjectEdit {
    pub(crate) body: Body,
    edits: Vec<(usize, usize, String)>,
    cipher: Option<std::sync::Arc<pdf_security::AuthenticatedSecurity>>,
}

impl ObjectEdit {
    pub(crate) fn of(
        source: &ByteStore,
        reference: Reference,
        credential: &[u8],
    ) -> Result<Self, SpikeError> {
        let (index, security) = crate::previous::readable_index(source, credential)
            .ok_or(SpikeError::RetypeUnsupported("this object cannot be read"))?;
        let cipher = security.filter(|_| !crate::incremental::strings_are_plain(&index, reference));
        Ok(Self {
            body: resolve(source, reference, credential)?,
            edits: Vec::new(),
            cipher,
        })
    }

    pub(crate) fn of_planned(write: &PlannedWrite) -> Result<Self, SpikeError> {
        let PlannedBody::Direct { body } = &write.body else {
            return Err(malformed());
        };
        let source = ByteStore::new(
            pdf_bytes::SourceId::new(0),
            std::sync::Arc::<[u8]>::from(body.as_slice()),
        );
        let value = pdf_syntax::ObjectParser::new(&source, 0, pdf_syntax::ParseLimits::default())
            .parse_next()
            .map_err(|_| malformed())?
            .ok_or_else(malformed)?;
        Ok(Self {
            body: Body {
                source,
                value,
                reference: write.reference,
                bytes: body.clone(),
                offset: 0,
            },
            edits: Vec::new(),
            cipher: None,
        })
    }

    pub(crate) fn value(&self) -> Object {
        self.body.value.clone()
    }

    fn local(&self, absolute: usize) -> Result<usize, SpikeError> {
        let at = absolute
            .checked_sub(self.body.offset)
            .ok_or_else(malformed)?;
        if at > self.body.bytes.len() {
            return Err(malformed());
        }
        Ok(at)
    }

    fn closing(&self, object: &Object, closing: &[u8]) -> Result<usize, SpikeError> {
        let end = self.local(object.span().end())?;
        let at = end.checked_sub(closing.len()).ok_or_else(malformed)?;
        if self.body.bytes.get(at..end) != Some(closing) {
            return Err(malformed());
        }
        Ok(at)
    }

    pub(crate) fn set(
        &mut self,
        dictionary: &Object,
        key: &[u8],
        text: &str,
    ) -> Result<(), SpikeError> {
        let ObjectKind::Dictionary(entries) = dictionary.kind() else {
            return Err(malformed());
        };
        if let Some(entry) = entries
            .iter()
            .find(|entry| entry.key_equals(&self.body.source, key))
        {
            let span = entry.value().span();
            let (from, to) = (self.local(span.start())?, self.local(span.end())?);
            self.edits.push((from, to, text.to_owned()));
        } else {
            let close = self.closing(dictionary, b">>")?;
            let name = String::from_utf8_lossy(key);
            self.edits.push((close, close, format!(" {name} {text} ")));
        }
        Ok(())
    }

    pub(crate) fn unset(&mut self, dictionary: &Object, key: &[u8]) -> Result<(), SpikeError> {
        let ObjectKind::Dictionary(entries) = dictionary.kind() else {
            return Err(malformed());
        };
        if let Some(entry) = entries
            .iter()
            .find(|entry| entry.key_equals(&self.body.source, key))
        {
            let from = self.local(entry.key().span().start())?;
            let to = self.local(entry.value().span().end())?;
            self.edits.push((from, to, String::new()));
        }
        Ok(())
    }

    pub(crate) fn replace(&mut self, object: &Object, text: &str) -> Result<(), SpikeError> {
        let from = self.local(object.span().start())?;
        let to = self.local(object.span().end())?;
        self.edits.push((from, to, text.to_owned()));
        Ok(())
    }

    pub(crate) fn append(&mut self, array: &Object, text: &str) -> Result<(), SpikeError> {
        if !matches!(array.kind(), ObjectKind::Array(_)) {
            return Err(malformed());
        }
        let close = self.closing(array, b"]")?;
        self.edits.push((close, close, format!(" {text} ")));
        Ok(())
    }

    pub(crate) fn remove(
        &mut self,
        array: &Object,
        reference: Reference,
    ) -> Result<usize, SpikeError> {
        let ObjectKind::Array(items) = array.kind() else {
            return Err(malformed());
        };
        let mut removed = 0;
        for item in items {
            if matches!(item.kind(), ObjectKind::Reference(found) if *found == reference) {
                let span = item.span();
                let (from, to) = (self.local(span.start())?, self.local(span.end())?);
                self.edits.push((from, to, String::new()));
                removed += 1;
            }
        }
        Ok(removed)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    pub(crate) fn written(mut self) -> Result<PlannedWrite, SpikeError> {
        self.edits.sort_by_key(|(from, to, _)| (*from, *to));
        if self.edits.windows(2).any(|pair| pair[0].1 > pair[1].0) {
            return Err(malformed());
        }
        let mut bytes = Vec::with_capacity(self.body.bytes.len() + 64);
        let mut at = 0;
        for (from, to, text) in &self.edits {
            bytes.extend_from_slice(&self.body.bytes[at..*from]);
            match &self.cipher {
                Some(security) if text.contains(['(', '<']) => bytes.extend_from_slice(
                    &crate::incremental::with_strings_encrypted(
                        security,
                        self.body.reference,
                        text.as_bytes(),
                    )
                    .map_err(SpikeError::Write)?,
                ),
                _ => bytes.extend_from_slice(text.as_bytes()),
            }
            at = *to;
        }
        bytes.extend_from_slice(&self.body.bytes[at..]);
        Ok(PlannedWrite {
            reference: self.body.reference,
            body: PlannedBody::Direct { body: bytes },
        })
    }
}

pub(crate) fn entry<'a>(body: &Body, dictionary: &'a Object, key: &[u8]) -> Option<&'a Object> {
    crate::new_font::entry(body, dictionary, key)
}

pub(crate) fn reference_text(reference: Reference) -> String {
    format!("{} {} R", reference.object_number(), reference.generation())
}
