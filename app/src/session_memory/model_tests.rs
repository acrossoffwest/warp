use std::sync::mpsc::sync_channel;
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::persistence::ModelEvent;

use super::model::{SessionMemoryModel, SessionMemoryModelEvent};
use super::restore::RunBounds;
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
        ModelEvent::MarkSessionMemoryRecordsOffered {
            ids,
            app_run_id,
            offered_run_id,
        } => {
            assert_eq!(ids, vec!["previous-run-session".to_string()]);
            assert_eq!(app_run_id, "previous-run");
            assert_eq!(offered_run_id, "current-run");
        }
        event => panic!("expected session memory offered event, got {event:?}"),
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

        let observer = app.add_model(|_| SessionMemoryModel::new(Vec::new(), None));
        observer.update(&mut app, {
            let model_handle = model_handle.clone();
            move |_, ctx| {
                ctx.subscribe_to_model(&model_handle, move |_, _, event, _| {
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

        let observer = app.add_model(|_| SessionMemoryModel::new(Vec::new(), None));
        observer.update(&mut app, {
            let model_handle = model_handle.clone();
            move |_, ctx| {
                ctx.subscribe_to_model(&model_handle, move |_, _, event, _| {
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

#[test]
fn startup_restore_candidates_only_include_previous_run() {
    let model = model_with_previous_run(vec![
        agent_record("previous-run-agent", "previous-run"),
        agent_record("older-run-agent", "older-run"),
        agent_record("current-run-agent", "current-run"),
    ]);

    assert_eq!(candidate_ids(&model), vec!["previous-run-agent"]);
}

#[test]
fn startup_restore_candidates_exclude_recent_agents_from_older_runs() {
    let mut recent_older = agent_record("recent-older-run-agent", "older-run");
    recent_older.last_seen_at = current_unix_seconds();
    let model = model_with_previous_run(vec![recent_older]);

    assert!(model.startup_restore_candidates().is_empty());
}

#[test]
fn startup_restore_candidates_include_idle_agents_and_skip_terminals() {
    let mut idle = agent_record("idle-claude", "previous-run");
    idle.status = SessionMemoryStatus::Success;
    let mut blocked = agent_record("blocked-codex", "previous-run");
    blocked.source = SessionMemorySource::Codex;
    blocked.status = SessionMemoryStatus::Blocked;
    let mut tmux = agent_record("tmux-terminal", "previous-run");
    tmux.source = SessionMemorySource::WarpTerminal;
    tmux.kind = SessionMemoryKind::Terminal;
    tmux.last_command = Some("tmux attach -t work".to_string());
    let mut restored_block = agent_record("restored-claude-block", "previous-run");
    restored_block.source = SessionMemorySource::WarpTerminal;
    restored_block.kind = SessionMemoryKind::Terminal;
    restored_block.last_command = Some("claude --resume abc".to_string());
    let model = model_with_previous_run(vec![idle, blocked, tmux, restored_block]);

    assert_eq!(candidate_ids(&model), vec!["idle-claude", "blocked-codex"]);
}

#[test]
fn startup_restore_candidates_exclude_ended_closed_and_offered_agents() {
    let mut ended = agent_record("ended-agent", "previous-run");
    ended.completed_at = Some(150);
    let mut closed = agent_record("closed-agent", "previous-run");
    closed.closed_intentionally_at = Some(150);
    let mut offered = agent_record("offered-agent", "previous-run");
    offered.recovery_offered_run_id = Some("current-run".to_string());
    let open = agent_record("open-agent", "previous-run");
    let model = model_with_previous_run(vec![ended, closed, offered, open]);

    assert_eq!(candidate_ids(&model), vec!["open-agent"]);
}

#[test]
fn startup_restore_candidates_keep_newest_record_per_native_session() {
    let mut old = agent_record("old-pane", "previous-run");
    old.native_session_id = Some("chat-1".to_string());
    old.last_seen_at = 100;
    let mut new = agent_record("new-pane", "previous-run");
    new.native_session_id = Some("chat-1".to_string());
    new.last_seen_at = 200;
    let mut other = agent_record("other-pane", "previous-run");
    other.native_session_id = Some("chat-2".to_string());
    let model = model_with_previous_run(vec![old, new, other]);

    assert_eq!(candidate_ids(&model), vec!["new-pane", "other-pane"]);
    assert_eq!(model.records().len(), 3);
}

#[test]
fn startup_restore_candidates_without_previous_run_are_empty() {
    let model = SessionMemoryModel::new_with_run_state(
        vec![agent_record("agent", "previous-run")],
        None,
        SessionMemoryRunState::with_previous_run("current-run", None, None),
    );

    assert!(model.startup_restore_candidates().is_empty());
}

#[test]
fn previous_run_native_session_ids_collects_only_previous_run_ids() {
    let mut previous = agent_record("previous", "previous-run");
    previous.native_session_id = Some("previous-id".to_string());
    let mut older = agent_record("older", "older-run");
    older.native_session_id = Some("older-id".to_string());
    let model = model_with_previous_run(vec![previous, older]);

    let ids = model.previous_run_native_session_ids();

    assert!(ids.contains("previous-id"));
    assert!(!ids.contains("older-id"));
}

#[test]
fn previous_run_ended_agent_records_include_completed_and_closed_agents_only() {
    let mut ended = agent_record("ended-agent", "previous-run");
    ended.completed_at = Some(150);
    let mut closed = agent_record("closed-agent", "previous-run");
    closed.closed_intentionally_at = Some(150);
    let open = agent_record("open-agent", "previous-run");
    let mut older = agent_record("older-agent", "older-run");
    older.completed_at = Some(150);
    let mut terminal = test_record("terminal");
    terminal.source = SessionMemorySource::WarpTerminal;
    terminal.kind = SessionMemoryKind::Terminal;
    terminal.app_run_id = Some("previous-run".to_string());
    terminal.completed_at = Some(150);
    let model = model_with_previous_run(vec![ended, closed, open, older, terminal]);

    let ids = model
        .previous_run_ended_agent_records()
        .into_iter()
        .map(|record| record.id)
        .collect::<Vec<_>>();

    assert_eq!(ids, vec!["ended-agent", "closed-agent"]);
}

#[test]
fn run_bounds_come_from_run_state() {
    let model = SessionMemoryModel::new_with_run_state(
        vec![],
        None,
        SessionMemoryRunState::with_previous_run(
            "current-run",
            Some("previous-run".to_string()),
            None,
        )
        .with_run_starts(500, Some(100)),
    );

    assert_eq!(
        model.run_bounds(),
        RunBounds {
            previous_run_started_at: Some(100),
            current_run_started_at: 500,
        }
    );
}

fn agent_record(id: &str, app_run_id: &str) -> SessionMemoryRecord {
    let mut record = test_record(id);
    record.source = SessionMemorySource::ClaudeCode;
    record.kind = SessionMemoryKind::AgentChat;
    record.status = SessionMemoryStatus::Live;
    record.app_run_id = Some(app_run_id.to_string());
    record
}

fn model_with_previous_run(records: Vec<SessionMemoryRecord>) -> SessionMemoryModel {
    SessionMemoryModel::new_with_run_state(
        records,
        None,
        SessionMemoryRunState::with_previous_run(
            "current-run",
            Some("previous-run".to_string()),
            None,
        ),
    )
}

fn candidate_ids(model: &SessionMemoryModel) -> Vec<String> {
    model
        .startup_restore_candidates()
        .into_iter()
        .map(|record| record.id)
        .collect()
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
