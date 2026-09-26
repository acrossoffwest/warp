use warp_core::settings::macros::define_settings_group;
use warp_core::settings::{SupportedPlatforms, SyncToCloud};

define_settings_group!(ForkControlSettings, settings: [
    enabled: ForkControlEnabled {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Never,
        surface: settings::SettingSurfaces::GUI,
        private: false,
        storage_key: "ForkControlEnabled",
        toml_path: "fork.control_api.enabled",
        description: "Serve the fork-only local control API on a Unix socket.",
    },
]);
