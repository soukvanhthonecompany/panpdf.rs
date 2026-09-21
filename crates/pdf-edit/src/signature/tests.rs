use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};

use super::{Covers, Integrity, Kind, Trust, signatures};

fn document(objects: &[String], trailer: &str) -> ByteStore {
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R {trailer} >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    ByteStore::new(SourceId::new(11), Arc::<[u8]>::from(bytes))
}

fn with_form(catalog_extra: &str, fields: &str, rest: &[String]) -> Vec<String> {
    let mut objects = vec![
        format!("<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R {catalog_extra} >>"),
        "<< /Type /Pages /MediaBox [0 0 300 300] /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << >> >>".to_owned(),
        "<< /Length 0 >>\nstream\n\nendstream".to_owned(),
        format!("<< /Fields [{fields}] /SigFlags 3 >>"),
    ];
    objects.extend_from_slice(rest);
    objects
}

fn signature_body(length: usize, extra: &str) -> String {
    let (first, hole) = (200, 600);
    let second = length.saturating_sub(first + hole);
    format!(
        "<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached \
         /ByteRange [0 {first} {} {second}] /Contents <00> {extra} >>",
        first + hole
    )
}

fn signed(extra: &str, catalog_extra: &str) -> ByteStore {
    let mut length = 0;
    for _ in 0..4 {
        let objects = with_form(
            catalog_extra,
            "6 0 R",
            &[
                "<< /Type /Annot /Subtype /Widget /FT /Sig /T (Signature1) /V 7 0 R \
                 /Rect [0 0 0 0] /P 3 0 R >>"
                    .to_owned(),
                signature_body(length, extra),
            ],
        );
        let built = document(&objects, "");
        let measured = built.as_bytes().len();
        if measured == length {
            return built;
        }
        length = measured;
    }
    panic!("the fixture's length did not settle");
}

#[test]
fn a_document_says_who_signed_it_and_what_the_signature_covers() {
    let source = signed(
        "/Name <FEFF00500068006100 6E> /M (D:20260919103000+07'00') \
         /Reason (I agree) /Location (Vientiane)",
        "",
    );
    let found = signatures(&source, b"").expect("it reads its signatures");
    assert_eq!(found.len(), 1);
    let one = &found[0];
    assert_eq!(one.field, "Signature1");
    assert_eq!(
        one.name, "Phan",
        "the name comes back as text, not as bytes"
    );
    assert_eq!(one.reason, "I agree");
    assert_eq!(one.location, "Vientiane");
    assert_eq!(one.encoding, "adbe.pkcs7.detached");
    assert_eq!(one.kind, Kind::Approval);
    assert_eq!(
        one.signed.expect("it states a moment").write(),
        "D:20260919103000+07'00'"
    );
    assert_eq!(
        one.covers,
        Covers::WholeDocument,
        "nothing has happened to the file since"
    );
}

#[test]
fn a_revision_added_after_signing_is_not_covered_by_it() {
    let source = signed("/Name (Phan)", "");
    let before = source.as_bytes().len();
    assert_eq!(
        signatures(&source, b"").expect("read")[0].covers,
        Covers::WholeDocument
    );

    let plan = crate::spike_move_text::plan_command(
        &source,
        &crate::plan::Command::SetDocumentInfo {
            edit: crate::info::InfoEdit {
                title: Some("Changed afterwards".to_owned()),
                ..crate::info::InfoEdit::default()
            },
        },
        b"",
    )
    .expect("the edit is planned");
    let after = plan.commit(&source, b"").expect("the plan commits");
    assert!(
        after.as_bytes().len() > before,
        "the file grew, which is what an appended revision does"
    );

    let found = signatures(&after, b"").expect("it still reads its signature");
    assert_eq!(
        found[0].covers,
        Covers::UpTo {
            signed_through: before as u64,
            of: after.as_bytes().len() as u64,
        },
        "the signature speaks for the file as it was, and says nothing about the rest"
    );
    assert_eq!(
        found[0].name, "Phan",
        "and it is still the same signature that says so"
    );
}

#[test]
fn the_catalogue_says_which_signature_certified_the_document() {
    let source = signed("/Name (Phan)", "/Perms << /DocMDP 7 0 R >>");
    let found = signatures(&source, b"").expect("read");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].kind, Kind::Certification);
    assert_eq!(found[0].field, "Signature1", "it is still on its field");
}

#[test]
fn a_signature_no_field_points_at_is_still_found() {
    let mut length = 0;
    let source = loop {
        let objects = with_form(
            "/Perms << /UR3 6 0 R >>",
            "",
            &[signature_body(length, "/Name (ARE Production)")],
        );
        let built = document(&objects, "");
        if built.as_bytes().len() == length {
            break built;
        }
        length = built.as_bytes().len();
    };

    let found = signatures(&source, b"").expect("read");
    assert_eq!(
        found.len(),
        1,
        "the document is signed, and says so nowhere else"
    );
    assert_eq!(found[0].kind, Kind::UsageRights);
    assert_eq!(found[0].name, "ARE Production");
    assert_eq!(
        found[0].field, "",
        "it sits on no field, and does not pretend to"
    );
    assert_eq!(found[0].covers, Covers::WholeDocument);
}

#[test]
fn an_empty_signature_field_is_not_a_signature() {
    let objects = with_form(
        "",
        "6 0 R",
        &[
            "<< /Type /Annot /Subtype /Widget /FT /Sig /T (Signature1) /Rect [0 0 0 0] >>"
                .to_owned(),
        ],
    );
    let source = document(&objects, "");
    assert!(
        signatures(&source, b"")
            .expect("a document with an empty field still reads")
            .is_empty()
    );
}

#[test]
fn a_byte_range_that_says_nothing_is_reported_as_saying_nothing() {
    let objects = with_form(
        "",
        "6 0 R",
        &[
            "<< /Type /Annot /Subtype /Widget /FT /Sig /T (Signature1) /V 7 0 R \
             /Rect [0 0 0 0] >>"
                .to_owned(),
            "<< /Type /Sig /ByteRange [0 200] /Contents <00> /Name (Phan) >>".to_owned(),
        ],
    );
    let source = document(&objects, "");
    let found = signatures(&source, b"").expect("read");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].covers, Covers::Unstated);
    assert_eq!(found[0].name, "Phan", "the rest of it still reads");
}

#[test]
fn a_document_with_no_form_has_no_signatures() {
    let source = document(
        &[
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /MediaBox [0 0 300 300] /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /Resources << >> >>".to_owned(),
        ],
        "",
    );
    assert!(signatures(&source, b"").expect("it reads").is_empty());
}

fn really_signed() -> ByteStore {
    ByteStore::new(
        SourceId::new(21),
        &include_bytes!("../../tests/data/signed-by-the-test-chain.pdf")[..],
    )
}

#[test]
fn a_real_signature_is_opened_and_checked_against_the_document() {
    let found = signatures(&really_signed(), b"").expect("it reads");
    assert_eq!(found.len(), 1);
    let checked = found[0].checked.clone().expect("there is one to open");
    assert_eq!(checked.integrity, Integrity::Intact);
    assert_eq!(
        checked.signer.expect("who signed").common,
        "Phan Somchai",
        "the name out of the certificate, not the `/Name` beside it"
    );
    assert_eq!(
        checked.trust,
        Trust::UnknownAuthority {
            top: "PanPDF Test Root".to_owned()
        },
        "the chain checks out and ends at a root this computer does not hold"
    );
    assert_eq!(checked.hash, Some("SHA-256"));
    assert!(checked.hash_is_sound);
    assert_eq!(checked.key_bits, Some(2048));
    assert!(checked.certificate_is_current);
    assert_eq!(found[0].covers, Covers::WholeDocument);
}

#[test]
fn an_edit_after_signing_leaves_the_signature_intact_and_covering_less() {
    let source = really_signed();
    let before = source.as_bytes().len();
    let plan = crate::spike_move_text::plan_command(
        &source,
        &crate::plan::Command::SetDocumentInfo {
            edit: crate::info::InfoEdit {
                title: Some("Added after it was signed".to_owned()),
                ..crate::info::InfoEdit::default()
            },
        },
        b"",
    )
    .expect("the edit is planned");
    let after = plan.commit(&source, b"").expect("the plan commits");

    let found = signatures(&after, b"").expect("it still reads");
    let checked = found[0].checked.clone().expect("still one to open");
    assert_eq!(
        checked.integrity,
        Integrity::Intact,
        "appending to a signed document does not break its signature"
    );
    assert_eq!(
        found[0].covers,
        Covers::UpTo {
            signed_through: before as u64,
            of: after.as_bytes().len() as u64,
        },
        "and the signature says nothing about what was appended"
    );
}

#[test]
fn a_byte_changed_inside_what_was_signed_breaks_the_signature() {
    let mut bytes = include_bytes!("../../tests/data/signed-by-the-test-chain.pdf").to_vec();
    let place = bytes
        .windows(9)
        .position(|window| window == b"(I agree)")
        .expect("the reason is in there, inside the signed range");
    bytes[place + 7] = b'd';
    let source = ByteStore::new(SourceId::new(22), bytes);

    let found = signatures(&source, b"").expect("it still reads");
    assert_eq!(found[0].reason, "I agred", "the document now says this");
    let checked = found[0].checked.clone().expect("still one to open");
    assert_eq!(
        checked.integrity,
        Integrity::ContentChanged,
        "and the signature says it is not what was signed"
    );
    assert_eq!(
        checked.signer.expect("who signed is still known").common,
        "Phan Somchai",
        "a broken signature still says whose it was"
    );
}

#[test]
fn checking_a_signature_does_not_change_what_kind_it_is() {
    let found = signatures(&really_signed(), b"").expect("read");
    assert_eq!(found[0].kind, Kind::Certification);
    assert_eq!(found[0].field, "Signature1");
}

#[test]
fn a_signature_in_a_protected_document_is_read_as_written() {
    let source = ByteStore::new(
        SourceId::new(23),
        &include_bytes!("../../tests/data/signed-and-protected-r3.pdf")[..],
    );
    let found = signatures(&source, b"").expect("it opens with no password");
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0].name, "Phan Somchai",
        "the name was encrypted, and reading it means decrypting it"
    );
    assert_eq!(found[0].reason, "I agree");
    let checked = found[0].checked.clone().expect("there is one to open");
    assert_eq!(
        checked.integrity,
        Integrity::Intact,
        "and the signature was not encrypted, and reading it means not decrypting it"
    );

    let reader = crate::form::Reader::open(&source, b"").expect("it opens");
    let node = crate::form::signature_nodes(&reader)
        .into_iter()
        .next()
        .expect("the field is there")
        .1;
    let value = reader.entry(&node, b"/V").expect("it is signed");
    let contents = reader
        .entry(&value, b"/Contents")
        .expect("the signature is there");
    let written = crate::form::Reader::written_bytes(&contents).expect("as written");
    assert_eq!(
        &written[..2],
        &[0x30, 0x82],
        "as written, it is the structure a signature is"
    );
    assert_ne!(
        reader.bytes(&contents).as_deref(),
        Some(&written[..]),
        "read the way every other string is read, it is something else"
    );
}
