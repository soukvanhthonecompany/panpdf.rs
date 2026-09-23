use super::{Line, Piece, marked_up};

fn plain(text: &str, bold: bool, italic: bool) -> Piece {
    Piece {
        text: text.to_owned(),
        bold,
        italic,
        code: false,
    }
}

#[test]
fn the_marks_become_emphasis_and_stop_showing() {
    let lines = marked_up("เอกสารนี้มีทั้งหมด **1 หน้า**");
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].plain(), "เอกสารนี้มีทั้งหมด 1 หน้า");
    assert_eq!(
        lines[0].pieces,
        vec![
            plain("เอกสารนี้มีทั้งหมด ", false, false),
            plain("1 หน้า", true, false),
        ]
    );
    assert_eq!(lines[0].heading, 0);
    assert!(!lines[0].bullet);
}

#[test]
fn a_whole_little_answer_is_read() {
    let lines = marked_up(
        "# Investing in 2026\n\
         \n\
         Some *thoughts* and `code`.\n\
         - first\n\
         * second\n\
         ### Smaller\n",
    );
    let shape: Vec<(u8, bool, usize, String)> = lines
        .iter()
        .map(|line| (line.heading, line.bullet, line.depth, line.plain()))
        .collect();
    assert_eq!(
        shape,
        vec![
            (1, false, 0, "Investing in 2026".to_owned()),
            (0, false, 0, String::new()),
            (0, false, 0, "Some thoughts and code.".to_owned()),
            (0, false, 0, String::new()),
            (0, true, 1, "\u{2022}  first".to_owned()),
            (0, false, 0, String::new()),
            (0, true, 1, "\u{2022}  second".to_owned()),
            (0, false, 0, String::new()),
            (3, false, 0, "Smaller".to_owned()),
        ],
        "{lines:#?}"
    );
    assert_eq!(
        lines[2].pieces,
        vec![
            plain("Some ", false, false),
            plain("thoughts", false, true),
            plain(" and ", false, false),
            Piece {
                text: "code".to_owned(),
                bold: false,
                italic: false,
                code: true,
            },
            plain(".", false, false),
        ]
    );
}

#[test]
fn a_numbered_list_is_numbered() {
    let lines = marked_up("3. three\n4. four\n5. five\n");
    let said: Vec<String> = lines.iter().map(Line::plain).collect();
    assert_eq!(said, vec!["3.  three", "4.  four", "5.  five"]);
    assert!(lines.iter().all(|line| line.bullet));
}

#[test]
fn a_quote_holds_its_own_blocks() {
    let lines = marked_up("> ## Careful\n> and lazy\n> continuation\n");
    assert_eq!(lines[0].heading, 2);
    assert_eq!(lines[0].depth, 1);
    assert_eq!(lines[0].plain(), "Careful");
    assert_eq!(lines[2].plain(), "and lazy continuation");
}

#[test]
fn a_fence_keeps_its_letters() {
    let lines = marked_up("```rust\nlet a = **b**;\n```\n");
    assert_eq!(lines[0].plain(), "let a = **b**;");
    assert!(lines[0].pieces[0].code);
    assert!(!lines[0].pieces[0].bold);
}

#[test]
fn a_table_is_read() {
    let lines = marked_up("| a | b |\n|---|--:|\n| 1 | 2 |\n");
    assert_eq!(lines[0].plain(), "a   b");
    assert!(
        lines[0]
            .pieces
            .iter()
            .filter(|piece| !piece.text.trim().is_empty())
            .all(|piece| piece.bold),
        "a heading row is set bold"
    );
    assert_eq!(lines[1].plain(), "1   2");
}

#[test]
fn a_mark_that_never_closes_is_just_a_letter() {
    assert_eq!(marked_up("2 * 3 = 6")[0].plain(), "2 * 3 = 6");
    assert_eq!(marked_up("2 * 3 = 6")[0].pieces.len(), 1);
    assert_eq!(marked_up("#1 in the list")[0].heading, 0);
    assert_eq!(marked_up("#1 in the list")[0].plain(), "#1 in the list");
    assert_eq!(marked_up("####### too many")[0].heading, 0);
    assert!(!marked_up("*starred")[0].bullet);
    assert_eq!(marked_up("*starred")[0].plain(), "*starred");
    assert_eq!(
        marked_up("an_underscored_name")[0].plain(),
        "an_underscored_name",
        "an underscore inside a word is part of the word"
    );
    assert_eq!(marked_up("an_underscored_name")[0].pieces.len(), 1);
    assert_eq!(
        marked_up("say _this_ softly")[0].pieces[1],
        plain("this", false, true),
        "an underscore beside a word is still emphasis"
    );
}

#[test]
fn nothing_is_nothing() {
    assert_eq!(marked_up(""), Vec::<Line>::new());
    let lines = marked_up("a\n\nb");
    assert_eq!(lines.len(), 3);
    assert!(lines[1].is_empty());
}
