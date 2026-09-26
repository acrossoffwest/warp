use std::path::{Path, PathBuf};

use super::{agent_lookup_cwd, layout_was_restored};
use crate::app_state::{AppState, WindowSnapshot};

#[test]
fn agent_lookup_cwd_strips_trailing_slash_of_missing_path() {
    assert_eq!(
        agent_lookup_cwd(Path::new("/definitely/not/here/project/")),
        PathBuf::from("/definitely/not/here/project")
    );
}

#[test]
fn agent_lookup_cwd_keeps_root() {
    assert_eq!(agent_lookup_cwd(Path::new("/")), PathBuf::from("/"));
}

#[test]
fn agent_lookup_cwd_expands_home() {
    let home = dirs::home_dir().expect("home dir should be known");
    assert_eq!(
        agent_lookup_cwd(Path::new("~/definitely-not-here-project")),
        home.join("definitely-not-here-project")
    );
}

#[cfg(unix)]
#[test]
fn agent_lookup_cwd_resolves_symlinks() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let real = tempdir.path().join("real");
    std::fs::create_dir(&real).expect("dir should be created");
    let link = tempdir.path().join("link");
    std::os::unix::fs::symlink(&real, &link).expect("symlink should be created");

    assert_eq!(
        agent_lookup_cwd(&link),
        std::fs::canonicalize(&real).expect("real dir should canonicalize")
    );
}

#[test]
fn layout_was_restored_only_with_restored_windows_and_setting_on() {
    let empty = AppState {
        windows: vec![],
        active_window_index: None,
        block_lists: Default::default(),
        running_mcp_servers: Default::default(),
    };
    let with_window = AppState {
        windows: vec![window_snapshot()],
        active_window_index: Some(0),
        ..empty.clone()
    };

    assert!(!layout_was_restored(None, true));
    assert!(!layout_was_restored(Some(&empty), true));
    assert!(!layout_was_restored(Some(&with_window), false));
    assert!(layout_was_restored(Some(&with_window), true));
}

fn window_snapshot() -> WindowSnapshot {
    WindowSnapshot {
        tabs: vec![],
        active_tab_index: 0,
        team_uid: None,
        bounds: None,
        fullscreen_state: Default::default(),
        quake_mode: false,
        universal_search_width: None,
        warp_ai_width: None,
        voltron_width: None,
        warp_drive_index_width: None,
        left_panel_open: false,
        vertical_tabs_panel_open: false,
        left_panel_width: None,
        right_panel_width: None,
        agent_management_filters: None,
        tab_groups: vec![],
    }
}
