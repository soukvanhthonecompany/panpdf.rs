use std::fmt::Write as _;

use pdf_bytes::ByteStore;
use pdf_syntax::{DictionaryEntry, ObjectKind, Reference};

use crate::form::{Found, Reader};

use crate::plan::{Capability, Effect, Plan, PlannedBody, PlannedWrite};
use crate::spike_move_text::{PlannerPage, SpikeError};
pub use pdf_security::{AccessLevel, CipherMethod, PrintAllowance};

const MOST_TEXT_CHARS: usize = 8 * 1024;

fn refused(reason: &'static str) -> SpikeError {
    crate::page_tree::refused(reason)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Stamp {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub offset_minutes: Option<i32>,
}

impl Stamp {
    #[must_use]
    pub fn read(text: &str) -> Option<Self> {
        let digits = text.strip_prefix("D:").unwrap_or(text);
        let number = |at: usize, len: usize| -> Option<u32> {
            let piece = digits.get(at..at + len)?;
            if !piece.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            piece.parse::<u32>().ok()
        };
        let year = i32::try_from(number(0, 4)?).ok()?;
        let stamp = Self {
            year,
            month: u8::try_from(number(4, 2).unwrap_or(1)).ok()?,
            day: u8::try_from(number(6, 2).unwrap_or(1)).ok()?,
            hour: u8::try_from(number(8, 2).unwrap_or(0)).ok()?,
            minute: u8::try_from(number(10, 2).unwrap_or(0)).ok()?,
            second: u8::try_from(number(12, 2).unwrap_or(0)).ok()?,
            offset_minutes: zone(digits.get(14..).unwrap_or("")),
        };
        stamp.sensible().then_some(stamp)
    }

    fn sensible(&self) -> bool {
        (1..=12).contains(&self.month)
            && (1..=31).contains(&self.day)
            && self.hour < 24
            && self.minute < 60
            && self.second <= 60
    }

    #[must_use]
    pub fn write(&self) -> String {
        let Self {
            year,
            month,
            day,
            hour,
            minute,
            second,
            offset_minutes,
        } = *self;
        let mut out = format!("D:{year:04}{month:02}{day:02}{hour:02}{minute:02}{second:02}");
        match offset_minutes {
            None => out.push('Z'),
            Some(offset) => {
                let sign = if offset < 0 { '-' } else { '+' };
                let (hours, minutes) = (offset.abs() / 60, offset.abs() % 60);
                let _ = write!(out, "{sign}{hours:02}'{minutes:02}'");
            }
        }
        out
    }
}

fn zone(rest: &str) -> Option<i32> {
    let sign = match rest.as_bytes().first()? {
        b'Z' => return Some(0),
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let hours: i32 = rest.get(1..3)?.parse().ok()?;
    let at = if rest.as_bytes().get(3) == Some(&b'\'') {
        4
    } else {
        3
    };
    let minutes: i32 = rest
        .get(at..at + 2)
        .filter(|piece| piece.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|piece| piece.parse().ok())
        .unwrap_or(0);
    (hours < 24 && minutes < 60).then_some(sign * (hours * 60 + minutes))
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DocumentInfo {
    pub title: String,
    pub author: String,
    pub subject: String,
    pub keywords: String,
    pub creator: String,
    pub producer: String,
    pub created: Option<Stamp>,
    pub modified: Option<Stamp>,
    pub other: Vec<(String, String)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Protection {
    pub revision: u8,
    pub stream_cipher: pdf_security::CipherMethod,
    pub string_cipher: pdf_security::CipherMethod,
    pub access: pdf_security::AccessLevel,
    pub may: Permissions,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Table 22 is a row of independent flags, and this is that row"
)]
pub struct Permissions {
    pub print: pdf_security::PrintAllowance,
    pub modify: bool,
    pub copy: bool,
    pub annotate: bool,
    pub fill_forms: bool,
    pub assemble: bool,
}

impl Permissions {
    fn of(access: pdf_security::AccessLevel, revision: u8, permissions: i32) -> Self {
        let owner = matches!(access, pdf_security::AccessLevel::Owner);
        let bit = |number: u32| owner || permissions & (1 << (number - 1)) != 0;
        Self {
            print: if owner {
                pdf_security::PrintAllowance::Faithful
            } else {
                pdf_security::PrintAllowance::of(revision, permissions)
            },
            modify: bit(4),
            copy: bit(5),
            annotate: bit(6),
            fill_forms: bit(6) || (revision >= 3 && bit(9)),
            assemble: bit(4) || (revision >= 3 && bit(11)),
        }
    }
}

impl Permissions {
    #[must_use]
    pub const fn all() -> Self {
        Self {
            print: pdf_security::PrintAllowance::Faithful,
            modify: true,
            copy: true,
            annotate: true,
            fill_forms: true,
            assemble: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DocumentFacts {
    pub info: DocumentInfo,
    pub protection: Option<Protection>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Lock {
    Open,
    Refused,
}

#[must_use]
pub fn lock(source: &ByteStore, credential: &[u8]) -> Lock {
    let Ok(chain) =
        pdf_syntax::parse_revision_chain_strict(source, pdf_syntax::XrefLimits::default())
    else {
        return Lock::Open;
    };
    if !crate::previous::protected(source, &chain) {
        return Lock::Open;
    }
    let Ok(index) = pdf_syntax::RevisionIndex::from_chain(&chain) else {
        return Lock::Open;
    };
    match pdf_security::authenticate_standard_password(
        source,
        &chain,
        &index,
        credential,
        pdf_syntax::ResolveLimits::default(),
    ) {
        Err(error) if error.kind() == pdf_security::SecurityErrorKind::InvalidPassword => {
            Lock::Refused
        }
        Ok(_) | Err(_) => Lock::Open,
    }
}

pub fn document_facts(source: &ByteStore, credential: &[u8]) -> Result<DocumentFacts, SpikeError> {
    let reader = Reader::open(source, credential)
        .map_err(|_| refused("this document cannot be opened to read what it says about itself"))?;
    let protection = reader.security.as_ref().map(|security| Protection {
        revision: security.revision(),
        stream_cipher: security.stream_method(),
        string_cipher: security.string_method(),
        access: security.access_level(),
        may: Permissions::of(
            security.access_level(),
            security.revision(),
            security.permissions(),
        ),
    });
    let info = information_reference(source)
        .and_then(|reference| reader.at(reference))
        .map(|found| read_info(&reader, &found))
        .unwrap_or_default();
    Ok(DocumentFacts { info, protection })
}

fn read_info(reader: &Reader, found: &Found) -> DocumentInfo {
    let text = |key: &[u8]| {
        reader
            .entry(found, key)
            .and_then(|entry| reader.text(&entry))
            .map(|text| text.chars().take(MOST_TEXT_CHARS).collect())
            .unwrap_or_default()
    };
    let date = |key: &[u8]| {
        reader
            .entry(found, key)
            .and_then(|entry| reader.text(&entry))
            .as_deref()
            .and_then(Stamp::read)
    };
    DocumentInfo {
        title: text(b"/Title"),
        author: text(b"/Author"),
        subject: text(b"/Subject"),
        keywords: text(b"/Keywords"),
        creator: text(b"/Creator"),
        producer: text(b"/Producer"),
        created: date(b"/CreationDate"),
        modified: date(b"/ModDate"),
        other: other_entries(reader, found),
    }
}

fn other_entries(reader: &Reader, found: &Found) -> Vec<(String, String)> {
    let ObjectKind::Dictionary(entries) = found.value.kind() else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            if NAMED
                .iter()
                .any(|named| entry.key_equals(&found.source, named))
            {
                return None;
            }
            let key = String::from_utf8(entry.decoded_key(&found.source).ok()?).ok()?;
            let value = reader.follow(found, entry.value())?;
            let shown = reader.text(&value).or_else(|| written(&value))?;
            Some((
                key.trim_start_matches('/').to_owned(),
                shown.chars().take(MOST_TEXT_CHARS).collect(),
            ))
        })
        .collect()
}

fn written(found: &Found) -> Option<String> {
    let bytes = found.source.resolve(found.value.span()).ok()?;
    let text = String::from_utf8_lossy(bytes).trim().to_owned();
    (text.len() <= 200).then_some(text)
}

const NAMED: [&[u8]; 8] = [
    b"/Title",
    b"/Author",
    b"/Subject",
    b"/Keywords",
    b"/Creator",
    b"/Producer",
    b"/CreationDate",
    b"/ModDate",
];

fn information_reference(source: &ByteStore) -> Option<Reference> {
    let chain =
        pdf_syntax::parse_revision_chain_strict(source, pdf_syntax::XrefLimits::default()).ok()?;
    chain.revisions().iter().find_map(|revision| {
        let ObjectKind::Dictionary(entries) = revision.trailer().kind() else {
            return None;
        };
        let value = entries
            .iter()
            .find(|entry| entry.key_equals(source, b"/Info"))?
            .value();
        match value.kind() {
            ObjectKind::Reference(reference) => Some(*reference),
            _ => None,
        }
    })
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InfoEdit {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
    pub creator: Option<String>,
    pub producer: Option<String>,
    pub created: Option<Stamp>,
    pub modified: Option<Stamp>,
}

impl InfoEdit {
    #[must_use]
    pub fn asks_for_nothing(&self) -> bool {
        *self == Self::default()
    }

    fn stated(&self) -> Vec<(&'static [u8], String)> {
        let mut stated = Vec::new();
        for (key, value) in [
            (b"/Title".as_slice(), self.title.as_ref()),
            (b"/Author", self.author.as_ref()),
            (b"/Subject", self.subject.as_ref()),
            (b"/Keywords", self.keywords.as_ref()),
            (b"/Creator", self.creator.as_ref()),
            (b"/Producer", self.producer.as_ref()),
        ] {
            if let Some(value) = value {
                stated.push((key, value.clone()));
            }
        }
        for (key, stamp) in [
            (b"/CreationDate".as_slice(), self.created),
            (b"/ModDate", self.modified),
        ] {
            if let Some(stamp) = stamp {
                stated.push((key, stamp.write()));
            }
        }
        stated
    }
}

pub(crate) fn plan_set_document_info(
    source: &ByteStore,
    page: PlannerPage<'_>,
    edit: &InfoEdit,
) -> Result<Plan, SpikeError> {
    if edit.asks_for_nothing() {
        return Err(refused("nothing was asked to be changed"));
    }
    let reader = Reader::open(source, page.credential)
        .map_err(|_| refused("this document cannot be opened to describe it"))?;
    let stated = edit.stated();
    let existing = information_reference(source);
    let body = information_body(&reader, existing, &stated)?;

    let target = match existing {
        Some(reference) => reference,
        None => Reference::new(crate::block_rewrite::next_object_number(source)?, 0),
    };
    let extras = crate::incremental::TrailerExtras {
        info: existing.is_none().then_some(target),
    };
    let body = match reader.security.as_ref() {
        Some(security) if !crate::incremental::strings_are_plain(&reader.index, target) => {
            crate::incremental::with_strings_encrypted(security, target, &body)
                .map_err(|_| refused("this document's description cannot be written"))?
        }
        _ => body,
    };
    let writes = vec![PlannedWrite {
        reference: target,
        body: PlannedBody::Direct { body },
    }];
    let document = crate::block_rewrite::commit_writes_with(
        source,
        &writes,
        (page.credential, page.restrictions),
        extras,
    )?;
    prove(&document, page.credential, &stated)?;
    let plan = Plan::new(
        Capability::Exact,
        writes,
        Effect {
            page_index: 0,
            moved: Vec::new(),
            target_stream: target,
            declared_region: None,
        },
    )
    .with_trailer(extras);
    if existing.is_some() {
        return Ok(plan);
    }
    Ok(plan.with_undo(vec![PlannedWrite {
        reference: target,
        body: PlannedBody::Direct {
            body: b"<< >>".to_vec(),
        },
    }]))
}

fn information_body(
    reader: &Reader,
    existing: Option<Reference>,
    stated: &[(&'static [u8], String)],
) -> Result<Vec<u8>, SpikeError> {
    let mut body = b"<<".to_vec();
    if let Some(found) = existing.and_then(|reference| reader.at(reference)) {
        let ObjectKind::Dictionary(entries) = found.value.kind() else {
            return Err(refused("this document's description is not a dictionary"));
        };
        for entry in entries {
            if stated
                .iter()
                .any(|(named, _)| entry.key_equals(&found.source, named))
            {
                continue;
            }
            let key = found
                .source
                .resolve(entry.key().span())
                .map_err(|_| refused("this document's description cannot be read"))?;
            body.push(b' ');
            body.extend_from_slice(key);
            body.push(b' ');
            body.extend_from_slice(&kept(reader, &found, entry)?);
        }
    }
    for (key, value) in stated {
        if value.is_empty() {
            continue;
        }
        body.push(b' ');
        body.extend_from_slice(key);
        body.push(b' ');
        body.extend_from_slice(written_value(key, value).as_bytes());
    }
    body.extend_from_slice(b" >>");
    Ok(body)
}

fn written_value(key: &[u8], value: &str) -> String {
    if matches!(key, b"/CreationDate" | b"/ModDate") {
        return format!("({value})");
    }
    crate::fill_field::pdf_text_string(value)
}

fn kept(reader: &Reader, found: &Found, entry: &DictionaryEntry) -> Result<Vec<u8>, SpikeError> {
    let unreadable = || refused("this document's description cannot be read");
    let value = reader.follow(found, entry.value()).ok_or_else(unreadable)?;
    if reader.security.is_some()
        && matches!(
            entry.value().kind(),
            ObjectKind::LiteralString | ObjectKind::HexString
        )
    {
        let text = reader.text(&value).ok_or_else(unreadable)?;
        return Ok(crate::fill_field::pdf_text_string(&text).into_bytes());
    }
    Ok(found
        .source
        .resolve(entry.value().span())
        .map_err(|_| unreadable())?
        .to_vec())
}

fn prove(
    document: &ByteStore,
    credential: &[u8],
    stated: &[(&'static [u8], String)],
) -> Result<(), SpikeError> {
    let facts = document_facts(document, credential)
        .map_err(|_| refused("the document does not read back after being described"))?;
    for (key, wanted) in stated {
        let said = match *key {
            b"/Title" => facts.info.title.clone(),
            b"/Author" => facts.info.author.clone(),
            b"/Subject" => facts.info.subject.clone(),
            b"/Keywords" => facts.info.keywords.clone(),
            b"/Creator" => facts.info.creator.clone(),
            b"/Producer" => facts.info.producer.clone(),
            b"/CreationDate" => facts
                .info
                .created
                .map(|stamp| stamp.write())
                .unwrap_or_default(),
            b"/ModDate" => facts
                .info
                .modified
                .map(|stamp| stamp.write())
                .unwrap_or_default(),
            _ => return Err(refused("this document's description cannot be written")),
        };
        if said != *wanted {
            return Err(refused("the document does not say what it was told to say"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
