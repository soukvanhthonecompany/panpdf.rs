#![allow(clippy::many_single_char_names)]

use zeroize::Zeroizing;

const H256: [u32; 8] = [
    0x6a09_e667,
    0xbb67_ae85,
    0x3c6e_f372,
    0xa54f_f53a,
    0x510e_527f,
    0x9b05_688c,
    0x1f83_d9ab,
    0x5be0_cd19,
];
const K256: [u32; 64] = [
    0x428a_2f98,
    0x7137_4491,
    0xb5c0_fbcf,
    0xe9b5_dba5,
    0x3956_c25b,
    0x59f1_11f1,
    0x923f_82a4,
    0xab1c_5ed5,
    0xd807_aa98,
    0x1283_5b01,
    0x2431_85be,
    0x550c_7dc3,
    0x72be_5d74,
    0x80de_b1fe,
    0x9bdc_06a7,
    0xc19b_f174,
    0xe49b_69c1,
    0xefbe_4786,
    0x0fc1_9dc6,
    0x240c_a1cc,
    0x2de9_2c6f,
    0x4a74_84aa,
    0x5cb0_a9dc,
    0x76f9_88da,
    0x983e_5152,
    0xa831_c66d,
    0xb003_27c8,
    0xbf59_7fc7,
    0xc6e0_0bf3,
    0xd5a7_9147,
    0x06ca_6351,
    0x1429_2967,
    0x27b7_0a85,
    0x2e1b_2138,
    0x4d2c_6dfc,
    0x5338_0d13,
    0x650a_7354,
    0x766a_0abb,
    0x81c2_c92e,
    0x9272_2c85,
    0xa2bf_e8a1,
    0xa81a_664b,
    0xc24b_8b70,
    0xc76c_51a3,
    0xd192_e819,
    0xd699_0624,
    0xf40e_3585,
    0x106a_a070,
    0x19a4_c116,
    0x1e37_6c08,
    0x2748_774c,
    0x34b0_bcb5,
    0x391c_0cb3,
    0x4ed8_aa4a,
    0x5b9c_ca4f,
    0x682e_6ff3,
    0x748f_82ee,
    0x78a5_636f,
    0x84c8_7814,
    0x8cc7_0208,
    0x90be_fffa,
    0xa450_6ceb,
    0xbef9_a3f7,
    0xc671_78f2,
];
const H512: [u64; 8] = [
    0x6a09_e667_f3bc_c908,
    0xbb67_ae85_84ca_a73b,
    0x3c6e_f372_fe94_f82b,
    0xa54f_f53a_5f1d_36f1,
    0x510e_527f_ade6_82d1,
    0x9b05_688c_2b3e_6c1f,
    0x1f83_d9ab_fb41_bd6b,
    0x5be0_cd19_137e_2179,
];
const H384: [u64; 8] = [
    0xcbbb_9d5d_c105_9ed8,
    0x629a_292a_367c_d507,
    0x9159_015a_3070_dd17,
    0x152f_ecd8_f70e_5939,
    0x6733_2667_ffc0_0b31,
    0x8eb4_4a87_6858_1511,
    0xdb0c_2e0d_64f9_8fa7,
    0x47b5_481d_befa_4fa4,
];
const K512: [u64; 80] = [
    0x428a_2f98_d728_ae22,
    0x7137_4491_23ef_65cd,
    0xb5c0_fbcf_ec4d_3b2f,
    0xe9b5_dba5_8189_dbbc,
    0x3956_c25b_f348_b538,
    0x59f1_11f1_b605_d019,
    0x923f_82a4_af19_4f9b,
    0xab1c_5ed5_da6d_8118,
    0xd807_aa98_a303_0242,
    0x1283_5b01_4570_6fbe,
    0x2431_85be_4ee4_b28c,
    0x550c_7dc3_d5ff_b4e2,
    0x72be_5d74_f27b_896f,
    0x80de_b1fe_3b16_96b1,
    0x9bdc_06a7_25c7_1235,
    0xc19b_f174_cf69_2694,
    0xe49b_69c1_9ef1_4ad2,
    0xefbe_4786_384f_25e3,
    0x0fc1_9dc6_8b8c_d5b5,
    0x240c_a1cc_77ac_9c65,
    0x2de9_2c6f_592b_0275,
    0x4a74_84aa_6ea6_e483,
    0x5cb0_a9dc_bd41_fbd4,
    0x76f9_88da_8311_53b5,
    0x983e_5152_ee66_dfab,
    0xa831_c66d_2db4_3210,
    0xb003_27c8_98fb_213f,
    0xbf59_7fc7_beef_0ee4,
    0xc6e0_0bf3_3da8_8fc2,
    0xd5a7_9147_930a_a725,
    0x06ca_6351_e003_826f,
    0x1429_2967_0a0e_6e70,
    0x27b7_0a85_46d2_2ffc,
    0x2e1b_2138_5c26_c926,
    0x4d2c_6dfc_5ac4_2aed,
    0x5338_0d13_9d95_b3df,
    0x650a_7354_8baf_63de,
    0x766a_0abb_3c77_b2a8,
    0x81c2_c92e_47ed_aee6,
    0x9272_2c85_1482_353b,
    0xa2bf_e8a1_4cf1_0364,
    0xa81a_664b_bc42_3001,
    0xc24b_8b70_d0f8_9791,
    0xc76c_51a3_0654_be30,
    0xd192_e819_d6ef_5218,
    0xd699_0624_5565_a910,
    0xf40e_3585_5771_202a,
    0x106a_a070_32bb_d1b8,
    0x19a4_c116_b8d2_d0c8,
    0x1e37_6c08_5141_ab53,
    0x2748_774c_df8e_eb99,
    0x34b0_bcb5_e19b_48a8,
    0x391c_0cb3_c5c9_5a63,
    0x4ed8_aa4a_e341_8acb,
    0x5b9c_ca4f_7763_e373,
    0x682e_6ff3_d6b2_b8a3,
    0x748f_82ee_5def_b2fc,
    0x78a5_636f_4317_2f60,
    0x84c8_7814_a1f0_ab72,
    0x8cc7_0208_1a64_39ec,
    0x90be_fffa_2363_1e28,
    0xa450_6ceb_de82_bde9,
    0xbef9_a3f7_b2c6_7915,
    0xc671_78f2_e372_532b,
    0xca27_3ece_ea26_619c,
    0xd186_b8c7_21c0_c207,
    0xeada_7dd6_cde0_eb1e,
    0xf57d_4f7f_ee6e_d178,
    0x06f0_67aa_7217_6fba,
    0x0a63_7dc5_a2c8_98a6,
    0x113f_9804_bef9_0dae,
    0x1b71_0b35_131c_471b,
    0x28db_77f5_2304_7d84,
    0x32ca_ab7b_40c7_2493,
    0x3c9e_be0a_15c9_bebc,
    0x431d_67c4_9c10_0d4c,
    0x4cc5_d4be_cb3e_42b6,
    0x597f_299c_fc65_7e2a,
    0x5fcb_6fab_3ad6_faec,
    0x6c44_198c_4a47_5817,
];
pub(crate) struct Running256 {
    state: [u32; 8],
    blocks: crate::blocks::Blocks<64>,
}

impl Running256 {
    pub(crate) const fn new() -> Self {
        Self {
            state: H256,
            blocks: crate::blocks::Blocks::new(),
        }
    }

    pub(crate) fn update(&mut self, data: &[u8]) {
        let Self { state, blocks } = self;
        blocks.update(data, |block| compress256(state, block));
    }

    pub(crate) fn finish(self) -> Zeroizing<[u8; 32]> {
        let Self { mut state, blocks } = self;
        blocks.finish(8, |block| compress256(&mut state, block));
        let mut out = Zeroizing::new([0u8; 32]);
        for (slot, word) in out.chunks_mut(4).zip(state) {
            slot.copy_from_slice(&word.to_be_bytes());
        }
        out
    }
}

pub(crate) struct Running512 {
    state: [u64; 8],
    blocks: crate::blocks::Blocks<128>,
}

impl Running512 {
    const fn new(start: [u64; 8]) -> Self {
        Self {
            state: start,
            blocks: crate::blocks::Blocks::new(),
        }
    }

    pub(crate) const fn short() -> Self {
        Self::new(H384)
    }

    pub(crate) const fn long() -> Self {
        Self::new(H512)
    }

    pub(crate) fn update(&mut self, data: &[u8]) {
        let Self { state, blocks } = self;
        blocks.update(data, |block| compress512(state, block));
    }

    pub(crate) fn finish(self) -> Zeroizing<[u8; 64]> {
        let Self { mut state, blocks } = self;
        blocks.finish(16, |block| compress512(&mut state, block));
        let mut out = Zeroizing::new([0u8; 64]);
        for (slot, word) in out.chunks_mut(8).zip(state) {
            slot.copy_from_slice(&word.to_be_bytes());
        }
        out
    }
}

#[must_use]
pub fn sha256(data: &[u8]) -> Zeroizing<[u8; 32]> {
    let mut running = Running256::new();
    running.update(data);
    running.finish()
}

#[must_use]
pub fn sha512(data: &[u8]) -> Zeroizing<[u8; 64]> {
    let mut running = Running512::long();
    running.update(data);
    running.finish()
}

#[must_use]
pub fn sha384(data: &[u8]) -> Zeroizing<[u8; 48]> {
    let mut running = Running512::short();
    running.update(data);
    let full = running.finish();
    let mut out = Zeroizing::new([0u8; 48]);
    out.copy_from_slice(&full[..48]);
    out
}

fn compress256(state: &mut [u32; 8], block: &[u8; 64]) {
    let mut schedule = [0u32; 64];
    for (index, slot) in schedule.iter_mut().enumerate().take(16) {
        let start = index * 4;
        *slot = u32::from_be_bytes([
            block[start],
            block[start + 1],
            block[start + 2],
            block[start + 3],
        ]);
    }
    for index in 16..64 {
        let previous = schedule[index - 15];
        let recent = schedule[index - 2];
        let s0 = previous.rotate_right(7) ^ previous.rotate_right(18) ^ (previous >> 3);
        let s1 = recent.rotate_right(17) ^ recent.rotate_right(19) ^ (recent >> 10);
        schedule[index] = schedule[index - 16]
            .wrapping_add(s0)
            .wrapping_add(schedule[index - 7])
            .wrapping_add(s1);
    }
    let mut working = *state;
    for index in 0..64 {
        let [a, b, c, d, e, f, g, h] = working;
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let choice = (e & f) ^ (!e & g);
        let temp1 = h
            .wrapping_add(s1)
            .wrapping_add(choice)
            .wrapping_add(K256[index])
            .wrapping_add(schedule[index]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let majority = (a & b) ^ (a & c) ^ (b & c);
        let temp2 = s0.wrapping_add(majority);
        working = [
            temp1.wrapping_add(temp2),
            a,
            b,
            c,
            d.wrapping_add(temp1),
            e,
            f,
            g,
        ];
    }
    for (slot, value) in state.iter_mut().zip(working) {
        *slot = slot.wrapping_add(value);
    }
}
fn compress512(state: &mut [u64; 8], block: &[u8; 128]) {
    let mut schedule = [0u64; 80];
    for (index, slot) in schedule.iter_mut().enumerate().take(16) {
        let start = index * 8;
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&block[start..start + 8]);
        *slot = u64::from_be_bytes(bytes);
    }
    for index in 16..80 {
        let previous = schedule[index - 15];
        let recent = schedule[index - 2];
        let s0 = previous.rotate_right(1) ^ previous.rotate_right(8) ^ (previous >> 7);
        let s1 = recent.rotate_right(19) ^ recent.rotate_right(61) ^ (recent >> 6);
        schedule[index] = schedule[index - 16]
            .wrapping_add(s0)
            .wrapping_add(schedule[index - 7])
            .wrapping_add(s1);
    }
    let mut working = *state;
    for index in 0..80 {
        let [a, b, c, d, e, f, g, h] = working;
        let s1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
        let choice = (e & f) ^ (!e & g);
        let temp1 = h
            .wrapping_add(s1)
            .wrapping_add(choice)
            .wrapping_add(K512[index])
            .wrapping_add(schedule[index]);
        let s0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
        let majority = (a & b) ^ (a & c) ^ (b & c);
        let temp2 = s0.wrapping_add(majority);
        working = [
            temp1.wrapping_add(temp2),
            a,
            b,
            c,
            d.wrapping_add(temp1),
            e,
            f,
            g,
        ];
    }
    for (slot, value) in state.iter_mut().zip(working) {
        *slot = slot.wrapping_add(value);
    }
}
#[cfg(test)]
mod tests {
    use super::{sha256, sha384, sha512};
    fn hex(bytes: &[u8]) -> String {
        use std::fmt::Write;
        bytes.iter().fold(String::new(), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
    }
    #[test]
    fn every_digest_matches_an_implementation_that_is_not_this_one() {
        const VECTORS: &[(&str, &str, &str)] = &[
            (
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                "38b060a751ac96384cd9327eb1b1e36a21fdb71114be07434c0cc7bf63f6e1da274edebfe76f65fbd51ad2f14898b95b",
                "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e",
            ),
            (
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
                "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7",
                "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
            ),
            (
                "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3",
                "f54480689c6b0b11d0303285d9a81b21a93bca6ba5a1b4472765dca4da45ee328082d469c650cd3b61b16d3266ab8ced",
                "67ba5535a46e3f86dbfbed8cbbaf0125c76ed549ff8b0b9e03e0c88cf90fa634fa7b12b47d77b694de488ace8d9a65967dc96df599727d3292a8d9d447709c97",
            ),
            (
                "40aff2e9d2d8922e47afd4648e6967497158785fbd1da870e7110266bf944880",
                "ffdaebff65ed05cf400f0221c4ccfb4b2104fb6a51f87e40be6c4309386bfdec2892e9179b34632331a59592737db5c5",
                "1e7b80bc8edc552c8feeb2780e111477e5bc70465fac1a77b29b35980c3f0ce4a036a6c9462036824bd56801e62af7e9feba5c22ed8a5af877bf7de117dcac6d",
            ),
        ];
        let inputs: [Vec<u8>; 4] = [
            Vec::new(),
            b"abc".to_vec(),
            vec![b'a'; 1000],
            (0..=255u8).collect(),
        ];
        for (input, (want256, want384, want512)) in inputs.iter().zip(VECTORS) {
            assert_eq!(
                &hex(&*sha256(input)),
                want256,
                "sha256 of {} bytes",
                input.len()
            );
            assert_eq!(
                &hex(&*sha384(input)),
                want384,
                "sha384 of {} bytes",
                input.len()
            );
            assert_eq!(
                &hex(&*sha512(input)),
                want512,
                "sha512 of {} bytes",
                input.len()
            );
        }
    }
    #[test]
    fn lengths_around_every_block_boundary_are_padded_correctly() {
        const AROUND: &[(usize, &str)] = &[
            (
                54,
                "c17917230c1e735a96829953f52db5339d0d82265e5699ee6451ae70ab0d559d",
            ),
            (
                55,
                "1deace58c745f3ecadde68a5923f494c3703fa73f0306483ccb898a5826e8d70",
            ),
            (
                56,
                "06dbe23685750e4d3881ded95047abaf93fa8f9c5d3501dc57c717a72ff1398e",
            ),
            (
                57,
                "011f4bced20a249afb834d24b3e4eca31ad5ea2bdd1d8a1a564c2d2410398e69",
            ),
            (
                63,
                "47fb38b12335c9298d09280515c0666489a189d1554bb0ac1a0740806ce9d8b6",
            ),
            (
                64,
                "dfa798724b1a8014994f363e5da7474ed26ce3757fb29e07aa47ad5a9352d37b",
            ),
            (
                65,
                "dd2eab5a5507d7717c63ce953d9ac61752c21e664425ad1227054722a0d97f69",
            ),
            (
                119,
                "c6e0f435df5d7d265baacca31e0602c00aa22fa6d3819aed664649294c743756",
            ),
            (
                120,
                "17eb8960823a644bde3065620bb9d45931fe8993fd8eb692a17aff0fd725db6a",
            ),
            (
                127,
                "2409a159329f4e770a7d8128130497abb1de01ffb6ad389d97d89ebbeeb7f02c",
            ),
            (
                128,
                "3a9c1e22e20578eebe442238f431befd688bd56698eb7c77d95d3f97d8f58207",
            ),
            (
                129,
                "956f5da998f608be52587f28902b608909b661f99a1b300e3d3046b4f69f6fb5",
            ),
        ];
        for (length, want) in AROUND {
            let message: Vec<u8> = (0..*length)
                .map(|index| u8::try_from((index * 7 + 3) % 251).unwrap_or(0))
                .collect();
            assert_eq!(&hex(&*sha256(&message)), want, "sha256 of {length} bytes");
        }
    }
}
