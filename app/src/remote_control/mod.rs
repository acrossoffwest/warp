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
                // External callers often make Warp inactive before the action is
                // drained, so fall back to the last frontmost window, then any
                // remaining window.
                use warpui::windowing::WindowManager;
                let windows = WindowManager::as_ref(ctx);
                let target_window = windows
                    .active_window()
                    .or_else(|| windows.frontmost_window_id())
                    .or_else(|| ctx.window_ids().next());

                if let Some(workspace) = target_window
                    .and_then(|window_id| ctx.views_of_type::<Workspace>(window_id))
                    .and_then(|workspaces| workspaces.into_iter().next())
                {
                    workspace.update(ctx, |ws, ctx| {
                        ws.handle_action(&workspace_action, ctx);
                    });
                } else {
                    log::warn!("remote_control: no workspace available for action");
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
