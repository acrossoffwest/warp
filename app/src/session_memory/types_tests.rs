use std::path::PathBuf;

use super::types::{
    AgentPermissionMode, SessionMemoryKind, SessionMemoryRecord, SessionMemorySource,
    SessionMemoryStatus, terminal_agent_command,
};

#[test]
fn stale_snapshot_of_ended_agent_keeps_end() {
    let mut incoming = agent_record(Some(100));

    incoming.keep_agent_end(&ended_agent(Some(100), Some(150)));

    assert_eq!(incoming.completed_at, Some(150));
    assert_eq!(incoming.status, SessionMemoryStatus::Success);
}

#[test]
fn new_agent_start_clears_previous_end() {
    let mut incoming = agent_record(Some(200));

    incoming.keep_agent_end(&ended_agent(Some(100), Some(150)));

    assert_eq!(incoming.completed_at, None);
    assert_eq!(incoming.status, SessionMemoryStatus::Live);
}

#[test]
fn new_agent_after_terminal_snapshot_clears_previous_end() {
    let mut incoming = agent_record(Some(200));

    incoming.keep_agent_end(&ended_agent(None, Some(150)));

    assert_eq!(incoming.completed_at, None);
}

#[test]
fn terminal_snapshot_after_agent_end_keeps_end_and_agent_identity() {
    let mut incoming = agent_record(None);
    incoming.source = SessionMemorySource::WarpTerminal;
    incoming.kind = SessionMemoryKind::Terminal;
    incoming.title = "/tmp/session-memory".to_string();
    incoming.launch_argv = None;
    incoming.permission_mode = AgentPermissionMode::Unknown;
    let mut existing = ended_agent(Some(100), Some(150));
    existing.native_session_id = Some("0a0a0a0a-0000-4000-8000-00000000000a".to_string());
    existing.transcript_path = Some(PathBuf::from("/tmp/session-memory/transcript.jsonl"));
    existing.permission_mode = AgentPermissionMode::Dangerous;

    incoming.keep_agent_end(&existing);

    assert_eq!(incoming.completed_at, Some(150));
    assert_eq!(incoming.started_at, Some(100));
    assert_eq!(incoming.source, SessionMemorySource::ClaudeCode);
    assert_eq!(incoming.kind, SessionMemoryKind::AgentChat);
    assert_eq!(incoming.status, SessionMemoryStatus::Success);
    assert_eq!(incoming.title, "claude");
    assert_eq!(incoming.native_session_id, existing.native_session_id);
    assert_eq!(incoming.transcript_path, existing.transcript_path);
    assert_eq!(incoming.launch_argv, Some(vec!["claude".to_string()]));
    assert_eq!(incoming.permission_mode, AgentPermissionMode::Dangerous);
}

#[test]
fn maintenance_subcommands_are_not_agent_commands() {
    for command in [
        "claude update",
        "claude mcp list",
        "claude setup-token",
        "claude doctor",
        "claude config set -g theme dark",
        "claude install stable",
        "claude migrate-installer",
        "FOO=1 claude update",
        "codex login",
        "codex logout",
        "codex mcp list",
        "codex completion zsh",
        "codex apply abc",
    ] {
        assert_eq!(terminal_agent_command(Some(command)), None, "{command}");
    }
}

#[test]
fn agent_commands_with_prompts_or_flags_are_still_agents() {
    for command in [
        "claude",
        "claude --dangerously-skip-permissions",
        "claude \"update the readme\"",
        "codex resume --last",
        "codex exec fix",
    ] {
        assert!(terminal_agent_command(Some(command)).is_some(), "{command}");
    }
}

#[test]
fn record_without_existing_end_is_unchanged() {
    let mut incoming = agent_record(Some(100));

    incoming.keep_agent_end(&ended_agent(Some(100), None));

    assert_eq!(incoming.completed_at, None);
    assert_eq!(incoming.status, SessionMemoryStatus::Live);
}

fn ended_agent(started_at: Option<i64>, completed_at: Option<i64>) -> SessionMemoryRecord {
    let mut record = agent_record(started_at);
    record.completed_at = completed_at;
    record
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
