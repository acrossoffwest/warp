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
