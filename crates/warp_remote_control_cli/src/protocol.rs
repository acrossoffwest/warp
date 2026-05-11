//! Local mirror of the bincode wire types from `app/src/remote_control/service.rs`.
//!
//! These are duplicated rather than imported from the `warp` crate because the
//! `warp` crate is the entire Warp application library and pulling it as a
//! dependency would make this CLI build the whole app. The bincode wire format
//! is structural — as long as variant order, field order, and field types match
//! the source of truth, the bytes will round-trip. Re-derive carefully when
//! changing the source.
//!
//! Source of truth: `app/src/remote_control/service.rs` on branch
//! `feat/remote-control`.

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

/// The service ID as registered by the Warp server.
///
/// This MUST match `std::any::type_name::<RemoteControlService>()` as evaluated
/// in the `app` crate, where the struct lives at
/// `warp::remote_control::service::RemoteControlService`.
pub const SERVICE_ID: &str = "warp::remote_control::service::RemoteControlService";
