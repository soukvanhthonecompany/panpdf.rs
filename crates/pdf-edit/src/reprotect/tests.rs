use std::sync::Arc;

use pdf_bytes::{ByteStore, SourceId};
use pdf_security::{AccessLevel, Allowed, CipherMethod, PrintAllowance};

use super::{Wanted, rewrite};

fn store(bytes: Vec<u8>) -> ByteStore {
    ByteStore::new(SourceId::new(9), Arc::<[u8]>::from(bytes))
}

fn protected_r3() -> ByteStore {
    ByteStore::new(
        SourceId::new(2),
        &include_bytes!("../../tests/data/modifiable-r3.pdf")[..],
    )
}

const TITLE: &str = "คู่มือ";

fn plain() -> ByteStore {
    let mut title = vec![0xfe_u8, 0xff];
    for unit in TITLE.encode_utf16() {
        title.extend_from_slice(&unit.to_be_bytes());
    }
    let mut hex = String::new();
    for byte in &title {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }

    let content = b"BT 1 0 0 1 20 20 Tm (Hello) Tj ET";
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /MediaBox [0 0 300 300] /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << >> >>".to_vec(),
        {
            let mut object = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
            object.extend_from_slice(content);
            object.extend_from_slice(b"\nendstream");
            object
        },
        format!("<< /Title <{hex}> /Producer (Some other program) >>").into_bytes(),
    ];

    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        bytes.extend_from_slice(object);
        bytes.extend_from_slice(b"\nendobj\n");
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
            "trailer\n<< /Size {} /Root 1 0 R /Info 5 0 R \
             /ID [<0102030405060708090a0b0c0d0e0f10> <0102030405060708090a0b0c0d0e0f10>] >>\n\
             startxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    store(bytes)
}

fn wanted(user: &[u8], owner: &[u8], allowed: Allowed) -> Wanted {
    Wanted::Protected(Box::new(pdf_security::Wanted {
        user: user.to_vec(),
        owner: owner.to_vec(),
        allowed,
    }))
}

fn facts(bytes: &ByteStore, credential: &[u8]) -> crate::info::DocumentFacts {
    crate::info::document_facts(bytes, credential).expect("the document describes itself")
}

fn painted(bytes: &ByteStore, credential: &[u8]) -> Vec<String> {
    let program = pdf_content::load_page_program_with_password(
        bytes,
        0,
        pdf_content::PageContentLimits::default(),
        credential,
    )
    .expect("the page loads");
    program
        .streams
        .iter()
        .map(|stream| String::from_utf8_lossy(stream.bytes.as_bytes()).into_owned())
        .collect()
}

fn pages(bytes: &ByteStore, credential: &[u8]) -> usize {
    pdf_content::page_geometries_with_password(
        bytes,
        pdf_content::PageContentLimits::default(),
        credential,
    )
    .expect("the pages are read")
    .len()
}

#[test]
fn an_unprotected_document_can_be_given_a_password() {
    let source = plain();
    assert_eq!(facts(&source, b"").info.title, TITLE);
    let before = painted(&source, b"");

    let written = store(
        rewrite(
            &source,
            b"",
            &wanted(
                b"\xe0\xb9\x80\xe0\xb8\x9b\xe0\xb8\xb4\xe0\xb8\x94",
                b"master",
                Allowed::default(),
            ),
        )
        .expect("it is written again"),
    );

    let credential = b"\xe0\xb9\x80\xe0\xb8\x9b\xe0\xb8\xb4\xe0\xb8\x94";
    let after = facts(&written, credential);
    assert_eq!(after.info.title, TITLE, "the title survives the re-keying");
    assert_eq!(after.info.producer, "Some other program");
    assert_eq!(pages(&written, credential), 1);
    assert_eq!(
        painted(&written, credential),
        before,
        "the page paints what it painted"
    );

    let protection = after.protection.expect("it is protected now");
    assert_eq!(protection.revision, 6);
    assert_eq!(protection.stream_cipher, CipherMethod::Aes256);
    assert_eq!(protection.string_cipher, CipherMethod::Aes256);
    assert_eq!(protection.access, AccessLevel::User);

    assert!(
        crate::info::document_facts(&written, b"").is_err(),
        "and without the password there is nothing to read"
    );
    assert_eq!(
        crate::info::lock(&written, b""),
        crate::info::Lock::Refused,
        "which a window asks about rather than calls a failure"
    );
}

#[test]
fn a_password_can_be_taken_off_again() {
    let source = protected_r3();
    let before = facts(&source, b"view");
    let before_paint = painted(&source, b"view");
    assert!(before.protection.is_some(), "it starts protected");

    let opened = store(rewrite(&source, b"view", &Wanted::Open).expect("it is written again"));

    let after = facts(&opened, b"");
    assert!(after.protection.is_none(), "nothing protects it now");
    assert_eq!(after.info, before.info, "and it says what it said");
    assert_eq!(pages(&opened, b""), 1);
    assert_eq!(painted(&opened, b""), before_paint);
    assert_eq!(crate::info::lock(&opened, b""), crate::info::Lock::Open);
}

#[test]
fn a_document_can_be_moved_from_one_cipher_to_another() {
    let source = protected_r3();
    let before = painted(&source, b"view");
    assert_eq!(
        facts(&source, b"view")
            .protection
            .expect("protected")
            .stream_cipher,
        CipherMethod::Rc4,
        "the fixture is RC4, which is what makes this a change of cipher"
    );

    let written = store(
        rewrite(
            &source,
            b"view",
            &wanted(b"fresh", b"owner", Allowed::default()),
        )
        .expect("it is written again"),
    );

    assert_eq!(
        facts(&written, b"fresh")
            .protection
            .expect("protected")
            .stream_cipher,
        CipherMethod::Aes256
    );
    assert_eq!(painted(&written, b"fresh"), before);
    assert_eq!(
        crate::info::lock(&written, b"view"),
        crate::info::Lock::Refused,
        "the password it used to have opens nothing"
    );
}

#[test]
fn the_permissions_asked_for_are_the_permissions_written() {
    let allowed = Allowed {
        print: PrintAllowance::Degraded,
        modify: false,
        copy: false,
        annotate: false,
        fill_forms: true,
        assemble: false,
    };
    let written = store(
        rewrite(&plain(), b"", &wanted(b"", b"master", allowed)).expect("it is written again"),
    );

    let reader = facts(&written, b"").protection.expect("protected");
    assert_eq!(reader.access, AccessLevel::User);
    assert_eq!(reader.may.print, PrintAllowance::Degraded);
    assert!(!reader.may.modify);
    assert!(!reader.may.copy);
    assert!(!reader.may.annotate);
    assert!(reader.may.fill_forms);
    assert!(!reader.may.assemble);

    let owner = facts(&written, b"master").protection.expect("protected");
    assert_eq!(owner.access, AccessLevel::Owner);
    assert!(owner.may.modify, "an owner is allowed everything");
    assert_eq!(owner.may.print, PrintAllowance::Faithful);
}

#[test]
fn objects_kept_in_object_streams_stay_in_them() {
    let source = store(compressed_document());
    assert_eq!(pages(&source, b""), 1);

    let written = rewrite(&source, b"", &wanted(b"open", b"", Allowed::default()))
        .expect("it is written again");
    let text = String::from_utf8_lossy(&written).into_owned();
    assert!(
        text.contains("/Type /ObjStm"),
        "the object stream is carried across, not exploded"
    );
    assert!(
        text.contains("/Type /XRef"),
        "and the index is a stream, because a table cannot name an object inside one"
    );
    assert!(!text.contains("\nxref\n"), "so there is no table as well");

    let written = store(written);
    assert_eq!(pages(&written, b"open"), 1);
    assert_eq!(facts(&written, b"open").info.title, "In a stream");
}

#[test]
fn a_signed_document_is_refused() {
    let mut bytes = plain().as_bytes().to_vec();
    let signature =
        b"6 0 obj\n<< /Type /Sig /ByteRange [0 100 200 300] /Contents <00> >>\nendobj\n";
    let at = bytes
        .windows(5)
        .position(|window| window == b"xref\n")
        .expect("the table");
    bytes.splice(at..at, signature.iter().copied());
    let patched = store(bytes);
    let refusal = rewrite(&patched, b"", &Wanted::Open);
    assert!(refusal.is_err(), "a signed document is refused");
}

fn compressed_document() -> Vec<u8> {
    let inside: [(&str, &str); 4] = [
        ("1", "<< /Type /Catalog /Pages 2 0 R >>"),
        (
            "2",
            "<< /Type /Pages /MediaBox [0 0 300 300] /Kids [3 0 R] /Count 1 >>",
        ),
        (
            "3",
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << >> >>",
        ),
        ("5", "<< /Title (In a stream) >>"),
    ];
    let mut pairs = String::new();
    let mut bodies = String::new();
    for (number, body) in inside {
        {
            use std::fmt::Write as _;
            let _ = write!(pairs, "{number} {} ", bodies.len());
        }
        bodies.push_str(body);
        bodies.push(' ');
    }
    let first = pairs.len();
    let contents = format!("{pairs}{bodies}");

    let content = b"BT 1 0 0 1 20 20 Tm (Hello) Tj ET";
    let mut bytes = b"%PDF-1.5\n".to_vec();
    let mut at = std::collections::BTreeMap::new();

    at.insert(4_u32, bytes.len());
    bytes.extend_from_slice(
        format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).as_bytes(),
    );
    bytes.extend_from_slice(content);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");

    at.insert(6, bytes.len());
    bytes.extend_from_slice(
        format!(
            "6 0 obj\n<< /Type /ObjStm /N {} /First {first} /Length {} >>\nstream\n{contents}\nendstream\nendobj\n",
            inside.len(),
            contents.len()
        )
        .as_bytes(),
    );

    let xref = bytes.len();
    let mut rows: Vec<u8> = Vec::new();
    let row = |kind: u8, first: u32, second: u16, rows: &mut Vec<u8>| {
        rows.push(kind);
        rows.extend_from_slice(&first.to_be_bytes());
        rows.extend_from_slice(&second.to_be_bytes());
    };
    row(0, 0, 0xffff, &mut rows);
    for (index, (number, _)) in inside.iter().enumerate() {
        let number: u32 = number.parse().expect("a number");
        while rows.len() / 7 < number as usize {
            let missing = u32::try_from(rows.len() / 7).expect("small");
            match at.get(&missing) {
                Some(offset) => row(1, u32::try_from(*offset).expect("small"), 0, &mut rows),
                None => row(0, 0, 0xffff, &mut rows),
            }
        }
        row(2, 6, u16::try_from(index).expect("small"), &mut rows);
    }
    while rows.len() / 7 < 7 {
        let missing = u32::try_from(rows.len() / 7).expect("small");
        match at.get(&missing) {
            Some(offset) => row(1, u32::try_from(*offset).expect("small"), 0, &mut rows),
            None => row(0, 0, 0xffff, &mut rows),
        }
    }
    row(1, u32::try_from(xref).expect("small"), 0, &mut rows);

    bytes.extend_from_slice(
        format!(
            "7 0 obj\n<< /Type /XRef /W [1 4 2] /Size 8 /Root 1 0 R /Info 5 0 R /Length {} >>\nstream\n",
            rows.len()
        )
        .as_bytes(),
    );
    bytes.extend_from_slice(&rows);
    bytes.extend_from_slice(b"\nendstream\nendobj\n");
    bytes.extend_from_slice(format!("startxref\n{xref}\n%%EOF\n").as_bytes());
    bytes
}

#[test]
fn the_old_lock_is_not_left_lying_in_the_file() {
    let source = protected_r3();
    let standards = |bytes: &[u8]| {
        bytes
            .windows(b"/Filter /Standard".len())
            .filter(|window| *window == b"/Filter /Standard")
            .count()
    };
    assert_eq!(
        standards(source.as_bytes()),
        1,
        "the fixture states its handler once"
    );

    let opened = rewrite(&source, b"view", &Wanted::Open).expect("it is written again");
    assert_eq!(
        standards(&opened),
        0,
        "a document with its protection taken off carries no encryption dictionary"
    );

    let protected = rewrite(&source, b"view", &wanted(b"fresh", b"", Allowed::default()))
        .expect("it is written again");
    assert_eq!(
        standards(&protected),
        1,
        "and one given new protection carries exactly one: the new one"
    );
}
