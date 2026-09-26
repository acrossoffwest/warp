use std::collections::{HashMap, HashSet};
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
    pub modified_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunBounds {
    pub previous_run_started_at: Option<i64>,
    pub current_run_started_at: i64,
}

pub type AgentFolder = (SessionMemorySource, PathBuf);

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

pub fn session_file_mtime_floor<'a>(
    records: impl IntoIterator<Item = &'a SessionMemoryRecord>,
    bounds: RunBounds,
) -> Option<i64> {
    records
        .into_iter()
        .filter_map(|record| record.started_at)
        .chain(bounds.previous_run_started_at)
        .min()
        .map(|earliest| earliest - SESSION_FILE_START_TOLERANCE_SECONDS)
}

/// Fills missing native session ids of `candidates` from agent session files and
/// returns the folders where an id-less resume (`--continue` / `--last`) would
/// open an unclaimed conversation from the previous run.
pub fn resolve_missing_session_ids(
    candidates: &mut [SessionMemoryRecord],
    ended: &[SessionMemoryRecord],
    mut claimed: HashSet<String>,
    bounds: RunBounds,
    folder_key: impl Fn(&Path) -> PathBuf,
    mut session_files: impl FnMut(SessionMemorySource, &Path) -> Vec<AgentSessionFile>,
) -> HashSet<AgentFolder> {
    claimed.extend(
        candidates
            .iter()
            .chain(ended)
            .filter_map(|record| valid_native_session_id(record).map(str::to_owned)),
    );
    let mut files_by_folder: HashMap<AgentFolder, Vec<AgentSessionFile>> = HashMap::new();
    let mut files_for = |source: SessionMemorySource, cwd: &Path| {
        let key = (source, folder_key(cwd));
        files_by_folder
            .entry(key)
            .or_insert_with_key(|(source, folder)| {
                let mut files = session_files(*source, folder);
                files.retain(|file| is_valid_session_id(&file.session_id));
                files
            })
            .clone()
    };

    let mut claimants = candidates
        .iter()
        .enumerate()
        .map(|(index, record)| (Some(index), record))
        .chain(ended.iter().map(|record| (None, record)))
        .filter(|(_, record)| record.is_agent() && valid_native_session_id(record).is_none())
        .filter_map(|(index, record)| {
            let started_at = record.started_at?;
            let cwd = record.cwd.clone()?;
            let observed_until = record
                .completed_at
                .or(record.closed_intentionally_at)
                .unwrap_or(bounds.current_run_started_at);
            Some((index, record.source, cwd, started_at, observed_until))
        })
        .collect::<Vec<_>>();
    claimants.sort_by_key(|(_, _, _, started_at, observed_until)| {
        (observed_until - started_at, *started_at)
    });

    for (index, source, cwd, started_at, observed_until) in claimants {
        let files = files_for(source, &cwd);
        let Some(session_id) = match_session_file(started_at, observed_until, &files, &claimed)
        else {
            continue;
        };
        claimed.insert(session_id.clone());
        if let Some(index) = index {
            candidates[index].native_session_id = Some(session_id);
        }
    }

    let mut continue_folders = HashSet::new();
    for record in candidates.iter() {
        let Some(cwd) = record
            .cwd
            .as_deref()
            .filter(|_| valid_native_session_id(record).is_none())
        else {
            continue;
        };
        let files = files_for(record.source, cwd);
        let newest = files
            .iter()
            .max_by_key(|file| (file.modified_at, &file.session_id));
        let continue_allowed = newest.is_some_and(|file| {
            !claimed.contains(&file.session_id)
                && file.created_at <= bounds.current_run_started_at
                && bounds
                    .previous_run_started_at
                    .is_none_or(|started_at| file.modified_at >= started_at)
        });
        if continue_allowed {
            continue_folders.insert((record.source, folder_key(cwd)));
        }
    }
    continue_folders
}

fn valid_native_session_id(record: &SessionMemoryRecord) -> Option<&str> {
    record
        .native_session_id
        .as_deref()
        .filter(|session_id| is_valid_session_id(session_id))
}

pub fn startup_agent_restore_plan(
    record: &SessionMemoryRecord,
    continue_allowed: bool,
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

    let dangerous = record.permission_mode == AgentPermissionMode::Dangerous;
    let command = match valid_native_session_id(record) {
        Some(session_id) => {
            agent.resume_command_preserving_permission(session_id, record.permission_mode)
        }
        None if continue_allowed => match (dangerous, agent.dangerous_flag()) {
            (true, Some(flag)) => format!("{continue_command} {flag}"),
            _ => continue_command.to_owned(),
        },
        None => agent.launch_command(dangerous),
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
    continue_folders: &HashSet<AgentFolder>,
    folder_key: impl Fn(&Path) -> PathBuf,
) -> Vec<(String, StartupRestoreTarget<W>)> {
    let mut ordered = candidates.iter().collect::<Vec<_>>();
    ordered.sort_by(|a, b| b.last_seen_at.cmp(&a.last_seen_at));

    let mut idless_folders: Vec<(SessionMemorySource, Option<PathBuf>)> = Vec::new();
    ordered
        .into_iter()
        .map(|record| {
            let folder = record.cwd.as_deref().map(&folder_key);
            let continue_allowed = folder
                .as_ref()
                .is_some_and(|folder| continue_folders.contains(&(record.source, folder.clone())));
            let key = (record.source, folder);
            let target = startup_restore_target(
                record,
                restored_panes,
                layout_restore_enabled,
                continue_allowed,
            );
            let target = match target {
                StartupRestoreTarget::Skip(_) => target,
                _ if valid_native_session_id(record).is_some() => target,
                _ if idless_folders.contains(&key) => {
                    StartupRestoreTarget::Skip(StartupRestoreSkip::DuplicateContinue)
                }
                _ => {
                    idless_folders.push(key);
                    target
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
    continue_allowed: bool,
) -> StartupRestoreTarget<W> {
    let plan = match startup_agent_restore_plan(record, continue_allowed) {
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
