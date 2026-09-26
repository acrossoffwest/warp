use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::restore::is_env_assignment;

pub const COMMAND_PREVIEW_MAX_CHARS: usize = 120;

pub fn is_internal_warp_command(command: &str) -> bool {
    let command = command.trim();
    command.contains("WARP_BOOTSTRAP_VAR")
        || command.contains("WARP_SESSION_ID=")
        || command.contains("_warp_emit_exit_shell")
        || command.contains("OSC_START_GENERATOR_OUTPUT")
}

pub fn user_command(command: Option<&str>) -> Option<String> {
    let command = command?.trim();
    if command.is_empty() || is_internal_warp_command(command) {
        return None;
    }

    Some(command.to_owned())
}

pub(crate) fn is_valid_session_id(id: &str) -> bool {
    const DASH_POSITIONS: [usize; 4] = [8, 13, 18, 23];
    let bytes = id.as_bytes();
    bytes.len() == 36
        && bytes
            .iter()
            .enumerate()
            .all(|(index, &byte)| match DASH_POSITIONS.contains(&index) {
                true => byte == b'-',
                false => byte.is_ascii_hexdigit(),
            })
}

pub fn command_preview(command: Option<&str>) -> Option<String> {
    let command = user_command(command)?;
    let first_line = command.lines().next().unwrap_or_default().trim();
    if first_line.is_empty() {
        return None;
    }

    let mut preview: String = first_line.chars().take(COMMAND_PREVIEW_MAX_CHARS).collect();
    if first_line.chars().count() > COMMAND_PREVIEW_MAX_CHARS {
        preview.push_str("...");
    }
    Some(preview)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SessionMemorySource {
    WarpTerminal,
    ClaudeCode,
    Codex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionMemoryKind {
    Terminal,
    AgentChat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionMemoryStatus {
    Live,
    Blocked,
    Success,
    UserClosed,
    Interrupted,
    Stale,
    Unknown,
}

impl SessionMemoryStatus {
    pub fn classify_startup(self, closed_intentionally_at: Option<i64>) -> Self {
        match (self, closed_intentionally_at) {
            (SessionMemoryStatus::Live, None) => SessionMemoryStatus::Interrupted,
            (SessionMemoryStatus::Live, Some(_)) => SessionMemoryStatus::UserClosed,
            (status, None | Some(_)) => status,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentPermissionMode {
    Normal,
    Dangerous,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMemoryRunState {
    pub current_run_id: String,
    pub previous_run_id: Option<String>,
    pub recoverable_run_id: Option<String>,
    #[serde(default)]
    pub current_run_started_at: i64,
    #[serde(default)]
    pub previous_run_started_at: Option<i64>,
}

impl SessionMemoryRunState {
    pub fn new(current_run_id: impl Into<String>, recoverable_run_id: Option<String>) -> Self {
        let recoverable_run_id = recoverable_run_id;
        Self {
            current_run_id: current_run_id.into(),
            previous_run_id: recoverable_run_id.clone(),
            recoverable_run_id,
            current_run_started_at: chrono::Utc::now().timestamp(),
            previous_run_started_at: None,
        }
    }

    pub fn with_previous_run(
        current_run_id: impl Into<String>,
        previous_run_id: Option<String>,
        recoverable_run_id: Option<String>,
    ) -> Self {
        Self {
            current_run_id: current_run_id.into(),
            previous_run_id,
            recoverable_run_id,
            current_run_started_at: chrono::Utc::now().timestamp(),
            previous_run_started_at: None,
        }
    }

    pub fn with_run_starts(
        mut self,
        current_run_started_at: i64,
        previous_run_started_at: Option<i64>,
    ) -> Self {
        self.current_run_started_at = current_run_started_at;
        self.previous_run_started_at = previous_run_started_at;
        self
    }

    pub fn test_default() -> Self {
        Self::new("test-run", None)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalAgentCommand {
    pub source: SessionMemorySource,
    pub permission_mode: AgentPermissionMode,
}

pub fn terminal_agent_command(command: Option<&str>) -> Option<TerminalAgentCommand> {
    let command = user_command(command)?;
    let command_token = command
        .split_whitespace()
        .find(|token| !is_env_assignment(token))?;

    let source = match command_token {
        "claude" => SessionMemorySource::ClaudeCode,
        "codex" => SessionMemorySource::Codex,
        _ => return None,
    };
    if is_agent_maintenance_command(source, &command) {
        return None;
    }

    let dangerous_flag = match source {
        SessionMemorySource::ClaudeCode => "--dangerously-skip-permissions",
        SessionMemorySource::Codex => "--dangerously-bypass-approvals-and-sandbox",
        SessionMemorySource::WarpTerminal => return None,
    };
    let permission_mode = if command
        .split_whitespace()
        .any(|token| token == dangerous_flag)
    {
        AgentPermissionMode::Dangerous
    } else {
        AgentPermissionMode::Normal
    };

    Some(TerminalAgentCommand {
        source,
        permission_mode,
    })
}

pub fn is_agent_maintenance_command(source: SessionMemorySource, command: &str) -> bool {
    let subcommand = command
        .split_whitespace()
        .skip_while(|token| is_env_assignment(token))
        .nth(1);
    match (source, subcommand) {
        (
            SessionMemorySource::ClaudeCode,
            Some(
                "update" | "mcp" | "setup-token" | "doctor" | "config" | "install"
                | "migrate-installer",
            ),
        ) => true,
        (SessionMemorySource::Codex, Some("login" | "logout" | "mcp" | "completion" | "apply")) => {
            true
        }
        _ => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMemoryRecord {
    pub id: String,
    pub source: SessionMemorySource,
    pub kind: SessionMemoryKind,
    pub status: SessionMemoryStatus,
    pub title: String,
    pub summary: Option<String>,
    pub cwd: Option<PathBuf>,
    pub project: Option<String>,
    pub native_session_id: Option<String>,
    pub transcript_path: Option<PathBuf>,
    pub terminal_pane_uuid: Option<Vec<u8>>,
    pub app_window_fingerprint: Option<String>,
    pub app_tab_fingerprint: Option<String>,
    pub last_command: Option<String>,
    pub last_exit_code: Option<i32>,
    pub launch_argv: Option<Vec<String>>,
    pub permission_mode: AgentPermissionMode,
    pub last_seen_at: i64,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub closed_intentionally_at: Option<i64>,
    pub app_run_id: Option<String>,
    pub recovery_offered_run_id: Option<String>,
    pub restore_payload: Option<serde_json::Value>,
}

impl SessionMemoryRecord {
    pub fn is_agent(&self) -> bool {
        matches!(
            self.source,
            SessionMemorySource::ClaudeCode | SessionMemorySource::Codex
        )
    }

    pub fn keep_agent_end(&mut self, existing: &SessionMemoryRecord) {
        let Some(completed_at) = existing.completed_at else {
            return;
        };
        if self.completed_at.is_some() {
            return;
        }
        let new_agent_started =
            self.is_agent() && self.started_at.is_some() && self.started_at != existing.started_at;
        if new_agent_started {
            return;
        }
        self.completed_at = Some(completed_at);
        if self.started_at.is_none() {
            self.started_at = existing.started_at;
        }
        if !self.is_agent() && existing.is_agent() {
            self.source = existing.source;
            self.kind = existing.kind;
            self.native_session_id = existing.native_session_id.clone();
            self.title = existing.title.clone();
            self.transcript_path = existing.transcript_path.clone();
            self.launch_argv = existing.launch_argv.clone();
            self.permission_mode = existing.permission_mode;
        }
        if self.is_agent() {
            self.status = SessionMemoryStatus::Success;
        }
    }

    pub fn is_interrupted(&self) -> bool {
        self.status == SessionMemoryStatus::Interrupted
    }

    pub fn matches_query(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return true;
        }

        [
            Some(self.title.as_str()),
            self.summary.as_deref(),
            self.cwd.as_ref().and_then(|path| path.to_str()),
            self.project.as_deref(),
            self.last_command.as_deref(),
            self.native_session_id.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|value| value.to_lowercase().contains(&query))
    }
}
