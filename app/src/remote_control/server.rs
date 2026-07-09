use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use ipc::ServerBuilder;
use warpui::r#async::executor::Background;

use super::{RemoteControlJob, RemoteControlServiceImpl};

/// Directory name holding the address file, scoped per data profile so a
/// dev instance (`WARP_DATA_PROFILE`, debug builds only) and the installed
/// release app don't overwrite each other's address.
///
/// Keep in sync with `socket_directory_name` in `warp_remote_control_cli`.
fn socket_directory_name(data_profile: Option<&str>) -> String {
    match data_profile {
        Some(profile) => format!("dev.warp.Warp-{profile}"),
        None => "dev.warp.Warp".to_string(),
    }
}

/// Returns the absolute path where the IPC connection address is published.
///
/// Clients (e.g. an MCP server) read this file to learn the socket path.
pub fn socket_address_path() -> Result<PathBuf> {
    let base = dirs::data_local_dir()
        .or_else(dirs::home_dir)
        .context("no data dir available")?;
    let data_profile = crate::channel::ChannelState::data_profile();
    Ok(base
        .join(socket_directory_name(data_profile.as_deref()))
        .join("remote_control.addr"))
}

#[cfg(test)]
mod socket_path_tests {
    use super::socket_directory_name;

    #[test]
    fn socket_directory_is_shared_default_without_profile() {
        assert_eq!(socket_directory_name(None), "dev.warp.Warp");
    }

    #[test]
    fn socket_directory_is_scoped_per_data_profile() {
        assert_eq!(socket_directory_name(Some("dev")), "dev.warp.Warp-dev");
    }
}

pub struct RemoteControlServerHandle {
    pub(crate) server: ipc::Server,
    pub(crate) job_rx: async_channel::Receiver<RemoteControlJob>,
}

pub fn start(background_executor: Arc<Background>) -> Result<RemoteControlServerHandle> {
    let (job_tx, job_rx) = async_channel::unbounded::<RemoteControlJob>();
    let service_impl = RemoteControlServiceImpl { job_tx };

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
        if let Err(e) = std::fs::set_permissions(&addr_file, std::fs::Permissions::from_mode(0o600))
        {
            log::warn!(
                "remote_control: could not set 0600 on {}: {e}",
                addr_file.display()
            );
        }
    }

    log::info!("remote_control IPC ready at {}", addr_file.display());
    Ok(RemoteControlServerHandle { server, job_rx })
}
