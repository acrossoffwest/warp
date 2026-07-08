use std::path::PathBuf;
use std::sync::mpsc::sync_channel;

use crate::persistence::ModelEvent;

use super::model::SessionMemoryModel;
use super::types::{
    AgentPermissionMode, SessionMemoryKind, SessionMemoryRecord, SessionMemorySource,
    SessionMemoryStatus,
};

#[test]
fn startup_live_without_intentional_close_becomes_interrupted() {
    let mut record = test_record("terminal-1");
    record.status = SessionMemoryStatus::Live;
    record.closed_intentionally_at = None;
    record.permission_mode = AgentPermissionMode::Dangerous;

    let model = SessionMemoryModel::new(vec![record], None);

    assert_eq!(model.records()[0].status, SessionMemoryStatus::Interrupted);
    assert!(model.records()[0].is_interrupted());
    assert_eq!(
        model.records()[0].permission_mode,
        AgentPermissionMode::Dangerous
    );
}

#[test]
fn startup_live_with_intentional_close_becomes_user_closed() {
    let mut record = test_record("terminal-1");
    record.status = SessionMemoryStatus::Live;
    record.closed_intentionally_at = Some(100);

    let model = SessionMemoryModel::new(vec![record], None);

    assert_eq!(model.records()[0].status, SessionMemoryStatus::UserClosed);
}

#[test]
fn startup_success_remains_success() {
    let mut record = test_record("terminal-1");
    record.status = SessionMemoryStatus::Success;
    record.closed_intentionally_at = None;

    let model = SessionMemoryModel::new(vec![record], None);

    assert_eq!(model.records()[0].status, SessionMemoryStatus::Success);
}

#[test]
fn from_persisted_records_classifies_and_preserves_recovery_fields() {
    let mut record = test_record("codex-1");
    record.status = SessionMemoryStatus::Live;
    record.closed_intentionally_at = None;
    record.permission_mode = AgentPermissionMode::Dangerous;
    record.launch_argv = Some(vec![
        "codex".to_string(),
        "--resume".to_string(),
        "abc123".to_string(),
        "--dangerously-bypass-approvals-and-sandbox".to_string(),
    ]);
    record.restore_payload = Some(serde_json::json!({
        "cwd": "/tmp/session-memory",
        "resume_id": "abc123"
    }));

    let model = SessionMemoryModel::from_persisted_records(vec![record.into()], None);

    assert_eq!(model.records()[0].status, SessionMemoryStatus::Interrupted);
    assert_eq!(
        model.records()[0].permission_mode,
        AgentPermissionMode::Dangerous
    );
    assert_eq!(
        model.records()[0].launch_argv.as_ref().unwrap(),
        &vec![
            "codex".to_string(),
            "--resume".to_string(),
            "abc123".to_string(),
            "--dangerously-bypass-approvals-and-sandbox".to_string(),
        ]
    );
    assert_eq!(
        model.records()[0]
            .restore_payload
            .as_ref()
            .and_then(|payload| payload.get("resume_id"))
            .and_then(|value| value.as_str()),
        Some("abc123")
    );
}

#[test]
fn filter_matches_title_cwd_command_and_session_id() {
    let mut record = test_record("codex-1");
    record.title = "Codex board spec".to_string();
    record.cwd = Some(PathBuf::from("/Users/[redacted]/projects/warp"));
    record.last_command = Some("cargo check -p warp".to_string());
    record.native_session_id = Some("abc123".to_string());

    assert!(record.matches_query("board"));
    assert!(record.matches_query("projects/warp"));
    assert!(record.matches_query("cargo check"));
    assert!(record.matches_query("abc123"));
    assert!(record.matches_query(" CODEX "));
    assert!(!record.matches_query("not-present"));
}

#[test]
fn filtered_records_uses_record_query_matching() {
    let mut board_record = test_record("codex-1");
    board_record.title = "Codex board spec".to_string();
    let mut other_record = test_record("terminal-1");
    other_record.title = "Shell build".to_string();
    let model = SessionMemoryModel::new(vec![board_record, other_record], None);

    let matches = model.filtered_records("board");

    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].id, "codex-1");
}

#[test]
fn interrupted_records_returns_only_interrupted_sessions() {
    let mut interrupted_record = test_record("terminal-1");
    interrupted_record.status = SessionMemoryStatus::Live;
    interrupted_record.closed_intentionally_at = None;
    let mut closed_record = test_record("terminal-2");
    closed_record.status = SessionMemoryStatus::Live;
    closed_record.closed_intentionally_at = Some(100);
    let model = SessionMemoryModel::new(vec![interrupted_record, closed_record], None);

    let interrupted = model.interrupted_records();

    assert_eq!(interrupted.len(), 1);
    assert_eq!(interrupted[0].id, "terminal-1");
    assert_eq!(model.interrupted_count(), 1);
}

#[test]
fn persistence_event_sink_forwards_upsert_and_delete_events() {
    let (sender, receiver) = sync_channel(2);
    let event_sink = SessionMemoryModel::persistence_event_sink(Some(sender))
        .expect("persistence sender should create an event sink");
    let mut model = SessionMemoryModel::new(Vec::new(), Some(event_sink));
    let mut record = test_record("codex-1");
    record.permission_mode = AgentPermissionMode::Dangerous;

    model.upsert(record.clone());
    model.delete("codex-1");

    match receiver.recv().unwrap() {
        ModelEvent::UpsertSessionMemoryRecord {
            record: persisted_record,
        } => {
            assert_eq!(persisted_record.id, record.id);
            assert_eq!(
                persisted_record.permission_mode,
                crate::persistence::AgentPermissionMode::Dangerous
            );
        }
        event => panic!("expected session memory upsert event, got {event:?}"),
    }

    match receiver.recv().unwrap() {
        ModelEvent::DeleteSessionMemoryRecord { id } => {
            assert_eq!(id, "codex-1");
        }
        event => panic!("expected session memory delete event, got {event:?}"),
    }
}

#[test]
fn persistence_event_sink_is_absent_without_sender() {
    assert!(SessionMemoryModel::persistence_event_sink(None).is_none());
}

#[test]
fn upsert_replaces_existing_record_and_delete_removes_by_id() {
    let record = test_record("codex-1");
    let mut model = SessionMemoryModel::new(vec![record], None);
    let mut replacement = test_record("codex-1");
    replacement.title = "Updated title".to_string();

    model.upsert(replacement);

    assert_eq!(model.records().len(), 1);
    assert_eq!(model.records()[0].title, "Updated title");

    model.delete("codex-1");

    assert!(model.records().is_empty());
}

fn test_record(id: &str) -> SessionMemoryRecord {
    SessionMemoryRecord {
        id: id.to_string(),
        source: SessionMemorySource::Codex,
        kind: SessionMemoryKind::AgentChat,
        status: SessionMemoryStatus::Unknown,
        title: "Test session".to_string(),
        summary: Some("A test session summary".to_string()),
        cwd: Some(PathBuf::from("/tmp/session-memory")),
        project: Some("warp".to_string()),
        native_session_id: None,
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
