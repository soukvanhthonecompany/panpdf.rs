use crate::der::{Element, Reader, tag};
use crate::digest::Hash;
use crate::rsa::PublicKey;

mod oid {
    pub(super) const RSA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
    pub(super) const RSA_WITH: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01];
    pub(super) const BASIC_CONSTRAINTS: &[u8] = &[0x55, 0x1d, 0x13];
    pub(super) const SUBJECT_KEY_ID: &[u8] = &[0x55, 0x1d, 0x0e];
    pub(super) const COMMON_NAME: &[u8] = &[0x55, 0x04, 0x03];
    pub(super) const ORGANISATION: &[u8] = &[0x55, 0x04, 0x0a];
    pub(super) const UNIT: &[u8] = &[0x55, 0x04, 0x0b];
    pub(super) const COUNTRY: &[u8] = &[0x55, 0x04, 0x06];
    pub(super) const LOCALITY: &[u8] = &[0x55, 0x04, 0x07];
    pub(super) const REGION: &[u8] = &[0x55, 0x04, 0x08];
    pub(super) const EMAIL: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x01];
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Moment {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl Moment {
    pub(crate) fn now() -> Self {
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        Self::from_epoch(seconds)
    }

    fn from_epoch(seconds: u64) -> Self {
        let days = i64::try_from(seconds / 86_400).unwrap_or(0);
        let rest = seconds % 86_400;
        let shifted = days + 719_468;
        let era = shifted.div_euclid(146_097);
        let day_of_era = shifted.rem_euclid(146_097);
        let year_of_era =
            (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let month_index = (5 * day_of_year + 2) / 153;
        let day = day_of_year - (153 * month_index + 2) / 5 + 1;
        let month = if month_index < 10 {
            month_index + 3
        } else {
            month_index - 9
        };
        let year = year_of_era + era * 400 + i64::from(month <= 2);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a date's parts are small and non-negative by construction"
        )]
        Self {
            year: year as i32,
            month: month as u32,
            day: day as u32,
            hour: (rest / 3600) as u32,
            minute: (rest % 3600 / 60) as u32,
            second: (rest % 60) as u32,
        }
    }

    pub(crate) fn read(element: &Element<'_>) -> Option<Self> {
        let digits = element.content;
        let (year, rest) = match element.tag {
            tag::UTC_TIME => {
                let two = number(digits.get(..2)?)?;
                (
                    if two >= 50 { 1900 + two } else { 2000 + two },
                    digits.get(2..)?,
                )
            }
            tag::GENERALIZED_TIME => (number(digits.get(..4)?)?, digits.get(4..)?),
            _ => return None,
        };
        #[expect(
            clippy::cast_possible_wrap,
            reason = "a year read from four digits is far below the limit"
        )]
        Some(Self {
            year: year as i32,
            month: number(rest.get(..2)?)?,
            day: number(rest.get(2..4)?)?,
            hour: number(rest.get(4..6)?)?,
            minute: number(rest.get(6..8)?)?,
            second: rest.get(8..10).and_then(number).unwrap_or(0),
        })
    }

    #[must_use]
    pub fn write(self) -> String {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02} UTC",
            self.year, self.month, self.day, self.hour, self.minute
        )
    }
}

fn number(digits: &[u8]) -> Option<u32> {
    std::str::from_utf8(digits).ok()?.parse().ok()
}

#[derive(Clone, Debug)]
pub(crate) struct Name<'a> {
    pub(crate) written: &'a [u8],
    pub(crate) common: String,
    pub(crate) full: String,
}

impl<'a> Name<'a> {
    fn read(element: &Element<'a>) -> Self {
        let mut common = String::new();
        let mut parts: Vec<String> = Vec::new();
        let mut names = element.reader();
        while let Ok(set) = names.next() {
            let mut inside = set.reader();
            while let Ok(pair) = inside.next() {
                let mut halves = pair.reader();
                let Ok(kind) = halves.next() else { continue };
                let Ok(value) = halves.next() else { continue };
                let Ok(kind) = kind.oid() else { continue };
                let text = string(&value);
                if text.is_empty() {
                    continue;
                }
                let label = match kind {
                    oid::COMMON_NAME => "CN",
                    oid::ORGANISATION => "O",
                    oid::UNIT => "OU",
                    oid::COUNTRY => "C",
                    oid::LOCALITY => "L",
                    oid::REGION => "ST",
                    oid::EMAIL => "E",
                    _ => "?",
                };
                if kind == oid::COMMON_NAME && common.is_empty() {
                    common.clone_from(&text);
                }
                parts.push(format!("{label}={text}"));
            }
        }
        parts.reverse();
        Self {
            written: element.whole,
            common,
            full: parts.join(", "),
        }
    }
}

fn string(element: &Element<'_>) -> String {
    match element.tag {
        0x1e => {
            let pairs: Vec<u16> = element
                .content
                .chunks_exact(2)
                .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                .collect();
            String::from_utf16_lossy(&pairs)
        }
        _ => String::from_utf8_lossy(element.content).into_owned(),
    }
}

pub(crate) struct Certificate<'a> {
    pub(crate) signed_part: &'a [u8],
    pub(crate) serial: &'a [u8],
    pub(crate) issuer: Name<'a>,
    pub(crate) subject: Name<'a>,
    pub(crate) not_before: Moment,
    pub(crate) not_after: Moment,
    key_algorithm: &'a [u8],
    key_bits: &'a [u8],
    pub(crate) signed_with: Option<Hash>,
    signed_by_rsa: bool,
    signature: &'a [u8],
    pub(crate) is_authority: bool,
    pub(crate) key_identifier: Option<&'a [u8]>,
}

impl<'a> Certificate<'a> {
    pub(crate) fn read(bytes: &'a [u8]) -> Option<Self> {
        let whole = Reader::new(bytes).next().ok()?;
        let mut outside = whole.reader();
        let tbs = outside.expect(tag::SEQUENCE).ok()?;
        if tbs.unstated_length {
            return None;
        }
        let algorithm = outside.expect(tag::SEQUENCE).ok()?;
        let signature = outside.expect(tag::BIT_STRING).ok()?.bits().ok()?;

        let mut inside = tbs.reader();
        let _ = inside.optional(tag::CONTEXT_0);
        let serial = inside.expect(tag::INTEGER).ok()?.unsigned().ok()?;
        let _inner_algorithm = inside.expect(tag::SEQUENCE).ok()?;
        let issuer = Name::read(&inside.expect(tag::SEQUENCE).ok()?);
        let validity = inside.expect(tag::SEQUENCE).ok()?;
        let subject = Name::read(&inside.expect(tag::SEQUENCE).ok()?);
        let key_info = inside.expect(tag::SEQUENCE).ok()?;

        let mut when = validity.reader();
        let not_before = Moment::read(&when.next().ok()?)?;
        let not_after = Moment::read(&when.next().ok()?)?;

        let mut key = key_info.reader();
        let key_algorithm = key.expect(tag::SEQUENCE).ok()?.first().ok()?.oid().ok()?;
        let key_bits = key.expect(tag::BIT_STRING).ok()?.bits().ok()?;

        let (mut is_authority, mut key_identifier) = (false, None);
        while let Ok(next) = inside.next() {
            if next.tag != 0xa3 {
                continue;
            }
            let Ok(list) = next.first() else { continue };
            let mut each = list.reader();
            while let Ok(extension) = each.next() {
                let mut parts = extension.reader();
                let Ok(kind) = parts.next().and_then(|element| element.oid()) else {
                    continue;
                };
                let _ = parts.optional(tag::BOOLEAN);
                let Ok(value) = parts.expect(tag::OCTET_STRING) else {
                    continue;
                };
                match kind {
                    oid::BASIC_CONSTRAINTS => {
                        is_authority = Reader::new(value.content)
                            .next()
                            .ok()
                            .and_then(|sequence| sequence.reader().optional(tag::BOOLEAN))
                            .is_some_and(|flag| flag.content.first().is_some_and(|&on| on != 0));
                    }
                    oid::SUBJECT_KEY_ID => {
                        key_identifier = Reader::new(value.content)
                            .next()
                            .ok()
                            .filter(|element| element.tag == tag::OCTET_STRING)
                            .map(|element| element.content);
                    }
                    _ => {}
                }
            }
        }

        let named = algorithm.first().ok()?.oid().ok()?;
        Some(Self {
            signed_part: tbs.whole,
            serial,
            issuer,
            subject,
            not_before,
            not_after,
            key_algorithm,
            key_bits,
            signed_with: signing(named),
            signed_by_rsa: named.starts_with(oid::RSA_WITH)
                && named.len() == oid::RSA_WITH.len() + 1,
            signature,
            is_authority,
            key_identifier,
        })
    }

    pub(crate) fn public_key(&self) -> Option<PublicKey> {
        if self.key_algorithm != oid::RSA {
            return None;
        }
        PublicKey::read(self.key_bits)
    }

    pub(crate) fn is_signed_by(&self, issuer: &Certificate<'_>) -> bool {
        let (Some(hash), Some(key)) = (self.signed_with, issuer.public_key()) else {
            return false;
        };
        if !self.signed_by_rsa {
            return false;
        }
        key.verifies(hash, &hash.of(self.signed_part), self.signature)
    }

    pub(crate) fn covers(&self, when: Moment) -> bool {
        self.not_before <= when && when <= self.not_after
    }

    pub(crate) fn is_self_issued(&self) -> bool {
        self.issuer.written == self.subject.written
    }
}

fn signing(oid: &[u8]) -> Option<Hash> {
    let (family, last) = oid.split_at(oid.len().checked_sub(1)?);
    if family != oid::RSA_WITH {
        return None;
    }
    match last[0] {
        0x05 => Some(Hash::Sha1),
        0x0b => Some(Hash::Sha256),
        0x0c => Some(Hash::Sha384),
        0x0d => Some(Hash::Sha512),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
