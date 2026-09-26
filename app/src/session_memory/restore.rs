use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::types::{
    AgentPermissionMode, SessionMemoryKind, SessionMemoryRecord, SessionMemorySource,
    is_valid_session_id, user_command,
};
use crate::terminal::CLIAgent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestorePlan {
    Terminal {
        cwd: Option<PathBuf>,
        command_for_composer: Option<String>,
        auto_run: bool,
    },
    Agent {
        agent: CLIAgent,
        cwd: PathBuf,
        command: String,
        permission_mode: AgentPermissionMode,
    },
}

impl RestorePlan {
    pub fn cwd(&self) -> Option<&Path> {
        match self {
            RestorePlan::Terminal { cwd, .. } => cwd.as_deref(),
            RestorePlan::Agent { cwd, .. } => Some(cwd.as_path()),
        }
    }

    pub fn command(&self) -> Option<&str> {
        match self {
            RestorePlan::Terminal {
                command_for_composer,
                ..
            } => command_for_composer.as_deref(),
            RestorePlan::Agent { command, .. } => Some(command.as_str()),
        }
    }

    pub fn command_for_composer(&self) -> Option<&str> {
        match self {
            RestorePlan::Terminal {
                command_for_composer,
                ..
            } => command_for_composer.as_deref(),
            RestorePlan::Agent { .. } => None,
        }
    }

    pub fn auto_run(&self) -> Option<bool> {
        match self {
            RestorePlan::Terminal { auto_run, .. } => Some(*auto_run),
            RestorePlan::Agent { .. } => None,
        }
    }

    pub fn permission_mode(&self) -> Option<AgentPermissionMode> {
        match self {
            RestorePlan::Terminal { .. } => None,
            RestorePlan::Agent {
                permission_mode, ..
            } => Some(*permission_mode),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreError {
    MissingWorkingDirectory(PathBuf),
    MissingSessionId,
    UnsupportedSource,
}

pub fn restore_plan_for_record(
    record: &SessionMemoryRecord,
    auto_run_restored_commands: bool,
) -> Result<RestorePlan, RestoreError> {
    if matches!(
        record.source,
        SessionMemorySource::ClaudeCode | SessionMemorySource::Codex
    ) && record.native_session_id.is_some()
    {
        return agent_restore_plan(record);
    }

    match record.kind {
        SessionMemoryKind::Terminal => {
            Ok(terminal_restore_plan(record, auto_run_restored_commands))
        }
        SessionMemoryKind::AgentChat => agent_restore_plan(record),
    }
}

pub fn terminal_restore_plan(
    record: &SessionMemoryRecord,
    auto_run_restored_commands: bool,
) -> RestorePlan {
    let command_for_composer = user_command(record.last_command.as_deref());
    let auto_run = command_for_composer
        .as_deref()
        .map(|command| auto_run_restored_commands || is_safe_tmux_restore_command(command))
        .unwrap_or(false);

    RestorePlan::Terminal {
        cwd: record.cwd.clone(),
        command_for_composer,
        auto_run,
    }
}

fn is_safe_tmux_restore_command(command: &str) -> bool {
    let mut tokens = command.split_whitespace();
    let Some(command_token) = tokens.find(|token| !is_env_assignment(token)) else {
        return false;
    };
    if command_token != "tmux" {
        return false;
    }

    match tokens.next() {
        None => true,
        Some("a" | "attach" | "attach-session") => true,
        Some("new" | "new-session") => tokens.any(|token| token == "-A" || token.contains('A')),
        _ => false,
    }
}

pub(super) fn is_env_assignment(token: &str) -> bool {
    token.split_once('=').map(|(name, _)| {
        !name.is_empty()
            && name
                .chars()
                .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
    }) == Some(true)
}

pub fn agent_restore_plan(record: &SessionMemoryRecord) -> Result<RestorePlan, RestoreError> {
    let agent = match record.source {
        SessionMemorySource::ClaudeCode => CLIAgent::Claude,
        SessionMemorySource::Codex => CLIAgent::Codex,
        SessionMemorySource::WarpTerminal => return Err(RestoreError::UnsupportedSource),
    };

    let cwd = record
        .cwd
        .clone()
        .ok_or_else(|| RestoreError::MissingWorkingDirectory(PathBuf::new()))?;
    if !cwd.exists() {
        return Err(RestoreError::MissingWorkingDirectory(cwd));
    }

    let session_id = record
        .native_session_id
        .as_deref()
        .ok_or(RestoreError::MissingSessionId)?;
    let command = agent.resume_command_preserving_permission(session_id, record.permission_mode);

    Ok(RestorePlan::Agent {
        agent,
        cwd,
        command,
        permission_mode: record.permission_mode,
    })
}

pub const SESSION_FILE_START_TOLERANCE_SECONDS: i64 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSessionFile {
    pub session_id: String,
    pub created_at: i64,
}

pub fn match_session_file(
    started_at: i64,
    observed_until: i64,
    files: &[AgentSessionFile],
    claimed: &HashSet<String>,
) -> Option<String> {
    files
        .iter()
        .filter(|file| {
            file.created_at + SESSION_FILE_START_TOLERANCE_SECONDS >= started_at
                && file.created_at <= observed_until
                && !claimed.contains(&file.session_id)
        })
        .min_by_key(|file| (file.created_at, &file.session_id))
        .map(|file| file.session_id.clone())
}

pub fn resolve_missing_session_ids(
    candidates: &mut [SessionMemoryRecord],
    mut claimed: HashSet<String>,
    mut session_files: impl FnMut(SessionMemorySource, &Path) -> Vec<AgentSessionFile>,
) {
    claimed.extend(
        candidates
            .iter()
            .filter_map(|candidate| valid_native_session_id(candidate).map(str::to_owned)),
    );
    let mut order = (0..candidates.len()).collect::<Vec<_>>();
    order.sort_by_key(|&index| candidates[index].started_at);

    for index in order {
        let candidate = &candidates[index];
        if valid_native_session_id(candidate).is_some() {
            continue;
        }
        let (Some(started_at), Some(cwd)) = (candidate.started_at, candidate.cwd.clone()) else {
            continue;
        };
        let mut files = session_files(candidate.source, &cwd);
        files.retain(|file| is_valid_session_id(&file.session_id));
        let observed_until = candidate.completed_at.unwrap_or(candidate.last_seen_at);
        if let Some(session_id) = match_session_file(started_at, observed_until, &files, &claimed) {
            claimed.insert(session_id.clone());
            candidates[index].native_session_id = Some(session_id);
        }
    }
}

fn valid_native_session_id(record: &SessionMemoryRecord) -> Option<&str> {
    record
        .native_session_id
        .as_deref()
        .filter(|session_id| is_valid_session_id(session_id))
}

pub fn startup_agent_restore_plan(
    record: &SessionMemoryRecord,
) -> Result<RestorePlan, RestoreError> {
    let (agent, continue_command) = match record.source {
        SessionMemorySource::ClaudeCode => (CLIAgent::Claude, "claude --continue"),
        SessionMemorySource::Codex => (CLIAgent::Codex, "codex resume --last"),
        SessionMemorySource::WarpTerminal => return Err(RestoreError::UnsupportedSource),
    };
    let cwd = record
        .cwd
        .clone()
        .ok_or_else(|| RestoreError::MissingWorkingDirectory(PathBuf::new()))?;
    if !cwd.exists() {
        return Err(RestoreError::MissingWorkingDirectory(cwd));
    }

    let command = match valid_native_session_id(record) {
        Some(session_id) => {
            agent.resume_command_preserving_permission(session_id, record.permission_mode)
        }
        None => match (record.permission_mode, agent.dangerous_flag()) {
            (AgentPermissionMode::Dangerous, Some(flag)) => format!("{continue_command} {flag}"),
            _ => continue_command.to_owned(),
        },
    };

    Ok(RestorePlan::Agent {
        agent,
        cwd,
        command,
        permission_mode: record.permission_mode,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupRestoreSkip {
    PaneGone,
    DuplicateContinue,
    Invalid(RestoreError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupRestoreTarget<W> {
    ExistingPane {
        window: W,
        terminal_pane_uuid: Vec<u8>,
        plan: RestorePlan,
    },
    NewTab {
        plan: RestorePlan,
    },
    Skip(StartupRestoreSkip),
}

pub fn plan_startup_restore<W: Clone>(
    candidates: &[SessionMemoryRecord],
    restored_panes: &[(W, Vec<u8>)],
    layout_restore_enabled: bool,
) -> Vec<(String, StartupRestoreTarget<W>)> {
    let mut ordered = candidates.iter().collect::<Vec<_>>();
    ordered.sort_by(|a, b| b.last_seen_at.cmp(&a.last_seen_at));

    let mut continued: Vec<(SessionMemorySource, Option<PathBuf>)> = Vec::new();
    ordered
        .into_iter()
        .map(|record| {
            let target = startup_restore_target(record, restored_panes, layout_restore_enabled);
            let target = match target {
                StartupRestoreTarget::Skip(_) => target,
                _ if valid_native_session_id(record).is_some() => target,
                _ => {
                    let key = (record.source, record.cwd.clone());
                    if continued.contains(&key) {
                        StartupRestoreTarget::Skip(StartupRestoreSkip::DuplicateContinue)
                    } else {
                        continued.push(key);
                        target
                    }
                }
            };
            (record.id.clone(), target)
        })
        .collect()
}

fn startup_restore_target<W: Clone>(
    record: &SessionMemoryRecord,
    restored_panes: &[(W, Vec<u8>)],
    layout_restore_enabled: bool,
) -> StartupRestoreTarget<W> {
    let plan = match startup_agent_restore_plan(record) {
        Ok(plan) => plan,
        Err(err) => return StartupRestoreTarget::Skip(StartupRestoreSkip::Invalid(err)),
    };
    let restored = record.terminal_pane_uuid.as_ref().and_then(|uuid| {
        restored_panes
            .iter()
            .find(|(_, restored_uuid)| restored_uuid == uuid)
    });
    match restored {
        Some((window, terminal_pane_uuid)) => StartupRestoreTarget::ExistingPane {
            window: window.clone(),
            terminal_pane_uuid: terminal_pane_uuid.clone(),
            plan,
        },
        None if layout_restore_enabled => StartupRestoreTarget::Skip(StartupRestoreSkip::PaneGone),
        None => StartupRestoreTarget::NewTab { plan },
    }
}
