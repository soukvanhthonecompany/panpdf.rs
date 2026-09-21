use pdf_bytes::{ByteStore, SourceId};

use super::{InfoEdit, Stamp, document_facts};
use crate::plan::Command;
use crate::spike_move_text::plan_command;

fn document(objects: &[&str], trailer: &str) -> ByteStore {
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
    ByteStore::new(SourceId::new(1), bytes)
}

const PAGES: [&str; 4] = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /MediaBox [0 0 300 300] /Kids [3 0 R] /Count 1 >>",
    "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << >> >>",
    "<< /Length 0 >>\nstream\n\nendstream",
];

fn described() -> ByteStore {
    let mut objects = PAGES.to_vec();
    objects.push(
        "<< /Title (Old title) /Producer (Some other program) /Trapped /False \
         /CreationDate (D:20200102030405+07'00') >>",
    );
    document(&objects, "/Info 5 0 R")
}

fn silent() -> ByteStore {
    document(&PAGES, "")
}

fn set(edit: InfoEdit) -> Command {
    Command::SetDocumentInfo { edit }
}

#[test]
fn a_document_says_what_it_was_told_to_say() {
    let source = described();
    let plan = plan_command(
        &source,
        &set(InfoEdit {
            title: Some("เอกสารของฉัน".to_owned()),
            author: Some("Phan".to_owned()),
            ..InfoEdit::default()
        }),
        b"",
    )
    .expect("the description is planned");
    let after = plan.commit(&source, b"").expect("the plan commits");
    let facts = document_facts(&after, b"").expect("the document describes itself");
    assert_eq!(facts.info.title, "เอกสารของฉัน");
    assert_eq!(facts.info.author, "Phan");
    assert_eq!(facts.info.producer, "Some other program");
    assert_eq!(
        facts.info.created,
        Some(Stamp {
            year: 2020,
            month: 1,
            day: 2,
            hour: 3,
            minute: 4,
            second: 5,
            offset_minutes: Some(420),
        })
    );
    assert_eq!(
        facts.info.other,
        vec![("Trapped".to_owned(), "/False".to_owned())],
        "an entry this editor does not know is kept and shown"
    );
    assert!(facts.protection.is_none());
}

#[test]
fn an_entry_set_to_nothing_is_taken_out() {
    let source = described();
    let plan = plan_command(
        &source,
        &set(InfoEdit {
            title: Some(String::new()),
            ..InfoEdit::default()
        }),
        b"",
    )
    .expect("the description is planned");
    let after = plan.commit(&source, b"").expect("the plan commits");
    let facts = document_facts(&after, b"").expect("the document describes itself");
    assert_eq!(facts.info.title, "");
    assert_eq!(facts.info.producer, "Some other program");
}

#[test]
fn a_document_that_says_nothing_can_be_given_a_title() {
    let source = silent();
    assert_eq!(
        document_facts(&source, b"")
            .expect("a silent document still describes itself")
            .info
            .title,
        "",
        "a document with no information dictionary says nothing, and is not a refusal"
    );
    let made = Stamp {
        year: 2026,
        month: 9,
        day: 19,
        hour: 11,
        minute: 52,
        second: 1,
        offset_minutes: Some(0),
    };
    let plan = plan_command(
        &source,
        &set(InfoEdit {
            title: Some("Started from nothing".to_owned()),
            created: Some(made),
            ..InfoEdit::default()
        }),
        b"",
    )
    .expect("the description is planned");
    let after = plan.commit(&source, b"").expect("the plan commits");
    assert_eq!(
        document_facts(&after, b"")
            .expect("the document describes itself")
            .info
            .title,
        "Started from nothing"
    );
    let written = String::from_utf8_lossy(after.as_bytes()).into_owned();
    let trailer = written
        .rfind("trailer")
        .expect("the new revision's trailer");
    assert!(
        written[trailer..].contains("/Info "),
        "the trailer of the revision that adds the dictionary is what names it"
    );
    assert_eq!(
        document_facts(&after, b"")
            .expect("the document describes itself")
            .info
            .created,
        Some(made)
    );
    assert!(
        written.contains("/CreationDate (D:20260919115201+00'00')"),
        "a date is written as the spec spells one"
    );
    assert!(
        written.contains("/Title <FEFF"),
        "a title is written as UTF-16, so it can be in any language"
    );
}

#[test]
fn an_edit_that_asks_for_nothing_is_refused() {
    let source = described();
    assert!(plan_command(&source, &set(InfoEdit::default()), b"").is_err());
}

#[test]
fn a_date_reads_and_writes_the_way_the_spec_spells_it() {
    let full = Stamp::read("D:20260919111000+07'00'").expect("a full date");
    assert_eq!(full.year, 2026);
    assert_eq!(full.month, 9);
    assert_eq!(full.day, 19);
    assert_eq!(full.hour, 11);
    assert_eq!(full.minute, 10);
    assert_eq!(full.offset_minutes, Some(420));
    assert_eq!(full.write(), "D:20260919111000+07'00'");
    assert_eq!(
        Stamp::read(&full.write()),
        Some(full),
        "a date this editor writes is a date it reads"
    );

    let year = Stamp::read("D:2019").expect("a date of a year alone");
    assert_eq!((year.year, year.month, year.day), (2019, 1, 1));
    let zoneless = Stamp::read("D:20240229120000").expect("a date with no zone");
    assert_eq!(zoneless.offset_minutes, None);
    assert_eq!(
        Stamp::read("D:20240229120000-0330")
            .expect("a zone written without its apostrophes")
            .offset_minutes,
        Some(-210)
    );

    assert_eq!(Stamp::read("yesterday"), None);
    assert_eq!(Stamp::read("D:20261332000000"), None);
    assert_eq!(Stamp::read("D:20260919251000"), None);
}

fn protected() -> ByteStore {
    ByteStore::new(
        SourceId::new(2),
        &include_bytes!("../../tests/data/modifiable-r3.pdf")[..],
    )
}

fn after(source: &ByteStore, edit: InfoEdit, credential: &[u8]) -> ByteStore {
    plan_command(source, &set(edit), credential)
        .expect("the description is planned")
        .commit(source, credential)
        .expect("the plan commits")
}

#[test]
fn an_entry_of_a_protected_document_is_not_encrypted_twice() {
    let source = protected();
    let once = after(
        &source,
        InfoEdit {
            producer: Some("PanPDF".to_owned()),
            title: Some("หนึ่ง".to_owned()),
            ..InfoEdit::default()
        },
        b"view",
    );
    assert!(
        document_facts(&once, b"view")
            .expect("it describes itself")
            .protection
            .is_some(),
        "this document is protected, which is what the test is about"
    );
    let twice = after(
        &once,
        InfoEdit {
            title: Some("สอง".to_owned()),
            ..InfoEdit::default()
        },
        b"view",
    );
    let facts = document_facts(&twice, b"view").expect("it describes itself");
    assert_eq!(facts.info.title, "สอง");
    assert_eq!(
        facts.info.producer, "PanPDF",
        "an entry carried across a protected document still reads as itself"
    );
}

#[test]
fn a_document_says_whether_the_password_offered_opens_it() {
    use super::{Lock, lock};

    assert_eq!(
        lock(&described(), b""),
        Lock::Open,
        "a document with no protection at all asks for nothing"
    );

    let protected = protected();
    assert_eq!(
        lock(&protected, b"view"),
        Lock::Open,
        "the user password opens it"
    );
    assert_eq!(
        lock(&protected, b"master"),
        Lock::Open,
        "so does the owner password"
    );
    assert_eq!(
        lock(&protected, b""),
        Lock::Refused,
        "and the empty password, which every open is tried with first, does not"
    );
    assert_eq!(lock(&protected, b"View"), Lock::Refused, "nor a near miss");

    let rubbish = ByteStore::new(SourceId::new(3), b"not a document".to_vec());
    assert_eq!(lock(&rubbish, b""), Lock::Open);
}
