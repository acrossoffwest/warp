use std::path::PathBuf;

use chrono::{Local, TimeZone};

use super::recording::{
    PaneRecordInput, block_timestamp_seconds, pane_session_memory_record, parse_agent_session_id,
    record_id_for_pane_uuid,
};
use super::types::{
    AgentPermissionMode, SessionMemoryKind, SessionMemorySource, SessionMemoryStatus,
};
use crate::terminal::CLIAgent;
use crate::terminal::cli_agent_sessions::{
    CLIAgentInputState, CLIAgentSession, CLIAgentSessionContext, CLIAgentSessionStatus,
};

const UUID: &str = "019e159b-717d-7663-9a93-95fd9c0790b1";
const OTHER_UUID: &str = "11111111-1111-4111-8111-111111111111";

#[test]
fn parse_reads_claude_resume_forms() {
    assert_eq!(
        parse_agent_session_id(&format!("claude --resume {UUID}")).as_deref(),
        Some(UUID)
    );
    assert_eq!(
        parse_agent_session_id(&format!("claude --resume={UUID}")).as_deref(),
        Some(UUID)
    );
    assert_eq!(
        parse_agent_session_id(&format!("claude -r {UUID}")).as_deref(),
        Some(UUID)
    );
    assert_eq!(
        parse_agent_session_id(&format!(
            "claude --dangerously-skip-permissions --resume {UUID}"
        ))
        .as_deref(),
        Some(UUID)
    );
    assert_eq!(
        parse_agent_session_id(&format!("CLAUDE_CONFIG_DIR=/tmp/x claude --resume {UUID}"))
            .as_deref(),
        Some(UUID)
    );
}

#[test]
fn parse_rejects_flag_or_missing_value_after_resume() {
    assert_eq!(
        parse_agent_session_id("claude --resume --dangerously-skip-permissions"),
        None
    );
    assert_eq!(
        parse_agent_session_id("claude --dangerously-skip-permissions --resume"),
        None
    );
    assert_eq!(parse_agent_session_id("claude -r"), None);
    assert_eq!(parse_agent_session_id("claude --resume="), None);
    assert_eq!(parse_agent_session_id("claude"), None);
}

#[test]
fn parse_reads_codex_resume_forms() {
    assert_eq!(
        parse_agent_session_id(&format!("codex resume {UUID}")).as_deref(),
        Some(UUID)
    );
    assert_eq!(
        parse_agent_session_id(&format!(
            "codex resume {UUID} --dangerously-bypass-approvals-and-sandbox"
        ))
        .as_deref(),
        Some(UUID)
    );
    assert_eq!(
        parse_agent_session_id(&format!(
            "codex --dangerously-bypass-approvals-and-sandbox resume {UUID}"
        ))
        .as_deref(),
        Some(UUID)
    );
    assert_eq!(parse_agent_session_id("codex resume --last"), None);
    assert_eq!(parse_agent_session_id("codex resume"), None);
    assert_eq!(parse_agent_session_id("codex"), None);
}

#[test]
fn parse_ignores_other_programs() {
    assert_eq!(
        parse_agent_session_id(&format!("vim --resume {UUID}")),
        None
    );
    assert_eq!(parse_agent_session_id(""), None);
}

#[test]
fn parse_rejects_non_uuid_session_ids() {
    assert_eq!(parse_agent_session_id("claude --resume abc;rm"), None);
    assert_eq!(parse_agent_session_id("claude --resume $(x)"), None);
    assert_eq!(parse_agent_session_id("claude --resume flag\""), None);
    assert_eq!(parse_agent_session_id("claude --resume abc"), None);
    assert_eq!(parse_agent_session_id("codex resume abc;rm"), None);
    assert_eq!(parse_agent_session_id("codex resume abc"), None);
    assert_eq!(parse_agent_session_id("claude -p \"please -r this\""), None);
}

#[test]
fn parse_requires_resume_in_codex_subcommand_position() {
    assert_eq!(
        parse_agent_session_id(&format!("codex exec \"fix and resume {UUID}\"")),
        None
    );
}

#[test]
fn running_claude_without_plugin_records_agent_with_parsed_id() {
    let record = pane_session_memory_record(input(
        Some(&format!(
            "claude --resume {UUID} --dangerously-skip-permissions"
        )),
        Some(1_000),
        None,
        None,
    ));

    assert_eq!(record.id, record_id_for_pane_uuid(&[1, 2, 3, 4]));
    assert_eq!(record.source, SessionMemorySource::ClaudeCode);
    assert_eq!(record.kind, SessionMemoryKind::AgentChat);
    assert_eq!(record.status, SessionMemoryStatus::Live);
    assert_eq!(record.native_session_id.as_deref(), Some(UUID));
    assert_eq!(record.permission_mode, AgentPermissionMode::Dangerous);
    assert_eq!(record.started_at, Some(1_000));
    assert_eq!(record.completed_at, None);
    assert_eq!(
        record.launch_argv,
        Some(vec![
            "claude".to_string(),
            "--resume".to_string(),
            UUID.to_string(),
            "--dangerously-skip-permissions".to_string(),
        ])
    );
    assert_eq!(record.terminal_pane_uuid, Some(vec![1, 2, 3, 4]));
    assert_eq!(record.app_run_id.as_deref(), Some("current-run"));
}

#[test]
fn agent_turn_states_all_record_live() {
    for status in [
        CLIAgentSessionStatus::InProgress,
        CLIAgentSessionStatus::Success,
        CLIAgentSessionStatus::Blocked { message: None },
        CLIAgentSessionStatus::Failed {
            error_type: None,
            message: None,
        },
        CLIAgentSessionStatus::Cancelled,
    ] {
        let session = plugin_session(CLIAgent::Claude, status.clone(), Some("plugin-id"));
        let record =
            pane_session_memory_record(input(Some("claude"), Some(10), None, Some(&session)));

        assert_eq!(record.status, SessionMemoryStatus::Live, "{status:?}");
        assert_eq!(record.completed_at, None, "{status:?}");
    }
}

#[test]
fn plugin_session_id_wins_over_command_id() {
    let session = plugin_session(
        CLIAgent::Claude,
        CLIAgentSessionStatus::InProgress,
        Some(OTHER_UUID),
    );
    let record = pane_session_memory_record(input(
        Some(&format!("claude --resume {UUID}")),
        Some(10),
        None,
        Some(&session),
    ));

    assert_eq!(record.native_session_id.as_deref(), Some(OTHER_UUID));
    assert_eq!(record.cwd, Some(PathBuf::from("/tmp/plugin-cwd")));
}

#[test]
fn plugin_session_with_invalid_id_falls_back_to_none() {
    let session = plugin_session(
        CLIAgent::Claude,
        CLIAgentSessionStatus::InProgress,
        Some("plugin-id"),
    );
    let record = pane_session_memory_record(input(Some("claude"), Some(10), None, Some(&session)));

    assert_eq!(record.native_session_id, None);
}

#[test]
fn plugin_session_with_alias_command_records_agent() {
    let session = plugin_session(CLIAgent::Codex, CLIAgentSessionStatus::InProgress, None);
    let record = pane_session_memory_record(input(Some("cx"), Some(10), None, Some(&session)));

    assert_eq!(record.source, SessionMemorySource::Codex);
    assert_eq!(record.kind, SessionMemoryKind::AgentChat);
    assert_eq!(record.permission_mode, AgentPermissionMode::Unknown);
    assert_eq!(record.native_session_id, None);
}

#[test]
fn restored_agent_block_without_running_command_records_terminal() {
    let last_command = format!("claude --resume {UUID} --dangerously-skip-permissions");
    let record = pane_session_memory_record(input(None, None, Some(&last_command), None));

    assert_eq!(record.source, SessionMemorySource::WarpTerminal);
    assert_eq!(record.kind, SessionMemoryKind::Terminal);
    assert_eq!(record.native_session_id, None);
    assert_eq!(record.permission_mode, AgentPermissionMode::Unknown);
    assert_eq!(record.started_at, None);
    assert_eq!(record.last_command.as_deref(), Some(last_command.as_str()));
}

#[test]
fn running_non_agent_command_records_terminal() {
    let record = pane_session_memory_record(input(Some("vim notes.md"), Some(10), None, None));

    assert_eq!(record.source, SessionMemorySource::WarpTerminal);
    assert_eq!(record.kind, SessionMemoryKind::Terminal);
    assert_eq!(record.started_at, None);
}

#[test]
fn agent_maintenance_subcommands_record_terminal() {
    let claude = plugin_session(CLIAgent::Claude, CLIAgentSessionStatus::InProgress, None);
    let codex = plugin_session(CLIAgent::Codex, CLIAgentSessionStatus::InProgress, None);
    for (command, session) in [
        ("claude update", &claude),
        ("claude mcp add foo", &claude),
        ("claude doctor", &claude),
        ("codex login", &codex),
        ("codex mcp list", &codex),
        ("codex completion zsh", &codex),
    ] {
        for session in [None, Some(session)] {
            let record = pane_session_memory_record(input(Some(command), Some(10), None, session));

            assert_eq!(
                record.source,
                SessionMemorySource::WarpTerminal,
                "{command}"
            );
            assert_eq!(record.kind, SessionMemoryKind::Terminal, "{command}");
        }
    }
}

#[test]
fn record_started_at_matches_block_timestamp_seconds() {
    let block_start = Local.timestamp_millis_opt(1_700_000_000_900).unwrap();
    let started_at = block_timestamp_seconds(Some(&block_start));

    let record = pane_session_memory_record(input(Some("claude"), started_at, None, None));

    assert_eq!(record.started_at, Some(1_700_000_000));
    assert_eq!(
        record.started_at,
        block_timestamp_seconds(Some(&block_start))
    );
}

fn input<'a>(
    running_command: Option<&str>,
    running_command_started_at: Option<i64>,
    last_command: Option<&str>,
    cli_agent_session: Option<&'a CLIAgentSession>,
) -> PaneRecordInput<'a> {
    PaneRecordInput {
        uuid: &[1, 2, 3, 4],
        cwd: Some(PathBuf::from("/tmp/pane-cwd")),
        running_command: running_command.map(str::to_owned),
        running_command_started_at,
        last_command: last_command.map(str::to_owned),
        cli_agent_session,
        restore_payload: None,
        app_run_id: Some("current-run".to_string()),
        now: 2_000,
    }
}

fn plugin_session(
    agent: CLIAgent,
    status: CLIAgentSessionStatus,
    session_id: Option<&str>,
) -> CLIAgentSession {
    CLIAgentSession {
        agent,
        status,
        session_context: CLIAgentSessionContext {
            session_id: session_id.map(str::to_owned),
            cwd: Some("/tmp/plugin-cwd".to_string()),
            ..Default::default()
        },
        input_state: CLIAgentInputState::Closed,
        should_auto_toggle_input: false,
        listener: None,
        plugin_version: None,
        remote_host: None,
        draft_text: None,
        custom_command_prefix: None,
        received_rich_notification: false,
    }
}
