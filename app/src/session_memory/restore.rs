use std::path::{Path, PathBuf};

use super::types::{AgentPermissionMode, SessionMemoryRecord, SessionMemorySource};
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

pub fn terminal_restore_plan(
    record: &SessionMemoryRecord,
    auto_run_restored_commands: bool,
) -> RestorePlan {
    let command_for_composer = record.last_command.clone();
    let auto_run = auto_run_restored_commands && command_for_composer.is_some();

    RestorePlan::Terminal {
        cwd: record.cwd.clone(),
        command_for_composer,
        auto_run,
    }
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
