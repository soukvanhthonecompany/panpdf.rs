use super::{Certificate, Moment};

const ROOT: &[u8] = include_bytes!("../../tests/data/chain-root.der");
const ISSUING: &[u8] = include_bytes!("../../tests/data/chain-issuing.der");
const SIGNER: &[u8] = include_bytes!("../../tests/data/chain-signer.der");

#[test]
fn a_certificate_says_whose_key_it_is_and_who_vouched_for_it() {
    let signer = Certificate::read(SIGNER).expect("it reads");
    assert_eq!(signer.subject.common, "Phan Somchai");
    assert_eq!(
        signer.subject.full, "E=phan@example.la, CN=Phan Somchai, OU=Signing, O=PanPDF Test, C=LA",
        "the whole name, written outwards from the person as RFC 4514 does"
    );
    assert_eq!(signer.issuer.common, "PanPDF Test Issuing");
    assert_eq!(signer.serial, &[3]);
    assert!(
        !signer.is_authority,
        "a person's certificate may not sign other certificates"
    );
    assert!(!signer.is_self_issued());

    let root = Certificate::read(ROOT).expect("it reads");
    assert!(root.is_self_issued(), "a root issues itself");
    assert!(root.is_authority);
}

#[test]
fn each_certificate_is_signed_by_the_one_above_it() {
    let signer = Certificate::read(SIGNER).expect("read");
    let issuing = Certificate::read(ISSUING).expect("read");
    let root = Certificate::read(ROOT).expect("read");

    assert!(signer.is_signed_by(&issuing));
    assert!(issuing.is_signed_by(&root));
    assert!(root.is_signed_by(&root), "a root signs itself");

    assert!(
        !signer.is_signed_by(&root),
        "the root did not sign the person's certificate, whatever else is true"
    );
    assert!(!issuing.is_signed_by(&signer));
}

#[test]
fn a_changed_certificate_is_no_longer_signed_by_its_issuer() {
    let issuing = Certificate::read(ISSUING).expect("read");
    let mut changed = SIGNER.to_vec();
    let place = changed
        .windows(3)
        .position(|window| window == [0x02, 0x01, 0x03])
        .expect("the serial is in there");
    changed[place + 2] = 0x04;
    let signer = Certificate::read(&changed).expect("it still reads");
    assert_eq!(signer.serial, &[4]);
    assert!(!signer.is_signed_by(&issuing));
}

#[test]
fn both_ways_of_writing_a_moment_read_as_the_same_kind_of_moment() {
    let signer = Certificate::read(SIGNER).expect("read");
    assert!(
        signer.not_before < signer.not_after,
        "a certificate begins before it ends"
    );
    assert!(
        signer.not_after.year >= 2031,
        "the five years it was made for"
    );
    assert!(
        signer.covers(signer.not_before),
        "the first moment is inside"
    );
    assert!(signer.covers(signer.not_after), "and so is the last");
    let before = Moment {
        year: signer.not_before.year - 1,
        ..signer.not_before
    };
    assert!(!signer.covers(before));
}

#[test]
fn seconds_since_nineteen_seventy_become_the_date_they_are() {
    assert_eq!(
        Moment::from_epoch(0),
        Moment {
            year: 1970,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0
        }
    );
    assert_eq!(
        Moment::from_epoch(1_000_000_000),
        Moment {
            year: 2001,
            month: 9,
            day: 9,
            hour: 1,
            minute: 46,
            second: 40
        }
    );
    assert_eq!(
        Moment::from_epoch(1_767_225_600),
        Moment {
            year: 2026,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0
        },
        "a leap-year boundary, where a wrong day count shows up"
    );
    assert_eq!(
        Moment::from_epoch(1_709_164_800),
        Moment {
            year: 2024,
            month: 2,
            day: 29,
            hour: 0,
            minute: 0,
            second: 0
        },
        "and a leap day itself"
    );
    assert!(Moment::now().year >= 2026);
}

#[test]
fn something_that_is_not_a_certificate_is_refused() {
    assert!(Certificate::read(&[]).is_none());
    assert!(Certificate::read(b"%PDF-1.7").is_none());
    assert!(
        Certificate::read(&SIGNER[..40]).is_none(),
        "half a certificate is not one"
    );
}
