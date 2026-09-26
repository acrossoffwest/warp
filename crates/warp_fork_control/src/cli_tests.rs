use clap::Parser;
use serde_json::json;

use super::*;

fn request(args: &[&str]) -> (String, serde_json::Value) {
    let cli = Cli::try_parse_from(std::iter::once("warp-fork-ctl").chain(args.iter().copied()))
        .expect("args should parse");
    cli.command.to_request().expect("request should build")
}

#[test]
fn maps_simple_commands() {
    assert_eq!(request(&["ping"]), ("ping".into(), json!({})));
    assert_eq!(request(&["list", "--json"]), ("list".into(), json!({})));
    assert_eq!(request(&["focus", "12"]), ("focus".into(), json!({"pane_id": 12})));
    assert_eq!(request(&["find-pid", "999"]), ("find_by_pid".into(), json!({"pid": 999})));
}

#[test]
fn maps_open_with_options() {
    let (method, params) = request(&[
        "open", "--cwd", "/tmp", "--cmd", "claude", "--title", "TEST", "--new-window", "--no-focus",
    ]);
    assert_eq!(method, "open_tab");
    assert_eq!(
        params,
        json!({"cwd": "/tmp", "command": "claude", "title": "TEST", "window": "new", "focus": false})
    );
}

#[test]
fn maps_title_set_and_clear() {
    assert_eq!(
        request(&["title", "5", "Hello"]),
        ("set_title".into(), json!({"tab_id": 5, "title": "Hello"}))
    );
    assert_eq!(
        request(&["title", "--pane", "5", "--clear"]),
        ("set_title".into(), json!({"pane_id": 5, "title": null}))
    );
}

#[test]
fn maps_send_flags() {
    assert_eq!(
        request(&["send", "3", "hi", "--submit", "--allow-shell", "--keys"]),
        (
            "send_input".into(),
            json!({"pane_id": 3, "text": "hi", "submit": true, "allow_shell": true, "mode": "keys"})
        )
    );
}

#[test]
fn title_without_text_or_clear_is_an_error() {
    let cli = Cli::try_parse_from(["warp-fork-ctl", "title", "5"]).unwrap();
    assert!(cli.command.to_request().is_err());
}
