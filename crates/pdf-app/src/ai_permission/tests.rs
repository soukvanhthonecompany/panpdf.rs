use super::{ALWAYS_ASK, Decision, Mode, Why, decide, refusal_text};

type Row = (Mode, &'static str, Option<(bool, bool)>, bool, Decision);

const READS: Option<(bool, bool)> = Some((true, false));
const CHANGES: Option<(bool, bool)> = Some((false, false));
const TAKES_OUT: Option<(bool, bool)> = Some((false, true));

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the permission table, one row per case, read as a table"
)]
fn the_table_is_the_rule() {
    let rows: Vec<Row> = vec![
        (
            Mode::ChatOnly,
            "read_text",
            READS,
            false,
            Decision::Refuse(Why::ToolsAreOff),
        ),
        (
            Mode::ChatOnly,
            "replace_text",
            CHANGES,
            true,
            Decision::Refuse(Why::ToolsAreOff),
        ),
        (
            Mode::ChatOnly,
            "insert_pages",
            CHANGES,
            false,
            Decision::Refuse(Why::ToolsAreOff),
        ),
        (
            Mode::AskBeforeChanges,
            "read_text",
            READS,
            false,
            Decision::Run,
        ),
        (
            Mode::AskBeforeChanges,
            "replace_text",
            CHANGES,
            false,
            Decision::Ask {
                may_allow_for_chat: true,
            },
        ),
        (
            Mode::AskBeforeChanges,
            "delete_pages",
            TAKES_OUT,
            false,
            Decision::Ask {
                may_allow_for_chat: true,
            },
        ),
        (
            Mode::AskBeforeChanges,
            "replace_text",
            CHANGES,
            true,
            Decision::Run,
        ),
        (
            Mode::AskBeforeChanges,
            "insert_pages",
            CHANGES,
            false,
            Decision::Ask {
                may_allow_for_chat: false,
            },
        ),
        (
            Mode::AskBeforeChanges,
            "insert_pages",
            CHANGES,
            true,
            Decision::Ask {
                may_allow_for_chat: false,
            },
        ),
        (Mode::DoIt, "read_text", READS, false, Decision::Run),
        (Mode::DoIt, "replace_text", CHANGES, false, Decision::Run),
        (Mode::DoIt, "delete_pages", TAKES_OUT, false, Decision::Run),
        (
            Mode::DoIt,
            "insert_pages",
            CHANGES,
            false,
            Decision::Ask {
                may_allow_for_chat: false,
            },
        ),
        (Mode::Free, "read_text", READS, false, Decision::Run),
        (Mode::Free, "replace_text", CHANGES, false, Decision::Run),
        (Mode::Free, "insert_pages", CHANGES, false, Decision::Run),
        (Mode::Free, "delete_pages", TAKES_OUT, false, Decision::Run),
        (
            Mode::Free,
            "format_the_disk",
            None,
            false,
            Decision::Refuse(Why::UnknownTool),
        ),
        (
            Mode::DoIt,
            "format_the_disk",
            None,
            true,
            Decision::Refuse(Why::UnknownTool),
        ),
        (
            Mode::ChatOnly,
            "format_the_disk",
            None,
            false,
            Decision::Refuse(Why::UnknownTool),
        ),
    ];
    for (mode, name, facts, allowed, wanted) in rows {
        assert_eq!(
            decide(mode, name, facts, allowed),
            wanted,
            "{mode:?} {name} allowed_for_chat={allowed}"
        );
    }
}

#[test]
fn a_decision_that_ignores_the_mode_fails_the_chat_only_row() {
    fn ignores_the_mode(
        _mode: Mode,
        name: &str,
        facts: Option<(bool, bool)>,
        allowed_for_chat: bool,
    ) -> Decision {
        decide(Mode::AskBeforeChanges, name, facts, allowed_for_chat)
    }
    assert_eq!(
        decide(Mode::ChatOnly, "read_text", READS, false),
        Decision::Refuse(Why::ToolsAreOff)
    );
    assert_eq!(
        ignores_the_mode(Mode::ChatOnly, "read_text", READS, false),
        Decision::Run,
        "the control must disagree, or it is not measuring anything"
    );
}

#[test]
fn a_mode_is_written_and_read_back_or_refused() {
    for mode in [
        Mode::ChatOnly,
        Mode::AskBeforeChanges,
        Mode::DoIt,
        Mode::Free,
    ] {
        assert_eq!(Mode::parse(mode.as_str()), Some(mode));
    }
    assert_eq!(Mode::parse("yolo"), None);
    assert_eq!(Mode::parse(""), None);
    assert_eq!(
        Mode::parse(crate::ai_choice::DEFAULT_MODE),
        Some(Mode::AskBeforeChanges)
    );
    assert_eq!(Mode::default(), Mode::AskBeforeChanges);
}

#[test]
fn a_refusal_says_not_to_retry() {
    assert!(refusal_text().contains("Do not retry"));
    assert_eq!(ALWAYS_ASK, ["insert_pages"]);
}

#[cfg(not(target_arch = "wasm32"))]
mod cards {
    use pdf_agent::json::Json;
    use pdf_agent::tools::request::{NewText, Request};

    use crate::ai_permission::describe_call;
    use crate::wording::Lang;

    #[test]
    #[expect(clippy::too_many_lines, reason = "one card per tool, read as a table")]
    fn every_tool_says_what_it_would_do() {
        let rows: Vec<(Request, &str)> = vec![
            (
                Request::DocumentInfo,
                "Read what the document says about itself",
            ),
            (
                Request::ReadText {
                    first: None,
                    last: None,
                },
                "Read the document's text",
            ),
            (
                Request::ReadText {
                    first: Some(1),
                    last: Some(3),
                },
                "Read the text of pages 2 to 4",
            ),
            (
                Request::FindText {
                    text: "Bangkok".to_owned(),
                    match_case: false,
                    first: None,
                    last: None,
                },
                "Find \u{201c}Bangkok\u{201d} in the document",
            ),
            (
                Request::RenderPage { page: 2, dpi: 96.0 },
                "Look at page 3 as a picture",
            ),
            (
                Request::ListFonts { name: None },
                "List the fonts new text can be set in",
            ),
            (
                Request::ReplaceText {
                    block: "p2-b3".to_owned(),
                    find: None,
                    text: "Hello".to_owned(),
                },
                "Replace the text of block p2-b3 with: Hello",
            ),
            (
                Request::ReplaceText {
                    block: "p2-b3".to_owned(),
                    find: Some("Hi".to_owned()),
                    text: "Hello".to_owned(),
                },
                "Replace \u{201c}Hi\u{201d} in block p2-b3 with: Hello",
            ),
            (
                Request::ReplaceText {
                    block: "p1-b1".to_owned(),
                    find: None,
                    text: String::new(),
                },
                "Delete all the text of block p1-b1",
            ),
            (
                Request::ReplaceText {
                    block: "p1-b1".to_owned(),
                    find: None,
                    text: "   \n ".to_owned(),
                },
                "Delete all the text of block p1-b1",
            ),
            (
                Request::ReplaceText {
                    block: "p1-b1".to_owned(),
                    find: Some("Hi".to_owned()),
                    text: String::new(),
                },
                "Delete \u{201c}Hi\u{201d} from block p1-b1",
            ),
            (
                Request::AddText {
                    page: 1,
                    area: [10.0, 10.0, 110.0, 30.0],
                    text: "Hello".to_owned(),
                    style: NewText {
                        family: "Noto Sans".to_owned(),
                        size: 12.0,
                        bold: false,
                        italic: false,
                        fill: None,
                    },
                },
                "Write new text on page 2: Hello",
            ),
            (
                Request::AddText {
                    page: 1,
                    area: [10.0, 10.0, 110.0, 30.0],
                    text: String::new(),
                    style: NewText {
                        family: "Noto Sans".to_owned(),
                        size: 12.0,
                        bold: false,
                        italic: false,
                        fill: None,
                    },
                },
                "Write nothing at all on page 2",
            ),
            (
                Request::SetProperties(pdf_edit::info::InfoEdit {
                    title: Some("A report".to_owned()),
                    author: Some("Phan".to_owned()),
                    ..pdf_edit::info::InfoEdit::default()
                }),
                "Set the document's properties (title, author)",
            ),
            (
                Request::FillField {
                    name: "Surname".to_owned(),
                    value: Json::text("Lattan"),
                },
                "Fill the form field \u{201c}Surname\u{201d} with: Lattan",
            ),
            (
                Request::AddBlankPage {
                    after: 2,
                    size: None,
                },
                "Add a blank page after page 2",
            ),
            (
                Request::AddBlankPage {
                    after: 0,
                    size: None,
                },
                "Add a blank page at the front",
            ),
            (Request::DeletePages(vec![2, 4]), "Delete pages 3 and 5"),
            (Request::DeletePages(vec![2]), "Delete page 3"),
            (
                Request::MovePages {
                    pages: vec![2],
                    to: 0,
                },
                "Move page 3 so that the first becomes page 1",
            ),
            (
                Request::RotatePages {
                    pages: vec![0, 1],
                    quarter_turns: -1,
                },
                "Turn pages 1 and 2 by -90 degrees",
            ),
            (
                Request::InsertPages {
                    from: std::path::PathBuf::from("/tmp/other.pdf"),
                    pages: Some(vec![0, 1]),
                    after: 0,
                    password: None,
                },
                "Put pages 1 and 2 of the file /tmp/other.pdf in at the front",
            ),
            (
                Request::InsertPages {
                    from: std::path::PathBuf::from("/tmp/other.pdf"),
                    pages: None,
                    after: 3,
                    password: None,
                },
                "Put every page of the file /tmp/other.pdf in after page 3",
            ),
            (Request::Undo, "Take back the last change"),
            (
                Request::Redo,
                "Put back the last change that was taken back",
            ),
        ];
        for (request, english) in rows {
            assert_eq!(describe_call(&request, Lang::English), english);
        }
    }

    #[test]
    fn a_long_piece_of_text_is_cut_to_one_line() {
        let said = describe_call(
            &Request::ReplaceText {
                block: "p1-b1".to_owned(),
                find: None,
                text: format!("{}\nand more", "x".repeat(400)),
            },
            Lang::English,
        );
        assert!(!said.contains('\n'));
        assert!(said.ends_with('\u{2026}'));
        assert!(said.chars().count() < 260);
    }
}
