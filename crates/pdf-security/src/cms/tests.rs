use super::{Integrity, Trust, check};

const SIGNATURE: &[u8] = include_bytes!("../../tests/data/detached-signature.der");
const CONTENT: &[u8] = include_bytes!("../../tests/data/detached-content.bin");

#[test]
fn a_signature_openssl_made_is_read_and_checked() {
    let checked = check(SIGNATURE, &[CONTENT]);
    assert_eq!(checked.integrity, Integrity::Intact);
    let signer = checked.signer.expect("it names who signed");
    assert_eq!(signer.common, "Phan Somchai");
    assert_eq!(
        checked.issuer.expect("and who issued that").common,
        "PanPDF Test Issuing"
    );
    assert_eq!(checked.hash, Some("SHA-256"));
    assert!(checked.hash_is_sound);
    assert_eq!(checked.key_bits, Some(2048));
    assert!(
        checked.certificate_is_current,
        "the certificate was made for five years from the day it was made"
    );
    assert!(
        checked.signed_at.is_some(),
        "the moment inside what was signed, which `/M` in the document is not"
    );
}

#[test]
fn a_chain_that_ends_outside_this_computers_list_is_said_to() {
    let checked = check(SIGNATURE, &[CONTENT]);
    assert_eq!(
        checked.trust,
        Trust::UnknownAuthority {
            top: "PanPDF Test Root".to_owned()
        },
        "every link was checked and the top of it is a stranger"
    );
    assert_eq!(checked.links, 2, "the person, the issuer, and the root");
}

#[test]
fn a_changed_document_is_not_what_was_signed() {
    let mut changed = CONTENT.to_vec();
    changed[0] ^= 0x01;
    let checked = check(SIGNATURE, &[&changed]);
    assert_eq!(checked.integrity, Integrity::ContentChanged);
    assert_eq!(
        checked.signer.expect("who signed is still known").common,
        "Phan Somchai",
        "a document that changed still says whose signature it broke"
    );
}

#[test]
fn the_pieces_are_digested_as_one_message() {
    for cut in [0, 1, 17, 63, 64, CONTENT.len()] {
        let checked = check(SIGNATURE, &[&CONTENT[..cut], &CONTENT[cut..]]);
        assert_eq!(checked.integrity, Integrity::Intact, "cut at {cut}");
    }
    assert_eq!(
        check(SIGNATURE, &[CONTENT, b"and a little more"]).integrity,
        Integrity::ContentChanged,
        "more than was signed is not what was signed"
    );
}

#[test]
fn an_altered_signature_is_told_apart_from_an_altered_document() {
    let mut altered = SIGNATURE.to_vec();
    let last = altered.len() - 1;
    altered[last] ^= 0x01;
    assert_eq!(
        check(&altered, &[CONTENT]).integrity,
        Integrity::SignatureWrong
    );
}

#[test]
fn changing_what_was_signed_about_the_signature_breaks_it() {
    let checked = check(SIGNATURE, &[CONTENT]);
    let when = checked.signed_at.expect("a moment is stated");
    let written = when.write();
    let mut altered = SIGNATURE.to_vec();
    let named = altered
        .windows(super::oid::SIGNING_TIME.len())
        .position(|window| window == super::oid::SIGNING_TIME)
        .expect("the attribute is in there");
    let digits = altered[named..]
        .iter()
        .position(u8::is_ascii_digit)
        .expect("written as digits")
        + named;
    altered[digits + 5] = if altered[digits + 5] == b'1' {
        b'2'
    } else {
        b'1'
    };
    let after = check(&altered, &[CONTENT]);
    assert_ne!(after.signed_at.map(super::Moment::write), Some(written));
    assert_eq!(
        after.integrity,
        Integrity::SignatureWrong,
        "the moment is inside what was signed, so moving it breaks the signature"
    );
}

#[test]
fn what_cannot_be_read_is_not_reported_as_a_failure() {
    for rubbish in [
        &[][..],
        &[0x00, 0x00][..],
        b"not a signature at all",
        &SIGNATURE[..20],
    ] {
        assert!(
            matches!(
                check(rubbish, &[CONTENT]).integrity,
                Integrity::CannotCheck(_)
            ),
            "{rubbish:?} is not a signature and is not a broken one either"
        );
    }
}
