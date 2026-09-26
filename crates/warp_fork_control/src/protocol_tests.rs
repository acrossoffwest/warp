use serde_json::json;

use super::*;

fn parse_ok(line: &str) -> Envelope {
    parse_request(line).unwrap_or_else(|(_, e)| panic!("expected ok, got {e:?}"))
}

fn parse_err(line: &str) -> (Option<serde_json::Value>, ErrorBody) {
    parse_request(line).expect_err("expected error")
}

#[test]
fn parses_ping_and_list_without_params() {
    assert_eq!(parse_ok(r#"{"method":"ping"}"#).request, Request::Ping);
    assert_eq!(parse_ok(r#"{"method":"list","params":{}}"#).request, Request::List);
}

#[test]
fn echoes_request_id() {
    let envelope = parse_ok(r#"{"id":"abc","method":"ping"}"#);
    assert_eq!(envelope.id, Some(json!("abc")));
}

#[test]
fn parses_open_tab_with_defaults() {
    let envelope = parse_ok(r#"{"method":"open_tab","params":{"cwd":"/tmp"}}"#);
    assert_eq!(
        envelope.request,
        Request::OpenTab(OpenTabParams {
            cwd: "/tmp".into(),
            command: None,
            title: None,
            window: WindowTarget::Current,
            focus: true,
        })
    );
}

#[test]
fn parses_send_input_with_defaults() {
    let envelope = parse_ok(r#"{"method":"send_input","params":{"pane_id":7,"text":"hi"}}"#);
    assert_eq!(
        envelope.request,
        Request::SendInput(SendInputParams {
            pane_id: 7,
            text: "hi".into(),
            submit: false,
            mode: InputMode::Paste,
            allow_shell: false,
        })
    );
}

#[test]
fn set_title_requires_exactly_one_target() {
    let (_, error) = parse_err(r#"{"method":"set_title","params":{"title":"x"}}"#);
    assert_eq!(error.code, ErrorCode::BadRequest);
    let (_, error) =
        parse_err(r#"{"method":"set_title","params":{"tab_id":1,"pane_id":2,"title":"x"}}"#);
    assert_eq!(error.code, ErrorCode::BadRequest);
    let envelope = parse_ok(r#"{"method":"set_title","params":{"tab_id":1,"title":null}}"#);
    assert_eq!(
        envelope.request,
        Request::SetTitle(SetTitleParams { tab_id: Some(1), pane_id: None, title: None })
    );
}

#[test]
fn invalid_json_is_bad_request() {
    let (id, error) = parse_err("{not json");
    assert_eq!(id, None);
    assert_eq!(error.code, ErrorCode::BadRequest);
}

#[test]
fn non_object_and_missing_method_are_bad_request() {
    assert_eq!(parse_err("[1,2]").1.code, ErrorCode::BadRequest);
    let (id, error) = parse_err(r#"{"id":5}"#);
    assert_eq!(id, Some(json!(5)));
    assert_eq!(error.code, ErrorCode::BadRequest);
}

#[test]
fn unknown_method_keeps_id() {
    let (id, error) = parse_err(r#"{"id":9,"method":"close_tab"}"#);
    assert_eq!(id, Some(json!(9)));
    assert_eq!(error.code, ErrorCode::UnknownMethod);
}

#[test]
fn wrong_param_types_are_bad_request() {
    let (_, error) = parse_err(r#"{"method":"focus","params":{"pane_id":"seven"}}"#);
    assert_eq!(error.code, ErrorCode::BadRequest);
    let (_, error) = parse_err(r#"{"method":"send_input","params":{"pane_id":1,"text":"x","mode":"typing"}}"#);
    assert_eq!(error.code, ErrorCode::BadRequest);
}

#[test]
fn responses_carry_api_version() {
    let ok = ok_response(Some(json!(1)), json!({"a": 1}));
    assert_eq!(ok, json!({"id": 1, "api_version": 1, "ok": true, "result": {"a": 1}}));
    let err = error_response(None, &ErrorBody::new(ErrorCode::NotInTui, "nope"));
    assert_eq!(
        err,
        json!({"id": null, "api_version": 1, "ok": false,
               "error": {"code": "not_in_tui", "message": "nope"}})
    );
}
