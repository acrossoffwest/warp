use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use super::*;
use crate::protocol::{ErrorBody, ErrorCode, Request};

fn echo_handler() -> Handler {
    Arc::new(|request| match request {
        Request::Ping => Ok(json!({"pong": true})),
        Request::List => panic!("boom"),
        _ => Err(ErrorBody::new(ErrorCode::NotFound, "nope")),
    })
}

fn roundtrip(stream: &mut UnixStream, line: &str) -> Value {
    writeln!(stream, "{line}").unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut response = String::new();
    reader.read_line(&mut response).unwrap();
    serde_json::from_str(&response).unwrap()
}

fn socket_in(dir: &tempfile::TempDir) -> std::path::PathBuf {
    dir.path().join("fc").join("control.sock")
}

#[test]
fn serves_requests_and_sets_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let path = socket_in(&dir);
    let server = Server::start(&path, echo_handler()).unwrap();

    let socket_mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(socket_mode, 0o600);
    let dir_mode = std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777;
    assert_eq!(dir_mode, 0o700);

    let mut stream = UnixStream::connect(server.path()).unwrap();
    let response = roundtrip(&mut stream, r#"{"id":1,"method":"ping"}"#);
    assert_eq!(response["ok"], json!(true));
    assert_eq!(response["result"], json!({"pong": true}));
    assert_eq!(response["api_version"], json!(1));
}

#[test]
fn bad_line_does_not_close_connection() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(&socket_in(&dir), echo_handler()).unwrap();
    let mut stream = UnixStream::connect(server.path()).unwrap();

    let bad = roundtrip(&mut stream, "{oops");
    assert_eq!(bad["error"]["code"], json!("bad_request"));
    let good = roundtrip(&mut stream, r#"{"method":"ping"}"#);
    assert_eq!(good["ok"], json!(true));
}

#[test]
fn handler_errors_and_panics_become_error_responses() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(&socket_in(&dir), echo_handler()).unwrap();
    let mut stream = UnixStream::connect(server.path()).unwrap();

    let not_found = roundtrip(&mut stream, r#"{"method":"focus","params":{"pane_id":1}}"#);
    assert_eq!(not_found["error"]["code"], json!("not_found"));
    let panic = roundtrip(&mut stream, r#"{"method":"list"}"#);
    assert_eq!(panic["error"]["code"], json!("internal"));
    let still_alive = roundtrip(&mut stream, r#"{"method":"ping"}"#);
    assert_eq!(still_alive["ok"], json!(true));
}

#[test]
fn non_utf8_input_closes_only_that_connection() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(&socket_in(&dir), echo_handler()).unwrap();
    let mut garbage = UnixStream::connect(server.path()).unwrap();
    garbage.write_all(&[0xff, 0xfe, b'\n']).unwrap();
    drop(garbage);

    let mut stream = UnixStream::connect(server.path()).unwrap();
    assert_eq!(roundtrip(&mut stream, r#"{"method":"ping"}"#)["ok"], json!(true));
}

#[test]
fn stale_socket_file_is_replaced_but_live_one_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = socket_in(&dir);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
    let server = Server::start(&path, echo_handler()).unwrap();

    assert!(Server::start(&path, echo_handler()).is_err());
    drop(server);
    assert!(!path.exists());
}

#[test]
fn does_not_chmod_existing_parent_dir() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = dir.path().join("control.sock");
    let _server = Server::start(&path, echo_handler()).unwrap();
    let mode = std::fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755);
}

#[test]
fn drop_does_not_hang_or_delete_replacement_socket() {
    let dir = tempfile::tempdir().unwrap();
    let path = socket_in(&dir);
    let server = Server::start(&path, echo_handler()).unwrap();
    std::fs::remove_file(&path).unwrap();
    let replacement = std::os::unix::net::UnixListener::bind(&path).unwrap();

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        drop(server);
        let _ = tx.send(());
    });
    rx.recv_timeout(Duration::from_secs(2)).expect("Server::drop hung");

    assert!(path.exists());
    drop(replacement);
}

#[test]
fn drop_closes_live_connections() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(&socket_in(&dir), echo_handler()).unwrap();
    let mut stream = UnixStream::connect(server.path()).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    roundtrip(&mut stream, r#"{"method":"ping"}"#);

    drop(server);

    let mut buf = [0u8; 1];
    let n = stream.read(&mut buf).unwrap();
    assert_eq!(n, 0);
}

#[test]
fn refuses_to_remove_non_socket_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = socket_in(&dir);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"not a socket").unwrap();

    assert!(Server::start(&path, echo_handler()).is_err());
    assert!(path.exists());
}
