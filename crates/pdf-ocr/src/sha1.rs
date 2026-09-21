#[must_use]
pub fn digest(data: &[u8]) -> [u8; 20] {
    let mut state: [u32; 5] = [
        0x6745_2301,
        0xefcd_ab89,
        0x98ba_dcfe,
        0x1032_5476,
        0xc3d2_e1f0,
    ];
    let mut block = [0_u8; 64];
    let mut filled = 0_usize;
    for &byte in data {
        block[filled] = byte;
        filled += 1;
        if filled == 64 {
            compress(&mut state, &block);
            filled = 0;
        }
    }
    let bits = u64::try_from(data.len())
        .unwrap_or(u64::MAX)
        .wrapping_mul(8);
    block[filled] = 0x80;
    filled += 1;
    if filled > 56 {
        block[filled..].fill(0);
        compress(&mut state, &block);
        filled = 0;
    }
    block[filled..56].fill(0);
    block[56..].copy_from_slice(&bits.to_be_bytes());
    compress(&mut state, &block);
    let mut out = [0_u8; 20];
    for (word, place) in state.iter().zip(out.chunks_exact_mut(4)) {
        place.copy_from_slice(&word.to_be_bytes());
    }
    out
}

#[expect(
    clippy::many_single_char_names,
    reason = "a, b, c, d, e and the words are RFC 3174's own names for these"
)]
fn compress(state: &mut [u32; 5], block: &[u8; 64]) {
    let mut words = [0_u32; 80];
    for (word, bytes) in words.iter_mut().zip(block.chunks_exact(4)) {
        *word = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    }
    for at in 16..80 {
        words[at] =
            (words[at - 3] ^ words[at - 8] ^ words[at - 14] ^ words[at - 16]).rotate_left(1);
    }
    let [mut a, mut b, mut c, mut d, mut e] = *state;
    for (at, word) in words.iter().enumerate() {
        let (mixed, constant) = match at {
            0..20 => ((b & c) | (!b & d), 0x5a82_7999),
            20..40 => (b ^ c ^ d, 0x6ed9_eba1),
            40..60 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdc),
            _ => (b ^ c ^ d, 0xca62_c1d6),
        };
        let next = a
            .rotate_left(5)
            .wrapping_add(mixed)
            .wrapping_add(e)
            .wrapping_add(constant)
            .wrapping_add(*word);
        e = d;
        d = c;
        c = b.rotate_left(30);
        b = a;
        a = next;
    }
    for (place, added) in state.iter_mut().zip([a, b, c, d, e]) {
        *place = place.wrapping_add(added);
    }
}

#[must_use]
pub fn hex(digest: &[u8; 20]) -> String {
    let mut text = String::with_capacity(40);
    for byte in digest {
        for nibble in [byte >> 4, byte & 0x0f] {
            text.push(char::from_digit(u32::from(nibble), 16).unwrap_or('0'));
        }
    }
    text
}

#[must_use]
pub fn blob_id(data: &[u8]) -> String {
    let mut header = format!("blob {}\0", data.len()).into_bytes();
    header.extend_from_slice(data);
    hex(&digest(&header))
}

#[cfg(test)]
mod tests {
    use super::{blob_id, digest, hex};

    #[test]
    fn the_published_test_vectors_come_out() {
        assert_eq!(
            hex(&digest(b"")),
            "da39a3ee5e6b4b0d3255bfef95601890afd80709"
        );
        assert_eq!(
            hex(&digest(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hex(&digest(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
        assert_eq!(
            hex(&digest(&vec![b'a'; 1_000_000])),
            "34aa973cd4c4daa4f61eeb2bdbad27316534016f"
        );
    }

    #[test]
    fn a_blob_is_named_as_git_names_it() {
        assert_eq!(
            blob_id(b"hello"),
            "b6fc4c620b67d95f953a5c1c1230aaab5db5a1b0"
        );
        assert_eq!(blob_id(b""), "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
        assert_eq!(
            blob_id(b"the page\n"),
            "c86ea27b78a20249a51c76a07a3a750dee303d5e"
        );
    }

    #[test]
    fn a_digest_is_forty_letters() {
        let text = hex(&[
            0x00, 0x0f, 0xf0, 0xff, 0xab, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ]);
        assert_eq!(text.len(), 40);
        assert!(text.starts_with("000ff0ffab"));
    }
}
