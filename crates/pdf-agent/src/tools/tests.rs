use std::collections::BTreeSet;

use super::{
    DocumentBrief, NOT_IN_A_WINDOW, colour, facts, listed, offered_to_a_window, window_instructions,
};
use crate::json::Json;

#[test]
fn a_window_offers_sixteen_of_the_twenty_tools() {
    let offered = offered_to_a_window();
    assert_eq!(offered.len(), 16, "{:?}", offered.len());
    let published = listed();
    let published = published.as_list().expect("a list");
    assert_eq!(published.len(), 20);
    for name in NOT_IN_A_WINDOW {
        assert!(
            !offered.iter().any(|tool| tool.name == name),
            "{name} is not offered to a window"
        );
    }
    for tool in &offered {
        let same = published
            .iter()
            .find(|it| it.get("name").and_then(Json::as_str) == Some(tool.name.as_str()))
            .expect("the server publishes it too");
        assert_eq!(
            same.get("description").and_then(Json::as_str),
            Some(tool.description.as_str()),
            "{}",
            tool.name
        );
        assert_eq!(same.get("inputSchema"), Some(&tool.schema), "{}", tool.name);
    }
}

#[test]
fn what_a_tool_does_is_read_from_the_same_table() {
    let read_only: BTreeSet<String> = offered_to_a_window()
        .iter()
        .filter(|tool| facts(&tool.name).expect("known").read_only)
        .map(|tool| tool.name.clone())
        .collect();
    assert_eq!(
        read_only,
        BTreeSet::from([
            "document_info".to_owned(),
            "find_text".to_owned(),
            "list_fonts".to_owned(),
            "read_text".to_owned(),
            "render_page".to_owned(),
        ])
    );
    let destructive: BTreeSet<String> = offered_to_a_window()
        .iter()
        .filter(|tool| facts(&tool.name).expect("known").destructive)
        .map(|tool| tool.name.clone())
        .collect();
    assert_eq!(destructive, BTreeSet::from(["delete_pages".to_owned()]));
    assert_eq!(
        facts("delete_pages"),
        Some(super::ToolFacts {
            read_only: false,
            destructive: true
        })
    );
    assert_eq!(facts("rewrite_the_whole_book"), None);
}

#[test]
fn a_window_tells_the_model_what_it_is_working_on() {
    let said = window_instructions(&DocumentBrief {
        file_name: "report.pdf".to_owned(),
        title: "Quarterly report".to_owned(),
        pages: 12,
    });
    assert!(said.contains("doc-1"), "{said}");
    assert!(
        said.contains("Quarterly report") && said.contains("report.pdf"),
        "{said}"
    );
    assert!(said.contains("12 pages long"), "{said}");
    assert!(said.contains("undo") && said.contains("Ctrl+S"), "{said}");
    assert!(said.contains("Read before you change"), "{said}");
    assert!(said.contains("p<page>-b<index>"), "{said}");
    assert!(said.contains("refus"), "{said}");
    let bare = window_instructions(&DocumentBrief {
        pages: 1,
        ..DocumentBrief::default()
    });
    assert!(bare.contains("`doc-1`, 1 page long"), "{bare}");
}

#[test]
fn every_tool_is_well_formed() {
    let tools = listed();
    let tools = tools.as_list().expect("a list");
    assert!(tools.len() >= 20, "{}", tools.len());
    let mut names = BTreeSet::new();
    for tool in tools {
        let name = tool.get("name").and_then(Json::as_str).expect("a name");
        assert!(
            !name.is_empty()
                && name.len() <= 128
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')),
            "{name}"
        );
        assert!(names.insert(name.to_owned()), "{name} twice");
        assert!(
            tool.get("description")
                .and_then(Json::as_str)
                .is_some_and(|text| text.len() > 20),
            "{name} is described"
        );
        let schema = tool.get("inputSchema").expect("a schema");
        assert_eq!(
            schema.get("type").and_then(Json::as_str),
            Some("object"),
            "{name}"
        );
        let listed: BTreeSet<&str> = match schema.get("properties") {
            Some(Json::Object(properties)) => properties.keys().map(String::as_str).collect(),
            _ => BTreeSet::new(),
        };
        for required in schema
            .get("required")
            .and_then(Json::as_list)
            .unwrap_or_default()
        {
            let required = required.as_str().expect("a name");
            assert!(
                listed.contains(required),
                "{name} requires {required} and lists it"
            );
        }
        let hints = tool.get("annotations").expect("annotations");
        let read_only = hints
            .get("readOnlyHint")
            .and_then(Json::as_bool)
            .expect("read-only hint");
        let destructive = hints
            .get("destructiveHint")
            .and_then(Json::as_bool)
            .expect("destructive hint");
        assert!(!(read_only && destructive), "{name} cannot be both");
    }
    for (name, read_only, destructive) in [
        ("read_text", true, false),
        ("render_page", true, false),
        ("delete_pages", false, true),
        ("save_document", false, true),
        ("replace_text", false, false),
    ] {
        let tool = tools
            .iter()
            .find(|tool| tool.get("name").and_then(Json::as_str) == Some(name))
            .expect("listed");
        let hints = tool.get("annotations").expect("annotations");
        assert_eq!(
            hints.get("readOnlyHint"),
            Some(&Json::Bool(read_only)),
            "{name}"
        );
        assert_eq!(
            hints.get("destructiveHint"),
            Some(&Json::Bool(destructive)),
            "{name}"
        );
    }
}

#[test]
fn colours_read_as_three_components() {
    assert_eq!(colour("#ff0000"), Ok([1.0, 0.0, 0.0]));
    assert_eq!(colour("000000"), Ok([0.0, 0.0, 0.0]));
    for bad in ["#fff", "red", "#gg0000", ""] {
        assert!(colour(bad).is_err(), "{bad}");
    }
}

fn fonts() -> std::sync::Arc<dyn pdf_content::FontProvider> {
    crate::desk::tests::fonts()
}

fn folder(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("panpdf-tools-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("a folder");
    path
}

fn document(folder: &std::path::Path, name: &str, text: &str, paragraphs: usize) -> String {
    let path = folder.join(name);
    std::fs::write(
        &path,
        pdf_session::blank_document([595.0, 842.0]).expect("a blank page"),
    )
    .expect("written");
    let mut desk = crate::desk::Desk::with_fonts(Some(fonts()));
    let opened = desk.open(&path, "", false).expect("opens");
    for at in 0..paragraphs {
        #[allow(clippy::cast_precision_loss)]
        let top = 40.0 + at as f64 * 24.0;
        desk.place_text(
            &opened.handle,
            0,
            [40.0, top, 555.0, top + 20.0],
            text,
            ("DejaVu Sans", 9.0, false, false, None),
        )
        .expect("a paragraph");
    }
    desk.save(&opened.handle, &path, true).expect("saved");
    path.display().to_string()
}

fn opened(desk: &mut crate::desk::Desk, path: &str) -> String {
    let answer = super::call(
        desk,
        "open_document",
        &Json::object([("path", Json::text(path.to_owned()))]),
    )
    .expect("opens");
    answer
        .data
        .get("document")
        .and_then(Json::as_str)
        .expect("a handle")
        .to_owned()
}

#[test]
fn a_search_stops_at_the_blocks_or_the_text_one_reply_holds() {
    let block = |characters: usize| {
        (
            crate::desk::Block {
                page: 0,
                index: 0,
                text: "x".repeat(characters),
                area: [0.0, 0.0, 10.0, 10.0],
                size: 10.0,
                fixed: None,
            },
            1,
        )
    };
    let room_for = |characters: usize| {
        let mut kept: Vec<(crate::desk::Block, usize)> = Vec::new();
        while super::keep(&kept) {
            kept.push(block(characters));
        }
        kept.len()
    };
    assert_eq!(room_for(100), super::MOST_HITS);
    assert_eq!(
        room_for(3_000),
        crate::desk::MOST_CHARACTERS / (super::MOST_HIT_CHARACTERS + 60) + 1
    );
    assert!(room_for(3_000) < super::MOST_HITS);
}

#[test]
fn a_search_carries_a_piece_of_each_block_not_all_of_it() {
    let folder = folder("find-clip");
    let long = "Confidential. ".repeat(200);
    let path = document(&folder, "long.pdf", &long, 1);
    let mut desk = crate::desk::Desk::with_fonts(Some(fonts()));
    let handle = opened(&mut desk, &path);
    let answer = super::call(
        &mut desk,
        "find_text",
        &Json::object([
            ("document", Json::text(handle.clone())),
            ("text", Json::text("confidential".to_owned())),
        ]),
    )
    .expect("finds");
    let hit = &answer.data.as_list().expect("hits")[0];
    let text = hit.get("text").and_then(Json::as_str).expect("its text");
    assert!(
        text.chars().count() <= super::MOST_HIT_CHARACTERS + 4,
        "{} characters",
        text.chars().count()
    );
    assert!(text.ends_with(" ..."), "{text}");

    let narrowed = super::call(
        &mut desk,
        "find_text",
        &Json::object([
            ("document", Json::text(handle.clone())),
            ("text", Json::text("confidential".to_owned())),
            ("first_page", Json::count(1)),
            ("last_page", Json::count(1)),
        ]),
    )
    .expect("finds");
    assert_eq!(narrowed.data.as_list().expect("hits").len(), 1);
    let refused = super::call(
        &mut desk,
        "find_text",
        &Json::object([
            ("document", Json::text(handle)),
            ("text", Json::text("confidential".to_owned())),
            ("first_page", Json::count(4)),
        ]),
    )
    .err()
    .expect("there is no page 4");
    assert!(refused.contains("no page 4"), "{refused}");
}
