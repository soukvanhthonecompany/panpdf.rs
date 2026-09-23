const VERSION: &str = "panpdf-key-1";

const FOR_THE_KEY: &[u8] = b"panpdf key v1";
const FOR_THE_STREAM: &[u8] = b"stream";
const FOR_THE_TAG: &[u8] = b"tag";

pub const SALT: usize = 16;

const MOST: usize = 4096;

#[must_use]
pub fn lock(secret: &str, machine: &[u8], salt: &[u8]) -> Option<String> {
    if secret.is_empty() || secret.len() > MOST || salt.len() != SALT {
        return None;
    }
    let locking = locking_key(machine, salt);
    let hidden = added(&locking, secret.as_bytes());
    let tag = tag_of(&locking, salt, &hidden);
    Some(format!(
        "{VERSION}\t{}\t{}\t{}\n",
        hex(salt),
        hex(&hidden),
        hex(&tag)
    ))
}

#[must_use]
pub fn unlock(text: &str, machine: &[u8]) -> Option<String> {
    let line = text.lines().next()?;
    let mut fields = line.split('\t');
    if fields.next()? != VERSION {
        return None;
    }
    let salt = unhex(fields.next()?)?;
    let hidden = unhex(fields.next()?)?;
    let tag = unhex(fields.next()?)?;
    if fields.next().is_some() || salt.len() != SALT || hidden.is_empty() || hidden.len() > MOST {
        return None;
    }
    let locking = locking_key(machine, &salt);
    if !same(&tag, &tag_of(&locking, &salt, &hidden)) {
        return None;
    }
    String::from_utf8(added(&locking, &hidden)).ok()
}

#[must_use]
pub fn looks_like_ours(text: &str) -> bool {
    text.lines()
        .next()
        .is_some_and(|line| line.starts_with(VERSION))
}

fn locking_key(machine: &[u8], salt: &[u8]) -> [u8; 32] {
    let mut message = FOR_THE_KEY.to_vec();
    message.extend_from_slice(salt);
    hmac(machine, &message)
}

fn tag_of(locking: &[u8; 32], salt: &[u8], hidden: &[u8]) -> [u8; 32] {
    let mut message = FOR_THE_TAG.to_vec();
    message.extend_from_slice(salt);
    message.extend_from_slice(hidden);
    hmac(locking, &message)
}

fn added(locking: &[u8; 32], bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for (block, part) in bytes.chunks(32).enumerate() {
        let mut message = FOR_THE_STREAM.to_vec();
        message.extend_from_slice(&(block as u64).to_be_bytes());
        let stream = hmac(locking, &message);
        for (at, byte) in part.iter().enumerate() {
            out.push(byte ^ stream[at]);
        }
    }
    out
}

fn same(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0_u8;
    for (a, b) in left.iter().zip(right) {
        difference |= a ^ b;
    }
    difference == 0
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
    }
    out
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || text.len() > MOST * 2 + 64 {
        return None;
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(text.len() / 2);
    for pair in bytes.chunks(2) {
        let high = char::from(pair[0]).to_digit(16)?;
        let low = char::from(pair[1]).to_digit(16)?;
        out.push(u8::try_from(high * 16 + low).ok()?);
    }
    Some(out)
}

#[must_use]
pub fn hmac(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut padded = [0_u8; BLOCK];
    if key.len() > BLOCK {
        padded[..32].copy_from_slice(&sha256(key));
    } else {
        padded[..key.len()].copy_from_slice(key);
    }
    let mut inner = Vec::with_capacity(BLOCK + message.len());
    let mut outer = Vec::with_capacity(BLOCK + 32);
    for byte in padded {
        inner.push(byte ^ 0x36);
        outer.push(byte ^ 0x5c);
    }
    inner.extend_from_slice(message);
    outer.extend_from_slice(&sha256(&inner));
    sha256(&outer)
}

const ROUNDS: [u32; 64] = [
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

#[expect(
    clippy::many_single_char_names,
    reason = "FIPS 180-4's own names for its eight working variables"
)]
#[must_use]
pub fn sha256(message: &[u8]) -> [u8; 32] {
    let mut hash: [u32; 8] = [
        0x6a09_e667,
        0xbb67_ae85,
        0x3c6e_f372,
        0xa54f_f53a,
        0x510e_527f,
        0x9b05_688c,
        0x1f83_d9ab,
        0x5be0_cd19,
    ];
    let mut padded = message.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    let bits = (message.len() as u64).wrapping_mul(8);
    padded.extend_from_slice(&bits.to_be_bytes());

    let mut schedule = [0_u32; 64];
    for block in padded.chunks_exact(64) {
        for (at, word) in block.chunks_exact(4).enumerate() {
            schedule[at] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for at in 16..64 {
            let a = schedule[at - 15];
            let b = schedule[at - 2];
            let s0 = a.rotate_right(7) ^ a.rotate_right(18) ^ (a >> 3);
            let s1 = b.rotate_right(17) ^ b.rotate_right(19) ^ (b >> 10);
            schedule[at] = schedule[at - 16]
                .wrapping_add(s0)
                .wrapping_add(schedule[at - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = hash;
        for at in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ ((!e) & g);
            let one = h
                .wrapping_add(s1)
                .wrapping_add(choose)
                .wrapping_add(ROUNDS[at])
                .wrapping_add(schedule[at]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let most = (a & b) ^ (a & c) ^ (b & c);
            let two = s0.wrapping_add(most);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(one);
            d = c;
            c = b;
            b = a;
            a = one.wrapping_add(two);
        }
        for (held, made) in hash.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *held = held.wrapping_add(made);
        }
    }
    let mut out = [0_u8; 32];
    for (at, word) in hash.iter().enumerate() {
        out[at * 4..at * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests;
