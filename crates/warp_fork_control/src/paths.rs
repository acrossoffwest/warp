use std::path::PathBuf;

pub const SOCKET_ENV: &str = "WARP_FORK_CONTROL_SOCKET";
const SOCKET_FILE_NAME: &str = "control.sock";

pub fn socket_dir_name(data_profile: Option<&str>) -> String {
    match data_profile {
        Some(profile) => format!("dev.warp.Warp-{profile}"),
        None => "dev.warp.Warp".to_string(),
    }
}

/// `$WARP_FORK_CONTROL_SOCKET` if set, otherwise the per-profile default.
pub fn default_socket_path(data_profile: Option<&str>) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(SOCKET_ENV) {
        return Some(PathBuf::from(path));
    }
    default_socket_path_without_env(data_profile)
}

fn default_socket_path_without_env(data_profile: Option<&str>) -> Option<PathBuf> {
    let app_dir = socket_dir_name(data_profile);
    #[cfg(target_os = "linux")]
    if let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        return Some(
            PathBuf::from(runtime_dir)
                .join(format!("{app_dir}-fork-control"))
                .join(SOCKET_FILE_NAME),
        );
    }
    dirs::data_local_dir().map(|base| {
        base.join(app_dir)
            .join("fork-control")
            .join(SOCKET_FILE_NAME)
    })
}

#[cfg(test)]
#[path = "paths_tests.rs"]
mod tests;
