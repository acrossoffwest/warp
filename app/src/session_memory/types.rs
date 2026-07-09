use std::path::PathBuf;

use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
}

impl SessionMemoryRunState {
    pub fn new(current_run_id: impl Into<String>, recoverable_run_id: Option<String>) -> Self {
        let recoverable_run_id = recoverable_run_id;
        Self {
            current_run_id: current_run_id.into(),
            previous_run_id: recoverable_run_id.clone(),
            recoverable_run_id,
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
        }
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
    let command_token = command.split_whitespace().find(|token| {
        token.split_once('=').map(|(name, _)| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
        }) != Some(true)
    })?;

    let source = match command_token {
        "claude" => SessionMemorySource::ClaudeCode,
        "codex" => SessionMemorySource::Codex,
        _ => return None,
    };

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
    pub fn normalize_terminal_agent_command(&mut self) {
        if self.kind != SessionMemoryKind::Terminal
            || self.source != SessionMemorySource::WarpTerminal
        {
            return;
        }

        let Some(agent_command) = terminal_agent_command(self.last_command.as_deref()) else {
            return;
        };

        self.source = agent_command.source;
        self.permission_mode = agent_command.permission_mode;
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
