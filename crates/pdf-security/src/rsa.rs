use crate::bignum::{Modulus, Number};
use crate::der::{Reader, tag};
use crate::digest::Hash;

pub(crate) struct PublicKey {
    modulus: Modulus,
    exponent: Number,
    width: usize,
}

impl PublicKey {
    pub(crate) fn read(bits: &[u8]) -> Option<Self> {
        let sequence = Reader::new(bits).next().ok()?;
        if sequence.tag != tag::SEQUENCE {
            return None;
        }
        let mut inside = sequence.reader();
        let modulus = inside.expect(tag::INTEGER).ok()?.unsigned().ok()?;
        let exponent = inside.expect(tag::INTEGER).ok()?.unsigned().ok()?;
        Some(Self {
            modulus: Modulus::new(&Number::from_be(modulus)?)?,
            exponent: Number::from_be(exponent)?,
            width: modulus.len(),
        })
    }

    pub(crate) const fn width_in_bits(&self) -> usize {
        self.width * 8
    }

    pub(crate) fn verifies(&self, hash: Hash, digest: &[u8], signature: &[u8]) -> bool {
        if signature.len() != self.width {
            return false;
        }
        let Some(number) = Number::from_be(signature) else {
            return false;
        };
        let Some(recovered) = self.modulus.power(&number, &self.exponent) else {
            return false;
        };
        let Some(recovered) = recovered.to_be(self.width) else {
            return false;
        };
        [true, false]
            .into_iter()
            .filter_map(|with_null| block(hash, digest, self.width, with_null))
            .any(|expected| expected == recovered)
    }
}

fn block(hash: Hash, digest: &[u8], width: usize, with_null: bool) -> Option<Vec<u8>> {
    let parameters = if with_null {
        vec![tag::NULL, 0x00]
    } else {
        Vec::new()
    };
    let mut algorithm = Vec::new();
    algorithm.extend_from_slice(&crate::der::retagged(tag::OID, hash.oid()));
    algorithm.extend_from_slice(&parameters);
    let mut info = Vec::new();
    info.extend_from_slice(&crate::der::retagged(tag::SEQUENCE, &algorithm));
    info.extend_from_slice(&crate::der::retagged(tag::OCTET_STRING, digest));
    let info = crate::der::retagged(tag::SEQUENCE, &info);

    let padding = width.checked_sub(info.len() + 3)?;
    if padding < 8 {
        return None;
    }
    let mut out = vec![0x00, 0x01];
    out.extend(std::iter::repeat_n(0xff, padding));
    out.push(0x00);
    out.extend_from_slice(&info);
    Some(out)
}

#[cfg(test)]
mod tests;
