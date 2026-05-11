mod server;
mod service;

pub use server::{socket_address_path, start, RemoteControlServerHandle};
pub use service::{
    PaneCommandStatus, RemoteControlAgent, RemoteControlRequest, RemoteControlResponse,
    RemoteControlService, RemotePaneInfo, SendCommandMode, SplitDirection,
};

use async_channel::Sender;
use async_trait::async_trait;
use ipc::ServiceImpl;
use service::{RemoteControlRequest as Req, RemoteControlResponse as Resp};
use std::collections::HashMap;
use warpui::{Entity, ModelContext, SingletonEntity};

/// Singleton model that keeps the remote-control IPC server alive for the
/// lifetime of the application.  Dropping this struct shuts down the server.
pub struct RemoteControlHost {
    _server: ipc::Server,
    /// Keep the stream alive so it doesn't cancel.
    _drain: warpui::r#async::SpawnedLocalStream,
    remote_panes: HashMap<String, RemotePaneBinding>,
}

impl RemoteControlHost {
    pub(crate) fn new(
        server: ipc::Server,
        job_rx: async_channel::Receiver<RemoteControlJob>,
        ctx: &mut ModelContext<Self>,
    ) -> Self {
        let drain = ctx.spawn_stream_local(
            job_rx,
            move |_this, job, _ctx| {
                let response = match job.request {
                    Req::Ping => Resp::Pong,
                    _ => Resp::Error {
                        message: "remote pane management not implemented yet".to_owned(),
                    },
                };
                let _ = job.response_tx.try_send(response);
            },
            |_, _| {
                log::info!("remote_control: job stream closed");
            },
        );

        Self {
            _server: server,
            _drain: drain,
            remote_panes: HashMap::new(),
        }
    }
}

impl Entity for RemoteControlHost {
    type Event = ();
}

impl SingletonEntity for RemoteControlHost {}

#[derive(Clone)]
pub(crate) struct RemoteControlServiceImpl {
    pub(crate) job_tx: async_channel::Sender<RemoteControlJob>,
}

#[derive(Debug)]
pub(crate) struct RemoteControlJob {
    pub(crate) request: Req,
    pub(crate) response_tx: Sender<Resp>,
}

#[derive(Clone, Debug)]
struct RemotePaneBinding {
    pane_id: crate::pane_group::PaneId,
    label: Option<String>,
}

#[async_trait]
impl ServiceImpl for RemoteControlServiceImpl {
    type Service = RemoteControlService;

    async fn handle_request(&self, request: Req) -> Resp {
        if matches!(request, Req::Ping) {
            return Resp::Pong;
        }

        let (response_tx, response_rx) = async_channel::bounded(1);
        match self.job_tx.try_send(RemoteControlJob {
            request,
            response_tx,
        }) {
            Ok(()) => response_rx.recv().await.unwrap_or_else(|e| Resp::Error {
                message: format!("remote control response failed: {e}"),
            }),
            Err(e) => Resp::Error {
                message: format!("dispatch failed: {e}"),
            },
        }
    }
}
