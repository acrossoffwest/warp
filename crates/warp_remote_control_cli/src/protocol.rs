//! Local mirror of the bincode wire types from `app/src/remote_control/service.rs`.
//!
//! These are duplicated rather than imported from the `warp` crate because the
//! `warp` crate is the entire Warp application library and pulling it as a
//! dependency would make this CLI build the whole app. The bincode wire format
//! is structural — as long as variant order, field names, and types match
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

/// The service ID as registered by the Warp server.
///
/// This MUST match `std::any::type_name::<RemoteControlService>()` as evaluated
/// in the `app` crate, where the struct lives at
/// `warp::remote_control::service::RemoteControlService`.
pub const SERVICE_ID: &str = "warp::remote_control::service::RemoteControlService";
