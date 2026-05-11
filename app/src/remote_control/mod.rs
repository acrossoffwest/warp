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
use uuid::Uuid;
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
            move |this, job, ctx| {
                let response = this.handle_job_request(job.request, ctx);
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

    fn target_workspace(
        ctx: &mut ModelContext<Self>,
    ) -> Option<warpui::ViewHandle<crate::workspace::Workspace>> {
        use crate::workspace::Workspace;
        use warpui::windowing::WindowManager;

        // External callers often make Warp inactive before the job is drained,
        // so fall back to the last frontmost window, then any remaining window.
        let windows = WindowManager::as_ref(ctx);
        let target_window = windows
            .active_window()
            .or_else(|| windows.frontmost_window_id())
            .or_else(|| ctx.window_ids().next());

        target_window
            .and_then(|window_id| ctx.views_of_type::<Workspace>(window_id))
            .and_then(|workspaces| workspaces.into_iter().next())
    }

    fn remote_id_for_pane(
        &mut self,
        pane_id: crate::pane_group::PaneId,
        label: Option<String>,
    ) -> String {
        if let Some((id, binding)) = self
            .remote_panes
            .iter_mut()
            .find(|(_, binding)| binding.pane_id == pane_id)
        {
            if label.is_some() {
                binding.label = label;
            }
            return id.clone();
        }

        let id = Uuid::new_v4().to_string();
        self.remote_panes.insert(
            id.clone(),
            RemotePaneBinding {
                pane_id,
                label,
            },
        );
        id
    }

    fn binding_for_remote_id(&self, remote_id: &str) -> Option<RemotePaneBinding> {
        self.remote_panes.get(remote_id).cloned()
    }

    fn handle_job_request(&mut self, request: Req, ctx: &mut ModelContext<Self>) -> Resp {
        let Some(workspace) = Self::target_workspace(ctx) else {
            log::warn!("remote_control: no workspace available for request");
            return Resp::Error {
                message: "no workspace available".to_owned(),
            };
        };

        match request {
            Req::Ping => Resp::Pong,
            Req::SplitActivePaneAndRun { command, direction } => {
                self.handle_split_active_pane_and_run(workspace, direction, command, ctx)
            }
            Req::ListPanes => workspace.update(ctx, |workspace, ctx| {
                let active_pane_group = workspace.active_tab_pane_group().clone();
                let panes = active_pane_group.update(ctx, |pane_group, ctx| {
                    let pane_ids: Vec<_> = pane_group.pane_ids().collect();
                    pane_ids
                        .into_iter()
                        .filter_map(|pane_id| {
                            let remote_id = self.remote_id_for_pane(pane_id, None);
                            let label = self
                                .remote_panes
                                .get(&remote_id)
                                .and_then(|binding| binding.label.clone());
                            pane_group.remote_control_pane_info(pane_id, remote_id, label, ctx)
                        })
                        .collect()
                });
                Resp::Panes { panes }
            }),
            Req::SplitPane { direction, label } => workspace.update(ctx, |workspace, ctx| {
                let pane_id = workspace.remote_control_split_pane(direction, ctx);
                let remote_id = self.remote_id_for_pane(pane_id, label);
                Resp::PaneCreated { pane_id: remote_id }
            }),
            Req::SendCommandToPane {
                pane_id,
                command,
                mode,
            } => {
                let Some(binding) = self.binding_for_remote_id(&pane_id) else {
                    return Resp::Error {
                        message: format!("unknown pane_id: {pane_id}"),
                    };
                };

                match workspace.update(ctx, |workspace, ctx| {
                    workspace.remote_control_send_command_to_pane(
                        binding.pane_id,
                        command,
                        mode,
                        ctx,
                    )
                }) {
                    Ok(()) => Resp::Ok,
                    Err(message) => Resp::Error { message },
                }
            }
            Req::ClosePane { pane_id } => {
                let Some(binding) = self.binding_for_remote_id(&pane_id) else {
                    self.remote_panes.remove(&pane_id);
                    return Resp::Error {
                        message: format!("unknown pane_id: {pane_id}"),
                    };
                };

                match workspace.update(ctx, |workspace, ctx| {
                    workspace.remote_control_close_pane(binding.pane_id, ctx)
                }) {
                    Ok(()) => {
                        self.remote_panes.remove(&pane_id);
                        Resp::Ok
                    }
                    Err(message) => Resp::Error { message },
                }
            }
        }
    }

    fn handle_split_active_pane_and_run(
        &mut self,
        workspace: warpui::ViewHandle<crate::workspace::Workspace>,
        direction: SplitDirection,
        command: String,
        ctx: &mut ModelContext<Self>,
    ) -> Resp {
        use crate::remote_control::SendCommandMode;

        workspace.update(ctx, |workspace, ctx| {
            let pane_id = workspace.remote_control_split_pane(direction, ctx);
            match workspace.remote_control_send_command_to_pane(
                pane_id,
                command,
                SendCommandMode::Shell,
                ctx,
            ) {
                Ok(()) => Resp::Ok,
                Err(message) => Resp::Error { message },
            }
        })
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
