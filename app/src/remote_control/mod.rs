mod server;
mod service;

pub use server::{socket_address_path, start, RemoteControlServerHandle};
pub use service::{
    RemoteControlRequest, RemoteControlResponse, RemoteControlService, SplitDirection,
};

use async_trait::async_trait;
use ipc::ServiceImpl;
use service::{RemoteControlRequest as Req, RemoteControlResponse as Resp};
use warpui::{Entity, SingletonEntity};

/// Singleton model that keeps the remote-control IPC server alive for the
/// lifetime of the application.  Dropping this struct shuts down the server.
pub struct RemoteControlHost {
    _server: ipc::Server,
}

impl RemoteControlHost {
    pub(crate) fn new(server: ipc::Server) -> Self {
        Self { _server: server }
    }
}

impl Entity for RemoteControlHost {
    type Event = ();
}

impl SingletonEntity for RemoteControlHost {}

#[derive(Clone)]
pub(crate) struct RemoteControlServiceImpl {
    pub(crate) action_tx: std::sync::mpsc::SyncSender<PendingAction>,
}

#[derive(Debug)]
pub(crate) enum PendingAction {
    SplitActiveAndRun {
        command: String,
        direction: SplitDirection,
    },
}

#[async_trait]
impl ServiceImpl for RemoteControlServiceImpl {
    type Service = RemoteControlService;

    async fn handle_request(&self, request: Req) -> Resp {
        match request {
            Req::Ping => Resp::Pong,
            Req::SplitActivePaneAndRun { command, direction } => {
                match self
                    .action_tx
                    .try_send(PendingAction::SplitActiveAndRun { command, direction })
                {
                    Ok(()) => Resp::Ok,
                    Err(e) => Resp::Error {
                        message: format!("dispatch failed: {e}"),
                    },
                }
            }
        }
    }
}
