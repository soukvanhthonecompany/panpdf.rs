use crate::der::{Element, Reader, retagged, tag};
use crate::digest::Hash;
use crate::x509::{Certificate, Moment};

mod oid {
    pub(super) const SIGNED_DATA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x02];
    pub(super) const CONTENT_TYPE: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x03];
    pub(super) const MESSAGE_DIGEST: &[u8] =
        &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x04];
    pub(super) const SIGNING_TIME: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x05];
}

const MOST_LINKS: usize = 12;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Integrity {
    Intact,
    ContentChanged,
    SignatureWrong,
    CannotCheck(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Who {
    pub common: String,
    pub full: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Trust {
    Anchored { root: String },
    UnknownAuthority { top: String },
    Incomplete,
    NoStore,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Checked {
    pub integrity: Integrity,
    pub signer: Option<Who>,
    pub issuer: Option<Who>,
    pub trust: Trust,
    pub links: usize,
    pub hash: Option<&'static str>,
    pub hash_is_sound: bool,
    pub key_bits: Option<usize>,
    pub signed_at: Option<Moment>,
    pub certificate_life: Option<(Moment, Moment)>,
    pub certificate_is_current: bool,
}

impl Checked {
    fn cannot(why: impl Into<String>) -> Self {
        Self {
            integrity: Integrity::CannotCheck(why.into()),
            signer: None,
            issuer: None,
            trust: Trust::Incomplete,
            links: 0,
            hash: None,
            hash_is_sound: true,
            key_bits: None,
            signed_at: None,
            certificate_life: None,
            certificate_is_current: false,
        }
    }
}

#[must_use]
pub fn check(contents: &[u8], covered: &[&[u8]]) -> Checked {
    match read(contents, covered) {
        Ok(checked) => checked,
        Err(why) => Checked::cannot(why),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "one structure read top to bottom in the order RFC 5652 writes it; \
              splitting it would hand the pieces to each other as arguments"
)]
fn read(contents: &[u8], covered: &[&[u8]]) -> Result<Checked, String> {
    let outer = Reader::new(contents)
        .next()
        .map_err(|error| format!("this signature is not a structure this reads: {error}"))?;
    let mut outer_parts = outer.reader();
    let kind = outer_parts
        .expect(tag::OID)
        .and_then(|element| element.oid())
        .map_err(|_| "this signature does not say what it is".to_owned())?;
    if kind != oid::SIGNED_DATA {
        return Err("this signature is not the signed data PDF uses".to_owned());
    }
    let signed_data = outer_parts
        .expect(tag::CONTEXT_0)
        .and_then(|element| element.first())
        .map_err(|_| "this signature holds nothing".to_owned())?;

    let mut parts = signed_data.reader();
    let _version = parts.expect(tag::INTEGER);
    let _digest_algorithms = parts.expect(tag::SET);
    let content = parts
        .expect(tag::SEQUENCE)
        .map_err(|_| "this signature does not say what was signed".to_owned())?;

    let bag: Vec<Certificate<'_>> = parts
        .optional(tag::CONTEXT_0)
        .map(|held| {
            let mut each = held.reader();
            let mut found = Vec::new();
            while let Ok(element) = each.next() {
                if let Some(certificate) = Certificate::read(element.whole) {
                    found.push(certificate);
                }
            }
            found
        })
        .unwrap_or_default();
    let _revocations = parts.optional(tag::CONTEXT_1);

    let signers = parts
        .expect(tag::SET)
        .map_err(|_| "this signature names nobody who signed".to_owned())?;
    let signer = signers
        .first()
        .map_err(|_| "this signature names nobody who signed".to_owned())?;

    let mut about = signer.reader();
    let _version = about.expect(tag::INTEGER);
    let sid = about
        .next()
        .map_err(|_| "this signature does not say whose it is".to_owned())?;
    let hash = Hash::named_in(
        &about
            .expect(tag::SEQUENCE)
            .map_err(|_| "this signature does not say how it was digested".to_owned())?,
    )
    .ok_or_else(|| "this signature uses a hash this does not have".to_owned())?;
    let attributes = about.optional(tag::CONTEXT_0);
    let signature_algorithm = about
        .expect(tag::SEQUENCE)
        .map_err(|_| "this signature does not say how it was made".to_owned())?;
    let signature = about
        .expect(tag::OCTET_STRING)
        .map_err(|_| "this signature holds no signature".to_owned())?
        .content;

    let Some(signing) = bag.iter().find(|certificate| names(&sid, certificate)) else {
        return Err("this signature does not carry the certificate that made it".to_owned());
    };
    let (trust, links) = follow(signing, &bag);
    let now = Moment::now();
    let told = Checked {
        hash: Some(hash.name()),
        hash_is_sound: hash.is_sound(),
        certificate_life: Some((signing.not_before, signing.not_after)),
        certificate_is_current: signing.covers(now),
        ..identify(&bag, &sid, trust, links)
    };

    let mut running = hash.running();
    for part in covered {
        running.update(part);
    }
    let of_the_document = running.finish();

    let (signed_bytes, signed_at) = if let Some(attributes) = attributes {
        {
            let stated = attribute(&attributes, oid::MESSAGE_DIGEST)
                .ok_or_else(|| "this signature does not say what it digested".to_owned())?;
            if attribute(&attributes, oid::CONTENT_TYPE).is_none() {
                return Err("this signature does not say what kind of thing it signed".to_owned());
            }
            if stated.content != of_the_document.as_slice() {
                return Ok(Checked {
                    integrity: Integrity::ContentChanged,
                    ..told
                });
            }
            let when =
                attribute(&attributes, oid::SIGNING_TIME).and_then(|value| Moment::read(&value));
            (retagged(tag::SET, attributes.content), when)
        }
    } else {
        let mut joined = Vec::new();
        for part in covered {
            joined.extend_from_slice(part);
        }
        (joined, None)
    };
    let _ = content;

    let Some(key) = signing.public_key() else {
        return Err("this signature was made with a kind of key this does not check".to_owned());
    };
    let named = signature_algorithm
        .first()
        .and_then(|element| element.oid())
        .unwrap_or_default();
    if !is_rsa(named) {
        return Err("this signature was made in a way this does not check".to_owned());
    }

    let digest = hash.of(&signed_bytes);
    let integrity = if key.verifies(hash, &digest, signature) {
        Integrity::Intact
    } else {
        Integrity::SignatureWrong
    };

    Ok(Checked {
        integrity,
        key_bits: Some(key.width_in_bits()),
        signed_at,
        ..told
    })
}

fn identify(bag: &[Certificate<'_>], sid: &Element<'_>, trust: Trust, links: usize) -> Checked {
    let signing = bag.iter().find(|certificate| names(sid, certificate));
    Checked {
        integrity: Integrity::Intact,
        signer: signing.map(|certificate| Who {
            common: certificate.subject.common.clone(),
            full: certificate.subject.full.clone(),
        }),
        issuer: signing.map(|certificate| Who {
            common: certificate.issuer.common.clone(),
            full: certificate.issuer.full.clone(),
        }),
        trust,
        links,
        hash: None,
        hash_is_sound: true,
        key_bits: None,
        signed_at: None,
        certificate_life: None,
        certificate_is_current: false,
    }
}

fn names(sid: &Element<'_>, certificate: &Certificate<'_>) -> bool {
    match sid.tag {
        tag::SEQUENCE => {
            let mut parts = sid.reader();
            let (Ok(issuer), Ok(serial)) = (parts.next(), parts.next()) else {
                return false;
            };
            issuer.whole == certificate.issuer.written
                && serial
                    .unsigned()
                    .is_ok_and(|serial| serial == certificate.serial)
        }
        0x80 => certificate.key_identifier == Some(sid.content),
        _ => false,
    }
}

fn is_rsa(oid: &[u8]) -> bool {
    const RSA_FAMILY: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01];
    oid.starts_with(RSA_FAMILY) && oid.len() == RSA_FAMILY.len() + 1 && oid[oid.len() - 1] != 0x0a
}

fn attribute<'a>(attributes: &Element<'a>, wanted: &[u8]) -> Option<Element<'a>> {
    let mut each = attributes.reader();
    while let Ok(attribute) = each.next() {
        let mut parts = attribute.reader();
        let Ok(kind) = parts.next().and_then(|element| element.oid()) else {
            continue;
        };
        if kind != wanted {
            continue;
        }
        return parts.next().ok()?.first().ok();
    }
    None
}

fn follow(signing: &Certificate<'_>, bag: &[Certificate<'_>]) -> (Trust, usize) {
    let store = crate::trust::Store::of_this_computer();
    if store.place.is_none() {
        return (Trust::NoStore, 0);
    }
    let roots: Vec<Certificate<'_>> = store
        .roots
        .iter()
        .filter_map(|der| Certificate::read(der))
        .collect();

    let mut current = signing;
    for links in 0..MOST_LINKS {
        if let Some(root) = roots.iter().find(|root| {
            root.subject.written == current.issuer.written && current.is_signed_by(root)
        }) {
            return (
                Trust::Anchored {
                    root: root.subject.common.clone(),
                },
                links + 1,
            );
        }
        if current.is_self_issued() {
            if roots
                .iter()
                .any(|root| root.subject.written == current.subject.written)
            {
                return (
                    Trust::Anchored {
                        root: current.subject.common.clone(),
                    },
                    links,
                );
            }
            return (
                Trust::UnknownAuthority {
                    top: current.subject.common.clone(),
                },
                links,
            );
        }
        let Some(next) = bag.iter().find(|held| {
            held.subject.written == current.issuer.written
                && held.is_authority
                && current.is_signed_by(held)
        }) else {
            return (Trust::Incomplete, links);
        };
        current = next;
    }
    (Trust::Incomplete, MOST_LINKS)
}

#[cfg(test)]
mod tests;
