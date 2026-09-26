use std::path::PathBuf;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use chrono::{DateTime, Local};

use super::restore::is_env_assignment;
use super::types::{
    AgentPermissionMode, SessionMemoryKind, SessionMemoryRecord, SessionMemorySource,
    SessionMemoryStatus, terminal_agent_command, user_command,
};
use crate::terminal::CLIAgent;
use crate::terminal::cli_agent_sessions::CLIAgentSession;

pub fn record_id_for_pane_uuid(uuid: &[u8]) -> String {
    format!("warp_terminal:{}", BASE64_STANDARD.encode(uuid))
}

pub fn block_timestamp_seconds(timestamp: Option<&DateTime<Local>>) -> Option<i64> {
    timestamp.map(DateTime::timestamp)
}

pub fn source_for_cli_agent(agent: CLIAgent) -> Option<SessionMemorySource> {
    match agent {
        CLIAgent::Claude => Some(SessionMemorySource::ClaudeCode),
        CLIAgent::Codex => Some(SessionMemorySource::Codex),
        _ => None,
    }
}

pub fn parse_agent_session_id(command: &str) -> Option<String> {
    let tokens = command
        .split_whitespace()
        .skip_while(|token| is_env_assignment(token))
        .collect::<Vec<_>>();
    let (program, args) = tokens.split_first()?;
    match *program {
        "claude" => claude_session_id(args),
        "codex" => codex_session_id(args),
        _ => None,
    }
}

fn claude_session_id(args: &[&str]) -> Option<String> {
    args.iter().enumerate().find_map(|(index, arg)| {
        if let Some(value) = arg.strip_prefix("--resume=") {
            return session_id_value(value);
        }
        if matches!(*arg, "--resume" | "-r") {
            return args
                .get(index + 1)
                .and_then(|value| session_id_value(value));
        }
        None
    })
}

fn codex_session_id(args: &[&str]) -> Option<String> {
    let resume_index = args.iter().position(|arg| *arg == "resume")?;
    let resume_args = &args[resume_index + 1..];
    if resume_args.contains(&"--last") {
        return None;
    }
    resume_args
        .first()
        .and_then(|value| session_id_value(value))
}

fn session_id_value(value: &str) -> Option<String> {
    (!value.is_empty() && !value.starts_with('-')).then(|| value.to_owned())
}

pub struct PaneRecordInput<'a> {
    pub uuid: &'a [u8],
    pub cwd: Option<PathBuf>,
    pub running_command: Option<String>,
    pub running_command_started_at: Option<i64>,
    pub last_command: Option<String>,
    pub cli_agent_session: Option<&'a CLIAgentSession>,
    pub restore_payload: Option<serde_json::Value>,
    pub app_run_id: Option<String>,
    pub now: i64,
}

pub fn pane_session_memory_record(input: PaneRecordInput<'_>) -> SessionMemoryRecord {
    let running_command = user_command(input.running_command.as_deref());
    let agent = running_command
        .as_deref()
        .and_then(|command| running_agent(command, input.cli_agent_session));
    let mut record = SessionMemoryRecord {
        id: record_id_for_pane_uuid(input.uuid),
        source: SessionMemorySource::WarpTerminal,
        kind: SessionMemoryKind::Terminal,
        status: SessionMemoryStatus::Live,
        title: input
            .last_command
            .clone()
            .or_else(|| {
                input
                    .cwd
                    .as_ref()
                    .map(|cwd| cwd.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "Terminal".to_string()),
        summary: None,
        cwd: input.cwd,
        project: None,
        native_session_id: None,
        transcript_path: None,
        terminal_pane_uuid: Some(input.uuid.to_vec()),
        app_window_fingerprint: None,
        app_tab_fingerprint: None,
        last_command: input.last_command,
        last_exit_code: None,
        launch_argv: None,
        permission_mode: AgentPermissionMode::Unknown,
        last_seen_at: input.now,
        started_at: None,
        completed_at: None,
        closed_intentionally_at: None,
        app_run_id: input.app_run_id,
        recovery_offered_run_id: None,
        restore_payload: input.restore_payload,
    };

    let (Some(command), Some((source, permission_mode))) = (running_command, agent) else {
        return record;
    };
    let context = input
        .cli_agent_session
        .filter(|session| source_for_cli_agent(session.agent) == Some(source))
        .map(|session| &session.session_context);

    record.source = source;
    record.kind = SessionMemoryKind::AgentChat;
    record.title = context
        .and_then(|context| context.display_title())
        .unwrap_or_else(|| command.clone());
    record.summary = context.and_then(|context| context.summary.clone());
    if let Some(cwd) = context.and_then(|context| context.cwd.clone()) {
        record.cwd = Some(PathBuf::from(cwd));
    }
    record.project = context.and_then(|context| context.project.clone());
    record.native_session_id = context
        .and_then(|context| context.session_id.clone())
        .or_else(|| parse_agent_session_id(&command));
    record.transcript_path = context
        .and_then(|context| context.transcript_path.as_ref())
        .map(PathBuf::from);
    record.launch_argv = Some(command.split_whitespace().map(str::to_owned).collect());
    record.permission_mode = permission_mode;
    record.started_at = input.running_command_started_at;
    record.last_command = Some(command);
    record
}

fn running_agent(
    command: &str,
    cli_agent_session: Option<&CLIAgentSession>,
) -> Option<(SessionMemorySource, AgentPermissionMode)> {
    if let Some(agent_command) = terminal_agent_command(Some(command)) {
        return Some((agent_command.source, agent_command.permission_mode));
    }
    let source = cli_agent_session.and_then(|session| source_for_cli_agent(session.agent))?;
    Some((source, AgentPermissionMode::Unknown))
}
