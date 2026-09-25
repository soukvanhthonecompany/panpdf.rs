use super::{HANDLE, Request, parse};
use crate::json::Json;
use crate::tools::offered_to_a_window;

fn args(text: &str) -> Json {
    Json::parse(text).expect("the arguments in this test are JSON")
}

#[expect(
    clippy::too_many_lines,
    reason = "one example call per tool, read as a table"
)]
fn examples() -> Vec<(&'static str, &'static str, Request)> {
    vec![
        (
            "document_info",
            r#"{"document":"doc-1"}"#,
            Request::DocumentInfo,
        ),
        (
            "read_text",
            r#"{"document":"doc-1","first_page":2,"last_page":4}"#,
            Request::ReadText {
                first: Some(1),
                last: Some(3),
            },
        ),
        (
            "read_text",
            r#"{"document":"doc-1"}"#,
            Request::ReadText {
                first: None,
                last: None,
            },
        ),
        (
            "find_text",
            r#"{"document":"doc-1","text":"Bangkok","match_case":true}"#,
            Request::FindText {
                text: "Bangkok".to_owned(),
                match_case: true,
                first: None,
                last: None,
            },
        ),
        (
            "render_page",
            r#"{"document":"doc-1","page":3,"dpi":150}"#,
            Request::RenderPage {
                page: 2,
                dpi: 150.0,
            },
        ),
        (
            "list_fonts",
            r#"{"name":"Noto"}"#,
            Request::ListFonts {
                name: Some("Noto".to_owned()),
            },
        ),
        (
            "replace_text",
            r#"{"document":"doc-1","block":"p2-b3","text":"Hello","find":"Hi"}"#,
            Request::ReplaceText {
                block: "p2-b3".to_owned(),
                find: Some("Hi".to_owned()),
                text: "Hello".to_owned(),
            },
        ),
        (
            "fill_field",
            r#"{"document":"doc-1","name":"Surname","value":"Lattan"}"#,
            Request::FillField {
                name: "Surname".to_owned(),
                value: Json::text("Lattan"),
            },
        ),
        (
            "add_blank_page",
            r#"{"document":"doc-1","after_page":2,"width":595,"height":842}"#,
            Request::AddBlankPage {
                after: 2,
                size: Some([595.0, 842.0]),
            },
        ),
        (
            "add_blank_page",
            r#"{"document":"doc-1","after_page":0}"#,
            Request::AddBlankPage {
                after: 0,
                size: None,
            },
        ),
        (
            "delete_pages",
            r#"{"document":"doc-1","pages":[3,5]}"#,
            Request::DeletePages(vec![2, 4]),
        ),
        (
            "move_pages",
            r#"{"document":"doc-1","pages":[3],"to":1}"#,
            Request::MovePages {
                pages: vec![2],
                to: 0,
            },
        ),
        (
            "rotate_pages",
            r#"{"document":"doc-1","pages":[1],"degrees":-90}"#,
            Request::RotatePages {
                pages: vec![0],
                quarter_turns: -1,
            },
        ),
        (
            "insert_pages",
            r#"{"document":"doc-1","from":"/tmp/other.pdf","pages":[1,2],"after_page":0,"password":"view"}"#,
            Request::InsertPages {
                from: std::path::PathBuf::from("/tmp/other.pdf"),
                pages: Some(vec![0, 1]),
                after: 0,
                password: Some("view".to_owned()),
            },
        ),
        ("undo", r#"{"document":"doc-1"}"#, Request::Undo),
        ("redo", r#"{"document":"doc-1"}"#, Request::Redo),
        (
            "ask_person",
            r#"{"question":" Which theme? ","options":[{"label":"ocean (Recommended)","description":"Blue and calm"},{"label":"forest"}]}"#,
            Request::AskPerson {
                question: "Which theme?".to_owned(),
                options: vec![
                    ("ocean (Recommended)".to_owned(), "Blue and calm".to_owned()),
                    ("forest".to_owned(), String::new()),
                ],
            },
        ),
    ]
}

#[test]
fn a_question_needs_two_to_four_answers() {
    let with = |count: usize| {
        let options: Vec<String> = (0..count)
            .map(|at| format!(r#"{{"label":"answer {at}"}}"#))
            .collect();
        parse(
            "ask_person",
            &args(&format!(
                r#"{{"question":"Which?","options":[{}]}}"#,
                options.join(",")
            )),
        )
    };
    assert!(with(2).is_ok());
    assert!(with(4).is_ok());
    for count in [1, 5] {
        let refused = with(count).expect_err("refused");
        assert!(refused.contains("two to 4 options"), "{refused}");
    }
    assert_eq!(
        parse(
            "ask_person",
            &args(r#"{"question":"Which?","options":[{"label":"a"},{"description":"b"}]}"#)
        ),
        Err("every option needs a `label`".to_owned())
    );
    assert!(
        parse("ask_person", &args(r#"{"question":"Which?"}"#))
            .expect_err("refused")
            .starts_with("`options` is needed")
    );
}

#[test]
fn every_tool_reads_its_own_example() {
    for (name, arguments, wanted) in examples() {
        assert_eq!(
            parse(name, &args(arguments)),
            Ok(wanted),
            "{name} did not read {arguments}"
        );
    }
}

#[test]
fn what_is_offered_is_what_can_be_read() {
    let offered: Vec<String> = offered_to_a_window()
        .into_iter()
        .map(|tool| tool.name)
        .collect();
    assert_eq!(offered.len(), 18);
    for name in &offered {
        let read = parse(name, &args(r#"{"document":"doc-1"}"#));
        assert_ne!(
            read,
            Err(format!("there is no tool called {name}")),
            "{name} is offered and cannot be read"
        );
    }
    assert_eq!(
        parse("save_document", &args(r#"{"document":"doc-1"}"#)),
        Err("there is no tool called save_document".to_owned()),
        "a tool the window does not offer is not performed because it was asked for"
    );
}

#[test]
fn new_text_is_given_a_frame_to_wrap_in() {
    let read = parse(
        "add_text",
        &args(
            r##"{"document":"doc-1","page":1,"left":72,"top":100,"width":200,
                "text":"one\ntwo","size":20,"font":"Noto Sans","bold":true,"color":"#ff0000"}"##,
        ),
    );
    let Ok(Request::AddText {
        page,
        area,
        text,
        style,
    }) = read
    else {
        panic!("add_text did not read as new text: {read:?}");
    };
    assert_eq!(page, 0);
    for (measured, wanted) in area.into_iter().zip([72.0, 100.0, 272.0, 156.0]) {
        assert!((measured - wanted).abs() < 1e-9, "{area:?}");
    }
    assert_eq!(text, "one\ntwo");
    assert_eq!(style.family, "Noto Sans");
    assert!((style.size - 20.0).abs() < 1e-9);
    assert!(style.bold);
    assert!(!style.italic);
    assert_eq!(style.fill, Some([1.0, 0.0, 0.0]));
}

#[test]
fn properties_are_read_and_an_empty_change_is_refused() {
    let read = parse(
        "set_properties",
        &args(r#"{"document":"doc-1","title":"A report"}"#),
    );
    let Ok(Request::SetProperties(edit)) = read else {
        panic!("set_properties did not read: {read:?}");
    };
    assert_eq!(edit.title.as_deref(), Some("A report"));
    assert_eq!(edit.author, None);
    assert_eq!(
        parse("set_properties", &args(r#"{"document":"doc-1"}"#)),
        Err("nothing to set: pass title, author, subject or keywords".to_owned())
    );
}

#[test]
fn a_missing_argument_is_refused_in_the_desks_words() {
    for (name, arguments, why) in [
        (
            "replace_text",
            r#"{"document":"doc-1","block":"p2-b3"}"#,
            "`text` is needed, as text",
        ),
        (
            "replace_text",
            r#"{"document":"doc-1","text":"Hello"}"#,
            "`block` is needed, as text",
        ),
        (
            "find_text",
            r#"{"document":"doc-1"}"#,
            "`text` is needed, as text",
        ),
        ("find_text", r#"{"text":""}"#, "`text` is empty"),
        (
            "render_page",
            r#"{"document":"doc-1"}"#,
            "`page` is needed, as a page number from 1",
        ),
        (
            "render_page",
            r#"{"document":"doc-1","page":0}"#,
            "`page` is needed, as a page number from 1",
        ),
        (
            "add_text",
            r#"{"document":"doc-1","page":1,"top":10,"width":100,"text":"x"}"#,
            "`left` is needed, in points",
        ),
        (
            "add_text",
            r#"{"document":"doc-1","page":1,"left":10,"top":10,"width":0,"text":"x"}"#,
            "`width` is needed, in points",
        ),
        (
            "fill_field",
            r#"{"document":"doc-1","name":"Surname"}"#,
            "`value` is needed",
        ),
        (
            "add_blank_page",
            r#"{"document":"doc-1"}"#,
            "`after_page` is needed: 0 puts it first",
        ),
        (
            "delete_pages",
            r#"{"document":"doc-1"}"#,
            "`pages` is needed, as a list of page numbers",
        ),
        (
            "delete_pages",
            r#"{"document":"doc-1","pages":[0]}"#,
            "`pages` holds something that is not a page number",
        ),
        (
            "rotate_pages",
            r#"{"document":"doc-1","pages":[1],"degrees":45}"#,
            "`degrees` is 90, 180 or 270, or the same negative",
        ),
        (
            "insert_pages",
            r#"{"document":"doc-1","after_page":0}"#,
            "`from` is needed, as text",
        ),
    ] {
        assert_eq!(
            parse(name, &args(arguments)),
            Err(why.to_owned()),
            "{name} with {arguments}"
        );
    }
}

#[test]
fn a_handle_that_is_not_the_open_document_is_refused() {
    assert_eq!(
        parse("read_text", &args(r#"{"document":"doc-2"}"#)),
        Err(format!(
            "there is no document called doc-2: the document open in this window is \
             {HANDLE}, and it is the only one"
        ))
    );
    assert_eq!(
        parse("read_text", &Json::Null),
        Ok(Request::ReadText {
            first: None,
            last: None
        })
    );
}

#[test]
fn a_request_says_whether_it_only_reads() {
    for (name, arguments, _) in examples() {
        let request = parse(name, &args(arguments)).expect("the examples parse");
        let facts = crate::tools::facts(name).expect("every example is a tool");
        assert_eq!(
            request.only_reads(),
            facts.read_only,
            "{name} disagrees with the table"
        );
    }
    assert!(!Request::Undo.only_reads());
}
