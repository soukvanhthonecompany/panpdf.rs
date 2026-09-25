use std::cell::RefCell;

use aes::cipher::{Array, BlockCipherEncrypt, KeyInit};
use zeroize::{Zeroize, Zeroizing};

use crate::{SecurityError, SecurityErrorKind};

pub const SEED_BYTES: usize = 48;

thread_local! {
    static GENERATOR: RefCell<Option<CtrDrbg>> = const { RefCell::new(None) };
}

pub fn seed_random(seed: &[u8]) -> Result<(), SecurityError> {
    let Some(first) = seed.get(..SEED_BYTES) else {
        return Err(SecurityError::new(SecurityErrorKind::WeakRandomSeed));
    };
    if first.iter().all(|&byte| byte == first[0]) {
        return Err(SecurityError::new(SecurityErrorKind::WeakRandomSeed));
    }
    GENERATOR.with_borrow_mut(|generator| {
        for chunk in seed.chunks(SEED_BYTES) {
            let mut material = Zeroizing::new([0_u8; SEED_BYTES]);
            material[..chunk.len()].copy_from_slice(chunk);
            match generator {
                Some(drbg) => drbg.reseed(&material),
                None => *generator = Some(CtrDrbg::instantiate(&material)),
            }
        }
    });
    Ok(())
}

pub(crate) fn draw(out: &mut [u8]) -> bool {
    GENERATOR.with_borrow_mut(|generator| match generator {
        Some(drbg) => {
            drbg.generate(out);
            true
        }
        None => false,
    })
}

struct CtrDrbg {
    key: [u8; 32],
    v: [u8; 16],
}

impl Drop for CtrDrbg {
    fn drop(&mut self) {
        self.key.zeroize();
        self.v.zeroize();
    }
}

impl CtrDrbg {
    fn instantiate(seed: &[u8; SEED_BYTES]) -> Self {
        let mut drbg = Self {
            key: [0; 32],
            v: [0; 16],
        };
        drbg.update(seed);
        drbg
    }

    fn reseed(&mut self, seed: &[u8; SEED_BYTES]) {
        self.update(seed);
    }

    fn generate(&mut self, out: &mut [u8]) {
        self.keystream(out);
        self.update(&[0; SEED_BYTES]);
    }

    fn update(&mut self, provided: &[u8; SEED_BYTES]) {
        let mut temp = Zeroizing::new([0_u8; SEED_BYTES]);
        self.keystream(&mut temp[..]);
        for (byte, with) in temp.iter_mut().zip(provided) {
            *byte ^= with;
        }
        self.key.copy_from_slice(&temp[..32]);
        self.v.copy_from_slice(&temp[32..]);
    }

    fn keystream(&mut self, out: &mut [u8]) {
        let cipher = aes::Aes256::new(&Array::from(self.key));
        for chunk in out.chunks_mut(16) {
            self.v = u128::from_be_bytes(self.v).wrapping_add(1).to_be_bytes();
            let mut block = Array::from(self.v);
            cipher.encrypt_block(&mut block);
            chunk.copy_from_slice(&block[..chunk.len()]);
            block.zeroize();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CtrDrbg, SEED_BYTES, draw, seed_random};
    use crate::SecurityErrorKind;

    fn hex(bytes: &[u8]) -> String {
        use std::fmt::Write;
        bytes.iter().fold(String::new(), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
    }

    #[test]
    fn the_generator_gives_openssls_known_answer() {
        let entropy: Vec<u8> = (0..48).collect();
        let mut drbg = CtrDrbg::instantiate(&entropy.try_into().unwrap());
        let mut first = [0_u8; 32];
        drbg.generate(&mut first);
        assert_eq!(
            hex(&first),
            "061550234d158c5ec95595fe04ef7a25767f2e24cc2bc479d09d86dc9abcfde7"
        );
        let mut second = [0_u8; 16];
        drbg.generate(&mut second);
        assert_eq!(hex(&second), "1a9fbcbc8da36dff2abe203296170fdb");
        let reseed: Vec<u8> = (0x80..0xb0).collect();
        drbg.reseed(&reseed.try_into().unwrap());
        let mut third = [0_u8; 20];
        drbg.generate(&mut third);
        assert_eq!(hex(&third), "6c018598223a5b0b56aac24a58eec90c46cfd33a");
    }

    fn fresh<T: Send + 'static>(case: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::spawn(case).join().unwrap()
    }

    #[test]
    fn nothing_is_drawn_before_a_seed() {
        fresh(|| {
            let mut out = [7_u8; 16];
            assert!(!draw(&mut out));
            assert_eq!(out, [7; 16]);
        });
    }

    #[test]
    fn a_short_or_unfilled_seed_is_refused_and_seeds_nothing() {
        fresh(|| {
            for seed in [vec![0x5a_u8; 47], vec![0; 64], vec![0xff; SEED_BYTES]] {
                let refused = seed_random(&seed).unwrap_err();
                assert_eq!(refused.kind(), SecurityErrorKind::WeakRandomSeed);
            }
            assert!(!draw(&mut [0; 16]));
        });
    }

    #[test]
    fn the_same_seed_twice_does_not_give_the_same_bytes_twice() {
        let seed: Vec<u8> = (0..64).collect();
        let once = {
            let seed = seed.clone();
            fresh(move || {
                seed_random(&seed).unwrap();
                let mut out = [0_u8; 32];
                assert!(draw(&mut out));
                out
            })
        };
        let twice = fresh(move || {
            seed_random(&seed).unwrap();
            seed_random(&seed).unwrap();
            let mut out = [0_u8; 32];
            assert!(draw(&mut out));
            out
        });
        assert_ne!(once, twice);
        assert_ne!(once, [0; 32]);
    }
}
