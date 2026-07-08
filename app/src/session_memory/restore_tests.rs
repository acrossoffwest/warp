use std::path::PathBuf;

use super::restore::{agent_restore_plan, terminal_restore_plan, RestoreError};
use super::types::{
    AgentPermissionMode, SessionMemoryKind, SessionMemoryRecord, SessionMemorySource,
    SessionMemoryStatus,
};

#[test]
fn codex_restore_uses_saved_cwd_and_dangerous_flag() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let record = dangerous_codex_record(tempdir.path().to_path_buf(), "abc123");

    let plan = agent_restore_plan(&record).expect("restore plan should be built");

    assert_eq!(plan.cwd(), Some(tempdir.path()));
    assert_eq!(
        plan.command(),
        Some("codex --resume abc123 --dangerously-bypass-approvals-and-sandbox")
    );
    assert_eq!(plan.permission_mode(), Some(AgentPermissionMode::Dangerous));
}

#[test]
fn agent_restore_fails_when_cwd_is_missing() {
    let missing_cwd = PathBuf::from("/definitely/not/here/session-memory-board");
    let record = codex_record(missing_cwd.clone(), "abc123");

    let result = agent_restore_plan(&record);

    assert_eq!(
        result,
        Err(RestoreError::MissingWorkingDirectory(missing_cwd))
    );
}

#[test]
fn terminal_restore_does_not_auto_run_by_default() {
    let record = terminal_record_with_last_command("cargo check -p warp");

    let plan = terminal_restore_plan(&record, false);

    assert_eq!(plan.command_for_composer(), Some("cargo check -p warp"));
    assert_eq!(plan.auto_run(), Some(false));
}

#[test]
fn terminal_restore_auto_run_requires_saved_command() {
    let mut record = terminal_record_with_last_command("cargo check -p warp");

    let plan = terminal_restore_plan(&record, true);

    assert_eq!(plan.auto_run(), Some(true));

    record.last_command = None;
    let plan = terminal_restore_plan(&record, true);

    assert_eq!(plan.command_for_composer(), None);
    assert_eq!(plan.auto_run(), Some(false));
}

#[test]
fn normal_and_unknown_agent_permission_do_not_add_dangerous_flags() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut normal_record = codex_record(tempdir.path().to_path_buf(), "abc123");
    normal_record.permission_mode = AgentPermissionMode::Normal;
    let mut unknown_record = normal_record.clone();
    unknown_record.permission_mode = AgentPermissionMode::Unknown;

    let normal_plan = agent_restore_plan(&normal_record).expect("normal plan should be built");
    let unknown_plan = agent_restore_plan(&unknown_record).expect("unknown plan should be built");

    assert_eq!(normal_plan.command(), Some("codex --resume abc123"));
    assert_eq!(unknown_plan.command(), Some("codex --resume abc123"));
}

#[test]
fn agent_restore_requires_native_session_id() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut record = codex_record(tempdir.path().to_path_buf(), "abc123");
    record.native_session_id = None;

    let result = agent_restore_plan(&record);

    assert_eq!(result, Err(RestoreError::MissingSessionId));
}

#[test]
fn unsupported_source_does_not_build_agent_plan() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut record = codex_record(tempdir.path().to_path_buf(), "abc123");
    record.source = SessionMemorySource::WarpTerminal;

    let result = agent_restore_plan(&record);

    assert_eq!(result, Err(RestoreError::UnsupportedSource));
}

fn dangerous_codex_record(cwd: PathBuf, session_id: &str) -> SessionMemoryRecord {
    let mut record = codex_record(cwd, session_id);
    record.permission_mode = AgentPermissionMode::Dangerous;
    record
}

fn codex_record(cwd: PathBuf, session_id: &str) -> SessionMemoryRecord {
    SessionMemoryRecord {
        id: format!("codex-{session_id}"),
        source: SessionMemorySource::Codex,
        kind: SessionMemoryKind::AgentChat,
        status: SessionMemoryStatus::Unknown,
        title: "Codex session".to_string(),
        summary: None,
        cwd: Some(cwd),
        project: None,
        native_session_id: Some(session_id.to_string()),
        transcript_path: None,
        terminal_pane_uuid: None,
        app_window_fingerprint: None,
        app_tab_fingerprint: None,
        last_command: None,
        last_exit_code: None,
        launch_argv: Some(vec!["codex".to_string()]),
        permission_mode: AgentPermissionMode::Normal,
        last_seen_at: 1,
        started_at: Some(1),
        completed_at: None,
        closed_intentionally_at: None,
        restore_payload: None,
    }
}

fn terminal_record_with_last_command(last_command: &str) -> SessionMemoryRecord {
    SessionMemoryRecord {
        id: "terminal-1".to_string(),
        source: SessionMemorySource::WarpTerminal,
        kind: SessionMemoryKind::Terminal,
        status: SessionMemoryStatus::Interrupted,
        title: "Interrupted terminal".to_string(),
        summary: None,
        cwd: Some(PathBuf::from("/tmp/session-memory")),
        project: None,
        native_session_id: None,
        transcript_path: None,
        terminal_pane_uuid: None,
        app_window_fingerprint: None,
        app_tab_fingerprint: None,
        last_command: Some(last_command.to_string()),
        last_exit_code: None,
        launch_argv: None,
        permission_mode: AgentPermissionMode::Unknown,
        last_seen_at: 1,
        started_at: Some(1),
        completed_at: None,
        closed_intentionally_at: None,
        restore_payload: None,
    }
}
