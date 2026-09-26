# Fork Control API Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A fork-only Unix-socket JSON-lines API (`warp_fork_control`) plus CLI `warp-fork-ctl` that lists Warp panes with pid/cwd/alt-screen info, focuses panes, opens tabs with a command, sets tab titles, sends input into TUI panes and finds a pane by descendant pid.

**Architecture:** A new crate `crates/warp_fork_control` holds everything that does not need the app: protocol types and parsing, input routing/encoding, pid ancestry walk, socket paths, a threaded Unix-socket server, a client and the CLI. The app side (`app/src/fork_control/`) is a singleton model that owns the server, drains requests on the UI thread and implements the handlers using existing public Warp APIs. Upstream files only get registration one-liners marked `// fork_control:`.

**Tech Stack:** Rust (edition 2024), `serde`/`serde_json`, `clap`, `libc`, `nix`, `sysinfo`, `async-channel`, warpui models/views.

**Spec:** `docs/superpowers/specs/2026-09-26-fork-control-api-design.md`

## Global Constraints

- Work on branch `feature/control-api`.
- Every edit inside an upstream-owned file carries a `// fork_control:` comment on the edited line or the line above it.
- No TCP, no telemetry, no network access from this feature.
- Socket dir created 0700 (existing dirs are never chmod-ed), socket file 0600, peer UID must equal effective UID.
- UI state is only touched on the UI thread via `ForkControlHost`'s job drain.
- `api_version` is `1` and is present in every response.
- No comments unless they explain a non-obvious why; no ticket/plan references in code.
- Tests live in sibling `*_tests.rs` files included via `#[cfg(test)] #[path = "x_tests.rs"] mod tests;` (inline `mod tests {` is rejected by `script/check_no_inline_test_modules`).
- Everything app-side is `#[cfg(unix)]`; Windows builds must still compile.

## Review Focus

- Client disconnects mid-request or sends garbage bytes / non-UTF-8 → server thread ends quietly, app keeps running (transport test with a raw non-UTF-8 write).
- Pane closed between `list` and `send_input`/`focus` → `not_found`, nothing written (handler uses `visible_pane_ids`, so hidden-for-close panes are not found; manual check 6).
- `set_title` on a background tab must not steal keyboard focus from the active tab (manual check in Task 11).
- `send_input` with empty text and `submit: true` → just Enter (input test `encode_empty_text_is_empty`, handler writes only `\r`).
- `WARP_FORK_CONTROL_SOCKET` pointing into a shared directory like `/tmp` must not chmod that directory (transport test `does_not_chmod_existing_parent_dir`).

---

## File Structure

```
crates/warp_fork_control/
  Cargo.toml
  src/lib.rs                 module list
  src/protocol.rs            request/response types, parse_request, ok/error_response
  src/protocol_tests.rs
  src/input.rs               route_input, encode_pty_text, PASTE_SUBMIT_DELAY
  src/input_tests.rs
  src/pids.rs                find_pane_for_pid
  src/pids_tests.rs
  src/paths.rs               default_socket_path, socket_dir_name
  src/paths_tests.rs
  src/server.rs              Server (accept loop, peer check, handle_line)
  src/server_tests.rs
  src/client.rs              call()
  src/cli.rs                 clap types + Command::to_request
  src/cli_tests.rs
  src/bin/warp_fork_ctl.rs   CLI entry point, output formatting
app/src/fork_control/
  mod.rs                     ForkControlHost, dispatch, is_enabled
  mod_tests.rs
  fork_settings.rs           ForkControlSettings group (not `settings.rs`: that name clashes with the `settings` crate)
  procinfo.rs                ProcessTable (sysinfo), foreground_pgid_of_fd
  handlers.rs                ping/list/focus/open_tab/set_title/send_input/find_by_pid
crates/integration/src/test/fork_control.rs
docs/fork-control-api.md
```

Upstream files touched (one-liners, `// fork_control:`): `Cargo.toml`, `app/Cargo.toml`, `app/src/lib.rs`, `app/src/settings/init.rs`, `crates/integration/Cargo.toml` (only if the crate needs the dependency), `crates/integration/src/test.rs`, `crates/integration/src/bin/integration.rs`, `README.md` (fork section, one line).

---

### Task 1: Crate scaffold + protocol

**Files:**
- Create: `crates/warp_fork_control/Cargo.toml`, `src/lib.rs`, `src/protocol.rs`, `src/protocol_tests.rs`
- Modify: `Cargo.toml` (workspace deps)

**Interfaces:**
- Produces: `warp_fork_control::protocol::{API_VERSION, ErrorCode, ErrorBody, Request, Envelope, WindowTarget, InputMode, FocusParams, OpenTabParams, SetTitleParams, SendInputParams, FindByPidParams, PingResult, PaneInfo, ListResult, OpenTabResult, SendInputResult, parse_request, ok_response, error_response}`.

- [ ] **Step 1: Create the crate manifest**

`crates/warp_fork_control/Cargo.toml`:

```toml
[package]
name = "warp_fork_control"
version = "0.1.0"
edition = "2024"
publish = false

[[bin]]
name = "warp-fork-ctl"
path = "src/bin/warp_fork_ctl.rs"

[dependencies]
anyhow.workspace = true
clap = { workspace = true, features = ["derive"] }
dirs.workspace = true
libc.workspace = true
log.workspace = true
serde.workspace = true
serde_json.workspace = true

[dev-dependencies]
tempfile.workspace = true
```

In root `Cargo.toml` `[workspace.dependencies]`, next to `ipc = { path = "crates/ipc" }`:

```toml
warp_fork_control = { path = "crates/warp_fork_control" } # fork_control:
```

`src/lib.rs` (grows in later tasks; for now):

```rust
//! Fork-only local control API: protocol, transport and CLI.
//! Contract: docs/fork-control-api.md.

pub mod protocol;
```

Create an empty-main placeholder so the bin target compiles until Task 6: `src/bin/warp_fork_ctl.rs` with `fn main() {}`.

- [ ] **Step 2: Write the failing tests**

`src/protocol_tests.rs`:

```rust
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
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p warp_fork_control protocol`
Expected: compile error (module `protocol` has no items).

- [ ] **Step 4: Implement `src/protocol.rs`**

```rust
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const API_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadRequest,
    UnknownMethod,
    NotFound,
    NotInTui,
    PaneBusy,
    Timeout,
    Unavailable,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: ErrorCode,
    pub message: String,
}

impl ErrorBody {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowTarget {
    #[default]
    Current,
    New,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputMode {
    #[default]
    Paste,
    Keys,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FocusParams {
    pub pane_id: u64,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenTabParams {
    pub cwd: String,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub window: WindowTarget,
    #[serde(default = "default_true")]
    pub focus: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetTitleParams {
    #[serde(default)]
    pub tab_id: Option<u64>,
    #[serde(default)]
    pub pane_id: Option<u64>,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendInputParams {
    pub pane_id: u64,
    pub text: String,
    #[serde(default)]
    pub submit: bool,
    #[serde(default)]
    pub mode: InputMode,
    #[serde(default)]
    pub allow_shell: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindByPidParams {
    pub pid: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Ping,
    List,
    Focus(FocusParams),
    OpenTab(OpenTabParams),
    SetTitle(SetTitleParams),
    SendInput(SendInputParams),
    FindByPid(FindByPidParams),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Envelope {
    pub id: Option<Value>,
    pub request: Request,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PingResult {
    pub api_version: u32,
    pub app_version: Option<String>,
    pub channel: String,
    pub pid: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneInfo {
    pub window_id: u64,
    pub tab_id: u64,
    pub tab_index: usize,
    pub pane_id: u64,
    pub title: String,
    pub custom_title: Option<String>,
    pub cwd: Option<String>,
    pub shell_pid: Option<u32>,
    pub foreground_pgid: Option<u32>,
    pub foreground_command: Option<String>,
    pub running_command: Option<String>,
    pub is_alt_screen: bool,
    pub is_focused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListResult {
    pub panes: Vec<PaneInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenTabResult {
    pub window_id: u64,
    pub tab_id: u64,
    pub pane_id: u64,
    pub shell_pid: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendInputResult {
    pub delivered_to: crate::input::InputRoute,
}

fn bad_request(message: impl Into<String>) -> ErrorBody {
    ErrorBody::new(ErrorCode::BadRequest, message)
}

fn from_params<T: DeserializeOwned>(params: Value) -> Result<T, ErrorBody> {
    serde_json::from_value(params).map_err(|e| bad_request(format!("invalid params: {e}")))
}

fn validate_set_title(params: SetTitleParams) -> Result<SetTitleParams, ErrorBody> {
    match (params.tab_id, params.pane_id) {
        (Some(_), None) | (None, Some(_)) => Ok(params),
        _ => Err(bad_request("exactly one of tab_id or pane_id is required")),
    }
}

/// Parses one request line. On failure returns the request id, when it could be
/// read, together with the error so the response can still echo it.
pub fn parse_request(line: &str) -> Result<Envelope, (Option<Value>, ErrorBody)> {
    let value: Value = serde_json::from_str(line)
        .map_err(|e| (None, bad_request(format!("invalid JSON: {e}"))))?;
    let Value::Object(mut object) = value else {
        return Err((None, bad_request("request must be a JSON object")));
    };
    let id = object.remove("id");
    let method = match object.remove("method") {
        Some(Value::String(method)) => method,
        Some(_) => return Err((id, bad_request("method must be a string"))),
        None => return Err((id, bad_request("missing method"))),
    };
    let params = object.remove("params").unwrap_or_else(|| json!({}));
    let parsed = match method.as_str() {
        "ping" => Ok(Request::Ping),
        "list" => Ok(Request::List),
        "focus" => from_params(params).map(Request::Focus),
        "open_tab" => from_params(params).map(Request::OpenTab),
        "set_title" => from_params(params)
            .and_then(validate_set_title)
            .map(Request::SetTitle),
        "send_input" => from_params(params).map(Request::SendInput),
        "find_by_pid" => from_params(params).map(Request::FindByPid),
        other => Err(ErrorBody::new(
            ErrorCode::UnknownMethod,
            format!("unknown method: {other}"),
        )),
    };
    match parsed {
        Ok(request) => Ok(Envelope { id, request }),
        Err(error) => Err((id, error)),
    }
}

pub fn ok_response(id: Option<Value>, result: Value) -> Value {
    json!({"id": id, "api_version": API_VERSION, "ok": true, "result": result})
}

pub fn error_response(id: Option<Value>, error: &ErrorBody) -> Value {
    json!({"id": id, "api_version": API_VERSION, "ok": false, "error": error})
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod tests;
```

`SendInputResult` references `crate::input::InputRoute`; add a minimal `src/input.rs` now so it compiles (Task 2 fills it in):

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputRoute {
    Pty,
    InputEditor,
}
```

and `pub mod input;` in `lib.rs`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p warp_fork_control protocol`
Expected: 10 passed.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock crates/warp_fork_control
git commit -m "feat(fork_control): add protocol crate"
```

---

### Task 2: Input routing and PTY encoding

**Files:**
- Modify: `crates/warp_fork_control/src/input.rs`
- Create: `crates/warp_fork_control/src/input_tests.rs`

**Interfaces:**
- Consumes: `protocol::{ErrorBody, ErrorCode, InputMode}`.
- Produces: `input::{InputRoute, PaneActivity, route_input(activity: PaneActivity, is_alt_screen: bool, allow_shell: bool) -> Result<InputRoute, ErrorBody>, encode_pty_text(text: &str, mode: InputMode, bracketed_paste_enabled: bool) -> Vec<u8>, PASTE_SUBMIT_DELAY: Duration}`.

- [ ] **Step 1: Write the failing tests**

`src/input_tests.rs`:

```rust
use super::*;
use crate::protocol::{ErrorCode, InputMode};

#[test]
fn running_command_goes_to_pty() {
    assert_eq!(
        route_input(PaneActivity::RunningCommand, false, false),
        Ok(InputRoute::Pty)
    );
}

#[test]
fn alt_screen_goes_to_pty_even_without_running_block() {
    assert_eq!(route_input(PaneActivity::AtPrompt, true, false), Ok(InputRoute::Pty));
}

#[test]
fn prompt_without_allow_shell_is_not_in_tui() {
    let error = route_input(PaneActivity::AtPrompt, false, false).unwrap_err();
    assert_eq!(error.code, ErrorCode::NotInTui);
}

#[test]
fn prompt_with_allow_shell_goes_to_input_editor() {
    assert_eq!(
        route_input(PaneActivity::AtPrompt, false, true),
        Ok(InputRoute::InputEditor)
    );
}

#[test]
fn warp_agent_block_is_busy() {
    let error = route_input(PaneActivity::WarpAgentRunning, false, true).unwrap_err();
    assert_eq!(error.code, ErrorCode::PaneBusy);
}

#[test]
fn paste_wraps_and_normalizes_newlines_when_bracketed() {
    assert_eq!(
        encode_pty_text("a\nb\r\nc", InputMode::Paste, true),
        b"\x1b[200~a\rb\rc\x1b[201~".to_vec()
    );
}

#[test]
fn paste_without_bracketed_mode_is_raw() {
    assert_eq!(encode_pty_text("a\nb", InputMode::Paste, false), b"a\rb".to_vec());
}

#[test]
fn keys_never_wrap() {
    assert_eq!(encode_pty_text("a\nb", InputMode::Keys, true), b"a\rb".to_vec());
}

#[test]
fn encode_empty_text_is_empty() {
    assert!(encode_pty_text("", InputMode::Keys, true).is_empty());
    assert!(encode_pty_text("", InputMode::Paste, false).is_empty());
}
```

Note: empty text in bracketed `paste` mode still produces the bare markers; the handler skips writing when `text` is empty (Task 10).

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p warp_fork_control input`
Expected: compile error (`route_input` not found).

- [ ] **Step 3: Implement `src/input.rs`**

```rust
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::protocol::{ErrorBody, ErrorCode, InputMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputRoute {
    Pty,
    InputEditor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneActivity {
    AtPrompt,
    RunningCommand,
    WarpAgentRunning,
}

const BRACKETED_PASTE_START: &str = "\x1b[200~";
const BRACKETED_PASTE_END: &str = "\x1b[201~";

/// Same delay upstream uses before the Enter that submits a bracketed paste to a
/// CLI agent; an Enter arriving together with the paste end marker is dropped.
pub const PASTE_SUBMIT_DELAY: Duration = Duration::from_millis(300);

pub fn route_input(
    activity: PaneActivity,
    is_alt_screen: bool,
    allow_shell: bool,
) -> Result<InputRoute, ErrorBody> {
    if is_alt_screen {
        return Ok(InputRoute::Pty);
    }
    match activity {
        PaneActivity::RunningCommand => Ok(InputRoute::Pty),
        PaneActivity::WarpAgentRunning => Err(ErrorBody::new(
            ErrorCode::PaneBusy,
            "a Warp agent is running in this pane",
        )),
        PaneActivity::AtPrompt if allow_shell => Ok(InputRoute::InputEditor),
        PaneActivity::AtPrompt => Err(ErrorBody::new(
            ErrorCode::NotInTui,
            "pane is at a shell prompt; pass allow_shell to type into the Warp input editor",
        )),
    }
}

pub fn encode_pty_text(text: &str, mode: InputMode, bracketed_paste_enabled: bool) -> Vec<u8> {
    if text.is_empty() {
        return Vec::new();
    }
    let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
    match mode {
        InputMode::Paste if bracketed_paste_enabled => {
            format!("{BRACKETED_PASTE_START}{normalized}{BRACKETED_PASTE_END}").into_bytes()
        }
        _ => normalized.into_bytes(),
    }
}

#[cfg(test)]
#[path = "input_tests.rs"]
mod tests;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p warp_fork_control input`
Expected: 9 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/warp_fork_control/src/input.rs crates/warp_fork_control/src/input_tests.rs
git commit -m "feat(fork_control): route and encode pane input"
```

---

### Task 3: Pid ancestry and socket paths

**Files:**
- Create: `crates/warp_fork_control/src/pids.rs`, `pids_tests.rs`, `paths.rs`, `paths_tests.rs`
- Modify: `crates/warp_fork_control/src/lib.rs`

**Interfaces:**
- Produces: `pids::find_pane_for_pid<K: Copy>(pid: u32, shells: &[(K, u32)], parent_of: impl Fn(u32) -> Option<u32>) -> Option<K>`; `paths::{SOCKET_ENV, socket_dir_name(data_profile: Option<&str>) -> String, default_socket_path(data_profile: Option<&str>) -> Option<PathBuf>}`.

- [ ] **Step 1: Write the failing tests**

`src/pids_tests.rs`:

```rust
use std::collections::HashMap;

use super::*;

fn parents(pairs: &[(u32, u32)]) -> impl Fn(u32) -> Option<u32> {
    let map: HashMap<u32, u32> = pairs.iter().copied().collect();
    move |pid| map.get(&pid).copied()
}

#[test]
fn finds_pane_through_ancestors() {
    let shells = [("a", 100), ("b", 200)];
    let parent_of = parents(&[(300, 250), (250, 200), (200, 1)]);
    assert_eq!(find_pane_for_pid(300, &shells, parent_of), Some("b"));
}

#[test]
fn shell_pid_itself_matches() {
    let shells = [("a", 100)];
    assert_eq!(find_pane_for_pid(100, &shells, parents(&[])), Some("a"));
}

#[test]
fn unrelated_pid_is_none() {
    let shells = [("a", 100)];
    assert_eq!(find_pane_for_pid(42, &shells, parents(&[(42, 1)])), None);
}

#[test]
fn parent_cycle_terminates() {
    let shells = [("a", 100)];
    assert_eq!(find_pane_for_pid(5, &shells, parents(&[(5, 6), (6, 5)])), None);
}
```

`src/paths_tests.rs`:

```rust
use super::*;

#[test]
fn dir_name_is_scoped_per_profile() {
    assert_eq!(socket_dir_name(None), "dev.warp.Warp");
    assert_eq!(socket_dir_name(Some("dev")), "dev.warp.Warp-dev");
}

#[test]
fn default_path_ends_with_socket_file() {
    let path = default_socket_path_without_env(None).expect("a data dir exists in tests");
    assert!(path.ends_with("control.sock"), "{}", path.display());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p warp_fork_control -- pids paths`
Expected: compile errors.

- [ ] **Step 3: Implement**

`src/pids.rs`:

```rust
const MAX_DEPTH: usize = 64;

/// Walks `pid`'s ancestors and returns the key of the first shell pid found.
pub fn find_pane_for_pid<K: Copy>(
    pid: u32,
    shells: &[(K, u32)],
    parent_of: impl Fn(u32) -> Option<u32>,
) -> Option<K> {
    let mut current = pid;
    for _ in 0..MAX_DEPTH {
        if let Some((key, _)) = shells.iter().find(|(_, shell_pid)| *shell_pid == current) {
            return Some(*key);
        }
        match parent_of(current) {
            Some(parent) if parent != current && parent != 0 => current = parent,
            _ => return None,
        }
    }
    None
}

#[cfg(test)]
#[path = "pids_tests.rs"]
mod tests;
```

`src/paths.rs`:

```rust
use std::path::PathBuf;

pub const SOCKET_ENV: &str = "WARP_FORK_CONTROL_SOCKET";
const SOCKET_FILE_NAME: &str = "control.sock";

pub fn socket_dir_name(data_profile: Option<&str>) -> String {
    match data_profile {
        Some(profile) => format!("dev.warp.Warp-{profile}"),
        None => "dev.warp.Warp".to_string(),
    }
}

/// `$WARP_FORK_CONTROL_SOCKET` if set, otherwise the per-profile default.
pub fn default_socket_path(data_profile: Option<&str>) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(SOCKET_ENV) {
        return Some(PathBuf::from(path));
    }
    default_socket_path_without_env(data_profile)
}

fn default_socket_path_without_env(data_profile: Option<&str>) -> Option<PathBuf> {
    let app_dir = socket_dir_name(data_profile);
    #[cfg(target_os = "linux")]
    if let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        return Some(
            PathBuf::from(runtime_dir)
                .join(format!("{app_dir}-fork-control"))
                .join(SOCKET_FILE_NAME),
        );
    }
    dirs::data_local_dir().map(|base| base.join(app_dir).join("fork-control").join(SOCKET_FILE_NAME))
}

#[cfg(test)]
#[path = "paths_tests.rs"]
mod tests;
```

`lib.rs` becomes:

```rust
//! Fork-only local control API: protocol, transport and CLI.
//! Contract: docs/fork-control-api.md.

pub mod input;
pub mod paths;
pub mod pids;
pub mod protocol;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p warp_fork_control`
Expected: all pass (protocol 10, input 9, pids 4, paths 2).

- [ ] **Step 5: Commit**

```bash
git add crates/warp_fork_control
git commit -m "feat(fork_control): pid ancestry lookup and socket paths"
```

---

### Task 4: Unix socket server

**Files:**
- Create: `crates/warp_fork_control/src/server.rs`, `server_tests.rs`
- Modify: `crates/warp_fork_control/src/lib.rs` (`#[cfg(unix)] pub mod server;`)

**Interfaces:**
- Consumes: `protocol::{parse_request, ok_response, error_response, Request, ErrorBody, ErrorCode}`.
- Produces: `server::{Handler = Arc<dyn Fn(Request) -> Result<serde_json::Value, ErrorBody> + Send + Sync>, Server::start(path: &Path, handler: Handler) -> anyhow::Result<Server>, Server::path(&self) -> &Path, handle_line(line: &str, handler: &Handler) -> serde_json::Value}`. Dropping `Server` stops accepting and removes the socket file.

- [ ] **Step 1: Write the failing tests**

`src/server_tests.rs`:

```rust
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::sync::Arc;

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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p warp_fork_control server`
Expected: compile error (`Server` not found).

- [ ] **Step 3: Implement `src/server.rs`**

```rust
use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::protocol::{
    Envelope, ErrorBody, ErrorCode, Request, error_response, ok_response, parse_request,
};

pub type Handler = Arc<dyn Fn(Request) -> Result<Value, ErrorBody> + Send + Sync>;

/// `sun_path` is 104 bytes on macOS (108 on Linux), including the NUL.
const MAX_SOCKET_PATH_LEN: usize = 103;

pub struct Server {
    path: PathBuf,
    stop: Arc<AtomicBool>,
    accept_thread: Option<JoinHandle<()>>,
}

impl Server {
    pub fn start(path: &Path, handler: Handler) -> Result<Server> {
        if path.as_os_str().len() > MAX_SOCKET_PATH_LEN {
            bail!("socket path is too long for a Unix socket: {}", path.display());
        }
        let dir = path.parent().context("socket path has no parent directory")?;
        if !dir.exists() {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)
                .with_context(|| format!("creating {}", dir.display()))?;
        }
        if path.exists() {
            if UnixStream::connect(path).is_ok() {
                bail!("another process is already serving {}", path.display());
            }
            std::fs::remove_file(path)
                .with_context(|| format!("removing stale socket {}", path.display()))?;
        }
        let listener =
            UnixListener::bind(path).with_context(|| format!("binding {}", path.display()))?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("chmod 0600 {}", path.display()))?;

        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let accept_thread = std::thread::Builder::new()
            .name("fork-control-accept".into())
            .spawn(move || accept_loop(listener, handler, thread_stop))
            .context("spawning accept thread")?;

        Ok(Server {
            path: path.to_owned(),
            stop,
            accept_thread: Some(accept_thread),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = UnixStream::connect(&self.path);
        if let Some(thread) = self.accept_thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

fn accept_loop(listener: UnixListener, handler: Handler, stop: Arc<AtomicBool>) {
    for stream in listener.incoming() {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let Ok(stream) = stream else { continue };
        if !peer_is_same_user(&stream) {
            log::warn!("fork_control: rejected a connection from another user");
            continue;
        }
        let handler = handler.clone();
        let _ = std::thread::Builder::new()
            .name("fork-control-conn".into())
            .spawn(move || serve_connection(stream, handler));
    }
}

fn serve_connection(stream: UnixStream, handler: Handler) {
    let Ok(read_half) = stream.try_clone() else { return };
    let mut writer = stream;
    for line in BufReader::new(read_half).lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let response = handle_line(&line, &handler);
        if writeln!(writer, "{response}").is_err() {
            break;
        }
    }
}

pub fn handle_line(line: &str, handler: &Handler) -> Value {
    match parse_request(line) {
        Err((id, error)) => {
            log::warn!("fork_control: rejected request: {}", error.message);
            error_response(id, &error)
        }
        Ok(Envelope { id, request }) => {
            match catch_unwind(AssertUnwindSafe(|| handler(request))) {
                Ok(Ok(result)) => ok_response(id, result),
                Ok(Err(error)) => error_response(id, &error),
                Err(_) => {
                    log::error!("fork_control: request handler panicked");
                    error_response(
                        id,
                        &ErrorBody::new(ErrorCode::Internal, "request handler panicked"),
                    )
                }
            }
        }
    }
}

fn peer_is_same_user(stream: &UnixStream) -> bool {
    // SAFETY: geteuid has no preconditions.
    let euid = unsafe { libc::geteuid() };
    peer_uid(stream).is_some_and(|uid| uid == euid)
}

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
fn peer_uid(stream: &UnixStream) -> Option<u32> {
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: the fd is a live Unix socket owned by `stream`; uid/gid are valid out-pointers.
    let rc = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    (rc == 0).then_some(uid)
}

#[cfg(target_os = "linux")]
fn peer_uid(stream: &UnixStream) -> Option<u32> {
    // SAFETY: ucred is plain data; getsockopt writes at most `len` bytes into it.
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    (rc == 0).then_some(cred.uid)
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "linux"
)))]
fn peer_uid(_stream: &UnixStream) -> Option<u32> {
    None
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
```

Add `#[cfg(unix)] pub mod server;` to `lib.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p warp_fork_control server`
Expected: 6 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/warp_fork_control
git commit -m "feat(fork_control): unix socket server with peer uid check"
```

---

### Task 5: Client and CLI `warp-fork-ctl`

**Files:**
- Create: `crates/warp_fork_control/src/client.rs`, `src/cli.rs`, `src/cli_tests.rs`
- Modify: `crates/warp_fork_control/src/bin/warp_fork_ctl.rs`, `src/lib.rs`

**Interfaces:**
- Consumes: `paths::default_socket_path`, `protocol::{ListResult, PaneInfo}`.
- Produces: `client::call(socket: &Path, method: &str, params: serde_json::Value) -> anyhow::Result<serde_json::Value>`; `cli::{Cli, Command, Command::to_request(&self) -> anyhow::Result<(String, serde_json::Value)>}`.

- [ ] **Step 1: Write the failing tests**

`src/cli_tests.rs`:

```rust
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p warp_fork_control cli`
Expected: compile error.

- [ ] **Step 3: Implement**

`src/client.rs`:

```rust
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{Value, json};

pub fn call(socket: &Path, method: &str, params: Value) -> Result<Value> {
    let mut stream = UnixStream::connect(socket)
        .with_context(|| format!("connecting to {} (is Warp running?)", socket.display()))?;
    stream.set_read_timeout(Some(Duration::from_secs(15)))?;
    let request = json!({"id": 1, "method": method, "params": params});
    writeln!(stream, "{request}")?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    serde_json::from_str(&line).context("invalid response from Warp")
}
```

`src/cli.rs`:

```rust
use std::io::Read;
use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};

#[derive(Debug, Parser)]
#[command(name = "warp-fork-ctl", about = "Drive this Warp fork over its local fork-control socket")]
pub struct Cli {
    /// Socket path (default: $WARP_FORK_CONTROL_SOCKET or the per-profile default).
    #[arg(long, global = true)]
    pub socket: Option<PathBuf>,
    /// Data profile of the target Warp instance (dev builds, WARP_DATA_PROFILE).
    #[arg(long, global = true, env = "WARP_DATA_PROFILE")]
    pub profile: Option<String>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// API and app version.
    Ping,
    /// All terminal panes.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Focus a pane and bring Warp to the front.
    Focus { pane_id: u64 },
    /// Open a new tab in a directory, optionally running a command.
    Open {
        #[arg(long)]
        cwd: PathBuf,
        #[arg(long)]
        cmd: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        new_window: bool,
        #[arg(long)]
        no_focus: bool,
    },
    /// Set (or --clear) a custom tab title. The id is a tab_id, or a pane_id with --pane.
    Title {
        id: u64,
        title: Option<String>,
        #[arg(long)]
        pane: bool,
        #[arg(long)]
        clear: bool,
    },
    /// Send text to a pane. Use "-" to read the text from stdin.
    Send {
        pane_id: u64,
        text: String,
        #[arg(long)]
        submit: bool,
        #[arg(long)]
        allow_shell: bool,
        #[arg(long)]
        keys: bool,
    },
    /// Find the pane whose process tree contains a pid.
    FindPid { pid: u32 },
}

impl Command {
    pub fn to_request(&self) -> Result<(String, Value)> {
        let request = match self {
            Command::Ping => ("ping", json!({})),
            Command::List { .. } => ("list", json!({})),
            Command::Focus { pane_id } => ("focus", json!({"pane_id": pane_id})),
            Command::Open { cwd, cmd, title, new_window, no_focus } => {
                let cwd = std::path::absolute(cwd)?;
                let mut params = json!({
                    "cwd": cwd.to_string_lossy(),
                    "window": if *new_window { "new" } else { "current" },
                    "focus": !no_focus,
                });
                if let Some(cmd) = cmd {
                    params["command"] = json!(cmd);
                }
                if let Some(title) = title {
                    params["title"] = json!(title);
                }
                ("open_tab", params)
            }
            Command::Title { id, title, pane, clear } => {
                let title = match (title, clear) {
                    (_, true) => Value::Null,
                    (Some(title), false) => json!(title),
                    (None, false) => bail!("pass a title or --clear"),
                };
                let key = if *pane { "pane_id" } else { "tab_id" };
                ("set_title", json!({ key: id, "title": title }))
            }
            Command::Send { pane_id, text, submit, allow_shell, keys } => {
                let text = if text == "-" {
                    let mut buffer = String::new();
                    std::io::stdin().read_to_string(&mut buffer)?;
                    buffer
                } else {
                    text.clone()
                };
                (
                    "send_input",
                    json!({
                        "pane_id": pane_id,
                        "text": text,
                        "submit": submit,
                        "allow_shell": allow_shell,
                        "mode": if *keys { "keys" } else { "paste" },
                    }),
                )
            }
            Command::FindPid { pid } => ("find_by_pid", json!({"pid": pid})),
        };
        Ok((request.0.to_string(), request.1))
    }
}

#[cfg(test)]
#[path = "cli_tests.rs"]
mod tests;
```

Note: `open --cwd /tmp` in the test resolves to `/tmp` because it is already absolute.

`src/bin/warp_fork_ctl.rs`:

```rust
use anyhow::{Context, Result};
use clap::Parser;
use serde_json::Value;
use warp_fork_control::cli::{Cli, Command};
use warp_fork_control::protocol::ListResult;
use warp_fork_control::{client, paths};

fn main() {
    if let Err(error) = run() {
        eprintln!("warp-fork-ctl: {error:#}");
        std::process::exit(2);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let socket = match cli.socket.clone() {
        Some(socket) => socket,
        None => paths::default_socket_path(cli.profile.as_deref())
            .context("could not determine the socket path; pass --socket")?,
    };
    let (method, params) = cli.command.to_request()?;
    let response = client::call(&socket, &method, params)?;

    if response["ok"] != Value::Bool(true) {
        eprintln!(
            "{}: {}",
            response["error"]["code"].as_str().unwrap_or("error"),
            response["error"]["message"].as_str().unwrap_or("")
        );
        std::process::exit(1);
    }

    match cli.command {
        Command::List { json: false } => {
            let list: ListResult = serde_json::from_value(response["result"].clone())?;
            print_table(&list);
        }
        _ => println!("{}", serde_json::to_string_pretty(&response["result"])?),
    }
    Ok(())
}

fn print_table(list: &ListResult) {
    println!(
        "{:>7} {:>7} {:>4} {:>7} {:>7} {:>3} {:>3} {:<24} {:<18} CWD",
        "PANE", "TAB", "WIN", "SHELL", "FG", "ALT", "FOC", "TITLE", "COMMAND"
    );
    let dash = || "-".to_string();
    for pane in &list.panes {
        println!(
            "{:>7} {:>7} {:>4} {:>7} {:>7} {:>3} {:>3} {:<24.24} {:<18.18} {}",
            pane.pane_id,
            pane.tab_id,
            pane.window_id,
            pane.shell_pid.map_or_else(dash, |pid| pid.to_string()),
            pane.foreground_pgid.map_or_else(dash, |pid| pid.to_string()),
            if pane.is_alt_screen { "yes" } else { "" },
            if pane.is_focused { "*" } else { "" },
            pane.title,
            pane.foreground_command.clone().unwrap_or_else(dash),
            pane.cwd.clone().unwrap_or_else(dash),
        );
    }
}
```

`lib.rs` adds:

```rust
pub mod cli;
#[cfg(unix)]
pub mod client;
```

On non-unix the bin has no client; guard the bin: wrap `run()`'s body so the crate still builds on Windows by adding at the top of `warp_fork_ctl.rs`:

```rust
#[cfg(not(unix))]
fn main() {
    eprintln!("warp-fork-ctl is only supported on macOS and Linux");
    std::process::exit(2);
}
```

and mark the existing `main`, `run`, `print_table` with `#[cfg(unix)]`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p warp_fork_control && cargo build -p warp_fork_control --bin warp-fork-ctl`
Expected: all tests pass; binary builds. `./target/debug/warp-fork-ctl ping` with Warp not running prints `warp-fork-ctl: connecting to … (is Warp running?)` and exits 2.

- [ ] **Step 5: Commit**

```bash
git add crates/warp_fork_control
git commit -m "feat(fork_control): warp-fork-ctl client and CLI"
```

---

### Task 6: App host, settings and ping

**Files:**
- Create: `app/src/fork_control/mod.rs`, `mod_tests.rs`, `fork_settings.rs`, `handlers.rs`, `procinfo.rs`
- Modify: `app/Cargo.toml`, `app/src/lib.rs`, `app/src/settings/init.rs`

**Interfaces:**
- Consumes: `warp_fork_control::{server::{Server, Handler}, paths::default_socket_path, protocol::*}`.
- Produces: `crate::fork_control::{ForkControlHost, ForkControlSettings}`; `handlers::handle(request: Request, procs: Option<ProcessTable>, ctx: &mut ModelContext<ForkControlHost>) -> Result<Value, ErrorBody>`; `procinfo::{ProcessTable::snapshot() -> ProcessTable, ProcessTable::name(&self, pid: u32) -> Option<String>, ProcessTable::parent(&self, pid: u32) -> Option<u32>, foreground_pgid_of_fd(fd: RawFd) -> Option<u32>}`.

- [ ] **Step 1: Write the failing test**

`app/src/fork_control/mod_tests.rs`:

```rust
use super::is_enabled;

#[test]
fn setting_and_env_both_gate_the_server() {
    assert!(is_enabled(true, None));
    assert!(is_enabled(true, Some("1")));
    assert!(!is_enabled(false, None));
    for off in ["0", "false", "off"] {
        assert!(!is_enabled(true, Some(off)), "{off}");
    }
}
```

- [ ] **Step 2: Wire dependencies and module**

`app/Cargo.toml` `[dependencies]` (next to `ipc.workspace = true`):

```toml
warp_fork_control.workspace = true # fork_control:
```

`app/src/lib.rs`, next to `pub mod remote_control;`:

```rust
#[cfg(unix)]
pub mod fork_control; // fork_control:
```

`app/src/lib.rs`, inside the `if matches!(launch_mode, LaunchMode::App { .. } | LaunchMode::Test { .. })` block, directly after the `remote_control::start` match:

```rust
        // fork_control: fork-only local control API (docs/fork-control-api.md).
        #[cfg(unix)]
        ctx.add_singleton_model(fork_control::ForkControlHost::new);
```

`app/src/settings/init.rs`, last line inside `register_all_settings`:

```rust
    #[cfg(unix)]
    crate::fork_control::ForkControlSettings::register(ctx); // fork_control:
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p warp --lib fork_control`
Expected: compile error (module files missing).

- [ ] **Step 4: Implement settings, procinfo, host and ping**

`app/src/fork_control/fork_settings.rs`:

```rust
use warp_core::settings::macros::define_settings_group;
use warp_core::settings::{SupportedPlatforms, SyncToCloud};

define_settings_group!(ForkControlSettings, settings: [
    enabled: ForkControlEnabled {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Never,
        surface: settings::SettingSurfaces::GUI,
        private: false,
        storage_key: "ForkControlEnabled",
        toml_path: "fork.control_api.enabled",
        description: "Serve the fork-only local control API on a Unix socket.",
    },
]);
```

If the settings schema/registry tests (`cargo test -p warp --lib settings`) complain about an unknown TOML section, follow what those tests require (e.g. a schema snapshot update) and keep the change inside fork-owned files where possible.

`app/src/fork_control/procinfo.rs`:

```rust
use std::os::fd::RawFd;

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

pub(crate) fn foreground_pgid_of_fd(fd: RawFd) -> Option<u32> {
    nix::unistd::tcgetpgrp(fd)
        .ok()
        .map(|pid| pid.as_raw() as u32)
        .filter(|&pgid| pgid > 0)
}

pub(crate) struct ProcessTable(System);

impl ProcessTable {
    pub(crate) fn snapshot() -> Self {
        let mut system = System::new();
        system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );
        Self(system)
    }

    pub(crate) fn name(&self, pid: u32) -> Option<String> {
        self.0
            .process(Pid::from_u32(pid))
            .map(|process| process.name().to_string_lossy().into_owned())
    }

    pub(crate) fn parent(&self, pid: u32) -> Option<u32> {
        self.0.process(Pid::from_u32(pid))?.parent().map(|pid| pid.as_u32())
    }
}
```

`app/src/fork_control/mod.rs`:

```rust
//! Fork-only local control API host. Contract: docs/fork-control-api.md.

mod fork_settings;
mod handlers;
mod procinfo;

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use serde_json::Value;
use warp_core::channel::ChannelState;
use warp_fork_control::paths::default_socket_path;
use warp_fork_control::protocol::{ErrorBody, ErrorCode, Request};
use warp_fork_control::server::{Handler, Server};
use warpui::r#async::SpawnedLocalStream;
use warpui::{Entity, ModelContext, SingletonEntity};

pub use fork_settings::ForkControlSettings;
use procinfo::ProcessTable;

const DISABLE_ENV: &str = "WARP_FORK_CONTROL";
const UI_TIMEOUT: Duration = Duration::from_secs(5);

struct Job {
    request: Request,
    procs: Option<ProcessTable>,
    reply: mpsc::Sender<Result<Value, ErrorBody>>,
}

pub struct ForkControlHost {
    server: Option<Server>,
    job_tx: async_channel::Sender<Job>,
    _drain: SpawnedLocalStream,
}

impl ForkControlHost {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        let (job_tx, job_rx) = async_channel::unbounded::<Job>();
        let drain = ctx.spawn_stream_local(
            job_rx,
            |_, job, ctx| {
                let result = handlers::handle(job.request, job.procs, ctx);
                let _ = job.reply.send(result);
            },
            |_, _| {},
        );
        ctx.subscribe_to_model(&ForkControlSettings::handle(ctx), |me, _, _event, ctx| {
            me.sync_with_settings(ctx);
        });
        let mut host = Self {
            server: None,
            job_tx,
            _drain: drain,
        };
        host.sync_with_settings(ctx);
        host
    }

    fn sync_with_settings(&mut self, ctx: &mut ModelContext<Self>) {
        let enabled = is_enabled(
            *ForkControlSettings::as_ref(ctx).enabled,
            std::env::var(DISABLE_ENV).ok().as_deref(),
        );
        match (enabled, self.server.is_some()) {
            (true, false) => self.server = start_server(self.job_tx.clone()),
            (false, true) => {
                self.server = None;
                log::info!("fork_control: stopped");
            }
            _ => {}
        }
    }
}

impl Entity for ForkControlHost {
    type Event = ();
}

impl SingletonEntity for ForkControlHost {}

fn is_enabled(setting: bool, env: Option<&str>) -> bool {
    setting && !matches!(env, Some("0" | "false" | "off"))
}

fn start_server(job_tx: async_channel::Sender<Job>) -> Option<Server> {
    let Some(path) = default_socket_path(ChannelState::data_profile().as_deref()) else {
        log::warn!("fork_control: no data directory for the socket");
        return None;
    };
    let handler: Handler = Arc::new(move |request| dispatch(&job_tx, request));
    match Server::start(&path, handler) {
        Ok(server) => {
            log::info!("fork_control: listening on {}", path.display());
            Some(server)
        }
        Err(error) => {
            log::warn!("fork_control disabled: {error:#}");
            None
        }
    }
}

fn dispatch(job_tx: &async_channel::Sender<Job>, request: Request) -> Result<Value, ErrorBody> {
    let procs = matches!(request, Request::List | Request::FindByPid(_))
        .then(ProcessTable::snapshot);
    let (reply, reply_rx) = mpsc::channel();
    job_tx
        .try_send(Job { request, procs, reply })
        .map_err(|_| ErrorBody::new(ErrorCode::Unavailable, "Warp is shutting down"))?;
    reply_rx
        .recv_timeout(UI_TIMEOUT)
        .map_err(|_| ErrorBody::new(ErrorCode::Timeout, "Warp did not answer in time"))?
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
```

`app/src/fork_control/handlers.rs` (ping only; later tasks add the rest):

```rust
use serde_json::Value;
use warp_core::channel::ChannelState;
use warp_fork_control::protocol::{API_VERSION, ErrorBody, ErrorCode, PingResult, Request};
use warpui::ModelContext;

use super::ForkControlHost;
use super::procinfo::ProcessTable;

pub(super) fn handle(
    request: Request,
    _procs: Option<ProcessTable>,
    _ctx: &mut ModelContext<ForkControlHost>,
) -> Result<Value, ErrorBody> {
    match request {
        Request::Ping => to_value(ping()),
        _ => Err(ErrorBody::new(ErrorCode::Internal, "not implemented yet")),
    }
}

fn ping() -> PingResult {
    PingResult {
        api_version: API_VERSION,
        app_version: ChannelState::app_version().map(str::to_owned),
        channel: format!("{:?}", ChannelState::channel()).to_lowercase(),
        pid: std::process::id(),
    }
}

fn to_value<T: serde::Serialize>(value: T) -> Result<Value, ErrorBody> {
    serde_json::to_value(value)
        .map_err(|e| ErrorBody::new(ErrorCode::Internal, format!("serialize: {e}")))
}
```

- [ ] **Step 5: Run tests and build**

Run: `cargo test -p warp --lib fork_control && cargo test -p warp --lib settings && cargo check -p warp`
Expected: `setting_and_env_both_gate_the_server` passes; settings tests pass; the crate checks.

- [ ] **Step 6: Smoke test against a dev instance**

Run (two terminals):

```bash
WARP_DATA_PROFILE=fc cargo run --bin warp-oss
WARP_DATA_PROFILE=fc ./target/debug/warp-fork-ctl ping
ls -l "$HOME/Library/Application Support/dev.warp.Warp-fc/fork-control/"
```

Expected: ping prints `{"api_version":1,"app_version":…,"channel":…,"pid":…}`; the socket is `srw-------`, the directory is `drwx------`.

- [ ] **Step 7: Commit**

```bash
git add app/Cargo.toml Cargo.lock app/src/lib.rs app/src/settings/init.rs app/src/fork_control
git commit -m "feat(fork_control): host model, settings gate and ping"
```

---

### Task 7: `list` and `find_by_pid`

**Files:**
- Modify: `app/src/fork_control/handlers.rs`

**Interfaces:**
- Consumes: `WorkspaceRegistry::as_ref(ctx).all_workspaces(ctx)`, `Workspace::{tab_views, active_tab_index}`, `PaneGroup::{visible_pane_ids, terminal_view_from_pane_id, display_title, custom_title, focused_pane_id}`, `TerminalView::{model, session_command_context, pwd_if_local}`, `TerminalModel::{shell_process_info, is_alt_screen_active}`, `warp_fork_control::pids::find_pane_for_pid`.
- Produces (used by Tasks 8–10): `PaneLocation { window_id, workspace, tab_index, pane_group, pane_id, terminal }`, `all_terminal_panes(ctx) -> Vec<PaneLocation>`, `find_pane(pane_id: u64, ctx) -> Result<PaneLocation, ErrorBody>`, `entity_number(EntityId) -> u64`, `window_number(WindowId) -> u64`, `not_found(msg) -> ErrorBody`.

- [ ] **Step 1: Implement**

Add to `handlers.rs`:

```rust
use warp_fork_control::pids::find_pane_for_pid;
use warp_fork_control::protocol::{ListResult, PaneInfo};
use warpui::{AppContext, EntityId, SingletonEntity, ViewHandle, WindowId};

use crate::pane_group::{PaneGroup, PaneId};
use crate::session_management::CommandContext;
use crate::terminal::TerminalView;
use crate::workspace::{Workspace, WorkspaceRegistry};

pub(super) struct PaneLocation {
    pub(super) window_id: WindowId,
    pub(super) workspace: ViewHandle<Workspace>,
    pub(super) tab_index: usize,
    pub(super) pane_group: ViewHandle<PaneGroup>,
    pub(super) pane_id: PaneId,
    pub(super) terminal: ViewHandle<TerminalView>,
}

pub(super) fn entity_number(id: EntityId) -> u64 {
    id.to_string().parse().unwrap_or_default()
}

pub(super) fn window_number(id: WindowId) -> u64 {
    id.to_string().parse().unwrap_or_default()
}

pub(super) fn not_found(message: impl Into<String>) -> ErrorBody {
    ErrorBody::new(ErrorCode::NotFound, message)
}

pub(super) fn sorted_workspaces(ctx: &AppContext) -> Vec<(WindowId, ViewHandle<Workspace>)> {
    let mut workspaces = WorkspaceRegistry::as_ref(ctx).all_workspaces(ctx);
    workspaces.sort_by_key(|(window_id, _)| window_number(*window_id));
    workspaces
}

pub(super) fn all_terminal_panes(ctx: &AppContext) -> Vec<PaneLocation> {
    let mut panes = Vec::new();
    for (window_id, workspace) in sorted_workspaces(ctx) {
        for (tab_index, pane_group) in workspace.as_ref(ctx).tab_views().enumerate() {
            let group = pane_group.as_ref(ctx);
            for pane_id in group.visible_pane_ids() {
                if let Some(terminal) = group.terminal_view_from_pane_id(pane_id, ctx) {
                    panes.push(PaneLocation {
                        window_id,
                        workspace: workspace.clone(),
                        tab_index,
                        pane_group: pane_group.clone(),
                        pane_id,
                        terminal,
                    });
                }
            }
        }
    }
    panes
}

pub(super) fn find_pane(pane_id: u64, ctx: &AppContext) -> Result<PaneLocation, ErrorBody> {
    all_terminal_panes(ctx)
        .into_iter()
        .find(|location| entity_number(location.terminal.id()) == pane_id)
        .ok_or_else(|| not_found(format!("no terminal pane with pane_id {pane_id}")))
}

fn shell_pid(location: &PaneLocation, ctx: &AppContext) -> Option<u32> {
    location
        .terminal
        .as_ref(ctx)
        .model
        .lock()
        .shell_process_info()
        .map(|shell| shell.pid)
}

fn pane_info(location: &PaneLocation, procs: &ProcessTable, ctx: &AppContext) -> PaneInfo {
    let group = location.pane_group.as_ref(ctx);
    let terminal = location.terminal.as_ref(ctx);
    let (shell_pid, foreground_pgid, is_alt_screen) = {
        let model = terminal.model.lock();
        let shell = model.shell_process_info();
        (
            shell.map(|shell| shell.pid),
            shell
                .and_then(|shell| shell.pty_leader_fd)
                .and_then(super::procinfo::foreground_pgid_of_fd),
            model.is_alt_screen_active(),
        )
    };
    let running_command = match terminal.session_command_context(ctx) {
        CommandContext::RunningCommand { running_command } => Some(running_command),
        _ => None,
    };
    let is_focused = ctx.windows().active_window() == Some(location.window_id)
        && location.workspace.as_ref(ctx).active_tab_index() == location.tab_index
        && group.focused_pane_id(ctx) == location.pane_id;
    PaneInfo {
        window_id: window_number(location.window_id),
        tab_id: entity_number(location.pane_group.id()),
        tab_index: location.tab_index,
        pane_id: entity_number(location.terminal.id()),
        title: group.display_title(ctx),
        custom_title: group.custom_title(ctx),
        cwd: terminal.pwd_if_local(ctx),
        shell_pid,
        foreground_pgid,
        foreground_command: foreground_pgid.and_then(|pgid| procs.name(pgid)),
        running_command,
        is_alt_screen,
        is_focused,
    }
}

fn list(procs: &ProcessTable, ctx: &AppContext) -> ListResult {
    ListResult {
        panes: all_terminal_panes(ctx)
            .iter()
            .map(|location| pane_info(location, procs, ctx))
            .collect(),
    }
}

fn find_by_pid(pid: u32, procs: &ProcessTable, ctx: &AppContext) -> Result<PaneInfo, ErrorBody> {
    let panes = all_terminal_panes(ctx);
    let shells: Vec<(usize, u32)> = panes
        .iter()
        .enumerate()
        .filter_map(|(index, location)| shell_pid(location, ctx).map(|pid| (index, pid)))
        .collect();
    let index = find_pane_for_pid(pid, &shells, |pid| procs.parent(pid))
        .ok_or_else(|| not_found(format!("no pane owns pid {pid}")))?;
    Ok(pane_info(&panes[index], procs, ctx))
}
```

Extend `handle` (replace the `_procs` parameter name with `procs`):

```rust
    let procs = procs.unwrap_or_else(ProcessTable::snapshot);
```

only inside the two arms that need it:

```rust
        Request::List => {
            let procs = procs.unwrap_or_else(ProcessTable::snapshot);
            to_value(list(&procs, ctx))
        }
        Request::FindByPid(params) => {
            let procs = procs.unwrap_or_else(ProcessTable::snapshot);
            find_by_pid(params.pid, &procs, ctx).and_then(to_value)
        }
```

If `ShellProcessInfo::pty_leader_fd` is `#[cfg(unix)]`, this module is unix-only already. If `crate::workspace::WorkspaceRegistry` is not re-exported, import it from `crate::workspace::registry::WorkspaceRegistry`.

- [ ] **Step 2: Build and smoke test**

Run: `cargo check -p warp`, restart the dev instance from Task 6, open a second tab and a split, then run `claude` (or `vim`) in one pane:

```bash
WARP_DATA_PROFILE=fc ./target/debug/warp-fork-ctl list
WARP_DATA_PROFILE=fc ./target/debug/warp-fork-ctl list --json
WARP_DATA_PROFILE=fc ./target/debug/warp-fork-ctl find-pid "$(pgrep -n claude)"
```

Expected: one row per terminal pane with a shell pid and cwd; the claude/vim pane shows its foreground pgid and command name (`ALT` is `yes` for vim; Claude Code usually renders in the main screen, so `ALT` may be empty while `COMMAND` is `claude`); exactly one row is focused; `find-pid` returns the claude pane. `find-pid 1` → `not_found`, exit 1.

- [ ] **Step 3: Commit**

```bash
git add app/src/fork_control/handlers.rs
git commit -m "feat(fork_control): list panes and find pane by pid"
```

---

### Task 8: `focus` and `set_title`

**Files:**
- Modify: `app/src/fork_control/handlers.rs`

**Interfaces:**
- Consumes: Task 7 helpers; `Workspace::{activate_tab, active_tab_index, tab_views}`, `PaneGroup::{focus_pane_by_id, set_title, clear_title}`, `ctx.windows().show_window_and_focus_app(WindowId)`.

- [ ] **Step 1: Implement**

```rust
use serde_json::json;
use warp_fork_control::protocol::SetTitleParams;

fn focus(pane_id: u64, ctx: &mut ModelContext<ForkControlHost>) -> Result<(), ErrorBody> {
    let location = find_pane(pane_id, ctx)?;
    location
        .workspace
        .update(ctx, |workspace, ctx| workspace.activate_tab(location.tab_index, ctx));
    location
        .pane_group
        .update(ctx, |group, ctx| group.focus_pane_by_id(location.pane_id, ctx));
    ctx.windows().show_window_and_focus_app(location.window_id);
    Ok(())
}

fn find_tab(
    tab_id: u64,
    ctx: &AppContext,
) -> Result<(ViewHandle<Workspace>, usize, ViewHandle<PaneGroup>), ErrorBody> {
    sorted_workspaces(ctx)
        .into_iter()
        .find_map(|(_, workspace)| {
            let found = workspace
                .as_ref(ctx)
                .tab_views()
                .enumerate()
                .find(|(_, group)| entity_number(group.id()) == tab_id)
                .map(|(index, group)| (index, group.clone()));
            found.map(|(index, group)| (workspace.clone(), index, group))
        })
        .ok_or_else(|| not_found(format!("no tab with tab_id {tab_id}")))
}

fn set_title(params: SetTitleParams, ctx: &mut ModelContext<ForkControlHost>) -> Result<(), ErrorBody> {
    let (workspace, tab_index, pane_group) = match (params.tab_id, params.pane_id) {
        (Some(tab_id), _) => find_tab(tab_id, ctx)?,
        (None, Some(pane_id)) => {
            let location = find_pane(pane_id, ctx)?;
            (location.workspace, location.tab_index, location.pane_group)
        }
        (None, None) => {
            return Err(ErrorBody::new(ErrorCode::BadRequest, "tab_id or pane_id is required"));
        }
    };
    let title = params.title.as_deref().map(str::trim).filter(|title| !title.is_empty());
    pane_group.update(ctx, |group, ctx| match title {
        Some(title) => group.set_title(title, ctx),
        None => group.clear_title(ctx),
    });
    // PaneGroup::set_title refocuses its own focused pane; hand focus back to the active tab.
    workspace.update(ctx, |workspace, ctx| {
        let active = workspace.active_tab_index();
        if active != tab_index {
            workspace.activate_tab(active, ctx);
        }
        ctx.notify();
    });
    Ok(())
}
```

`handle` arms:

```rust
        Request::Focus(params) => focus(params.pane_id, ctx).map(|()| json!({})),
        Request::SetTitle(params) => set_title(params, ctx).map(|()| json!({})),
```

- [ ] **Step 2: Build and smoke test**

Run: `cargo check -p warp`, restart the dev instance, then:

```bash
CTL="./target/debug/warp-fork-ctl"; export WARP_DATA_PROFILE=fc
$CTL list                               # note a pane in a background tab, and its tab_id
$CTL title <background tab_id> "BG TITLE"
# while typing in the active tab: keystrokes must still land in the active tab
$CTL focus <pane in the background tab>  # tab switches, pane focused
# Cmd-H to hide Warp, or open a second window with Cmd-N and focus a pane in the first window:
$CTL focus <pane>                        # Warp comes to front, correct window/tab/pane
$CTL title <tab_id> --clear              # title goes back to automatic
$CTL focus 999999                        # not_found, exit 1
```

- [ ] **Step 3: Commit**

```bash
git add app/src/fork_control/handlers.rs
git commit -m "feat(fork_control): focus panes and set tab titles"
```

---

### Task 9: `open_tab`

**Files:**
- Modify: `app/src/fork_control/handlers.rs`

**Interfaces:**
- Consumes: `Workspace::{add_tab_with_pane_layout, active_tab_pane_group, activate_tab, active_tab_index}`, `PanesLayout::SingleTerminal`, `NewTerminalOptions`, `crate::root_view::{open_new_with_workspace_source, NewWorkspaceSource}`, `PaneGroup::{active_session_view, set_title}`, `TerminalView::execute_command_or_set_pending`, `WindowManager::{active_window, frontmost_window_id}`.

- [ ] **Step 1: Implement**

```rust
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use warp_fork_control::protocol::{OpenTabParams, OpenTabResult, WindowTarget};
use warpui::windowing::WindowManager;

use crate::pane_group::NewTerminalOptions;
use crate::root_view::{NewWorkspaceSource, open_new_with_workspace_source};
use crate::workspace::PanesLayout;

fn target_window(ctx: &AppContext) -> Option<(WindowId, ViewHandle<Workspace>)> {
    let windows = WindowManager::as_ref(ctx);
    let preferred = windows.active_window().or_else(|| windows.frontmost_window_id());
    let registry = WorkspaceRegistry::as_ref(ctx);
    preferred
        .and_then(|window_id| registry.get(window_id, ctx).map(|ws| (window_id, ws)))
        .or_else(|| sorted_workspaces(ctx).into_iter().next())
}

fn open_tab(
    params: OpenTabParams,
    ctx: &mut ModelContext<ForkControlHost>,
) -> Result<OpenTabResult, ErrorBody> {
    let cwd = PathBuf::from(&params.cwd);
    if !cwd.is_absolute() || !cwd.is_dir() {
        return Err(ErrorBody::new(
            ErrorCode::BadRequest,
            format!("cwd must be an existing absolute directory: {}", params.cwd),
        ));
    }
    let title = params.title.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned);
    let options = NewTerminalOptions {
        initial_directory: Some(cwd),
        hide_homepage: true,
        ..Default::default()
    };

    let (window_id, pane_group) = match params.window {
        WindowTarget::New => {
            let (window_id, _root) = open_new_with_workspace_source(
                NewWorkspaceSource::Session { options: Box::new(options), initial_team_uid: None },
                ctx,
            );
            let workspace = WorkspaceRegistry::as_ref(ctx)
                .get(window_id, ctx)
                .ok_or_else(|| ErrorBody::new(ErrorCode::Internal, "new window has no workspace"))?;
            let pane_group = workspace.as_ref(ctx).active_tab_pane_group().clone();
            if let Some(title) = &title {
                pane_group.update(ctx, |group, ctx| group.set_title(title, ctx));
            }
            (window_id, pane_group)
        }
        WindowTarget::Current => {
            let (window_id, workspace) = target_window(ctx).ok_or_else(|| {
                ErrorBody::new(ErrorCode::Unavailable, "no Warp window is open; use window \"new\"")
            })?;
            let previous_tab = workspace.as_ref(ctx).active_tab_index();
            let pane_group = workspace.update(ctx, |workspace, ctx| {
                workspace.add_tab_with_pane_layout(
                    PanesLayout::SingleTerminal(Box::new(options)),
                    Arc::new(HashMap::new()),
                    title.clone(),
                    ctx,
                );
                let pane_group = workspace.active_tab_pane_group().clone();
                if !params.focus {
                    workspace.activate_tab(previous_tab, ctx);
                }
                pane_group
            });
            (window_id, pane_group)
        }
    };

    let terminal = pane_group
        .as_ref(ctx)
        .active_session_view(ctx)
        .ok_or_else(|| ErrorBody::new(ErrorCode::Internal, "new tab has no terminal"))?;
    if let Some(command) = params.command.as_deref().filter(|c| !c.trim().is_empty()) {
        terminal.update(ctx, |terminal, ctx| terminal.execute_command_or_set_pending(command, ctx));
    }
    if params.focus {
        ctx.windows().show_window_and_focus_app(window_id);
    }
    let shell_pid = terminal
        .as_ref(ctx)
        .model
        .lock()
        .shell_process_info()
        .map(|shell| shell.pid);
    Ok(OpenTabResult {
        window_id: window_number(window_id),
        tab_id: entity_number(pane_group.id()),
        pane_id: entity_number(terminal.id()),
        shell_pid,
    })
}
```

`handle` arm:

```rust
        Request::OpenTab(params) => open_tab(params, ctx).and_then(to_value),
```

Import paths to verify while compiling: `PanesLayout` (search `pub enum PanesLayout`), `NewTerminalOptions` (`app/src/pane_group/mod.rs:800`), `open_new_with_workspace_source` / `NewWorkspaceSource` (`app/src/root_view.rs:919`, `:1581`). All are `pub` or `pub(crate)`; do not change their visibility.

- [ ] **Step 2: Build and smoke test**

Run: `cargo check -p warp`, restart the dev instance:

```bash
$CTL open --cwd ~/projects --cmd "claude" --title "TEST"   # new tab, claude starts, tab title TEST
$CTL list                                                     # new pane present with shell_pid
$CTL open --cwd /tmp --no-focus                               # tab created, previous tab stays active
$CTL open --cwd /tmp --cmd "echo hi" --new-window            # new window, command ran, in history (↑)
$CTL open --cwd /does/not/exist                               # bad_request
```

- [ ] **Step 3: Commit**

```bash
git add app/src/fork_control/handlers.rs
git commit -m "feat(fork_control): open tabs with cwd, command and title"
```

---

### Task 10: `send_input`

**Files:**
- Modify: `app/src/fork_control/handlers.rs`

**Interfaces:**
- Consumes: `warp_fork_control::input::{route_input, encode_pty_text, PaneActivity, InputRoute, PASTE_SUBMIT_DELAY}`; `TerminalView::{write_user_bytes_to_pty, execute_command_or_set_pending, input}`, `Input::system_insert`, `TerminalModel::{is_alt_screen_active, needs_bracketed_paste}`, `warpui::r#async::Timer`.

- [ ] **Step 1: Implement**

```rust
use warp_fork_control::input::{
    InputRoute, PASTE_SUBMIT_DELAY, PaneActivity, encode_pty_text, route_input,
};
use warp_fork_control::protocol::{InputMode, SendInputParams, SendInputResult};
use warpui::r#async::Timer;

fn send_input(
    params: SendInputParams,
    ctx: &mut ModelContext<ForkControlHost>,
) -> Result<SendInputResult, ErrorBody> {
    let location = find_pane(params.pane_id, ctx)?;
    let (activity, is_alt_screen, bracketed) = {
        let terminal = location.terminal.as_ref(ctx);
        let activity = match terminal.session_command_context(ctx) {
            CommandContext::RunningCommand { .. } => PaneActivity::RunningCommand,
            CommandContext::RunningAIBlock { .. } => PaneActivity::WarpAgentRunning,
            _ => PaneActivity::AtPrompt,
        };
        let mut model = terminal.model.lock();
        (activity, model.is_alt_screen_active(), model.needs_bracketed_paste())
    };
    let route = route_input(activity, is_alt_screen, params.allow_shell)?;

    match route {
        InputRoute::Pty => {
            let bytes = encode_pty_text(&params.text, params.mode, bracketed);
            let submit_delay =
                (params.mode == InputMode::Paste && bracketed && !bytes.is_empty())
                    .then_some(PASTE_SUBMIT_DELAY);
            let submit = params.submit;
            let delivered = location.terminal.update(ctx, |terminal, ctx| {
                if !bytes.is_empty() && !terminal.write_user_bytes_to_pty(bytes, ctx) {
                    return false;
                }
                if submit {
                    match submit_delay {
                        Some(delay) => {
                            ctx.spawn(Timer::after(delay), |terminal, _, ctx| {
                                terminal.write_user_bytes_to_pty(b"\r".to_vec(), ctx);
                            });
                        }
                        None => {
                            if !terminal.write_user_bytes_to_pty(b"\r".to_vec(), ctx) {
                                return false;
                            }
                        }
                    }
                }
                true
            });
            if !delivered {
                return Err(ErrorBody::new(
                    ErrorCode::PaneBusy,
                    "a Warp agent controls this pane's input",
                ));
            }
        }
        InputRoute::InputEditor => {
            location.terminal.update(ctx, |terminal, ctx| {
                if params.submit {
                    terminal.execute_command_or_set_pending(&params.text, ctx);
                } else {
                    terminal
                        .input()
                        .update(ctx, |input, ctx| input.system_insert(&params.text, ctx));
                }
            });
        }
    }
    Ok(SendInputResult { delivered_to: route })
}
```

`handle` arm:

```rust
        Request::SendInput(params) => send_input(params, ctx).and_then(to_value),
```

Remove the `_ => Err("not implemented yet")` fallback arm — `handle` now matches every `Request` variant.

If `Timer` lives elsewhere, use the import from `app/src/terminal/view/use_agent_footer/mod.rs:39` (`use warpui::r#async::Timer;`).

- [ ] **Step 2: Build and smoke test**

Run: `cargo check -p warp`, restart the dev instance:

```bash
CLAUDE_PANE=$($CTL find-pid "$(pgrep -n claude)" | jq .pane_id)
$CTL send "$CLAUDE_PANE" $'line one\nline two' --submit      # arrives as ONE message in Claude Code
$CTL list                                                     # pick a pane at a shell prompt → SHELL_PANE
$CTL send "$SHELL_PANE" "echo nope"                           # not_in_tui, nothing typed
$CTL send "$SHELL_PANE" "echo yes" --allow-shell              # text appears in Warp's input editor, not run
$CTL send "$SHELL_PANE" "echo yes" --allow-shell --submit     # runs as a normal block, visible in history
$CTL send "$CLAUDE_PANE" "" --submit                          # a bare Enter
```

- [ ] **Step 3: Commit**

```bash
git add app/src/fork_control/handlers.rs
git commit -m "feat(fork_control): send input to TUI panes and the input editor"
```

---

### Task 11: Integration test

**Files:**
- Create: `crates/integration/src/test/fork_control.rs`
- Modify: `crates/integration/src/test.rs`, `crates/integration/src/bin/integration.rs`, `crates/integration/Cargo.toml`

**Interfaces:**
- Consumes: `warp_fork_control::client::call`, `warp::integration_testing::{step::new_step_with_default_assertions, terminal::wait_until_bootstrapped_single_pane_for_tab}`, `warpui_core::{async_assert, integration::TestStep}`.

- [ ] **Step 1: Write the test**

`crates/integration/Cargo.toml` `[dependencies]`:

```toml
warp_fork_control.workspace = true # fork_control:
```

`crates/integration/src/test/fork_control.rs`:

```rust
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use warp::integration_testing::step::new_step_with_default_assertions;
use warp::integration_testing::terminal::wait_until_bootstrapped_single_pane_for_tab;
use warpui_core::async_assert;
use warpui_core::integration::TestStep;

use super::new_builder;
use crate::Builder;

fn socket_path() -> PathBuf {
    std::env::temp_dir()
        .join(format!("wfc-{}", std::process::id()))
        .join("control.sock")
}

/// Drives the fork control API from a background thread (the UI thread must stay
/// free to answer) and checks the results once they arrive.
pub fn test_fork_control_api() -> Builder {
    let socket = socket_path();
    // SAFETY: runs before the app starts, while the test process is single-threaded.
    unsafe { std::env::set_var(warp_fork_control::paths::SOCKET_ENV, &socket) };
    let results: Arc<Mutex<Option<Vec<Value>>>> = Arc::default();
    let writer = results.clone();
    let reader = results.clone();
    let cwd = std::env::temp_dir();

    new_builder()
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(TestStep::new("fork_control: drive the API").with_action(
            move |_app, _window_id, _step_data| {
                let socket = socket.clone();
                let writer = writer.clone();
                let cwd = cwd.clone();
                std::thread::spawn(move || {
                    let call = |method: &str, params: Value| {
                        warp_fork_control::client::call(&socket, method, params)
                            .expect("fork control call should succeed")
                    };
                    let ping = call("ping", json!({}));
                    let list = call("list", json!({}));
                    let first_pane = list["result"]["panes"][0]["pane_id"].clone();
                    let open = call(
                        "open_tab",
                        json!({"cwd": cwd, "title": "FC-TEST", "command": "echo fork-control"}),
                    );
                    let at_prompt = call("send_input", json!({"pane_id": first_pane, "text": "x"}));
                    let missing = call("focus", json!({"pane_id": 999_999_999u64}));
                    *writer.lock().unwrap() = Some(vec![ping, list, open, at_prompt, missing]);
                });
            },
        ))
        .with_step(
            new_step_with_default_assertions("fork_control: results").add_assertion(
                move |_app, _window_id| {
                    let guard = reader.lock().unwrap();
                    let Some(results) = guard.as_ref() else {
                        return async_assert!(false, "waiting for fork control results");
                    };
                    let [ping, list, open, at_prompt, missing] = results.as_slice() else {
                        return async_assert!(false, "expected 5 results");
                    };
                    async_assert!(
                        ping["ok"] == json!(true)
                            && ping["api_version"] == json!(1)
                            && list["result"]["panes"].as_array().is_some_and(|p| p.len() == 1)
                            && list["result"]["panes"][0]["shell_pid"].is_u64()
                            && open["ok"] == json!(true)
                            && open["result"]["pane_id"].is_u64()
                            && at_prompt["error"]["code"] == json!("not_in_tui")
                            && missing["error"]["code"] == json!("not_found"),
                        "unexpected results: {results:?}"
                    )
                },
            ),
        )
        .with_step(
            new_step_with_default_assertions("fork_control: new tab has the custom title")
                .add_assertion(|app, window_id| {
                    warp::integration_testing::tab::assert_tab_title(app, window_id, 1, "FC-TEST")
                }),
        )
}
```

If `warp::integration_testing` has no tab-title assertion helper, replace the last step with an assertion that reads the workspace (see how `app/src/integration_testing/pane_group/assertions.rs` reads views) and compares `pane_group.display_title(ctx)` of tab index 1 with `"FC-TEST"`; add that helper next to the other fork-owned test code in the test file, not in upstream files. Match `async_assert!`'s real signature from `crates/warpui_core` (check an existing use such as `crates/integration/src/test/block_filtering.rs`).

`crates/integration/src/test.rs`, next to `mod remote_control;` / `pub use remote_control::*;`:

```rust
mod fork_control; // fork_control:
pub use fork_control::*; // fork_control:
```

`crates/integration/src/bin/integration.rs`, next to `register_test!(test_remote_control_split_and_run);`:

```rust
    register_test!(test_fork_control_api); // fork_control:
```

- [ ] **Step 2: Run it**

Run: `cargo run -p integration --bin integration -- test_fork_control_api` (check `crates/integration` README / the gui-integration-test skill for the exact runner invocation).
Expected: PASS. If it fails, read the assertion message — it prints all 5 responses.

- [ ] **Step 3: Commit**

```bash
git add crates/integration Cargo.lock
git commit -m "test(fork_control): integration test over the socket"
```

---

### Task 12: Contract documentation

**Files:**
- Create: `docs/fork-control-api.md`
- Modify: `README.md` (fork features section: one bullet linking the doc)

- [ ] **Step 1: Write `docs/fork-control-api.md`**

Sections, in order, with concrete examples copied from real `warp-fork-ctl` / `socat` sessions from Tasks 6–10:

1. **What it is** — fork-only, local-only; not upstream `warpctrl`.
2. **Socket** — paths for macOS/Linux, data profiles, `WARP_FORK_CONTROL_SOCKET`, 0700/0600, peer UID check, stale-socket behavior.
3. **Enable / disable** — `fork.control_api.enabled` in the settings file (live), `WARP_FORK_CONTROL=0`.
4. **Wire format** — one JSON object per line each way; request/response envelopes; `id` echo; `api_version`; requests on one connection are answered in order; a bad line does not close the connection.
5. **Methods** — for each of `ping`, `list`, `focus`, `open_tab`, `set_title`, `send_input`, `find_by_pid`: params table (name, type, default, meaning), result shape, errors it can return, one request/response example.
6. **Error codes** — table of all 8 codes with when they occur.
7. **Guarantees** — ID stability (lifetime only, not across restarts), only terminal panes listed, all actions run on the UI thread, 5 s UI timeout, `open_tab` may return `shell_pid: null` and the command runs once the shell bootstraps.
8. **send_input rules** — the routing table from the spec, bracketed paste, 300 ms submit delay, `keys` mode, `allow_shell` behavior, Warp agent blocks → `pane_busy`.
9. **Recipes for the session board** — (a) focus existing session: `find_by_pid` → `focus`; (b) open a session: `open_tab {cwd, command: "claude -r <id>", title}`; (c) deliver a reply: `find_by_pid` → `send_input {text, submit: true}`; (d) restore pinned sessions after reboot: loop `open_tab`.
10. **Minimal Node client** — ~20 lines using `net.createConnection(path)` + line splitting.
11. **Limitations** — no events/subscriptions, no screen reading, no tab closing; macOS/Linux only; bracketed paste only when the program enables it.

`README.md`: in the fork features list add one bullet: "Local control API for external tools (`warp-fork-ctl`, see docs/fork-control-api.md)" with `<!-- fork_control -->` if that section has no other marker convention.

- [ ] **Step 2: Verify examples**

Re-run each example in the doc against the dev instance; every example output in the doc must match the real shape.

- [ ] **Step 3: Commit**

```bash
git add docs/fork-control-api.md README.md
git commit -m "docs(fork_control): document the control API contract"
```

---

### Task 13: Full verification

- [ ] **Step 1: Automated checks**

```bash
cargo fmt --all -- --check
cargo test -p warp_fork_control
cargo test -p warp --lib fork_control
cargo test -p warp --lib settings
cargo check --workspace --tests
cargo clippy -p warp_fork_control -p warp --tests -- -D warnings
./script/check_no_inline_test_modules
```

Expected: all green. Report exact output for anything that fails.

- [ ] **Step 2: Manual scenario on the built app** (spec checks 1–8)

1. `list` shows all tabs/panes with pid, cwd, alt-screen.
2. `open --cwd <dir> --cmd "claude" --title "TEST"` → new tab, claude running, title TEST.
3. `find-pid <claude pid>` → that pane; `focus` switches to it from another tab and from another window; with Warp hidden/behind, it comes to front.
4. `send <pane> $'multi\nline' --submit` in the claude pane → one message.
5. `send` to a shell-prompt pane without `--allow-shell` → `not_in_tui`, nothing typed.
6. Invalid JSON (`echo '{oops' | socat - UNIX-CONNECT:<sock>`), unknown pane_id, and a tab closed right before `send` → proper errors; app alive.
7. Set `fork.control_api.enabled = false` in the settings file → socket disappears without restart; set back → socket returns. `WARP_FORK_CONTROL=0` at launch → no socket.
8. `ls -l` on the socket → `srw-------`.
9. Regression pass: open/close tabs, splits, blocks, input editor typing, tab rename via UI — unchanged.

- [ ] **Step 3: Review gates** (substantial tier: local IPC)

`/codex:review --base master` and an Opus diff review focused on the socket security and UI-thread safety; address findings.

- [ ] **Step 4: Final summary for the user**

What was built, hook points (`rg -n "fork_control:"`), how to build/enable, socket path, link to `docs/fork-control-api.md`, known limitations, what to watch on upstream rebase.
