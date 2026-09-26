use std::path::PathBuf;

use super::types::{
    AgentPermissionMode, SessionMemoryKind, SessionMemoryRecord, SessionMemorySource,
    SessionMemoryStatus,
};

#[test]
fn stale_snapshot_of_ended_agent_keeps_end() {
    let mut incoming = agent_record(Some(100));

    incoming.keep_agent_end(Some(100), Some(150));

    assert_eq!(incoming.completed_at, Some(150));
    assert_eq!(incoming.status, SessionMemoryStatus::Success);
}

#[test]
fn new_agent_start_clears_previous_end() {
    let mut incoming = agent_record(Some(200));

    incoming.keep_agent_end(Some(100), Some(150));

    assert_eq!(incoming.completed_at, None);
    assert_eq!(incoming.status, SessionMemoryStatus::Live);
}

#[test]
fn new_agent_after_terminal_snapshot_clears_previous_end() {
    let mut incoming = agent_record(Some(200));

    incoming.keep_agent_end(None, Some(150));

    assert_eq!(incoming.completed_at, None);
}

#[test]
fn terminal_snapshot_after_agent_end_keeps_end() {
    let mut incoming = agent_record(None);
    incoming.source = SessionMemorySource::WarpTerminal;
    incoming.kind = SessionMemoryKind::Terminal;

    incoming.keep_agent_end(Some(100), Some(150));

    assert_eq!(incoming.completed_at, Some(150));
    assert_eq!(incoming.status, SessionMemoryStatus::Live);
}

#[test]
fn record_without_existing_end_is_unchanged() {
    let mut incoming = agent_record(Some(100));

    incoming.keep_agent_end(Some(100), None);

    assert_eq!(incoming.completed_at, None);
    assert_eq!(incoming.status, SessionMemoryStatus::Live);
}

fn agent_record(started_at: Option<i64>) -> SessionMemoryRecord {
    SessionMemoryRecord {
        id: "warp_terminal:AQIDBA==".to_string(),
        source: SessionMemorySource::ClaudeCode,
        kind: SessionMemoryKind::AgentChat,
        status: SessionMemoryStatus::Live,
        title: "claude".to_string(),
        summary: None,
        cwd: Some(PathBuf::from("/tmp/session-memory")),
        project: None,
        native_session_id: None,
        transcript_path: None,
        terminal_pane_uuid: Some(vec![1, 2, 3, 4]),
        app_window_fingerprint: None,
        app_tab_fingerprint: None,
        last_command: Some("claude".to_string()),
        last_exit_code: None,
        launch_argv: Some(vec!["claude".to_string()]),
        permission_mode: AgentPermissionMode::Normal,
        last_seen_at: 160,
        started_at,
        completed_at: None,
        closed_intentionally_at: None,
        app_run_id: Some("current-run".to_string()),
        recovery_offered_run_id: None,
        restore_payload: None,
    }
}
