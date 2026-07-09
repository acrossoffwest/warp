use std::sync::mpsc::sync_channel;
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::persistence::ModelEvent;

use super::model::{SessionMemoryModel, SessionMemoryModelEvent};
use super::types::{
    AgentPermissionMode, SessionMemoryKind, SessionMemoryRecord, SessionMemoryRunState,
    SessionMemorySource, SessionMemoryStatus,
};
use warpui::App;

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
fn startup_reclassifies_terminal_hosted_claude_command() {
    let mut record = test_record("terminal-claude");
    record.source = SessionMemorySource::WarpTerminal;
    record.kind = SessionMemoryKind::Terminal;
    record.status = SessionMemoryStatus::Live;
    record.last_command = Some("claude --dangerously-skip-permissions".to_string());
    record.permission_mode = AgentPermissionMode::Unknown;

    let model = SessionMemoryModel::new(vec![record], None);

    assert_eq!(model.records()[0].source, SessionMemorySource::ClaudeCode);
    assert_eq!(model.records()[0].kind, SessionMemoryKind::Terminal);
    assert_eq!(
        model.records()[0].permission_mode,
        AgentPermissionMode::Dangerous
    );
    assert_eq!(model.records()[0].status, SessionMemoryStatus::Interrupted);
}

#[test]
fn startup_reclassifies_terminal_hosted_codex_command() {
    let mut record = test_record("terminal-codex");
    record.source = SessionMemorySource::WarpTerminal;
    record.kind = SessionMemoryKind::Terminal;
    record.last_command = Some(
        "OPENAI_API_KEY=x codex --dangerously-bypass-approvals-and-sandbox resume".to_string(),
    );
    record.permission_mode = AgentPermissionMode::Unknown;

    let model = SessionMemoryModel::new(vec![record], None);

    assert_eq!(model.records()[0].source, SessionMemorySource::Codex);
    assert_eq!(model.records()[0].kind, SessionMemoryKind::Terminal);
    assert_eq!(
        model.records()[0].permission_mode,
        AgentPermissionMode::Dangerous
    );
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
fn startup_auto_restore_records_only_returns_previous_unoffered_run() {
    let mut previous = test_record("previous-run-session");
    previous.source = SessionMemorySource::ClaudeCode;
    previous.status = SessionMemoryStatus::Live;
    previous.app_run_id = Some("previous-run".to_string());
    previous.native_session_id = Some("previous-session-id".to_string());

    let mut older = test_record("older-run-session");
    older.source = SessionMemorySource::ClaudeCode;
    older.status = SessionMemoryStatus::Live;
    older.app_run_id = Some("older-run".to_string());
    older.native_session_id = Some("older-session-id".to_string());

    let mut already_offered = test_record("already-offered-session");
    already_offered.source = SessionMemorySource::ClaudeCode;
    already_offered.status = SessionMemoryStatus::Live;
    already_offered.app_run_id = Some("previous-run".to_string());
    already_offered.native_session_id = Some("already-offered-session-id".to_string());
    already_offered.recovery_offered_run_id = Some("current-run".to_string());

    let model = SessionMemoryModel::new_with_run_state(
        vec![previous, older, already_offered],
        None,
        SessionMemoryRunState::new("current-run", Some("previous-run".to_string())),
    );

    let records = model.startup_auto_restore_records();

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].id, "previous-run-session");
}

#[test]
fn startup_auto_restore_records_excludes_completed_agent_sessions() {
    let mut ended = test_record("ended-claude-session");
    ended.source = SessionMemorySource::ClaudeCode;
    ended.status = SessionMemoryStatus::Live;
    ended.app_run_id = Some("previous-run".to_string());
    ended.native_session_id = Some("ended-session-id".to_string());
    ended.completed_at = Some(1234);

    let mut running = test_record("running-claude-session");
    running.source = SessionMemorySource::ClaudeCode;
    running.status = SessionMemoryStatus::Live;
    running.app_run_id = Some("previous-run".to_string());
    running.native_session_id = Some("running-session-id".to_string());

    let model = SessionMemoryModel::new_with_run_state(
        vec![ended, running],
        None,
        SessionMemoryRunState::new("current-run", Some("previous-run".to_string())),
    );

    let records = model.startup_auto_restore_records();

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].id, "running-claude-session");
}

#[test]
fn upsert_removes_older_records_for_same_native_session() {
    let mut old = test_record("old-pane-record");
    old.native_session_id = Some("chat-1".to_string());
    old.terminal_pane_uuid = Some(vec![1]);
    old.last_seen_at = 100;

    let mut unrelated = test_record("unrelated-record");
    unrelated.native_session_id = Some("chat-2".to_string());

    let mut model = SessionMemoryModel::new_with_run_state(
        vec![old, unrelated],
        None,
        SessionMemoryRunState::new("current-run", None),
    );

    let mut new = test_record("new-pane-record");
    new.native_session_id = Some("chat-1".to_string());
    new.terminal_pane_uuid = Some(vec![2]);
    new.last_seen_at = 200;
    model.upsert(new);

    let ids = model
        .records()
        .iter()
        .map(|record| record.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["unrelated-record", "new-pane-record"]);
}

#[test]
fn load_dedupes_records_sharing_native_session_keeping_newest() {
    let mut old = test_record("old-duplicate");
    old.native_session_id = Some("chat-1".to_string());
    old.last_seen_at = 100;

    let mut newest = test_record("newest-duplicate");
    newest.native_session_id = Some("chat-1".to_string());
    newest.last_seen_at = 200;

    let mut no_native = test_record("plain-terminal");
    no_native.native_session_id = None;

    let model = SessionMemoryModel::new_with_run_state(
        vec![old, newest, no_native],
        None,
        SessionMemoryRunState::new("current-run", None),
    );

    let ids = model
        .records()
        .iter()
        .map(|record| record.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["newest-duplicate", "plain-terminal"]);
}

#[test]
fn should_suppress_restored_tab_only_when_every_pane_was_closed_intentionally() {
    let mut closed = test_record("closed-terminal");
    closed.terminal_pane_uuid = Some(vec![1, 1, 1, 1]);
    closed.closed_intentionally_at = Some(1300);

    let mut live = test_record("live-terminal");
    live.terminal_pane_uuid = Some(vec![2, 2, 2, 2]);
    live.closed_intentionally_at = None;

    let model = SessionMemoryModel::new_with_run_state(
        vec![closed, live],
        None,
        SessionMemoryRunState::new("current-run", None),
    );

    // Every pane in the tab is marked closed -> suppress.
    assert!(model.should_suppress_restored_tab(&[vec![1, 1, 1, 1]]));
    // A pane is still live -> keep the tab.
    assert!(!model.should_suppress_restored_tab(&[vec![1, 1, 1, 1], vec![2, 2, 2, 2]]));
    // Unknown pane (no record) -> keep the tab.
    assert!(!model.should_suppress_restored_tab(&[vec![9, 9, 9, 9]]));
    // No terminal panes -> keep the tab.
    assert!(!model.should_suppress_restored_tab(&[]));
}

#[test]
fn mark_agent_session_ended_sets_completed_at() {
    let mut record = test_record("live-claude-session");
    record.source = SessionMemorySource::ClaudeCode;
    record.status = SessionMemoryStatus::Live;
    record.native_session_id = Some("session-id".to_string());
    record.completed_at = None;

    let mut model = SessionMemoryModel::new_with_run_state(
        vec![record],
        None,
        SessionMemoryRunState::new("current-run", None),
    );

    model.mark_agent_session_ended("live-claude-session");

    let record = &model.records()[0];
    assert!(record.completed_at.is_some());
}

#[test]
fn startup_auto_restore_records_includes_resumable_sessions_from_clean_previous_run() {
    let mut claude = test_record("claude-session");
    claude.source = SessionMemorySource::ClaudeCode;
    claude.kind = SessionMemoryKind::Terminal;
    claude.status = SessionMemoryStatus::Live;
    claude.app_run_id = Some("previous-clean-run".to_string());
    claude.native_session_id = Some("claude-session-id".to_string());

    let mut tmux = test_record("tmux-session");
    tmux.source = SessionMemorySource::WarpTerminal;
    tmux.status = SessionMemoryStatus::Live;
    tmux.app_run_id = Some("previous-clean-run".to_string());
    tmux.last_command = Some("tmux attach -t work".to_string());

    let mut plain_terminal = test_record("plain-terminal");
    plain_terminal.source = SessionMemorySource::WarpTerminal;
    plain_terminal.status = SessionMemoryStatus::Live;
    plain_terminal.app_run_id = Some("previous-clean-run".to_string());
    plain_terminal.last_command = Some("cargo check -p warp".to_string());

    let model = SessionMemoryModel::new_with_run_state(
        vec![claude, tmux, plain_terminal],
        None,
        SessionMemoryRunState::with_previous_run(
            "current-run",
            Some("previous-clean-run".to_string()),
            None,
        ),
    );

    let records = model.startup_auto_restore_records();
    let ids = records
        .iter()
        .map(|record| record.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(ids, vec!["claude-session", "tmux-session"]);
}

#[test]
fn startup_auto_restore_records_includes_recent_resumable_agent_from_older_run() {
    let mut claude = test_record("recent-claude-session");
    claude.source = SessionMemorySource::ClaudeCode;
    claude.kind = SessionMemoryKind::AgentChat;
    claude.status = SessionMemoryStatus::Live;
    claude.app_run_id = Some("older-run".to_string());
    claude.native_session_id = Some("claude-session-id".to_string());
    claude.last_seen_at = current_unix_seconds();

    let mut stale = claude.clone();
    stale.id = "stale-claude-session".to_string();
    stale.last_seen_at = current_unix_seconds() - 31 * 60;

    let model = SessionMemoryModel::new_with_run_state(
        vec![claude, stale],
        None,
        SessionMemoryRunState::with_previous_run(
            "current-run",
            Some("previous-run".to_string()),
            None,
        ),
    );

    let records = model.startup_auto_restore_records();

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].id, "recent-claude-session");
}

#[test]
fn mark_startup_recovery_offered_persists_one_shot_marker() {
    let (sender, receiver) = sync_channel(1);
    let event_sink = SessionMemoryModel::persistence_event_sink(Some(sender))
        .expect("persistence sender should create an event sink");
    let mut record = test_record("previous-run-session");
    record.status = SessionMemoryStatus::Live;
    record.app_run_id = Some("previous-run".to_string());
    let mut model = SessionMemoryModel::new_with_run_state(
        vec![record],
        Some(event_sink),
        SessionMemoryRunState::new("current-run", Some("previous-run".to_string())),
    );

    model.mark_startup_recovery_offered(&["previous-run-session".to_string()]);

    assert_eq!(
        model.records()[0].recovery_offered_run_id.as_deref(),
        Some("current-run")
    );
    match receiver.recv().unwrap() {
        ModelEvent::UpsertSessionMemoryRecord { record } => {
            assert_eq!(record.id, "previous-run-session");
            assert_eq!(
                record.recovery_offered_run_id.as_deref(),
                Some("current-run")
            );
        }
        event => panic!("expected session memory upsert event, got {event:?}"),
    }
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

#[test]
fn upsert_preserves_existing_app_run_id() {
    let mut record = test_record("codex-1");
    record.app_run_id = Some("older-run".to_string());
    let mut model = SessionMemoryModel::new_with_run_state(
        Vec::new(),
        None,
        SessionMemoryRunState::new("current-run", None),
    );

    model.upsert(record);

    assert_eq!(model.records()[0].app_run_id.as_deref(), Some("older-run"));
}

#[test]
fn delete_and_notify_emits_model_event_for_board_subscribers() {
    App::test((), |mut app| async move {
        let record = test_record("codex-1");
        let model_handle = app.add_model(|_| SessionMemoryModel::new(vec![record], None));
        let (sender, receiver) = async_channel::unbounded();

        model_handle.update(&mut app, {
            let model_handle = model_handle.clone();
            move |_, ctx| {
                ctx.subscribe_to_model(&model_handle, move |_, event, _| {
                    let _ = sender.try_send(event.clone());
                });
            }
        });

        model_handle.update(&mut app, |model, ctx| {
            model.delete_and_notify("codex-1", ctx);
        });

        assert_eq!(
            receiver.try_recv().unwrap(),
            SessionMemoryModelEvent::DeleteRecord {
                id: "codex-1".to_owned()
            }
        );
        assert!(receiver.try_recv().is_err());
    });
}

#[test]
fn upsert_and_notify_emits_model_event_for_board_subscribers() {
    App::test((), |mut app| async move {
        let model_handle = app.add_model(|_| SessionMemoryModel::new(Vec::new(), None));
        let (sender, receiver) = async_channel::unbounded();
        let record = test_record("codex-1");

        model_handle.update(&mut app, {
            let model_handle = model_handle.clone();
            move |_, ctx| {
                ctx.subscribe_to_model(&model_handle, move |_, event, _| {
                    let _ = sender.try_send(event.clone());
                });
            }
        });

        model_handle.update(&mut app, |model, ctx| {
            model.upsert_and_notify(record.clone(), ctx);
        });

        assert_eq!(
            receiver.try_recv().unwrap(),
            SessionMemoryModelEvent::UpsertRecord { record }
        );
        assert!(receiver.try_recv().is_err());
    });
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
        app_run_id: None,
        recovery_offered_run_id: None,
        restore_payload: None,
    }
}

fn current_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}
