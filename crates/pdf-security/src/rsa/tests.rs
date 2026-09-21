use super::PublicKey;
use crate::digest::Hash;

fn octets(hex: &str) -> Vec<u8> {
    let clean: Vec<u8> = hex
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    clean
        .chunks_exact(2)
        .map(|pair| {
            u8::from_str_radix(std::str::from_utf8(pair).expect("hexadecimal"), 16)
                .expect("hexadecimal")
        })
        .collect()
}

const MESSAGE: &[u8] = b"a message that was signed";

#[test]
fn a_signature_openssl_made_verifies() {
    let key = PublicKey::read(&octets(KEY)).expect("the key reads");
    assert_eq!(key.width_in_bits(), 2048);
    let digest = Hash::Sha256.of(MESSAGE);
    assert!(key.verifies(Hash::Sha256, &digest, &octets(SIGNATURE)));
}

#[test]
fn a_changed_message_is_not_what_was_signed() {
    let key = PublicKey::read(&octets(KEY)).expect("the key reads");
    let mut changed = MESSAGE.to_vec();
    changed[0] ^= 0x01;
    let digest = Hash::Sha256.of(&changed);
    assert!(!key.verifies(Hash::Sha256, &digest, &octets(SIGNATURE)));
}

#[test]
fn a_changed_signature_does_not_verify() {
    let key = PublicKey::read(&octets(KEY)).expect("the key reads");
    let digest = Hash::Sha256.of(MESSAGE);
    for place in [0, 1, 128, 255] {
        let mut signature = octets(SIGNATURE);
        signature[place] ^= 0x01;
        assert!(
            !key.verifies(Hash::Sha256, &digest, &signature),
            "changed at {place}"
        );
    }
}

#[test]
fn the_same_digest_under_another_hash_does_not_verify() {
    let key = PublicKey::read(&octets(KEY)).expect("the key reads");
    let digest = Hash::Sha256.of(MESSAGE);
    assert!(!key.verifies(Hash::Sha1, &digest, &octets(SIGNATURE)));
}

#[test]
fn a_signature_of_the_wrong_width_is_refused() {
    let key = PublicKey::read(&octets(KEY)).expect("the key reads");
    let digest = Hash::Sha256.of(MESSAGE);
    let mut short = octets(SIGNATURE);
    short.pop();
    assert!(!key.verifies(Hash::Sha256, &digest, &short));
    let mut long = octets(SIGNATURE);
    long.push(0);
    assert!(!key.verifies(Hash::Sha256, &digest, &long));
}

#[test]
fn a_block_with_rubbish_after_the_digest_is_not_the_block() {
    let digest = Hash::Sha256.of(MESSAGE);
    let honest = super::block(Hash::Sha256, &digest, 256, true).expect("a block is built");
    assert_eq!(honest.len(), 256);
    assert_eq!(&honest[..2], &[0x00, 0x01]);
    assert!(
        honest.ends_with(&digest),
        "the honest block ends with the digest and nothing follows it"
    );

    let mut forged = honest.clone();
    forged.copy_within(12.., 2);
    forged[246..].copy_from_slice(&[0x41; 10]);
    assert!(
        forged
            .windows(digest.len())
            .any(|window| window == digest.as_slice()),
        "the digest is still in there, which is what a lenient verifier would find"
    );
    assert_ne!(forged, honest, "and the comparison this makes rejects it");
}

#[test]
fn a_key_too_small_for_the_hash_has_no_block() {
    let digest = Hash::Sha512.of(MESSAGE);
    assert!(super::block(Hash::Sha512, &digest, 64, true).is_none());
    assert!(super::block(Hash::Sha512, &digest, 128, true).is_some());
}

#[test]
fn something_that_is_not_a_key_does_not_read_as_one() {
    assert!(PublicKey::read(&[]).is_none());
    assert!(PublicKey::read(&[0x30, 0x03, 0x02, 0x01, 0x07]).is_none());
    assert!(
        PublicKey::read(&octets("3006020102020101")).is_none(),
        "an even modulus is not an RSA modulus"
    );
}

const KEY: &str = "\
     3082010a0282010100d8ab2dbb3a8dd73b07c83528d2163fe931a41f17085a53bcbefd\
     a85dd9f04bb17bc60304387f7d9ea17b907c6f56c58ec686d00d53d9ea435415ed263d\
     65bf54d7170ac7862dac3cb561881129ec4315f4716dfb074fcb7350846ce3c71ee992\
     a28360faf93203777368a00d2eaaf0837446cb094dd6624b2ec3ceb5e50f6a238bb0b4\
     1796883d118e1beaafb79eed72add8b30fe0a48be09cbea37388777633354956a00620\
     92604f096c918c203607d35710d562c7619bfb72b05b5f725bdbd0bdf50ed22dd5be29\
     a6add197e58011b28d2f7cdeab4c52de0b4f3cd998ed0d3a38edb90cc4fd0e7d240591\
     20c70136ec36226d66da3333a942a97393c5e2650203010001
    ";

const SIGNATURE: &str = "\
     c43d1691cfa93e12df2e984dff83e722bc1ecdec9c567d395a321de9d36c280f4d28da\
     dc1353a7cc33c05cc47b4b5b6f328f970811966302e28024934a0f937d1e143606849d\
     ade11e5fe69b4b12972720183f37aea080fd794789e4c0d41c41e6822edb34309b6a95\
     34d65a2fc10e2d6f2551e88214fdc2f52ac361149c67acc7f78b3536a9e7893a0c3717\
     80995d4be7ad106b6f082f59ef7f7dd047969364f6875fbdf4613d609ff09dfb53241b\
     a58618a8d70adee01490b7e5d0d0eae5619c6b9cfc06ee6546e9a6ecf99b5928628eb9\
     4dfdce5157df674802c3c123f502d7dadfb734456657744386b20a900ee970320c1199\
     2214838a7c4ae442bd2691
    ";
