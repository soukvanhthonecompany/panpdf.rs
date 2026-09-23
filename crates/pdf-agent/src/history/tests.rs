use std::collections::BTreeSet;

use super::{Chat, chat_about, name_for, newest_first, read, title_of, write};
use crate::connect::{Attachment, AttachmentKind, Provider, Raw, ToolCall, ToolResult, Turn};
use crate::json::Json;

fn a_chat() -> Chat {
    Chat {
        id: "000000012345".to_owned(),
        title: "Summarise page 2".to_owned(),
        changed: 12_345,
        model: "qwen/qwen3.8-27b".to_owned(),
        documents: vec!["report.pdf".to_owned(), "notes.pdf".to_owned()],
        places: vec![
            "/home/someone/report.pdf".to_owned(),
            "/home/someone/notes.pdf".to_owned(),
        ],
        turns: vec![
            Turn::person_with(
                "Summarise page 2",
                vec![
                    Attachment::text("notes.pdf", "the words of the file"),
                    Attachment::image("cover.png", "image/png", vec![1, 2, 3]),
                ],
            ),
            Turn::Model {
                text: "Let me read it.".to_owned(),
                calls: vec![ToolCall {
                    id: "call_1".to_owned(),
                    name: "read_text".to_owned(),
                    arguments: Json::object([("first", Json::count(2))]),
                }],
                raw: Some(Raw {
                    provider: Provider::Gemini,
                    items: vec![Json::object([("thought_signature", Json::text("abc"))])],
                }),
            },
            Turn::Results {
                results: vec![ToolResult::said("call_1", "Two paragraphs.")],
            },
            Turn::model("Page 2 is about bonds."),
        ],
    }
}

#[test]
fn a_conversation_written_reads_back_as_itself() {
    let chat = a_chat();
    let back = read(&write(&chat)).expect("it reads");
    assert_eq!(back.id, chat.id);
    assert_eq!(back.title, chat.title);
    assert_eq!(back.changed, chat.changed);
    assert_eq!(back.model, chat.model);
    assert_eq!(back.documents, chat.documents);
    assert_eq!(back.places, chat.places);
    assert_eq!(back.turns.len(), chat.turns.len());
    assert_eq!(back.turns[1], chat.turns[1]);
    assert_eq!(back.turns[2], chat.turns[2]);
    assert_eq!(back.turns[3], chat.turns[3]);
}

#[test]
fn an_attachment_keeps_its_name_and_not_its_bytes() {
    let back = read(&write(&a_chat())).expect("it reads");
    let Turn::Person { attachments, .. } = &back.turns[0] else {
        panic!("the first turn is the person's");
    };
    assert_eq!(attachments.len(), 2);
    assert_eq!(attachments[0].name, "notes.pdf");
    assert_eq!(attachments[0].kind, AttachmentKind::Text);
    assert!(attachments[0].bytes.is_empty());
    assert_eq!(attachments[1].name, "cover.png");
    assert_eq!(
        attachments[1].kind,
        AttachmentKind::Image {
            media_type: "image/png".to_owned()
        }
    );
    assert!(attachments[1].bytes.is_empty());
}

#[test]
fn what_cannot_be_read_faithfully_is_not_read() {
    assert_eq!(read(""), None);
    assert_eq!(read("{"), None);
    assert_eq!(read("{\"turns\":[]}"), None);
    assert_eq!(read("{\"version\":99,\"turns\":[]}"), None);
    assert_eq!(
        read("{\"version\":1,\"turns\":[{\"said\":\"nobody\"}]}"),
        None
    );
}

#[test]
fn an_empty_conversation_is_still_a_conversation() {
    let empty = Chat {
        turns: Vec::new(),
        ..a_chat()
    };
    assert_eq!(read(&write(&empty)), Some(empty));
}

#[test]
fn a_chat_is_titled_by_the_first_question() {
    assert_eq!(
        title_of(&[
            Turn::model("Hello"),
            Turn::person("  Change page 2\nand then page 3  "),
        ]),
        "Change page 2"
    );
    assert_eq!(title_of(&[]), "");
    assert_eq!(title_of(&[Turn::person("   ")]), "");
}

#[test]
fn a_long_title_is_shortened_at_a_word() {
    let long = "please summarise every interesting investment for the coming year and write it out";
    let title = title_of(&[Turn::person(long)]);
    assert!(title.ends_with('\u{2026}'), "{title}");
    assert!(title.chars().count() <= 61, "{title}");
    assert!(long.starts_with(title.trim_end_matches('\u{2026}')));
    assert!(!title.contains("comin\u{2026}"));
}

#[test]
fn the_list_is_newest_first_and_survives_a_bad_file() {
    let one = write(&Chat {
        id: "a".to_owned(),
        changed: 10,
        ..a_chat()
    });
    let two = write(&Chat {
        id: "b".to_owned(),
        changed: 20,
        ..a_chat()
    });
    let listed = newest_first(vec![one, "not a chat at all".to_owned(), two]);
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].id, "b");
    assert_eq!(listed[1].id, "a");
}

#[test]
fn a_name_is_free_sortable_and_safe_to_join_to_a_path() {
    let mut taken = BTreeSet::new();
    let first = name_for(12_345, &taken);
    assert_eq!(first, "000000012345");
    taken.insert(first.clone());
    let second = name_for(12_345, &taken);
    assert_eq!(second, "000000012345-1");
    assert!(name_for(99, &taken) < first);
    for name in [&first, &second] {
        assert!(
            name.chars()
                .all(|letter| letter.is_ascii_digit() || letter == '-'),
            "{name}"
        );
    }
}

#[test]
fn a_chat_from_before_documents_were_kept_still_reads() {
    let old = r#"{"version":1,"id":"a","title":"t","changed":1,"model":"m","turns":[]}"#;
    let chat = read(old).expect("it reads");
    assert!(chat.documents.is_empty());
    assert!(chat.places.is_empty(), "and it is about no file");
}

#[test]
fn a_document_brings_back_its_own_chat() {
    let about = |id: &str, changed: u64, places: &[&str]| Chat {
        id: id.to_owned(),
        changed,
        documents: vec!["report.pdf".to_owned()],
        places: places.iter().map(|place| (*place).to_owned()).collect(),
        ..Chat::default()
    };
    let chats = newest_first([
        write(&about("old", 10, &["/a/report.pdf"])),
        write(&about(
            "new",
            20,
            &["/a/report.pdf", "/a/report-edited.pdf"],
        )),
        write(&about("other", 30, &["/c/other.pdf"])),
    ]);
    let found = |place: &str| chat_about(&chats, place).map(|chat| chat.id.as_str());
    assert_eq!(found("/a/report.pdf"), Some("new"));
    assert_eq!(found("/a/report-edited.pdf"), Some("new"));
    assert_eq!(found("/b/report.pdf"), None, "the same name elsewhere");
    assert_eq!(found(""), None, "a document saved nowhere yet");
    assert_eq!(found("/c/other.pdf"), Some("other"));
}
