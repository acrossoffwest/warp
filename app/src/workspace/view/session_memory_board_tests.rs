use std::path::PathBuf;

use super::{
    AgentPermissionMode, RowActionKind, SessionMemoryBoardFilter, SessionMemoryBoardRow,
    SessionMemorySource, SessionMemoryStatus, filter_rows, row_actions, short_row_id,
};

fn terminal_row(id: &str, status: SessionMemoryStatus) -> SessionMemoryBoardRow {
    SessionMemoryBoardRow {
        id: id.to_owned(),
        source: SessionMemorySource::WarpTerminal,
        status,
        title: "cargo check".to_owned(),
        cwd: Some(PathBuf::from("/Users/test/projects/warp")),
        project: Some("warp".to_owned()),
        native_session_id: None,
        transcript_path: None,
        last_command: Some("cargo check -p warp".to_owned()),
        permission_mode: AgentPermissionMode::Normal,
    }
}

fn codex_row(id: &str, permission_mode: AgentPermissionMode) -> SessionMemoryBoardRow {
    SessionMemoryBoardRow {
        id: id.to_owned(),
        source: SessionMemorySource::Codex,
        status: SessionMemoryStatus::Blocked,
        title: "session memory board design".to_owned(),
        cwd: Some(PathBuf::from("/Users/test/projects/warp")),
        project: Some("warp".to_owned()),
        native_session_id: Some("codex-session-123".to_owned()),
        transcript_path: Some(PathBuf::from("/Users/test/.codex/sessions/session.jsonl")),
        last_command: None,
        permission_mode,
    }
}

fn claude_row(id: &str, status: SessionMemoryStatus) -> SessionMemoryBoardRow {
    SessionMemoryBoardRow {
        id: id.to_owned(),
        source: SessionMemorySource::ClaudeCode,
        status,
        title: "Atuin and Warp integration".to_owned(),
        cwd: Some(PathBuf::from("/Users/test/projects/dotfiles")),
        project: Some("dotfiles".to_owned()),
        native_session_id: Some("claude-session-456".to_owned()),
        transcript_path: Some(PathBuf::from("/Users/test/.claude/projects/chat.jsonl")),
        last_command: None,
        permission_mode: AgentPermissionMode::Unknown,
    }
}

fn test_rows() -> Vec<SessionMemoryBoardRow> {
    vec![
        terminal_row("terminal-interrupted", SessionMemoryStatus::Interrupted),
        terminal_row("terminal-live", SessionMemoryStatus::Live),
        codex_row("codex-blocked", AgentPermissionMode::Dangerous),
        claude_row("claude-success", SessionMemoryStatus::Success),
    ]
}

#[test]
fn interrupted_filter_only_shows_interrupted_rows() {
    let visible = filter_rows(&test_rows(), SessionMemoryBoardFilter::Interrupted, "");

    assert_eq!(visible.len(), 1);
    assert!(
        visible
            .iter()
            .all(|row| row.status == SessionMemoryStatus::Interrupted)
    );
}

#[test]
fn source_filters_match_agent_sources() {
    let rows = test_rows();

    let codex = filter_rows(&rows, SessionMemoryBoardFilter::Codex, "");
    assert_eq!(codex.len(), 1);
    assert_eq!(codex[0].source, SessionMemorySource::Codex);

    let claude = filter_rows(&rows, SessionMemoryBoardFilter::ClaudeCode, "");
    assert_eq!(claude.len(), 1);
    assert_eq!(claude[0].source, SessionMemorySource::ClaudeCode);
}

#[test]
fn live_filter_only_shows_live_rows() {
    let visible = filter_rows(&test_rows(), SessionMemoryBoardFilter::Live, "");

    assert_eq!(visible.len(), 1);
    assert!(
        visible
            .iter()
            .all(|row| row.status == SessionMemoryStatus::Live)
    );
}

#[test]
fn dangerous_rows_are_badged() {
    let row = codex_row("codex-dangerous", AgentPermissionMode::Dangerous);

    assert!(row.should_show_dangerous_badge());
}

#[test]
fn normal_rows_are_not_dangerous_badged() {
    let row = codex_row("codex-normal", AgentPermissionMode::Normal);

    assert!(!row.should_show_dangerous_badge());
}

#[test]
fn query_matches_title_cwd_and_session_id() {
    let rows = test_rows();

    assert_eq!(
        filter_rows(&rows, SessionMemoryBoardFilter::All, "memory board")[0].id,
        "codex-blocked"
    );
    assert_eq!(
        filter_rows(&rows, SessionMemoryBoardFilter::All, "dotfiles")[0].id,
        "claude-success"
    );
    assert_eq!(
        filter_rows(&rows, SessionMemoryBoardFilter::All, "codex-session")[0].id,
        "codex-blocked"
    );
}

#[test]
fn terminal_rows_offer_restore_copy_and_delete_actions() {
    let row = terminal_row("terminal-interrupted", SessionMemoryStatus::Interrupted);
    let actions = row_actions(&row);
    let action_kinds: Vec<_> = actions.iter().map(|action| action.kind).collect();

    assert_eq!(
        action_kinds,
        vec![
            RowActionKind::Restore,
            RowActionKind::CopyLastCommand,
            RowActionKind::Delete,
        ]
    );
}

#[test]
fn agent_rows_offer_resume_split_transcript_and_delete_actions() {
    let row = codex_row("codex-blocked", AgentPermissionMode::Dangerous);
    let actions = row_actions(&row);
    let action_kinds: Vec<_> = actions.iter().map(|action| action.kind).collect();

    assert_eq!(
        action_kinds,
        vec![
            RowActionKind::Restore,
            RowActionKind::RestoreInSplit,
            RowActionKind::OpenTranscript,
            RowActionKind::Delete,
        ]
    );
}

#[test]
fn short_row_id_truncates_long_ids() {
    assert_eq!(short_row_id("abcdefghi"), "abcdefgh...");
    assert_eq!(short_row_id("abc"), "abc");
}
