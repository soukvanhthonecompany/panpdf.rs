use crate::der::Element;
use crate::sha1::Running as Running1;
use crate::sha2::{Running256, Running512};

mod oid {
    pub(super) const SHA1: &[u8] = &[0x2b, 0x0e, 0x03, 0x02, 0x1a];
    pub(super) const SHA256: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01];
    pub(super) const SHA384: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x02];
    pub(super) const SHA512: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Hash {
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

impl Hash {
    pub(crate) fn named_by(oid: &[u8]) -> Option<Self> {
        match oid {
            oid::SHA1 => Some(Self::Sha1),
            oid::SHA256 => Some(Self::Sha256),
            oid::SHA384 => Some(Self::Sha384),
            oid::SHA512 => Some(Self::Sha512),
            _ => None,
        }
    }

    pub(crate) fn named_in(algorithm: &Element<'_>) -> Option<Self> {
        Self::named_by(algorithm.first().ok()?.oid().ok()?)
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Sha1 => "SHA-1",
            Self::Sha256 => "SHA-256",
            Self::Sha384 => "SHA-384",
            Self::Sha512 => "SHA-512",
        }
    }

    pub(crate) const fn is_sound(self) -> bool {
        !matches!(self, Self::Sha1)
    }

    pub(crate) const fn oid(self) -> &'static [u8] {
        match self {
            Self::Sha1 => oid::SHA1,
            Self::Sha256 => oid::SHA256,
            Self::Sha384 => oid::SHA384,
            Self::Sha512 => oid::SHA512,
        }
    }

    pub(crate) fn of(self, data: &[u8]) -> Vec<u8> {
        let mut running = self.running();
        running.update(data);
        running.finish()
    }

    pub(crate) fn running(self) -> Running {
        match self {
            Self::Sha1 => Running::One(Box::new(Running1::new())),
            Self::Sha256 => Running::Narrow(Box::new(Running256::new())),
            Self::Sha384 => Running::Short(Box::new(Running512::short())),
            Self::Sha512 => Running::Long(Box::new(Running512::long())),
        }
    }
}

pub(crate) enum Running {
    One(Box<Running1>),
    Narrow(Box<Running256>),
    Short(Box<Running512>),
    Long(Box<Running512>),
}

impl Running {
    pub(crate) fn update(&mut self, data: &[u8]) {
        match self {
            Self::One(running) => running.update(data),
            Self::Narrow(running) => running.update(data),
            Self::Short(running) | Self::Long(running) => running.update(data),
        }
    }

    pub(crate) fn finish(self) -> Vec<u8> {
        match self {
            Self::One(running) => running.finish().to_vec(),
            Self::Narrow(running) => running.finish().to_vec(),
            Self::Short(running) => running.finish()[..48].to_vec(),
            Self::Long(running) => running.finish().to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Hash;

    #[test]
    fn each_identifier_names_the_hash_it_is_the_identifier_of() {
        let data = b"the quick brown fox";
        for (hash, expected) in [
            (Hash::Sha256, crate::sha2::sha256(data).to_vec()),
            (Hash::Sha384, crate::sha2::sha384(data).to_vec()),
            (Hash::Sha512, crate::sha2::sha512(data).to_vec()),
        ] {
            let named = Hash::named_by(hash.oid()).expect("its own identifier names it");
            assert_eq!(named, hash);
            assert_eq!(named.of(data), expected);
        }
        assert_eq!(
            Hash::named_by(&[0x2b, 0x0e, 0x03, 0x02, 0x1a]),
            Some(Hash::Sha1)
        );
    }

    #[test]
    fn an_unknown_identifier_names_no_hash() {
        assert_eq!(
            Hash::named_by(&[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x02, 0x05]),
            None
        );
        assert_eq!(Hash::named_by(&[]), None);
    }

    #[test]
    fn a_message_in_pieces_digests_like_the_whole_message() {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a remainder below 251 is one octet"
        )]
        let data: Vec<u8> = (0..1000usize).map(|index| (index % 251) as u8).collect();
        for hash in [Hash::Sha1, Hash::Sha256, Hash::Sha384, Hash::Sha512] {
            let whole = hash.of(&data);
            for cut in [0, 1, 55, 63, 64, 65, 111, 127, 128, 129, 256, 999, 1000] {
                let mut running = hash.running();
                running.update(&data[..cut]);
                running.update(&data[cut..]);
                assert_eq!(running.finish(), whole, "{} cut at {cut}", hash.name());
            }
            let mut running = hash.running();
            running.update(&data[..70]);
            running.update(&[]);
            running.update(&data[70..]);
            assert_eq!(running.finish(), whole, "{} in three", hash.name());
        }
    }

    #[test]
    fn the_broken_hash_is_marked_as_broken() {
        assert!(!Hash::Sha1.is_sound());
        assert!(Hash::Sha256.is_sound());
    }
}
