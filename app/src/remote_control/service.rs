use ipc::Service;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SplitDirection {
    Right,
    Down,
    Left,
    Up,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SendCommandMode {
    Shell,
    Pty,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RemoteControlRequest {
    SplitActivePaneAndRun {
        command: String,
        direction: SplitDirection,
    },
    Ping,
    ListPanes,
    SplitPane {
        direction: SplitDirection,
        label: Option<String>,
    },
    SendCommandToPane {
        pane_id: String,
        command: String,
        mode: SendCommandMode,
    },
    ClosePane {
        pane_id: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum PaneCommandStatus {
    Idle,
    RunningCommand,
    LastCommand,
    AiBlock,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RemoteControlAgent {
    Codex,
    ClaudeCode,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemotePaneInfo {
    pub pane_id: String,
    pub label: Option<String>,
    pub focused: bool,
    pub available_for_command: bool,
    pub status: PaneCommandStatus,
    pub command: Option<String>,
    pub agent: Option<RemoteControlAgent>,
    pub is_likely_agent_host: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RemoteControlResponse {
    Ok,
    Pong,
    Error { message: String },
    Panes { panes: Vec<RemotePaneInfo> },
    PaneCreated { pane_id: String },
}

pub struct RemoteControlService {}
impl Service for RemoteControlService {
    type Request = RemoteControlRequest;
    type Response = RemoteControlResponse;
}
