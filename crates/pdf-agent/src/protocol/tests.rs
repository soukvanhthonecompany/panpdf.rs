use super::{LEGACY, MODERN, Server};
use crate::desk::Desk;
use crate::json::Json;

fn server() -> Server {
    Server::with_desk(Desk::with_fonts(None))
}

fn ask(server: &mut Server, line: &str) -> Json {
    let reply = server.answer_line(line).expect("a request is answered");
    assert!(!reply.contains('\n'), "one message is one line: {reply}");
    Json::parse(&reply).expect("the reply is JSON")
}

fn error_code(reply: &Json) -> Option<f64> {
    reply.get("error")?.get("code")?.as_f64()
}

#[test]
fn initialize_answers_in_the_revision_asked_for() {
    let mut server = server();
    for version in LEGACY {
        let reply = ask(
            &mut server,
            &format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"{version}","capabilities":{{}},"clientInfo":{{"name":"t","version":"1"}}}}}}"#
            ),
        );
        let result = reply.get("result").expect("a result");
        assert_eq!(
            result.get("protocolVersion").and_then(Json::as_str),
            Some(version)
        );
        assert!(
            result
                .get("capabilities")
                .and_then(|c| c.get("tools"))
                .is_some()
        );
        assert_eq!(
            result
                .get("serverInfo")
                .and_then(|info| info.get("name"))
                .and_then(Json::as_str),
            Some("panpdf")
        );
        assert!(
            result.get("resultType").is_none(),
            "no modern fields for a legacy client"
        );
    }
    let reply = ask(
        &mut server,
        r#"{"jsonrpc":"2.0","id":"x","method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#,
    );
    assert_eq!(
        reply
            .get("result")
            .and_then(|r| r.get("protocolVersion"))
            .and_then(Json::as_str),
        Some(LEGACY[0])
    );
    assert_eq!(
        reply.get("id"),
        Some(&Json::text("x")),
        "the id is echoed as sent"
    );
}

#[test]
fn a_notification_gets_no_reply() {
    let mut server = server();
    assert!(
        server
            .answer_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .is_none()
    );
    assert!(server.answer_line("   ").is_none());
}

#[test]
fn discovery_names_both_eras() {
    let mut server = server();
    let reply = ask(
        &mut server,
        &format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{{"_meta":{{"io.modelcontextprotocol/protocolVersion":"{MODERN}"}}}}}}"#
        ),
    );
    let result = reply.get("result").expect("a result");
    let versions: Vec<&str> = result
        .get("supportedVersions")
        .and_then(Json::as_list)
        .expect("versions")
        .iter()
        .filter_map(Json::as_str)
        .collect();
    assert_eq!(versions[0], MODERN);
    for version in LEGACY {
        assert!(versions.contains(&version), "{versions:?}");
    }
    assert_eq!(
        result.get("resultType").and_then(Json::as_str),
        Some("complete")
    );
    assert!(
        result
            .get("_meta")
            .and_then(|meta| meta.get("io.modelcontextprotocol/serverInfo"))
            .is_some()
    );
}

#[test]
fn an_unknown_revision_is_refused_by_name() {
    let mut server = server();
    let reply = ask(
        &mut server,
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"1900-01-01"}}}"#,
    );
    assert_eq!(error_code(&reply), Some(-32022.0));
    let data = reply
        .get("error")
        .and_then(|e| e.get("data"))
        .expect("data");
    assert_eq!(
        data.get("requested").and_then(Json::as_str),
        Some("1900-01-01")
    );
    assert!(
        data.get("supported")
            .and_then(Json::as_list)
            .is_some_and(|list| !list.is_empty())
    );
}

#[test]
fn malformed_messages_get_json_rpc_errors() {
    let mut server = server();
    let reply = ask(&mut server, "{not json");
    assert_eq!(error_code(&reply), Some(-32700.0));
    assert_eq!(reply.get("id"), Some(&Json::Null));
    let reply = ask(&mut server, "[1,2]");
    assert_eq!(error_code(&reply), Some(-32600.0));
    let reply = ask(
        &mut server,
        r#"{"jsonrpc":"2.0","id":{"a":1},"method":"ping"}"#,
    );
    assert_eq!(error_code(&reply), Some(-32600.0));
    let reply = ask(
        &mut server,
        r#"{"jsonrpc":"2.0","id":2,"method":"no/such"}"#,
    );
    assert_eq!(error_code(&reply), Some(-32601.0));
    let reply = ask(
        &mut server,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"no_such_tool","arguments":{}}}"#,
    );
    assert_eq!(error_code(&reply), Some(-32602.0));
    let reply = ask(&mut server, r#"{"jsonrpc":"2.0","id":5,"method":"ping"}"#);
    assert_eq!(reply.get("result"), Some(&Json::object([])));
}

#[test]
fn a_tool_failure_is_told_to_the_model() {
    let mut server = server();
    let reply = ask(
        &mut server,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"read_text","arguments":{"document":"doc-9"}}}"#,
    );
    let result = reply.get("result").expect("a result, not an error");
    assert_eq!(result.get("isError"), Some(&Json::Bool(true)));
    let text = result
        .get("content")
        .and_then(Json::as_list)
        .and_then(|content| content.first())
        .and_then(|item| item.get("text"))
        .and_then(Json::as_str)
        .expect("text");
    assert!(text.contains("no document is open as doc-9"), "{text}");
}

#[test]
fn a_tool_answer_carries_structured_content() {
    let mut server = server();
    let reply = ask(
        &mut server,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_documents","arguments":{}}}"#,
    );
    let result = reply.get("result").expect("a result");
    assert_eq!(result.get("isError"), Some(&Json::Bool(false)));
    assert!(matches!(
        result.get("structuredContent"),
        Some(Json::Object(_))
    ));
}
