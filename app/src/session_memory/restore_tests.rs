use std::collections::HashSet;
use std::path::PathBuf;

use super::restore::{
    AgentSessionFile, RestoreError, RestoredTerminalPane, StartupRestoreAction, StartupRestoreSkip,
    StartupRestoreTarget, agent_restore_plan, match_session_file, plan_startup_restore,
    resolve_missing_session_ids, restore_plan_for_record, startup_agent_restore_plan,
    startup_restore_action_for_record, terminal_restore_plan,
};
use super::types::{
    AgentPermissionMode, SessionMemoryKind, SessionMemoryRecord, SessionMemorySource,
    SessionMemoryStatus,
};

#[test]
fn codex_restore_uses_saved_cwd_and_dangerous_flag() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let record = dangerous_codex_record(
        tempdir.path().to_path_buf(),
        "019e159b-717d-7663-9a93-95fd9c0790b1",
    );

    let plan = agent_restore_plan(&record).expect("restore plan should be built");

    assert_eq!(plan.cwd(), Some(tempdir.path()));
    assert_eq!(
        plan.command(),
        Some(
            "codex resume 019e159b-717d-7663-9a93-95fd9c0790b1 --dangerously-bypass-approvals-and-sandbox"
        )
    );
    assert_eq!(plan.permission_mode(), Some(AgentPermissionMode::Dangerous));
}

#[test]
fn claude_restore_uses_saved_session_id_and_dangerous_flag() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut record = codex_record(
        tempdir.path().to_path_buf(),
        "11111111-1111-4111-8111-111111111111",
    );
    record.source = SessionMemorySource::ClaudeCode;
    record.title = "Claude session".to_string();
    record.permission_mode = AgentPermissionMode::Dangerous;

    let plan = agent_restore_plan(&record).expect("restore plan should be built");

    assert_eq!(plan.cwd(), Some(tempdir.path()));
    assert_eq!(
        plan.command(),
        Some("claude --resume 11111111-1111-4111-8111-111111111111 --dangerously-skip-permissions")
    );
    assert_eq!(plan.permission_mode(), Some(AgentPermissionMode::Dangerous));
}

#[test]
fn restore_plan_treats_agent_source_with_session_id_as_agent_chat_even_if_kind_is_terminal() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut record = codex_record(
        tempdir.path().to_path_buf(),
        "11111111-1111-4111-8111-111111111111",
    );
    record.source = SessionMemorySource::ClaudeCode;
    record.kind = SessionMemoryKind::Terminal;
    record.last_command = Some("claude --dangerously-skip-permissions".to_string());
    record.permission_mode = AgentPermissionMode::Dangerous;

    let plan = restore_plan_for_record(&record, false).expect("restore plan should be built");

    assert_eq!(
        plan.command(),
        Some("claude --resume 11111111-1111-4111-8111-111111111111 --dangerously-skip-permissions")
    );
    assert_eq!(plan.permission_mode(), Some(AgentPermissionMode::Dangerous));
    assert_eq!(plan.auto_run(), None);
}

#[test]
fn startup_restore_routes_agent_chat_to_existing_restored_pane() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut record = codex_record(
        tempdir.path().to_path_buf(),
        "019e159b-717d-7663-9a93-95fd9c0790b1",
    );
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
            assert_eq!(
                plan.command(),
                Some("codex resume 019e159b-717d-7663-9a93-95fd9c0790b1")
            );
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
    let mut record = codex_record(
        tempdir.path().to_path_buf(),
        "019e159b-717d-7663-9a93-95fd9c0790b1",
    );
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
            assert_eq!(
                plan.command(),
                Some("codex resume 019e159b-717d-7663-9a93-95fd9c0790b1")
            );
        }
        other => panic!("expected new pane action, got {other:?}"),
    }
}

#[test]
fn startup_restore_routes_agent_chat_to_restored_pane_with_matching_cwd_when_uuid_changed() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut record = codex_record(
        tempdir.path().to_path_buf(),
        "019e159b-717d-7663-9a93-95fd9c0790b1",
    );
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
            assert_eq!(
                plan.command(),
                Some("codex resume 019e159b-717d-7663-9a93-95fd9c0790b1")
            );
        }
        other => panic!("expected existing pane action, got {other:?}"),
    }
}

#[test]
fn startup_restore_opens_new_pane_when_no_restored_pane_matches_uuid_or_cwd() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut record = codex_record(
        tempdir.path().to_path_buf(),
        "019e159b-717d-7663-9a93-95fd9c0790b1",
    );
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
            assert_eq!(
                plan.command(),
                Some("codex resume 019e159b-717d-7663-9a93-95fd9c0790b1")
            );
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
    let record = codex_record(missing_cwd.clone(), "019e159b-717d-7663-9a93-95fd9c0790b1");

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
    let mut normal_record = codex_record(
        tempdir.path().to_path_buf(),
        "019e159b-717d-7663-9a93-95fd9c0790b1",
    );
    normal_record.permission_mode = AgentPermissionMode::Normal;
    let mut unknown_record = normal_record.clone();
    unknown_record.permission_mode = AgentPermissionMode::Unknown;

    let normal_plan = agent_restore_plan(&normal_record).expect("normal plan should be built");
    let unknown_plan = agent_restore_plan(&unknown_record).expect("unknown plan should be built");

    assert_eq!(
        normal_plan.command(),
        Some("codex resume 019e159b-717d-7663-9a93-95fd9c0790b1")
    );
    assert_eq!(
        unknown_plan.command(),
        Some("codex resume 019e159b-717d-7663-9a93-95fd9c0790b1")
    );
}

#[test]
fn agent_restore_requires_native_session_id() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut record = codex_record(
        tempdir.path().to_path_buf(),
        "019e159b-717d-7663-9a93-95fd9c0790b1",
    );
    record.native_session_id = None;

    let result = agent_restore_plan(&record);

    assert_eq!(result, Err(RestoreError::MissingSessionId));
}

#[test]
fn unsupported_source_does_not_build_agent_plan() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut record = codex_record(
        tempdir.path().to_path_buf(),
        "019e159b-717d-7663-9a93-95fd9c0790b1",
    );
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

const SESSION_A: &str = "0a0a0a0a-0000-4000-8000-00000000000a";
const SESSION_B: &str = "0b0b0b0b-0000-4000-8000-00000000000b";
const SESSION_TAKEN: &str = "0c0c0c0c-0000-4000-8000-00000000000c";

#[test]
fn startup_plan_resumes_claude_with_id_and_dangerous_flag() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let record = startup_agent(
        tempdir.path().to_path_buf(),
        SessionMemorySource::ClaudeCode,
        Some(SESSION_A),
        AgentPermissionMode::Dangerous,
    );

    let plan = startup_agent_restore_plan(&record).expect("plan should be built");

    assert_eq!(
        plan.command(),
        Some(format!("claude --resume {SESSION_A} --dangerously-skip-permissions").as_str())
    );
    assert_eq!(plan.cwd(), Some(tempdir.path()));
}

#[test]
fn startup_plan_continues_claude_without_id() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let record = startup_agent(
        tempdir.path().to_path_buf(),
        SessionMemorySource::ClaudeCode,
        None,
        AgentPermissionMode::Dangerous,
    );

    let plan = startup_agent_restore_plan(&record).expect("plan should be built");

    assert_eq!(
        plan.command(),
        Some("claude --continue --dangerously-skip-permissions")
    );
}

#[test]
fn startup_plan_continues_claude_when_stored_id_is_not_a_uuid() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let record = startup_agent(
        tempdir.path().to_path_buf(),
        SessionMemorySource::ClaudeCode,
        Some("not-a-uuid"),
        AgentPermissionMode::Normal,
    );

    let plan = startup_agent_restore_plan(&record).expect("plan should be built");

    assert_eq!(plan.command(), Some("claude --continue"));
}

#[test]
fn startup_plan_resumes_last_codex_without_id() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let record = startup_agent(
        tempdir.path().to_path_buf(),
        SessionMemorySource::Codex,
        None,
        AgentPermissionMode::Normal,
    );

    let plan = startup_agent_restore_plan(&record).expect("plan should be built");

    assert_eq!(plan.command(), Some("codex resume --last"));
}

#[test]
fn startup_plan_resumes_codex_with_id() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let record = startup_agent(
        tempdir.path().to_path_buf(),
        SessionMemorySource::Codex,
        Some(SESSION_A),
        AgentPermissionMode::Unknown,
    );

    let plan = startup_agent_restore_plan(&record).expect("plan should be built");

    assert_eq!(
        plan.command(),
        Some(format!("codex resume {SESSION_A}").as_str())
    );
}

#[test]
fn startup_plan_rejects_missing_cwd() {
    let missing = PathBuf::from("/tmp/session-memory-missing-cwd-for-startup-plan");
    let record = startup_agent(
        missing.clone(),
        SessionMemorySource::ClaudeCode,
        Some(SESSION_A),
        AgentPermissionMode::Normal,
    );

    assert_eq!(
        startup_agent_restore_plan(&record),
        Err(RestoreError::MissingWorkingDirectory(missing))
    );
}

#[test]
fn startup_restore_targets_panes_across_two_windows() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut first = startup_agent(
        tempdir.path().to_path_buf(),
        SessionMemorySource::ClaudeCode,
        Some(SESSION_A),
        AgentPermissionMode::Normal,
    );
    first.id = "first".to_string();
    first.terminal_pane_uuid = Some(vec![1]);
    let mut second = startup_agent(
        tempdir.path().to_path_buf(),
        SessionMemorySource::Codex,
        Some(SESSION_B),
        AgentPermissionMode::Normal,
    );
    second.id = "second".to_string();
    second.terminal_pane_uuid = Some(vec![2]);
    let restored = vec![
        ("window-a", vec![1]),
        ("window-b", vec![9]),
        ("window-b", vec![2]),
    ];

    let targets = plan_startup_restore(&[first, second], &restored, true);

    let target_of = |id: &str| {
        targets
            .iter()
            .find(|(record_id, _)| record_id == id)
            .map(|(_, target)| target.clone())
            .expect("target should exist")
    };
    match target_of("first") {
        StartupRestoreTarget::ExistingPane {
            window,
            terminal_pane_uuid,
            plan,
        } => {
            assert_eq!(window, "window-a");
            assert_eq!(terminal_pane_uuid, vec![1]);
            assert_eq!(
                plan.command(),
                Some(format!("claude --resume {SESSION_A}").as_str())
            );
        }
        other => panic!("expected existing pane, got {other:?}"),
    }
    match target_of("second") {
        StartupRestoreTarget::ExistingPane {
            window,
            terminal_pane_uuid,
            plan,
        } => {
            assert_eq!(window, "window-b");
            assert_eq!(terminal_pane_uuid, vec![2]);
            assert_eq!(
                plan.command(),
                Some(format!("codex resume {SESSION_B}").as_str())
            );
        }
        other => panic!("expected existing pane, got {other:?}"),
    }
}

#[test]
fn startup_restore_skips_missing_pane_when_layout_restore_is_on() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut record = startup_agent(
        tempdir.path().to_path_buf(),
        SessionMemorySource::ClaudeCode,
        Some(SESSION_A),
        AgentPermissionMode::Normal,
    );
    record.terminal_pane_uuid = Some(vec![7]);

    let targets = plan_startup_restore(&[record], &[("window-a", vec![1])], true);

    assert_eq!(
        targets[0].1,
        StartupRestoreTarget::Skip(StartupRestoreSkip::PaneGone)
    );
}

#[test]
fn startup_restore_opens_new_tab_when_layout_restore_is_off() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut record = startup_agent(
        tempdir.path().to_path_buf(),
        SessionMemorySource::ClaudeCode,
        Some(SESSION_A),
        AgentPermissionMode::Normal,
    );
    record.terminal_pane_uuid = Some(vec![7]);

    let targets = plan_startup_restore::<&str>(&[record], &[], false);

    match &targets[0].1 {
        StartupRestoreTarget::NewTab { plan } => {
            assert_eq!(
                plan.command(),
                Some(format!("claude --resume {SESSION_A}").as_str())
            );
            assert_eq!(plan.cwd(), Some(tempdir.path()));
        }
        other => panic!("expected new tab, got {other:?}"),
    }
}

#[test]
fn startup_restore_continues_only_newest_idless_candidate_per_cwd() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut older = startup_agent(
        tempdir.path().to_path_buf(),
        SessionMemorySource::ClaudeCode,
        None,
        AgentPermissionMode::Normal,
    );
    older.id = "older".to_string();
    older.terminal_pane_uuid = Some(vec![1]);
    older.last_seen_at = 100;
    let mut newer = older.clone();
    newer.id = "newer".to_string();
    newer.terminal_pane_uuid = Some(vec![2]);
    newer.last_seen_at = 200;
    let restored = vec![("window-a", vec![1]), ("window-a", vec![2])];

    let targets = plan_startup_restore(&[older, newer], &restored, true);

    let target_of = |id: &str| {
        targets
            .iter()
            .find(|(record_id, _)| record_id == id)
            .map(|(_, target)| target.clone())
            .expect("target should exist")
    };
    assert!(matches!(
        target_of("newer"),
        StartupRestoreTarget::ExistingPane { .. }
    ));
    assert_eq!(
        target_of("older"),
        StartupRestoreTarget::Skip(StartupRestoreSkip::DuplicateContinue)
    );
}

#[test]
fn match_session_file_picks_earliest_unclaimed_file_after_start() {
    let files = vec![
        session_file("before-start", 90),
        session_file("claimed", 101),
        session_file("second", 120),
        session_file("first", 105),
    ];
    let claimed = HashSet::from(["claimed".to_string()]);

    assert_eq!(
        match_session_file(100, 1000, &files, &claimed).as_deref(),
        Some("first")
    );
}

#[test]
fn match_session_file_ignores_files_created_before_start() {
    let files = vec![session_file("old", 50)];

    assert_eq!(match_session_file(100, 1000, &files, &HashSet::new()), None);
}

#[test]
fn match_session_file_accepts_tolerance_before_start() {
    let files = vec![session_file("just-before", 99)];

    assert_eq!(
        match_session_file(100, 1000, &files, &HashSet::new()).as_deref(),
        Some("just-before")
    );
}

#[test]
fn resolve_missing_session_ids_does_not_assign_one_file_to_two_panes() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut first = startup_agent(
        tempdir.path().to_path_buf(),
        SessionMemorySource::ClaudeCode,
        None,
        AgentPermissionMode::Normal,
    );
    first.id = "first".to_string();
    first.started_at = Some(100);
    first.last_seen_at = 1000;
    let mut second = first.clone();
    second.id = "second".to_string();
    second.started_at = Some(110);
    let mut with_id = first.clone();
    with_id.id = "with-id".to_string();
    with_id.native_session_id = Some(SESSION_TAKEN.to_string());
    let mut candidates = vec![second, first, with_id];

    resolve_missing_session_ids(&mut candidates, HashSet::new(), |_, _| {
        vec![
            session_file(SESSION_TAKEN, 100),
            session_file(SESSION_A, 101),
            session_file(SESSION_B, 111),
        ]
    });

    let id_of = |id: &str| {
        candidates
            .iter()
            .find(|candidate| candidate.id == id)
            .and_then(|candidate| candidate.native_session_id.clone())
    };
    assert_eq!(id_of("first").as_deref(), Some(SESSION_A));
    assert_eq!(id_of("second").as_deref(), Some(SESSION_B));
    assert_eq!(id_of("with-id").as_deref(), Some(SESSION_TAKEN));
}

#[test]
fn resolve_missing_session_ids_ignores_files_without_uuid_ids() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut record = startup_agent(
        tempdir.path().to_path_buf(),
        SessionMemorySource::Codex,
        None,
        AgentPermissionMode::Normal,
    );
    record.started_at = Some(100);
    record.last_seen_at = 1000;
    let mut candidates = vec![record];

    resolve_missing_session_ids(&mut candidates, HashSet::new(), |_, _| {
        vec![
            session_file("not-a-uuid", 101),
            session_file(SESSION_A, 102),
        ]
    });

    assert_eq!(candidates[0].native_session_id.as_deref(), Some(SESSION_A));
}

#[test]
fn match_session_file_ignores_files_created_after_pane_was_observed() {
    let files = vec![session_file(SESSION_A, 300)];

    assert_eq!(match_session_file(100, 200, &files, &HashSet::new()), None);
}

#[test]
fn match_session_file_breaks_same_second_ties_by_session_id() {
    let files = vec![session_file(SESSION_B, 105), session_file(SESSION_A, 105)];

    assert_eq!(
        match_session_file(100, 1000, &files, &HashSet::new()).as_deref(),
        Some(SESSION_A)
    );
}

#[test]
fn resolve_missing_session_ids_uses_completed_at_as_upper_bound() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut record = startup_agent(
        tempdir.path().to_path_buf(),
        SessionMemorySource::ClaudeCode,
        None,
        AgentPermissionMode::Normal,
    );
    record.started_at = Some(100);
    record.completed_at = Some(150);
    record.last_seen_at = 1000;
    let mut candidates = vec![record];

    resolve_missing_session_ids(&mut candidates, HashSet::new(), |_, _| {
        vec![session_file(SESSION_A, 200)]
    });

    assert_eq!(candidates[0].native_session_id, None);
}

#[test]
fn resolve_missing_session_ids_does_not_match_file_created_after_last_seen() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut record = startup_agent(
        tempdir.path().to_path_buf(),
        SessionMemorySource::ClaudeCode,
        None,
        AgentPermissionMode::Normal,
    );
    record.started_at = Some(100);
    record.last_seen_at = 200;
    let mut candidates = vec![record];

    resolve_missing_session_ids(&mut candidates, HashSet::new(), |_, _| {
        vec![session_file(SESSION_A, 201)]
    });

    assert_eq!(candidates[0].native_session_id, None);
}

#[test]
fn resolve_missing_session_ids_replaces_non_uuid_stored_id_from_file() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut record = startup_agent(
        tempdir.path().to_path_buf(),
        SessionMemorySource::ClaudeCode,
        Some("not-a-uuid"),
        AgentPermissionMode::Normal,
    );
    record.started_at = Some(100);
    record.last_seen_at = 1000;
    let mut candidates = vec![record];

    resolve_missing_session_ids(&mut candidates, HashSet::new(), |_, _| {
        vec![session_file(SESSION_A, 101)]
    });

    assert_eq!(candidates[0].native_session_id.as_deref(), Some(SESSION_A));
}

#[test]
fn startup_plan_resumes_last_codex_with_dangerous_flag() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let record = startup_agent(
        tempdir.path().to_path_buf(),
        SessionMemorySource::Codex,
        None,
        AgentPermissionMode::Dangerous,
    );

    let plan = startup_agent_restore_plan(&record).expect("plan should be built");

    assert_eq!(
        plan.command(),
        Some("codex resume --last --dangerously-bypass-approvals-and-sandbox")
    );
}

#[test]
fn startup_restore_opens_new_tab_without_pane_uuid_when_layout_restore_is_off() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let record = startup_agent(
        tempdir.path().to_path_buf(),
        SessionMemorySource::ClaudeCode,
        None,
        AgentPermissionMode::Normal,
    );

    let targets = plan_startup_restore(&[record], &[("window-a", vec![1])], false);

    match &targets[0].1 {
        StartupRestoreTarget::NewTab { plan } => {
            assert_eq!(plan.command(), Some("claude --continue"));
        }
        other => panic!("expected new tab, got {other:?}"),
    }
}

#[test]
fn startup_restore_record_with_id_does_not_consume_continue_slot() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let mut with_id = startup_agent(
        tempdir.path().to_path_buf(),
        SessionMemorySource::ClaudeCode,
        Some(SESSION_A),
        AgentPermissionMode::Normal,
    );
    with_id.id = "with-id".to_string();
    with_id.terminal_pane_uuid = Some(vec![1]);
    with_id.last_seen_at = 200;
    let mut idless = with_id.clone();
    idless.id = "idless".to_string();
    idless.native_session_id = None;
    idless.terminal_pane_uuid = Some(vec![2]);
    idless.last_seen_at = 100;
    let restored = vec![("window-a", vec![1]), ("window-a", vec![2])];

    let targets = plan_startup_restore(&[with_id, idless], &restored, true);

    assert!(
        targets
            .iter()
            .all(|(_, target)| matches!(target, StartupRestoreTarget::ExistingPane { .. }))
    );
}

#[test]
fn startup_restore_continues_idless_records_with_different_source_or_cwd() {
    let tempdir = tempfile::tempdir().expect("tempdir should be created");
    let other_dir = tempfile::tempdir().expect("tempdir should be created");
    let mut claude = startup_agent(
        tempdir.path().to_path_buf(),
        SessionMemorySource::ClaudeCode,
        None,
        AgentPermissionMode::Normal,
    );
    claude.id = "claude".to_string();
    claude.terminal_pane_uuid = Some(vec![1]);
    let mut codex = claude.clone();
    codex.id = "codex".to_string();
    codex.source = SessionMemorySource::Codex;
    codex.terminal_pane_uuid = Some(vec![2]);
    let mut other_folder = claude.clone();
    other_folder.id = "other-folder".to_string();
    other_folder.cwd = Some(other_dir.path().to_path_buf());
    other_folder.terminal_pane_uuid = Some(vec![3]);
    let restored = vec![
        ("window-a", vec![1]),
        ("window-a", vec![2]),
        ("window-b", vec![3]),
    ];

    let targets = plan_startup_restore(&[claude, codex, other_folder], &restored, true);

    assert_eq!(targets.len(), 3);
    assert!(
        targets
            .iter()
            .all(|(_, target)| matches!(target, StartupRestoreTarget::ExistingPane { .. }))
    );
}

fn startup_agent(
    cwd: PathBuf,
    source: SessionMemorySource,
    session_id: Option<&str>,
    permission_mode: AgentPermissionMode,
) -> SessionMemoryRecord {
    let mut record = codex_record(cwd, "unused");
    record.source = source;
    record.native_session_id = session_id.map(str::to_owned);
    record.permission_mode = permission_mode;
    record.status = SessionMemoryStatus::Interrupted;
    record
}

fn session_file(session_id: &str, created_at: i64) -> AgentSessionFile {
    AgentSessionFile {
        session_id: session_id.to_string(),
        created_at,
    }
}
