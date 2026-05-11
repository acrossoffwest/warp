use ipc::Service;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SplitDirection {
    Right,
    Down,
    Left,
    Up,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RemoteControlRequest {
    SplitActivePaneAndRun {
        command: String,
        direction: SplitDirection,
    },
    Ping,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RemoteControlResponse {
    Ok,
    Pong,
    Error { message: String },
}

pub enum RemoteControlService {}
impl Service for RemoteControlService {
    type Request = RemoteControlRequest;
    type Response = RemoteControlResponse;
}
