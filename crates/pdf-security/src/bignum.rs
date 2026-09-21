pub(crate) const MOST_BITS: usize = 8192;

const MOST_WORDS: usize = MOST_BITS / 64;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Number(Vec<u64>);

impl Number {
    pub(crate) fn from_be(octets: &[u8]) -> Option<Self> {
        if octets.len() > MOST_WORDS * 8 {
            return None;
        }
        let mut words = vec![0u64; octets.len().div_ceil(8).max(1)];
        for (index, &octet) in octets.iter().rev().enumerate() {
            words[index / 8] |= u64::from(octet) << ((index % 8) * 8);
        }
        Some(Self(words))
    }

    pub(crate) fn to_be(&self, width: usize) -> Option<Vec<u8>> {
        let mut octets = vec![0u8; width];
        for (index, word) in self.0.iter().enumerate() {
            for byte in 0..8 {
                #[expect(clippy::cast_possible_truncation, reason = "one octet is taken")]
                let value = (word >> (byte * 8)) as u8;
                let place = index * 8 + byte;
                if value == 0 {
                    continue;
                }
                if place >= width {
                    return None;
                }
                octets[width - 1 - place] = value;
            }
        }
        Some(octets)
    }

    fn words(&self) -> &[u64] {
        &self.0
    }

    fn is_below(&self, other: &[u64]) -> bool {
        below(&self.0, other)
    }
}

pub(crate) struct Modulus {
    words: Vec<u64>,
    inverse: u64,
    squared: Vec<u64>,
}

impl Modulus {
    pub(crate) fn new(n: &Number) -> Option<Self> {
        let mut words = n.words().to_vec();
        while words.len() > 1 && *words.last()? == 0 {
            words.pop();
        }
        if words[0].is_multiple_of(2) || words == [0] {
            return None;
        }
        let inverse = negated_inverse(words[0]);
        let squared = two_to_the(words.len() * 128, &words);
        Some(Self {
            words,
            inverse,
            squared,
        })
    }

    fn width(&self) -> usize {
        self.words.len()
    }

    pub(crate) fn power(&self, base: &Number, exponent: &Number) -> Option<Number> {
        if !base.is_below(&self.words) {
            return None;
        }
        let width = self.width();
        let mut fitted = base.words().to_vec();
        fitted.resize(width, 0);

        let one = {
            let mut one = vec![0u64; width];
            one[0] = 1;
            one
        };
        let mut answer = self.mont(&one, &self.squared);
        let carried = self.mont(&fitted, &self.squared);

        let mut seen = false;
        for word in exponent.words().iter().rev() {
            for bit in (0..64).rev() {
                if seen {
                    answer = self.mont(&answer, &answer);
                }
                if word >> bit & 1 == 1 {
                    answer = if seen {
                        self.mont(&answer, &carried)
                    } else {
                        carried.clone()
                    };
                    seen = true;
                }
            }
        }
        Some(Number(self.mont(&answer, &one)))
    }

    fn mont(&self, a: &[u64], b: &[u64]) -> Vec<u64> {
        let width = self.width();
        let mut t = vec![0u64; width + 2];
        for index in 0..width {
            let multiplier = u128::from(*b.get(index).unwrap_or(&0));
            let mut carry = 0u128;
            for (place, slot) in t.iter_mut().take(width).enumerate() {
                let sum = u128::from(*slot)
                    + u128::from(*a.get(place).unwrap_or(&0)) * multiplier
                    + carry;
                *slot = low(sum);
                carry = sum >> 64;
            }
            let sum = u128::from(t[width]) + carry;
            t[width] = low(sum);
            t[width + 1] = low(sum >> 64);

            let m = t[0].wrapping_mul(self.inverse);
            let sum = u128::from(t[0]) + u128::from(m) * u128::from(self.words[0]);
            let mut carry = sum >> 64;
            for place in 1..width {
                let sum =
                    u128::from(t[place]) + u128::from(m) * u128::from(self.words[place]) + carry;
                t[place - 1] = low(sum);
                carry = sum >> 64;
            }
            let sum = u128::from(t[width]) + carry;
            t[width - 1] = low(sum);
            t[width] = t[width + 1].wrapping_add(low(sum >> 64));
            t[width + 1] = 0;
        }
        if t[width] != 0 || !below(&t[..width], &self.words) {
            subtract(&mut t[..width], &self.words);
        }
        t.truncate(width);
        t
    }
}

#[expect(clippy::cast_possible_truncation, reason = "the low word is wanted")]
const fn low(value: u128) -> u64 {
    value as u64
}

fn below(a: &[u64], b: &[u64]) -> bool {
    for index in (0..a.len().max(b.len())).rev() {
        let (mine, theirs) = (
            a.get(index).copied().unwrap_or(0),
            b.get(index).copied().unwrap_or(0),
        );
        if mine != theirs {
            return mine < theirs;
        }
    }
    false
}

fn subtract(a: &mut [u64], b: &[u64]) {
    let mut borrow = 0u64;
    for (index, slot) in a.iter_mut().enumerate() {
        let (first, one) = slot.overflowing_sub(b.get(index).copied().unwrap_or(0));
        let (second, two) = first.overflowing_sub(borrow);
        *slot = second;
        borrow = u64::from(one || two);
    }
}

fn negated_inverse(value: u64) -> u64 {
    let mut inverse = 1u64;
    for _ in 0..6 {
        inverse = inverse.wrapping_mul(2u64.wrapping_sub(value.wrapping_mul(inverse)));
    }
    inverse.wrapping_neg()
}

fn two_to_the(power: usize, n: &[u64]) -> Vec<u64> {
    let mut value = vec![0u64; n.len()];
    value[0] = 1;
    for _ in 0..power {
        let mut carry = 0u64;
        for word in &mut value {
            let doubled = (*word << 1) | carry;
            carry = *word >> 63;
            *word = doubled;
        }
        if carry != 0 || !below(&value, n) {
            subtract(&mut value, n);
        }
    }
    value
}

#[cfg(test)]
mod tests;
