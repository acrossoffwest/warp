use std::path::{Path, PathBuf};

use super::types::{
    user_command, AgentPermissionMode, SessionMemoryKind, SessionMemoryRecord, SessionMemorySource,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupRestoreAction {
    ExistingPane {
        terminal_pane_uuid: Vec<u8>,
        plan: RestorePlan,
    },
    AlreadyRestoredPane {
        terminal_pane_uuid: Vec<u8>,
    },
    NewPane {
        plan: RestorePlan,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoredTerminalPane {
    pub uuid: Vec<u8>,
    pub cwd: Option<PathBuf>,
}

pub fn startup_restore_action_for_record(
    record: &SessionMemoryRecord,
    restored_terminal_panes: &[RestoredTerminalPane],
    auto_run_restored_commands: bool,
    layout_restore_enabled: bool,
) -> Result<StartupRestoreAction, RestoreError> {
    let plan = restore_plan_for_record(record, auto_run_restored_commands)?;

    if let Some(terminal_pane_uuid) = &record.terminal_pane_uuid {
        if restored_terminal_panes
            .iter()
            .any(|restored| restored.uuid == *terminal_pane_uuid)
        {
            return if should_apply_restore_plan_to_existing_pane(&plan) {
                Ok(StartupRestoreAction::ExistingPane {
                    terminal_pane_uuid: terminal_pane_uuid.clone(),
                    plan,
                })
            } else {
                Ok(StartupRestoreAction::AlreadyRestoredPane {
                    terminal_pane_uuid: terminal_pane_uuid.clone(),
                })
            };
        }

        if layout_restore_enabled {
            if let Some(restored) = restored_terminal_panes.iter().find(|restored| {
                record.cwd.is_some() && restored.cwd.as_ref() == record.cwd.as_ref()
            }) {
                return if should_apply_restore_plan_to_existing_pane(&plan) {
                    Ok(StartupRestoreAction::ExistingPane {
                        terminal_pane_uuid: restored.uuid.clone(),
                        plan,
                    })
                } else {
                    Ok(StartupRestoreAction::AlreadyRestoredPane {
                        terminal_pane_uuid: restored.uuid.clone(),
                    })
                };
            }
        }
    }

    Ok(StartupRestoreAction::NewPane { plan })
}

fn should_apply_restore_plan_to_existing_pane(plan: &RestorePlan) -> bool {
    match plan {
        RestorePlan::Agent { .. } => true,
        RestorePlan::Terminal { auto_run, .. } => *auto_run,
    }
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

fn is_env_assignment(token: &str) -> bool {
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
