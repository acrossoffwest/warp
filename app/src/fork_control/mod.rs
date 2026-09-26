//! Fork-only local control API host. Contract: docs/fork-control-api.md.

mod fork_settings;
mod handlers;
mod procinfo;

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use serde_json::Value;
use warp_core::channel::ChannelState;
use warp_fork_control::paths::default_socket_path;
use warp_fork_control::protocol::{ErrorBody, ErrorCode, Request};
use warp_fork_control::server::{Handler, Server};
use warpui::r#async::SpawnedLocalStream;
use warpui::{Entity, ModelContext, SingletonEntity};

pub use fork_settings::ForkControlSettings;
use procinfo::ProcessTable;

const DISABLE_ENV: &str = "WARP_FORK_CONTROL";
const UI_TIMEOUT: Duration = Duration::from_secs(5);

struct Job {
    request: Request,
    procs: Option<ProcessTable>,
    reply: mpsc::Sender<Result<Value, ErrorBody>>,
}

pub struct ForkControlHost {
    server: Option<Server>,
    job_tx: async_channel::Sender<Job>,
    _drain: SpawnedLocalStream,
}

impl ForkControlHost {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        let (job_tx, job_rx) = async_channel::unbounded::<Job>();
        let drain = ctx.spawn_stream_local(
            job_rx,
            |_, job, ctx| {
                let result = handlers::handle(job.request, job.procs, ctx);
                let _ = job.reply.send(result);
            },
            |_, _| {},
        );
        ctx.subscribe_to_model(&ForkControlSettings::handle(ctx), |me, _, _event, ctx| {
            me.sync_with_settings(ctx);
        });
        let mut host = Self {
            server: None,
            job_tx,
            _drain: drain,
        };
        host.sync_with_settings(ctx);
        host
    }

    fn sync_with_settings(&mut self, ctx: &mut ModelContext<Self>) {
        let enabled = is_enabled(
            *ForkControlSettings::as_ref(ctx).enabled,
            std::env::var(DISABLE_ENV).ok().as_deref(),
        );
        match (enabled, self.server.is_some()) {
            (true, false) => self.server = start_server(self.job_tx.clone()),
            (false, true) => {
                self.server = None;
                log::info!("fork_control: stopped");
            }
            _ => {}
        }
    }
}

impl Entity for ForkControlHost {
    type Event = ();
}

impl SingletonEntity for ForkControlHost {}

fn is_enabled(setting: bool, env: Option<&str>) -> bool {
    setting && !matches!(env, Some("0" | "false" | "off"))
}

fn start_server(job_tx: async_channel::Sender<Job>) -> Option<Server> {
    let Some(path) = default_socket_path(ChannelState::data_profile().as_deref()) else {
        log::warn!("fork_control: no data directory for the socket");
        return None;
    };
    let handler: Handler = Arc::new(move |request| dispatch(&job_tx, request));
    match Server::start(&path, handler) {
        Ok(server) => {
            log::info!("fork_control: listening on {}", path.display());
            Some(server)
        }
        Err(error) => {
            log::warn!("fork_control disabled: {error:#}");
            None
        }
    }
}

fn dispatch(job_tx: &async_channel::Sender<Job>, request: Request) -> Result<Value, ErrorBody> {
    let procs = matches!(request, Request::List | Request::FindByPid(_))
        .then(ProcessTable::snapshot);
    let (reply, reply_rx) = mpsc::channel();
    job_tx
        .try_send(Job { request, procs, reply })
        .map_err(|_| ErrorBody::new(ErrorCode::Unavailable, "Warp is shutting down"))?;
    reply_rx
        .recv_timeout(UI_TIMEOUT)
        .map_err(|_| ErrorBody::new(ErrorCode::Timeout, "Warp did not answer in time"))?
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
