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
