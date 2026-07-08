use std::path::PathBuf;

use serde::{Deserialize, Serialize};

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
    pub restore_payload: Option<serde_json::Value>,
}

impl SessionMemoryRecord {
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
