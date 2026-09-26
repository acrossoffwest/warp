use std::collections::HashMap;
use std::path::{Path, PathBuf};

use warpui::{AppContext, SingletonEntity};

use crate::app_state::AppState;
use crate::session_memory::model::SessionMemoryModel;
use crate::session_memory::restore::{
    AgentSessionFile, StartupRestoreTarget, plan_startup_restore, resolve_missing_session_ids,
};
use crate::session_memory::types::SessionMemorySource;
use crate::settings::AISettings;
use crate::terminal::CLIAgent;
use crate::workspace::WorkspaceRegistry;
use crate::workspace::agent_session_reader;

pub(crate) fn layout_was_restored(app_state: Option<&AppState>, restore_session: bool) -> bool {
    restore_session && app_state.is_some_and(|state| !state.windows.is_empty())
}

pub(crate) fn restore_open_agent_sessions(layout_restored: bool, ctx: &mut AppContext) {
    if !ctx.has_singleton_model::<SessionMemoryModel>() {
        return;
    }
    let session_memory = SessionMemoryModel::as_ref(ctx);
    let mut candidates = session_memory.startup_restore_candidates();
    if candidates.is_empty() {
        return;
    }
    let workspaces = WorkspaceRegistry::as_ref(ctx).all_workspaces(ctx);
    if workspaces.is_empty() || workspaces.len() < ctx.window_ids().count() {
        log::info!("Session memory startup restore: skipped, not every window has a workspace");
        return;
    }
    let claimed = session_memory.previous_run_native_session_ids();
    let mut session_files_by_cwd = HashMap::new();
    resolve_missing_session_ids(&mut candidates, claimed, |source, cwd| {
        let Some(agent) = session_agent(source) else {
            return Vec::new();
        };
        session_files_by_cwd
            .entry((agent, agent_lookup_cwd(cwd)))
            .or_insert_with_key(|(agent, cwd)| agent_session_files(*agent, cwd))
            .clone()
    });

    let restored_panes = workspaces
        .into_iter()
        .flat_map(|(window_id, workspace)| {
            workspace
                .as_ref(ctx)
                .terminal_pane_session_uuids(ctx)
                .into_iter()
                .map(move |uuid| (window_id, uuid))
        })
        .collect::<Vec<_>>();
    let run_resume_commands =
        *AISettings::as_ref(ctx).session_memory_auto_restore_interrupted_sessions;
    let first_window = ctx
        .windows()
        .active_window()
        .or_else(|| ctx.windows().ordered_window_ids().first().copied());
    let offered_ids = candidates
        .iter()
        .map(|record| record.id.clone())
        .collect::<Vec<_>>();

    for (record_id, target) in plan_startup_restore(&candidates, &restored_panes, layout_restored) {
        let (window_id, terminal_pane_uuid, plan) = match target {
            StartupRestoreTarget::ExistingPane {
                window,
                terminal_pane_uuid,
                plan,
            } => (Some(window), Some(terminal_pane_uuid), plan),
            StartupRestoreTarget::NewTab { plan } => (first_window, None, plan),
            StartupRestoreTarget::Skip(reason) => {
                log::info!("Session memory startup restore: skipping {record_id}: {reason:?}");
                continue;
            }
        };
        let Some(workspace) =
            window_id.and_then(|window_id| WorkspaceRegistry::as_ref(ctx).get(window_id, ctx))
        else {
            log::warn!("Session memory startup restore: no window for {record_id}");
            continue;
        };
        let terminal_view = workspace.update(ctx, |workspace, ctx| match &terminal_pane_uuid {
            Some(uuid) => workspace.terminal_view_for_session_uuid(uuid, ctx),
            None => workspace.open_terminal_for_restore_plan(&plan, false, ctx),
        });
        let (Some(terminal_view), Some(command)) =
            (terminal_view, plan.command().map(str::to_owned))
        else {
            log::warn!("Session memory startup restore: no terminal for {record_id}");
            continue;
        };

        log::info!("Session memory startup restore: {record_id} -> {command}");
        terminal_view.update(ctx, |terminal_view, ctx| {
            if run_resume_commands {
                terminal_view.execute_command_when_bootstrapped_or_defer(&command, ctx);
            } else {
                terminal_view.insert_command_when_bootstrapped_or_defer(&command, ctx);
            }
        });
    }

    SessionMemoryModel::handle(ctx).update(ctx, |model, ctx| {
        model.mark_startup_recovery_offered_and_notify(&offered_ids, ctx);
    });
}

fn session_agent(source: SessionMemorySource) -> Option<CLIAgent> {
    match source {
        SessionMemorySource::ClaudeCode => Some(CLIAgent::Claude),
        SessionMemorySource::Codex => Some(CLIAgent::Codex),
        SessionMemorySource::WarpTerminal => None,
    }
}

fn agent_session_files(agent: CLIAgent, cwd: &Path) -> Vec<AgentSessionFile> {
    agent_session_reader::read_all_sessions(agent, cwd)
        .into_iter()
        .map(|entry| AgentSessionFile {
            session_id: entry.session_id,
            created_at: entry.created_at,
        })
        .collect()
}

fn agent_lookup_cwd(cwd: &Path) -> PathBuf {
    let expanded = PathBuf::from(shellexpand::tilde(&cwd.to_string_lossy()).into_owned());
    dunce::canonicalize(&expanded).unwrap_or_else(|_| expanded.components().collect())
}

#[cfg(test)]
#[path = "session_memory_startup_tests.rs"]
mod tests;
