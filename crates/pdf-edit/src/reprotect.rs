use std::collections::BTreeMap;
use std::sync::Arc;

use pdf_bytes::ByteStore;
use pdf_security::AuthenticatedSecurity;
use pdf_syntax::{
    ObjectKind, Reference, ResolveLimits, RevisionIndex, XrefEntryKind, XrefLimits,
    parse_revision_chain_strict,
};

use crate::spike_move_text::SpikeError;

pub use pdf_security::{Allowed, Wanted as Asked};

fn refused(reason: &'static str) -> SpikeError {
    crate::page_tree::refused(reason)
}

#[derive(Clone, Debug)]
pub enum Wanted {
    Open,
    Protected(Box<Asked>),
}

const MOST_OBJECTS: usize = 2_000_000;

pub fn rewrite(
    source: &ByteStore,
    credential: &[u8],
    wanted: &Wanted,
) -> Result<Vec<u8>, SpikeError> {
    let made = match wanted {
        Wanted::Open => None,
        Wanted::Protected(asked) => Some(
            pdf_security::make_protection(asked)
                .map_err(|_| refused("this document's new protection could not be made"))?,
        ),
    };
    let opens = match wanted {
        Wanted::Open => Vec::new(),
        Wanted::Protected(asked) => {
            if asked.owner.is_empty() {
                asked.user.clone()
            } else {
                asked.owner.clone()
            }
        }
    };
    written_under(
        source,
        credential,
        (
            made.as_ref()
                .map(|made| (made.dictionary.as_slice(), &made.security)),
            None,
        ),
        &opens,
    )
}

fn written_under(
    source: &ByteStore,
    credential: &[u8],
    (made, identifier): (Option<(&[u8], &AuthenticatedSecurity)>, Option<String>),
    opens: &[u8],
) -> Result<Vec<u8>, SpikeError> {
    let read = Read::open(source, credential)?;
    let new_security = made.map(|(_, security)| security);

    let kept = read.objects()?;
    let encrypt_at = made
        .is_some()
        .then(|| Reference::new(Read::unused_number(&kept), 0));

    let mut out = Vec::with_capacity(source.len());
    out.extend_from_slice(header(&read, made.is_some()).as_bytes());
    out.extend_from_slice(b"%\xe2\xe3\xcf\xd3\n");

    let mut offsets: BTreeMap<u32, Written> = BTreeMap::new();
    for object in &kept {
        match &object.holds {
            Holds::InStream { stream, index } => {
                offsets.insert(
                    object.at.object_number(),
                    Written::InStream {
                        stream: *stream,
                        index: *index,
                    },
                );
            }
            Holds::Body { body, stream } => {
                let offset = out.len();
                let written = written_object(object.at, body, stream.as_deref(), new_security)?;
                out.extend_from_slice(&written);
                offsets.insert(
                    object.at.object_number(),
                    Written::At {
                        offset,
                        generation: object.at.generation(),
                    },
                );
            }
        }
    }
    if let (Some(at), Some((dictionary, _))) = (encrypt_at, made) {
        let offset = out.len();
        out.extend_from_slice(
            format!(
                "{} {} obj\n{}\nendobj\n",
                at.object_number(),
                at.generation(),
                String::from_utf8_lossy(dictionary)
            )
            .as_bytes(),
        );
        offsets.insert(
            at.object_number(),
            Written::At {
                offset,
                generation: 0,
            },
        );
    }

    let identifier = match identifier {
        Some(given) => given,
        None => read.identifier(made.is_some())?,
    };
    let tail = Tail {
        root: read.root,
        encrypt: encrypt_at,
        identifier: &identifier,
        info: read.info,
    };
    let compressed = offsets
        .values()
        .any(|written| matches!(written, Written::InStream { .. }));
    if compressed {
        write_xref_stream(&mut out, &offsets, &tail, Read::unused_number(&kept) + 1)?;
    } else {
        write_xref_table(&mut out, &offsets, &tail);
    }

    let written = ByteStore::new(pdf_bytes::SourceId::new(0), Arc::<[u8]>::from(out.clone()));
    prove(&read, &written, opens, &kept)?;
    Ok(out)
}

#[cfg(test)]
pub(crate) fn locked_rc4(plain: &ByteStore) -> ByteStore {
    let r3 = ByteStore::new(
        pdf_bytes::SourceId::new(0x7263),
        &include_bytes!("../tests/data/modifiable-r3.pdf")[..],
    );
    let (_, security) =
        crate::previous::readable_index(&r3, b"view").expect("the fixture opens with `view`");
    let security = security.expect("the fixture is protected");
    let dictionary = crate::previous::direct_body(&r3, Reference::new(6, 0), b"view")
        .expect("the fixture's /Encrypt reads");
    let identifier = "[<66d36a30a97e0f16f39955c6221e0c2a> <66d36a30a97e0f16f39955c6221e0c2a>]";
    let written = written_under(
        plain,
        b"",
        (
            Some((dictionary.as_slice(), &security)),
            Some(identifier.to_owned()),
        ),
        b"view",
    )
    .expect("the document is locked");
    ByteStore::new(
        pdf_bytes::SourceId::next_document(),
        Arc::<[u8]>::from(written),
    )
}

fn header(read: &Read<'_>, protecting: bool) -> String {
    if protecting {
        return "%PDF-2.0\n".to_owned();
    }
    read.version.clone()
}

#[derive(Clone, Copy)]
enum Written {
    At { offset: usize, generation: u16 },
    InStream { stream: u32, index: u32 },
}

enum Holds {
    Body {
        body: Vec<u8>,
        stream: Option<Vec<u8>>,
    },
    InStream {
        stream: u32,
        index: u32,
    },
}

struct Object {
    at: Reference,
    holds: Holds,
}

struct Read<'a> {
    source: &'a ByteStore,
    index: RevisionIndex,
    security: Option<Arc<AuthenticatedSecurity>>,
    version: String,
    root: Reference,
    info: Option<Reference>,
    encrypt: Option<Reference>,
    identifier: Option<Vec<u8>>,
}

impl<'a> Read<'a> {
    fn open(source: &'a ByteStore, credential: &[u8]) -> Result<Self, SpikeError> {
        let chain = parse_revision_chain_strict(source, XrefLimits::default()).map_err(|_| {
            refused(
                "this document has to be repaired to be read, and its protection cannot be \
                 changed without writing out every byte exactly as it is",
            )
        })?;
        let (index, security) = crate::previous::readable_index(source, credential)
            .ok_or_else(|| refused("this document cannot be opened to change its protection"))?;
        let newest = chain
            .revisions()
            .first()
            .ok_or_else(|| refused("this document has no revision to read"))?;
        let ObjectKind::Dictionary(trailer) = newest.trailer().kind() else {
            return Err(refused("this document's trailer is not a dictionary"));
        };
        let reference = |key: &[u8]| -> Option<Reference> {
            trailer
                .iter()
                .find(|entry| entry.key_equals(source, key))
                .and_then(|entry| match entry.value().kind() {
                    ObjectKind::Reference(reference) => Some(*reference),
                    _ => None,
                })
        };
        let identifier = trailer
            .iter()
            .find(|entry| entry.key_equals(source, b"/ID"))
            .and_then(|entry| source.resolve(entry.value().span()).ok())
            .map(<[u8]>::to_vec);
        let root = reference(b"/Root")
            .ok_or_else(|| refused("this document's trailer does not name its catalogue"))?;
        Ok(Self {
            source,
            index,
            security,
            version: version_line(source),
            root,
            info: reference(b"/Info"),
            encrypt: reference(b"/Encrypt"),
            identifier,
        })
    }

    fn objects(&self) -> Result<Vec<Object>, SpikeError> {
        let mut numbers: Vec<_> = self
            .index
            .selected_entries()
            .filter(|selected| !matches!(selected.entry().kind(), XrefEntryKind::Free { .. }))
            .map(|selected| (selected.entry().object_number(), selected.entry()))
            .collect();
        if numbers.len() > MOST_OBJECTS {
            return Err(refused(
                "this document holds more objects than can be written",
            ));
        }
        numbers.sort_by_key(|(number, _)| *number);

        let mut kept = Vec::with_capacity(numbers.len());
        for (number, entry) in numbers {
            let at = Reference::new(number, entry.generation());
            if self.encrypt == Some(at) {
                continue;
            }
            match entry.kind() {
                XrefEntryKind::Free { .. } => {}
                XrefEntryKind::Compressed {
                    object_stream_number,
                    index,
                } => kept.push(Object {
                    at,
                    holds: Holds::InStream {
                        stream: object_stream_number,
                        index,
                    },
                }),
                XrefEntryKind::InUse { .. } => {
                    let Some(object) = self.body(at)? else {
                        continue;
                    };
                    if signed(&object) {
                        return Err(refused(
                            "this document is signed, and writing it again would leave the \
                             signature covering bytes that are no longer there",
                        ));
                    }
                    kept.push(object);
                }
            }
        }
        Ok(kept)
    }

    fn body(&self, at: Reference) -> Result<Option<Object>, SpikeError> {
        let resolved = self
            .index
            .resolve_object(self.source, at, ResolveLimits::default())
            .map_err(|_| refused("an object of this document cannot be read"))?;
        if let ObjectKind::Dictionary(entries) = resolved.value().kind()
            && entries.iter().any(|entry| {
                entry.key_equals(resolved.source(), b"/Type")
                    && entry.value().name_equals(resolved.source(), b"/XRef")
            })
        {
            return Ok(None);
        }
        let body = resolved
            .source()
            .resolve(resolved.value().span())
            .map_err(|_| refused("an object of this document cannot be read"))?
            .to_vec();
        let stream = match resolved.stream() {
            None => None,
            Some(stream) => {
                let encoded = resolved
                    .source()
                    .resolve(stream.data_span())
                    .map_err(|_| refused("a stream of this document cannot be read"))?;
                Some(match self.security.as_ref() {
                    None => encoded.to_vec(),
                    Some(security) => security
                        .decrypt_stream(at, encoded)
                        .map_err(|_| refused("a stream of this document cannot be decrypted"))?,
                })
            }
        };
        let body = match self.security.as_ref() {
            None => body,
            Some(security) => plain_strings(&body, at, security)?,
        };
        Ok(Some(Object {
            at,
            holds: Holds::Body { body, stream },
        }))
    }

    fn unused_number(kept: &[Object]) -> u32 {
        kept.iter()
            .map(|object| object.at.object_number())
            .max()
            .unwrap_or(0)
            + 1
    }

    fn identifier(&self, protecting: bool) -> Result<String, SpikeError> {
        if let Some(existing) = &self.identifier {
            return Ok(String::from_utf8_lossy(existing).into_owned());
        }
        if !protecting {
            return Ok(String::new());
        }
        let fresh = pdf_security::random_identifier()
            .map_err(|_| refused("this computer gave no randomness for a file identifier"))?;
        Ok(format!("[<{fresh}> <{fresh}>]"))
    }
}

fn signed(object: &Object) -> bool {
    let Holds::Body { body, .. } = &object.holds else {
        return false;
    };
    let says = |name: &[u8]| body.windows(name.len()).any(|window| window == name);
    says(b"/ByteRange") || says(b"/Type /Sig") || says(b"/Type/Sig")
}

fn version_line(source: &ByteStore) -> String {
    let bytes = source.ahead(0, 16);
    let end = bytes.iter().take(16).position(|byte| *byte == b'\n');
    match end {
        Some(end) if bytes.starts_with(b"%PDF-") => {
            String::from_utf8_lossy(&bytes[..=end]).into_owned()
        }
        _ => "%PDF-1.7\n".to_owned(),
    }
}

struct Tail<'a> {
    root: Reference,
    encrypt: Option<Reference>,
    identifier: &'a str,
    info: Option<Reference>,
}

impl Tail<'_> {
    fn entries(&self, size: u32) -> String {
        use std::fmt::Write as _;

        let mut out = format!(
            "/Size {size} /Root {} {} R",
            self.root.object_number(),
            self.root.generation()
        );
        if let Some(info) = self.info {
            let _ = write!(
                out,
                " /Info {} {} R",
                info.object_number(),
                info.generation()
            );
        }
        if let Some(encrypt) = self.encrypt {
            let _ = write!(
                out,
                " /Encrypt {} {} R",
                encrypt.object_number(),
                encrypt.generation()
            );
        }
        if !self.identifier.is_empty() {
            let _ = write!(out, " /ID {}", self.identifier);
        }
        out
    }
}

fn written_object(
    at: Reference,
    body: &[u8],
    stream: Option<&[u8]>,
    security: Option<&AuthenticatedSecurity>,
) -> Result<Vec<u8>, SpikeError> {
    let body = match security {
        None => body.to_vec(),
        Some(security) => encrypted_strings(body, at, security)?,
    };
    let mut out = format!("{} {} obj\n", at.object_number(), at.generation()).into_bytes();
    let Some(data) = stream else {
        out.extend_from_slice(&body);
        out.extend_from_slice(b"\nendobj\n");
        return Ok(out);
    };
    let data = match security {
        None => data.to_vec(),
        Some(security) => security
            .encrypt_stream(at, data)
            .map_err(|_| refused("a stream of this document cannot be encrypted"))?,
    };
    out.extend_from_slice(&with_length(&body, data.len())?);
    out.extend_from_slice(b"\nstream\n");
    out.extend_from_slice(&data);
    out.extend_from_slice(b"\nendstream\nendobj\n");
    Ok(out)
}

fn with_length(body: &[u8], length: usize) -> Result<Vec<u8>, SpikeError> {
    let store = ByteStore::new(
        pdf_bytes::SourceId::new(0),
        Arc::<[u8]>::from(body.to_vec()),
    );
    let value = pdf_syntax::ObjectParser::new(&store, 0, pdf_syntax::ParseLimits::default())
        .parse_next()
        .map_err(|_| refused("a stream of this document has a dictionary that cannot be read"))?
        .ok_or_else(|| refused("a stream of this document has no dictionary"))?;
    let ObjectKind::Dictionary(entries) = value.kind() else {
        return Err(refused("a stream of this document has no dictionary"));
    };
    let mut out = format!("<< /Length {length}").into_bytes();
    for entry in entries {
        if entry.key_equals(&store, b"/Length") {
            continue;
        }
        let key = store
            .resolve(entry.key().span())
            .map_err(|_| refused("a stream dictionary of this document cannot be read"))?;
        let held = store
            .resolve(entry.value().span())
            .map_err(|_| refused("a stream dictionary of this document cannot be read"))?;
        out.push(b' ');
        out.extend_from_slice(key);
        out.push(b' ');
        out.extend_from_slice(held);
    }
    out.extend_from_slice(b" >>");
    Ok(out)
}

fn plain_strings(
    body: &[u8],
    at: Reference,
    security: &AuthenticatedSecurity,
) -> Result<Vec<u8>, SpikeError> {
    restrung(body, |plain| {
        security
            .decrypt_string(at, plain)
            .map_err(|_| refused("a string of this document cannot be decrypted"))
    })
}

fn encrypted_strings(
    body: &[u8],
    at: Reference,
    security: &AuthenticatedSecurity,
) -> Result<Vec<u8>, SpikeError> {
    restrung(body, |plain| {
        security
            .encrypt_string(at, plain)
            .map_err(|_| refused("a string of this document cannot be encrypted"))
    })
}

fn restrung(
    body: &[u8],
    mut change: impl FnMut(&[u8]) -> Result<Vec<u8>, SpikeError>,
) -> Result<Vec<u8>, SpikeError> {
    use std::fmt::Write as _;

    let unreadable = || refused("an object of this document cannot be read");
    let store = ByteStore::new(
        pdf_bytes::SourceId::new(0),
        Arc::<[u8]>::from(body.to_vec()),
    );
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
        let changed = change(&plain)?;
        out.extend_from_slice(&body[copied..span.start()]);
        out.push(b'<');
        let mut hex = String::with_capacity(changed.len() * 2);
        for byte in changed {
            let _ = write!(hex, "{byte:02x}");
        }
        out.extend_from_slice(hex.as_bytes());
        out.push(b'>');
        copied = span.end();
    }
    out.extend_from_slice(&body[copied..]);
    Ok(out)
}

fn write_xref_table(out: &mut Vec<u8>, offsets: &BTreeMap<u32, Written>, tail: &Tail<'_>) {
    let size = offsets.keys().copied().max().unwrap_or(0) + 1;
    let at = out.len();
    out.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for number in 1..size {
        match offsets.get(&number) {
            Some(Written::At { offset, generation }) => {
                out.extend_from_slice(format!("{offset:010} {generation:05} n \n").as_bytes());
            }
            _ => out.extend_from_slice(b"0000000000 65535 f \n"),
        }
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< {} >>\nstartxref\n{at}\n%%EOF\n",
            tail.entries(size)
        )
        .as_bytes(),
    );
}

fn write_xref_stream(
    out: &mut Vec<u8>,
    offsets: &BTreeMap<u32, Written>,
    tail: &Tail<'_>,
    number: u32,
) -> Result<(), SpikeError> {
    let size = offsets.keys().copied().max().unwrap_or(0).max(number) + 1;
    let at = out.len();
    let mut rows = Vec::with_capacity(size as usize * 7);
    let row = |kind: u8, first: u32, second: u16, rows: &mut Vec<u8>| {
        rows.push(kind);
        rows.extend_from_slice(&first.to_be_bytes());
        rows.extend_from_slice(&second.to_be_bytes());
    };
    row(0, 0, 0xffff, &mut rows);
    for object in 1..size {
        if object == number {
            let offset = u32::try_from(at)
                .map_err(|_| refused("this document is too large to write again"))?;
            row(1, offset, 0, &mut rows);
            continue;
        }
        match offsets.get(&object) {
            Some(Written::At { offset, generation }) => {
                let offset = u32::try_from(*offset)
                    .map_err(|_| refused("this document is too large to write again"))?;
                row(1, offset, *generation, &mut rows);
            }
            Some(Written::InStream { stream, index }) => {
                let index = u16::try_from(*index)
                    .map_err(|_| refused("an object stream of this document holds too many"))?;
                row(2, *stream, index, &mut rows);
            }
            None => row(0, 0, 0xffff, &mut rows),
        }
    }
    out.extend_from_slice(
        format!(
            "{number} 0 obj\n<< /Type /XRef /W [1 4 2] {} /Length {} >>\nstream\n",
            tail.entries(size),
            rows.len()
        )
        .as_bytes(),
    );
    out.extend_from_slice(&rows);
    out.extend_from_slice(b"\nendstream\nendobj\n");
    out.extend_from_slice(format!("startxref\n{at}\n%%EOF\n").as_bytes());
    Ok(())
}

fn prove(
    read: &Read<'_>,
    written: &ByteStore,
    credential: &[u8],
    kept: &[Object],
) -> Result<(), SpikeError> {
    let (index, security) = crate::previous::readable_index(written, credential)
        .ok_or_else(|| refused("the document written again cannot be opened"))?;
    let after = Read {
        source: written,
        index,
        security,
        version: String::new(),
        root: read.root,
        info: read.info,
        encrypt: None,
        identifier: None,
    };
    for object in kept {
        let Holds::Body { body, stream } = &object.holds else {
            continue;
        };
        let Some(again) = after.body(object.at)? else {
            return Err(refused("the document written again lost an object"));
        };
        let Holds::Body {
            body: written_body,
            stream: written_stream,
        } = &again.holds
        else {
            return Err(refused(
                "the document written again stores an object differently",
            ));
        };
        if stream.as_deref() != written_stream.as_deref() {
            return Err(refused(
                "a stream of the document written again is not what it was",
            ));
        }
        if canonical(body, stream.is_some())? != canonical(written_body, stream.is_some())? {
            return Err(refused(
                "an object of the document written again is not what it was",
            ));
        }
    }
    Ok(())
}

fn canonical(body: &[u8], is_stream: bool) -> Result<Vec<u8>, SpikeError> {
    let plain = restrung(body, |plain| Ok(plain.to_vec()))?;
    if !is_stream {
        return Ok(plain);
    }
    let store = ByteStore::new(
        pdf_bytes::SourceId::new(0),
        Arc::<[u8]>::from(plain.clone()),
    );
    let value = pdf_syntax::ObjectParser::new(&store, 0, pdf_syntax::ParseLimits::default())
        .parse_next()
        .map_err(|_| refused("a stream dictionary cannot be read back"))?
        .ok_or_else(|| refused("a stream has no dictionary to read back"))?;
    let ObjectKind::Dictionary(entries) = value.kind() else {
        return Ok(plain);
    };
    let mut pairs = Vec::with_capacity(entries.len());
    for entry in entries {
        if entry.key_equals(&store, b"/Length") {
            continue;
        }
        let key = store
            .resolve(entry.key().span())
            .map_err(|_| refused("a stream dictionary cannot be read back"))?
            .to_vec();
        let held = store
            .resolve(entry.value().span())
            .map_err(|_| refused("a stream dictionary cannot be read back"))?
            .to_vec();
        pairs.push((key, held));
    }
    pairs.sort();
    let mut out = Vec::with_capacity(plain.len());
    for (key, held) in pairs {
        out.extend_from_slice(&key);
        out.push(b' ');
        out.extend_from_slice(&held);
        out.push(b'\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod reading_tests;
