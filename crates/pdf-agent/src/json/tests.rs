use super::{Json, base64};

#[test]
fn a_request_reads_as_its_members() {
    let request = Json::parse(
        r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"read_text","arguments":{"page":3,"exact":true,"list":[1,2.5,-3e2],"none":null}}}"#,
    )
    .expect("reads");
    assert_eq!(request.get("id").and_then(Json::as_count), Some(7));
    let arguments = request
        .get("params")
        .and_then(|params| params.get("arguments"))
        .expect("arguments");
    assert_eq!(arguments.get("page").and_then(Json::as_count), Some(3));
    assert_eq!(arguments.get("exact").and_then(Json::as_bool), Some(true));
    assert_eq!(arguments.get("none"), Some(&Json::Null));
    let list = arguments.get("list").and_then(Json::as_list).expect("list");
    assert_eq!(
        list.iter().filter_map(Json::as_f64).collect::<Vec<_>>(),
        [1.0, 2.5, -300.0]
    );
}

#[test]
fn text_in_every_script_goes_round_unchanged() {
    let read = Json::parse(r#""ສະບາຍດີ สวัสดี \ud83d\ude00 \"q\" \\ \/ \n\t\u0001""#).expect("reads");
    assert_eq!(read.as_str(), Some("ສະບາຍດີ สวัสดี 😀 \"q\" \\ / \n\t\u{1}"));
    let written = read.write();
    assert!(
        !written.contains('\n'),
        "one message is one line: {written}"
    );
    assert_eq!(Json::parse(&written).expect("reads back"), read);
}

#[test]
fn line_separators_are_escaped() {
    let written = Json::text("a\u{2028}b\u{2029}c").write();
    assert_eq!(written, r#""a\u2028b\u2029c""#);
}

#[test]
fn numbers_are_written_as_json_has_them() {
    assert_eq!(Json::Number(3.0).write(), "3");
    assert_eq!(Json::Number(-0.5).write(), "-0.5");
    assert_eq!(Json::Number(f64::NAN).write(), "null");
    assert_eq!(Json::Number(f64::INFINITY).write(), "null");
}

#[test]
fn objects_are_written_in_key_order() {
    let one = Json::object([("b", Json::Null), ("a", Json::Bool(true))]);
    assert_eq!(one.write(), r#"{"a":true,"b":null}"#);
}

#[test]
fn what_is_not_json_is_refused() {
    for bad in [
        "",
        "{",
        "[1,]",
        "{\"a\" 1}",
        "01",
        "1.",
        "-",
        "\"unterminated",
        "\"\\x\"",
        "\"\\ud800\"",
        "\"\\udc00\"",
        "\"\\u12g4\"",
        "\"\\u+123\"",
        "tru",
        "1 2",
        "\"a\u{1}b\"",
        "1e999",
    ] {
        assert!(Json::parse(bad).is_err(), "{bad:?} was read");
    }
}

#[test]
fn nesting_too_deep_is_refused() {
    let deep = "[".repeat(super::MOST_DEPTH + 2) + &"]".repeat(super::MOST_DEPTH + 2);
    assert!(Json::parse(&deep).is_err());
    let shallow = "[".repeat(10) + &"]".repeat(10);
    assert!(Json::parse(&shallow).is_ok());
}

#[test]
fn only_whole_numbers_are_counts() {
    assert_eq!(Json::Number(4.0).as_count(), Some(4));
    assert_eq!(Json::Number(4.5).as_count(), None);
    assert_eq!(Json::Number(-1.0).as_count(), None);
    assert_eq!(Json::text("4").as_count(), None);
}

#[test]
fn base64_matches_the_rfc() {
    for (plain, coded) in [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
    ] {
        assert_eq!(base64(plain.as_bytes()), coded);
    }
}
