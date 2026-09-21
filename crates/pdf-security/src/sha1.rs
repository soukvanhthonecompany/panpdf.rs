#![allow(clippy::many_single_char_names)]

const START: [u32; 5] = [
    0x6745_2301,
    0xefcd_ab89,
    0x98ba_dcfe,
    0x1032_5476,
    0xc3d2_e1f0,
];

const K: [u32; 4] = [0x5a82_7999, 0x6ed9_eba1, 0x8f1b_bcdc, 0xca62_c1d6];

pub(crate) struct Running {
    state: [u32; 5],
    blocks: crate::blocks::Blocks<64>,
}

impl Running {
    pub(crate) const fn new() -> Self {
        Self {
            state: START,
            blocks: crate::blocks::Blocks::new(),
        }
    }

    pub(crate) fn update(&mut self, data: &[u8]) {
        let Self { state, blocks } = self;
        blocks.update(data, |block| compress(state, block));
    }

    pub(crate) fn finish(self) -> [u8; 20] {
        let Self { mut state, blocks } = self;
        blocks.finish(8, |block| compress(&mut state, block));
        let mut digest = [0u8; 20];
        for (out, word) in digest.chunks_exact_mut(4).zip(state) {
            out.copy_from_slice(&word.to_be_bytes());
        }
        digest
    }
}

#[cfg(test)]
pub(crate) fn sha1(data: &[u8]) -> [u8; 20] {
    let mut running = Running::new();
    running.update(data);
    running.finish()
}

fn compress(state: &mut [u32; 5], block: &[u8; 64]) {
    let mut w = [0u32; 80];
    for (word, octets) in w.iter_mut().zip(block.chunks_exact(4)) {
        *word = u32::from_be_bytes(octets.try_into().expect("four octets"));
    }
    for index in 16..80 {
        w[index] = (w[index - 3] ^ w[index - 8] ^ w[index - 14] ^ w[index - 16]).rotate_left(1);
    }

    let [mut a, mut b, mut c, mut d, mut e] = *state;
    for (index, word) in w.iter().enumerate() {
        let (mixed, k) = match index / 20 {
            0 => ((b & c) | (!b & d), K[0]),
            1 => (b ^ c ^ d, K[1]),
            2 => ((b & c) | (b & d) | (c & d), K[2]),
            _ => (b ^ c ^ d, K[3]),
        };
        let next = a
            .rotate_left(5)
            .wrapping_add(mixed)
            .wrapping_add(e)
            .wrapping_add(k)
            .wrapping_add(*word);
        e = d;
        d = c;
        c = b.rotate_left(30);
        b = a;
        a = next;
    }
    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
}

#[cfg(test)]
mod tests {
    use super::sha1;

    fn hex(digest: &[u8]) -> String {
        use std::fmt::Write as _;
        digest.iter().fold(String::new(), |mut text, octet| {
            let _ = write!(text, "{octet:02x}");
            text
        })
    }

    #[test]
    fn the_standards_own_examples_come_out_right() {
        assert_eq!(
            hex(&sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hex(&sha1(b"")),
            "da39a3ee5e6b4b0d3255bfef95601890afd80709",
            "the empty message, which is all padding and no data"
        );
        assert_eq!(
            hex(&sha1(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
    }

    #[test]
    fn a_million_letters_hash_to_the_published_answer() {
        let long = vec![b'a'; 1_000_000];
        assert_eq!(
            hex(&sha1(&long)),
            "34aa973cd4c4daa4f61eeb2bdbad27316534016f"
        );
    }

    #[test]
    fn the_lengths_where_padding_changes_shape_agree_with_another_implementation() {
        const KNOWN: [(usize, &str); 12] = [
            (54, "cfae6d86a767b9c700b5081a54265fb2fe0f6fd9"),
            (55, "8ae2d46729cfe68ff927af5eec9c7d1b66d65ac2"),
            (56, "636e2ec698dac903498e648bd2f3af641d3c88cb"),
            (57, "7cb1330f35244b57437539253304ea78a6b7c443"),
            (63, "6d942da0c4392b123528f2905c713a3ce28364bd"),
            (64, "c6138d514ffa2135bfce0ed0b8fac65669917ec7"),
            (65, "69bd728ad6e13cd76ff19751fde427b00e395746"),
            (119, "41c89d06001bab4ab78736b44efe7ce18ce6ae08"),
            (120, "d3dbd653bd8597b7475321b60a36891278e6a04a"),
            (121, "3723f8ab857804f89f80970e9fc88cf8f890adc2"),
            (128, "e6434bc401f98603d7eda504790c98c67385d535"),
            (1000, "c9c960a0b925474fab83942cc27d504fc24ac37b"),
        ];
        for (length, expected) in KNOWN {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "a remainder below 251 is one octet"
            )]
            let data: Vec<u8> = (0..length).map(|index| (index % 251) as u8).collect();
            assert_eq!(hex(&sha1(&data)), expected, "at length {length}");
        }
    }
}
