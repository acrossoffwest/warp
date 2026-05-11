use std::path::PathBuf;
use std::sync::{mpsc, Arc};

use anyhow::{Context, Result};
use ipc::ServerBuilder;
use warpui::r#async::executor::Background;

use super::{PendingAction, RemoteControlServiceImpl};

/// Returns the absolute path where the IPC connection address is published.
///
/// Clients (e.g. an MCP server) read this file to learn the socket path.
pub fn socket_address_path() -> Result<PathBuf> {
    let base = dirs::data_local_dir()
        .or_else(dirs::home_dir)
        .context("no data dir available")?;
    Ok(base.join("dev.warp.Warp").join("remote_control.addr"))
}

pub struct RemoteControlServerHandle {
    pub(crate) server: ipc::Server,
    pub action_rx: mpsc::Receiver<PendingAction>,
}

pub fn start(background_executor: Arc<Background>) -> Result<RemoteControlServerHandle> {
    let (tx, rx) = mpsc::sync_channel::<PendingAction>(32);
    let service_impl = RemoteControlServiceImpl { action_tx: tx };

    let (server, connection_address) = ServerBuilder::default()
        .with_service(service_impl)
        .build_and_run(background_executor)
        .context("failed to start remote_control IPC server")?;

    let addr_file = socket_address_path()?;
    if let Some(parent) = addr_file.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&addr_file, connection_address.to_string())
        .with_context(|| format!("writing address file {}", addr_file.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) =
            std::fs::set_permissions(&addr_file, std::fs::Permissions::from_mode(0o600))
        {
            log::warn!(
                "remote_control: could not set 0600 on {}: {e}",
                addr_file.display()
            );
        }
    }

    log::info!("remote_control IPC ready at {}", addr_file.display());
    Ok(RemoteControlServerHandle {
        server,
        action_rx: rx,
    })
}
