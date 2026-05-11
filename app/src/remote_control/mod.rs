mod server;
mod service;

pub use server::{socket_address_path, start, RemoteControlServerHandle};
pub use service::{
    RemoteControlRequest, RemoteControlResponse, RemoteControlService, SplitDirection,
};

use async_trait::async_trait;
use ipc::ServiceImpl;
use service::{RemoteControlRequest as Req, RemoteControlResponse as Resp};
use warpui::{Entity, ModelContext, SingletonEntity};

/// Singleton model that keeps the remote-control IPC server alive for the
/// lifetime of the application.  Dropping this struct shuts down the server.
pub struct RemoteControlHost {
    _server: ipc::Server,
    /// Keep the stream alive so it doesn't cancel.
    _drain: warpui::r#async::SpawnedLocalStream,
}

impl RemoteControlHost {
    pub(crate) fn new(
        server: ipc::Server,
        action_rx: async_channel::Receiver<PendingAction>,
        ctx: &mut ModelContext<Self>,
    ) -> Self {
        use crate::workspace::{Workspace, WorkspaceAction};
        use warpui::{SingletonEntity as _, TypedActionView as _};

        let drain = ctx.spawn_stream_local(
            action_rx,
            move |_this, action, ctx| {
                let workspace_action = match action {
                    PendingAction::SplitActiveAndRun { command, direction } => {
                        WorkspaceAction::RemoteControlSplitAndRun { command, direction }
                    }
                };
                // Route to the active workspace window, mirroring dispatch_to_active_workspace.
                use warpui::windowing::WindowManager;
                if let Some(window_id) = WindowManager::as_ref(ctx).active_window() {
                    if let Some(workspaces) = ctx.views_of_type::<Workspace>(window_id) {
                        if let Some(workspace) = workspaces.into_iter().next() {
                            workspace.update(ctx, |ws, ctx| {
                                ws.handle_action(&workspace_action, ctx);
                            });
                        }
                    }
                }
            },
            |_, _| {
                log::info!("remote_control: action stream closed");
            },
        );

        Self {
            _server: server,
            _drain: drain,
        }
    }
}

impl Entity for RemoteControlHost {
    type Event = ();
}

impl SingletonEntity for RemoteControlHost {}

#[derive(Clone)]
pub(crate) struct RemoteControlServiceImpl {
    pub(crate) action_tx: async_channel::Sender<PendingAction>,
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
