use std::path::PathBuf;

use pathfinder_color::ColorU;
use warpui::fonts::FamilyId;
use warpui::ui_components::components::Coords;

use crate::session_memory::types::{is_internal_warp_command, SessionMemoryRecord};

use super::{
    command_preview, filter_rows, remove_row_by_id, row_actions, rows_from_records, short_row_id,
    text_button_styles, AgentPermissionMode, RowActionKind, SessionMemoryBoardFilter,
    SessionMemoryBoardRow, SessionMemoryKind, SessionMemorySource, SessionMemoryStatus,
};

fn terminal_row(id: &str, status: SessionMemoryStatus) -> SessionMemoryBoardRow {
    SessionMemoryBoardRow {
        id: id.to_owned(),
        source: SessionMemorySource::WarpTerminal,
        kind: SessionMemoryKind::Terminal,
        status,
        title: "cargo check".to_owned(),
        cwd: Some(PathBuf::from("/Users/test/projects/warp")),
        project: Some("warp".to_owned()),
        native_session_id: None,
        transcript_path: None,
        last_command: Some("cargo check -p warp".to_owned()),
        permission_mode: AgentPermissionMode::Normal,
        updated_at: 1_725_000_001,
    }
}

fn codex_row(id: &str, permission_mode: AgentPermissionMode) -> SessionMemoryBoardRow {
    SessionMemoryBoardRow {
        id: id.to_owned(),
        source: SessionMemorySource::Codex,
        kind: SessionMemoryKind::AgentChat,
        status: SessionMemoryStatus::Blocked,
        title: "session memory board design".to_owned(),
        cwd: Some(PathBuf::from("/Users/test/projects/warp")),
        project: Some("warp".to_owned()),
        native_session_id: Some("codex-session-123".to_owned()),
        transcript_path: Some(PathBuf::from("/Users/test/.codex/sessions/session.jsonl")),
        last_command: None,
        permission_mode,
        updated_at: 1_725_000_002,
    }
}

fn claude_row(id: &str, status: SessionMemoryStatus) -> SessionMemoryBoardRow {
    SessionMemoryBoardRow {
        id: id.to_owned(),
        source: SessionMemorySource::ClaudeCode,
        kind: SessionMemoryKind::AgentChat,
        status,
        title: "Atuin and Warp integration".to_owned(),
        cwd: Some(PathBuf::from("/Users/test/projects/dotfiles")),
        project: Some("dotfiles".to_owned()),
        native_session_id: Some("claude-session-456".to_owned()),
        transcript_path: Some(PathBuf::from("/Users/test/.claude/projects/chat.jsonl")),
        last_command: None,
        permission_mode: AgentPermissionMode::Unknown,
        updated_at: 1_725_000_003,
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

fn persisted_record(
    id: &str,
    source: SessionMemorySource,
    kind: SessionMemoryKind,
    permission_mode: AgentPermissionMode,
) -> SessionMemoryRecord {
    SessionMemoryRecord {
        id: id.to_owned(),
        source,
        kind,
        status: SessionMemoryStatus::Blocked,
        title: "resume critical session".to_owned(),
        summary: Some("Important session summary".to_owned()),
        cwd: Some(PathBuf::from("/Users/test/projects/warp")),
        project: Some("warp".to_owned()),
        native_session_id: Some(format!("{id}-native")),
        transcript_path: Some(PathBuf::from(format!("/Users/test/.sessions/{id}.jsonl"))),
        terminal_pane_uuid: Some(vec![1, 2, 3, 4]),
        app_window_fingerprint: Some("window-fingerprint".to_owned()),
        app_tab_fingerprint: Some("tab-fingerprint".to_owned()),
        last_command: Some("cargo test -p warp session_memory_board".to_owned()),
        last_exit_code: Some(101),
        launch_argv: Some(vec!["codex".to_owned(), "resume".to_owned()]),
        permission_mode,
        last_seen_at: 1_725_000_001,
        started_at: Some(1_725_000_000),
        completed_at: None,
        closed_intentionally_at: None,
        app_run_id: Some("previous-run".to_owned()),
        recovery_offered_run_id: None,
        restore_payload: Some(serde_json::json!({ "pane": "left" })),
    }
}

#[test]
fn board_row_maps_restore_fields_from_persisted_record() {
    let record = persisted_record(
        "codex-live-wire",
        SessionMemorySource::Codex,
        SessionMemoryKind::AgentChat,
        AgentPermissionMode::Dangerous,
    );

    let row = SessionMemoryBoardRow::from(&record);

    assert_eq!(row.id, record.id);
    assert_eq!(row.source, record.source);
    assert_eq!(row.kind, record.kind);
    assert_eq!(row.status, record.status);
    assert_eq!(row.title, record.title);
    assert_eq!(row.cwd, record.cwd);
    assert_eq!(row.native_session_id, record.native_session_id);
    assert_eq!(row.last_command, record.last_command);
    assert_eq!(row.transcript_path, record.transcript_path);
    assert_eq!(row.permission_mode, record.permission_mode);
    assert_eq!(row.updated_at, record.last_seen_at);
}

#[test]
fn rows_from_records_preserves_order_and_maps_all_records() {
    let records = vec![
        persisted_record(
            "terminal-row",
            SessionMemorySource::WarpTerminal,
            SessionMemoryKind::Terminal,
            AgentPermissionMode::Normal,
        ),
        persisted_record(
            "claude-row",
            SessionMemorySource::ClaudeCode,
            SessionMemoryKind::AgentChat,
            AgentPermissionMode::Unknown,
        ),
    ];

    let rows = rows_from_records(&records);

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, "terminal-row");
    assert_eq!(rows[0].kind, SessionMemoryKind::Terminal);
    assert_eq!(rows[1].id, "claude-row");
    assert_eq!(rows[1].source, SessionMemorySource::ClaudeCode);
}

#[test]
fn interrupted_filter_only_shows_interrupted_rows() {
    let visible = filter_rows(&test_rows(), SessionMemoryBoardFilter::Interrupted, "");

    assert_eq!(visible.len(), 1);
    assert!(visible
        .iter()
        .all(|row| row.status == SessionMemoryStatus::Interrupted));
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
    assert!(visible
        .iter()
        .all(|row| row.status == SessionMemoryStatus::Live));
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
fn internal_warp_bootstrap_commands_are_hidden() {
    let bootstrap = r#"unsetopt ZLE; WARP_SESSION_ID="$(command -p date +%s)$RANDOM"; read -r -d '' WARP_BOOTSTRAP_VAR <<'EOM'"#;

    assert!(is_internal_warp_command(bootstrap));
    assert_eq!(command_preview(Some(bootstrap)), None);
}

#[test]
fn command_preview_is_single_line_and_truncated() {
    let command =
        "cargo test -p warp session_memory_board -- --nocapture\nsecond line that must not show";

    assert_eq!(
        command_preview(Some(command)).as_deref(),
        Some("cargo test -p warp session_memory_board -- --nocapture")
    );

    let long = "x".repeat(180);
    let preview = command_preview(Some(&long)).unwrap();
    assert!(preview.len() <= 123);
    assert!(preview.ends_with("..."));
}

#[test]
fn button_interaction_styles_preserve_required_text_fields() {
    let styles = text_button_styles(
        FamilyId(42),
        12.,
        30.,
        8.,
        Coords::uniform(0.).left(11.).right(11.),
        ColorU::new(10, 10, 10, 255),
        ColorU::new(20, 20, 20, 255),
        ColorU::new(30, 30, 30, 255),
        ColorU::new(240, 240, 240, 255),
        ColorU::new(230, 230, 230, 255),
        ColorU::new(220, 220, 220, 255),
    );

    for style in [styles.0, styles.1, styles.2] {
        assert_eq!(style.font_family_id, Some(FamilyId(42)));
        assert_eq!(style.font_size, Some(12.));
        assert_eq!(style.height, Some(30.));
        assert!(style.padding.is_some());
    }
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
fn delete_action_removes_row_from_board_immediately() {
    let mut rows = test_rows();

    assert!(remove_row_by_id(&mut rows, "codex-blocked"));
    assert!(!rows.iter().any(|row| row.id == "codex-blocked"));
    assert!(!remove_row_by_id(&mut rows, "codex-blocked"));
}

#[test]
fn short_row_id_truncates_long_ids() {
    assert_eq!(short_row_id("abcdefghi"), "abcdefgh...");
    assert_eq!(short_row_id("abc"), "abc");
}
