use std::path::PathBuf;

use super::restore::{
    agent_restore_plan, restore_plan_for_record, startup_restore_action_for_record,
    terminal_restore_plan, RestoreError, RestoredTerminalPane, StartupRestoreAction,
};
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
        Some("codex resume abc123 --dangerously-bypass-approvals-and-sandbox")
    );
    assert_eq!(plan.permission_mode(), Some(AgentPermissionMode::Dangerous));
}

#[test]
fn claude_restore_uses_saved_session_id_and_dangerous_flag() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut record = codex_record(tempdir.path().to_path_buf(), "claude-session-123");
    record.source = SessionMemorySource::ClaudeCode;
    record.title = "Claude session".to_string();
    record.permission_mode = AgentPermissionMode::Dangerous;

    let plan = agent_restore_plan(&record).expect("restore plan should be built");

    assert_eq!(plan.cwd(), Some(tempdir.path()));
    assert_eq!(
        plan.command(),
        Some("claude --resume claude-session-123 --dangerously-skip-permissions")
    );
    assert_eq!(plan.permission_mode(), Some(AgentPermissionMode::Dangerous));
}

#[test]
fn restore_plan_treats_agent_source_with_session_id_as_agent_chat_even_if_kind_is_terminal() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut record = codex_record(tempdir.path().to_path_buf(), "claude-session-123");
    record.source = SessionMemorySource::ClaudeCode;
    record.kind = SessionMemoryKind::Terminal;
    record.last_command = Some("claude --dangerously-skip-permissions".to_string());
    record.permission_mode = AgentPermissionMode::Dangerous;

    let plan = restore_plan_for_record(&record, false).expect("restore plan should be built");

    assert_eq!(
        plan.command(),
        Some("claude --resume claude-session-123 --dangerously-skip-permissions")
    );
    assert_eq!(plan.permission_mode(), Some(AgentPermissionMode::Dangerous));
    assert_eq!(plan.auto_run(), None);
}

#[test]
fn startup_restore_routes_agent_chat_to_existing_restored_pane() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut record = codex_record(tempdir.path().to_path_buf(), "abc123");
    let pane_uuid = vec![1, 2, 3, 4];
    record.terminal_pane_uuid = Some(pane_uuid.clone());

    let action = startup_restore_action_for_record(
        &record,
        std::slice::from_ref(&restored_pane(pane_uuid.clone(), record.cwd.clone())),
        false,
        true,
    )
    .expect("restore action should be built");

    match action {
        StartupRestoreAction::ExistingPane {
            terminal_pane_uuid,
            plan,
        } => {
            assert_eq!(terminal_pane_uuid, pane_uuid);
            assert_eq!(plan.command(), Some("codex resume abc123"));
        }
        other => panic!("expected existing pane action, got {other:?}"),
    }
}

#[test]
fn startup_restore_does_not_duplicate_terminal_pane_restored_by_layout() {
    let mut record = terminal_record_with_last_command("cargo check -p warp");
    let pane_uuid = vec![5, 6, 7, 8];
    record.terminal_pane_uuid = Some(pane_uuid.clone());

    let action = startup_restore_action_for_record(
        &record,
        std::slice::from_ref(&restored_pane(pane_uuid.clone(), record.cwd.clone())),
        false,
        true,
    )
    .expect("restore action should be built");

    assert_eq!(
        action,
        StartupRestoreAction::AlreadyRestoredPane {
            terminal_pane_uuid: pane_uuid,
        }
    );
}

#[test]
fn startup_restore_runs_auto_runnable_terminal_command_in_existing_restored_pane() {
    let mut record = terminal_record_with_last_command("tmux attach -t work");
    let pane_uuid = vec![5, 6, 7, 8];
    record.terminal_pane_uuid = Some(pane_uuid.clone());

    let action = startup_restore_action_for_record(
        &record,
        std::slice::from_ref(&restored_pane(pane_uuid.clone(), record.cwd.clone())),
        false,
        true,
    )
    .expect("restore action should be built");

    match action {
        StartupRestoreAction::ExistingPane {
            terminal_pane_uuid,
            plan,
        } => {
            assert_eq!(terminal_pane_uuid, pane_uuid);
            assert_eq!(plan.command(), Some("tmux attach -t work"));
            assert_eq!(plan.auto_run(), Some(true));
        }
        other => panic!("expected existing pane action, got {other:?}"),
    }
}

#[test]
fn startup_restore_opens_new_pane_when_layout_did_not_restore_original_pane() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut record = codex_record(tempdir.path().to_path_buf(), "abc123");
    record.terminal_pane_uuid = Some(vec![1, 2, 3, 4]);

    let action = startup_restore_action_for_record(
        &record,
        &[restored_pane(
            vec![9, 9, 9, 9],
            Some(PathBuf::from("/other")),
        )],
        false,
        false,
    )
    .expect("restore action should be built");

    match action {
        StartupRestoreAction::NewPane { plan } => {
            assert_eq!(plan.command(), Some("codex resume abc123"));
        }
        other => panic!("expected new pane action, got {other:?}"),
    }
}

#[test]
fn startup_restore_routes_agent_chat_to_restored_pane_with_matching_cwd_when_uuid_changed() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut record = codex_record(tempdir.path().to_path_buf(), "abc123");
    record.terminal_pane_uuid = Some(vec![1, 2, 3, 4]);
    let restored_uuid = vec![9, 9, 9, 9];

    let action = startup_restore_action_for_record(
        &record,
        std::slice::from_ref(&restored_pane(restored_uuid.clone(), record.cwd.clone())),
        false,
        true,
    )
    .expect("restore action should be built");

    match action {
        StartupRestoreAction::ExistingPane {
            terminal_pane_uuid,
            plan,
        } => {
            assert_eq!(terminal_pane_uuid, restored_uuid);
            assert_eq!(plan.command(), Some("codex resume abc123"));
        }
        other => panic!("expected existing pane action, got {other:?}"),
    }
}

#[test]
fn startup_restore_opens_new_pane_when_no_restored_pane_matches_uuid_or_cwd() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut record = codex_record(tempdir.path().to_path_buf(), "abc123");
    record.terminal_pane_uuid = Some(vec![1, 2, 3, 4]);

    let action = startup_restore_action_for_record(
        &record,
        &[restored_pane(
            vec![9, 9, 9, 9],
            Some(PathBuf::from("/other")),
        )],
        false,
        true,
    )
    .expect("restore action should be built");

    match action {
        StartupRestoreAction::NewPane { plan } => {
            assert_eq!(plan.command(), Some("codex resume abc123"));
        }
        other => panic!("expected new pane action, got {other:?}"),
    }
}

#[test]
fn startup_restore_cwd_fallback_ignores_panes_without_known_cwd() {
    let mut record = terminal_record_with_last_command("tmux attach");
    record.terminal_pane_uuid = Some(vec![1, 2, 3, 4]);
    record.cwd = None;

    let action = startup_restore_action_for_record(
        &record,
        &[restored_pane(vec![9, 9, 9, 9], None)],
        false,
        true,
    );

    assert!(
        !matches!(
            action,
            Ok(StartupRestoreAction::ExistingPane { .. })
                | Ok(StartupRestoreAction::AlreadyRestoredPane { .. })
        ),
        "record without cwd must not match a pane without cwd, got {action:?}"
    );
}

fn restored_pane(uuid: Vec<u8>, cwd: Option<PathBuf>) -> RestoredTerminalPane {
    RestoredTerminalPane { uuid, cwd }
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
fn terminal_restore_auto_runs_safe_tmux_restore_commands() {
    for command in [
        "tmux",
        "tmux attach -t work",
        "tmux a -t work",
        "tmux attach-session -t work",
        "tmux new-session -A -s work",
        "TMUX_TMPDIR=/tmp tmux new -As work",
    ] {
        let record = terminal_record_with_last_command(command);
        let plan = terminal_restore_plan(&record, false);

        assert_eq!(plan.command_for_composer(), Some(command));
        assert_eq!(plan.auto_run(), Some(true), "{command}");
    }
}

#[test]
fn terminal_restore_does_not_auto_run_non_restore_tmux_commands_by_default() {
    for command in [
        "tmux kill-server",
        "tmux list-sessions",
        "tmux source-file ~/.tmux.conf",
    ] {
        let record = terminal_record_with_last_command(command);
        let plan = terminal_restore_plan(&record, false);

        assert_eq!(plan.command_for_composer(), Some(command));
        assert_eq!(plan.auto_run(), Some(false), "{command}");
    }
}

#[test]
fn terminal_restore_ignores_internal_warp_bootstrap_command() {
    let record = terminal_record_with_last_command(
        r#"unsetopt ZLE; WARP_SESSION_ID="$(command -p date +%s)$RANDOM"; read -r -d '' WARP_BOOTSTRAP_VAR <<'EOM'; eval "$WARP_BOOTSTRAP_VAR""#,
    );

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

    assert_eq!(normal_plan.command(), Some("codex resume abc123"));
    assert_eq!(unknown_plan.command(), Some("codex resume abc123"));
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
        app_run_id: None,
        recovery_offered_run_id: None,
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
        app_run_id: None,
        recovery_offered_run_id: None,
        restore_payload: None,
    }
}
