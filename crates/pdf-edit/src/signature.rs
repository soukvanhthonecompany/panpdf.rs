use pdf_bytes::ByteStore;
use pdf_syntax::{ObjectKind, Reference};

use crate::form::{Found, Reader};
use crate::info::Stamp;
use crate::spike_move_text::SpikeError;

pub use pdf_security::{Checked, Integrity, Moment, Trust, Who};

fn refused(reason: &'static str) -> SpikeError {
    crate::page_tree::refused(reason)
}

const MOST_TEXT_CHARS: usize = 2 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Approval,
    Certification,
    Timestamp,
    UsageRights,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Covers {
    WholeDocument,
    UpTo { signed_through: u64, of: u64 },
    Unstated,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Signature {
    pub field: String,
    pub name: String,
    pub signed: Option<Stamp>,
    pub reason: String,
    pub location: String,
    pub kind: Kind,
    pub encoding: String,
    pub covers: Covers,
    pub checked: Option<pdf_security::Checked>,
}

pub fn signatures(source: &ByteStore, credential: &[u8]) -> Result<Vec<Signature>, SpikeError> {
    let reader = Reader::open(source, credential)
        .map_err(|_| refused("this document cannot be opened to read its signatures"))?;
    let certifying = permission_signature(&reader, b"/DocMDP");

    let rights = permission_signature(&reader, b"/UR3");

    let mut found = Vec::new();
    let mut already = Vec::new();
    for (name, node) in crate::form::signature_nodes(&reader) {
        let Some(value) = reader.entry(&node, b"/V") else {
            continue;
        };
        let at = reference_at(&node, b"/V");
        if let Some(at) = at {
            already.push(at);
        }
        let kind = if at.is_some() && at == certifying {
            Some(Kind::Certification)
        } else if at.is_some() && at == rights {
            Some(Kind::UsageRights)
        } else {
            None
        };
        found.push(read_signature(&reader, source, &value, name, kind));
    }

    for (at, kind) in [
        (certifying, Kind::Certification),
        (rights, Kind::UsageRights),
    ] {
        let Some(at) = at.filter(|at| !already.contains(at)) else {
            continue;
        };
        already.push(at);
        if let Some(value) = reader.at(at) {
            found.push(read_signature(
                &reader,
                source,
                &value,
                String::new(),
                Some(kind),
            ));
        }
    }
    Ok(found)
}

fn permission_signature(reader: &Reader, key: &[u8]) -> Option<Reference> {
    let catalog = reader.catalog()?;
    let perms = reader.entry(&catalog, b"/Perms")?;
    reference_at(&perms, key)
}

fn reference_at(holder: &Found, key: &[u8]) -> Option<Reference> {
    match Reader::raw_entry(holder, key).map(pdf_syntax::Object::kind) {
        Some(ObjectKind::Reference(reference)) => Some(*reference),
        _ => None,
    }
}

fn read_signature(
    reader: &Reader,
    source: &ByteStore,
    value: &Found,
    field: String,
    named_by_the_catalogue: Option<Kind>,
) -> Signature {
    let length = source.len() as u64;
    let range = byte_range(reader, value);
    let text = |key: &[u8]| {
        reader
            .entry(value, key)
            .and_then(|found| reader.text(&found))
            .map(|text| text.chars().take(MOST_TEXT_CHARS).collect())
            .unwrap_or_default()
    };
    let named = |key: &[u8]| {
        reader
            .entry(value, key)
            .and_then(|found| Reader::name(&found))
            .unwrap_or_default()
    };
    let timestamp = named(b"/Type") == "DocTimeStamp";
    Signature {
        field,
        name: text(b"/Name"),
        signed: Stamp::read(&text(b"/M")),
        reason: text(b"/Reason"),
        location: text(b"/Location"),
        kind: match named_by_the_catalogue {
            Some(kind) => kind,
            None if timestamp => Kind::Timestamp,
            None => Kind::Approval,
        },
        encoding: named(b"/SubFilter"),
        covers: covered(range, length),
        checked: checked(reader, source, value, range),
    }
}

fn checked(
    reader: &Reader,
    source: &ByteStore,
    value: &Found,
    range: Option<[u64; 4]>,
) -> Option<pdf_security::Checked> {
    let held = reader.entry(value, b"/Contents")?;
    let blob = Reader::written_bytes(&held)?;
    let [first, count, second, more] = range?;
    let stretch = |from: u64, many: u64| -> &[u8] {
        let from = usize::try_from(from)
            .unwrap_or(usize::MAX)
            .min(source.len());
        let many = usize::try_from(many).unwrap_or(usize::MAX);
        source
            .get(from..from.saturating_add(many).min(source.len()))
            .unwrap_or_default()
    };
    Some(pdf_security::check_signature(
        &blob,
        &[stretch(first, count), stretch(second, more)],
    ))
}

fn byte_range(reader: &Reader, value: &Found) -> Option<[u64; 4]> {
    let range = reader.entry(value, b"/ByteRange")?;
    let ObjectKind::Array(items) = range.value.kind() else {
        return None;
    };
    let numbers: Vec<f64> = items
        .iter()
        .filter_map(|item| Reader::number(&reader.follow(&range, item)?))
        .collect();
    let [first, count, second, more] = numbers[..] else {
        return None;
    };
    if [first, count, second, more]
        .iter()
        .any(|number| *number < 0.0)
    {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "checked non-negative above, and a byte offset is a whole number"
    )]
    Some([first as u64, count as u64, second as u64, more as u64])
}

fn covered(range: Option<[u64; 4]>, length: u64) -> Covers {
    let Some([_, _, second, more]) = range else {
        return Covers::Unstated;
    };
    let through = second.saturating_add(more).min(length);
    if through >= length {
        Covers::WholeDocument
    } else {
        Covers::UpTo {
            signed_through: through,
            of: length,
        }
    }
}

#[cfg(test)]
mod tests;
