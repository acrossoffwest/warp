use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};
use warp_core::channel::ChannelState;
use warp_fork_control::input::{
    InputRoute, PaneActivity, encode_pty_text, route_input, submit_delay,
};
use warp_fork_control::pids::find_pane_for_pid;
use warp_fork_control::protocol::{
    API_VERSION, ErrorBody, ErrorCode, InputMode, ListResult, OpenTabParams, OpenTabResult,
    PaneInfo, PingResult, Request, SendInputParams, SendInputResult, SetTitleParams, WindowTarget,
};
use warpui::r#async::Timer;
use warpui::windowing::WindowManager;
use warpui::{AppContext, EntityId, ModelContext, SingletonEntity, ViewHandle, WindowId};

use super::ForkControlHost;
use super::procinfo::ProcessTable;
use crate::pane_group::{NewTerminalOptions, PaneGroup, PaneId, PanesLayout};
use crate::root_view::{NewWorkspaceSource, open_new_with_workspace_source};
use crate::session_management::CommandContext;
use crate::terminal::TerminalView;
#[cfg(feature = "local_tty")]
use crate::terminal::cli_agent_sessions::CLIAgentSessionsModel;
use crate::workspace::{Workspace, WorkspaceRegistry};

pub(super) struct PaneLocation {
    pub(super) window_id: WindowId,
    pub(super) workspace: ViewHandle<Workspace>,
    pub(super) tab_index: usize,
    pub(super) pane_group: ViewHandle<PaneGroup>,
    pub(super) pane_id: PaneId,
    pub(super) terminal: ViewHandle<TerminalView>,
}

pub(super) fn entity_number(id: EntityId) -> u64 {
    id.to_string().parse().unwrap_or_default()
}

pub(super) fn window_number(id: WindowId) -> u64 {
    id.to_string().parse().unwrap_or_default()
}

pub(super) fn not_found(message: impl Into<String>) -> ErrorBody {
    ErrorBody::new(ErrorCode::NotFound, message)
}

pub(super) fn sorted_workspaces(ctx: &AppContext) -> Vec<(WindowId, ViewHandle<Workspace>)> {
    let mut workspaces = WorkspaceRegistry::as_ref(ctx).all_workspaces(ctx);
    workspaces.sort_by_key(|(window_id, _)| window_number(*window_id));
    workspaces
}

pub(super) fn all_terminal_panes(ctx: &AppContext) -> Vec<PaneLocation> {
    let mut panes = Vec::new();
    for (window_id, workspace) in sorted_workspaces(ctx) {
        for (tab_index, pane_group) in workspace.as_ref(ctx).tab_views().enumerate() {
            let group = pane_group.as_ref(ctx);
            for pane_id in group.visible_pane_ids() {
                if let Some(terminal) = group.terminal_view_from_pane_id(pane_id, ctx) {
                    panes.push(PaneLocation {
                        window_id,
                        workspace: workspace.clone(),
                        tab_index,
                        pane_group: pane_group.clone(),
                        pane_id,
                        terminal,
                    });
                }
            }
        }
    }
    panes
}

pub(super) fn find_pane(pane_id: u64, ctx: &AppContext) -> Result<PaneLocation, ErrorBody> {
    all_terminal_panes(ctx)
        .into_iter()
        .find(|location| entity_number(location.terminal.id()) == pane_id)
        .ok_or_else(|| not_found(format!("no terminal pane with pane_id {pane_id}")))
}

fn shell_pid(location: &PaneLocation, ctx: &AppContext) -> Option<u32> {
    location
        .terminal
        .as_ref(ctx)
        .model
        .lock()
        .shell_process_info()
        .map(|shell| shell.pid)
}

fn pane_info(location: &PaneLocation, procs: &ProcessTable, ctx: &AppContext) -> PaneInfo {
    let group = location.pane_group.as_ref(ctx);
    let terminal = location.terminal.as_ref(ctx);
    let (shell_pid, foreground_pgid, is_alt_screen) = {
        let model = terminal.model.lock();
        let shell = model.shell_process_info();
        (
            shell.map(|shell| shell.pid),
            shell
                .and_then(|shell| shell.pty_leader_fd)
                .and_then(super::procinfo::foreground_pgid_of_fd),
            model.is_alt_screen_active(),
        )
    };
    let running_command = match terminal.session_command_context(ctx) {
        CommandContext::RunningCommand { running_command } => Some(running_command),
        _ => None,
    };
    let is_focused = ctx.windows().active_window() == Some(location.window_id)
        && location.workspace.as_ref(ctx).active_tab_index() == location.tab_index
        && group.focused_pane_id(ctx) == location.pane_id;
    PaneInfo {
        window_id: window_number(location.window_id),
        tab_id: entity_number(location.pane_group.id()),
        tab_index: location.tab_index,
        pane_id: entity_number(location.terminal.id()),
        title: group.display_title(ctx),
        custom_title: group.custom_title(ctx),
        cwd: terminal.pwd_if_local(ctx),
        shell_pid,
        foreground_pgid,
        foreground_command: foreground_pgid.and_then(|pgid| procs.name(pgid)),
        running_command,
        is_alt_screen,
        is_focused,
    }
}

fn list(procs: &ProcessTable, ctx: &AppContext) -> ListResult {
    ListResult {
        panes: all_terminal_panes(ctx)
            .iter()
            .map(|location| pane_info(location, procs, ctx))
            .collect(),
    }
}

fn find_by_pid(pid: u32, procs: &ProcessTable, ctx: &AppContext) -> Result<PaneInfo, ErrorBody> {
    let panes = all_terminal_panes(ctx);
    let shells: Vec<(usize, u32)> = panes
        .iter()
        .enumerate()
        .filter_map(|(index, location)| shell_pid(location, ctx).map(|pid| (index, pid)))
        .collect();
    let index = find_pane_for_pid(pid, &shells, |pid| procs.parent(pid))
        .ok_or_else(|| not_found(format!("no pane owns pid {pid}")))?;
    Ok(pane_info(&panes[index], procs, ctx))
}

fn focus(pane_id: u64, ctx: &mut ModelContext<ForkControlHost>) -> Result<(), ErrorBody> {
    let location = find_pane(pane_id, ctx)?;
    location.workspace.update(ctx, |workspace, ctx| {
        workspace.activate_tab(location.tab_index, ctx)
    });
    location.pane_group.update(ctx, |group, ctx| {
        group.focus_pane_by_id(location.pane_id, ctx)
    });
    ctx.windows().show_window_and_focus_app(location.window_id);
    Ok(())
}

fn find_tab(
    tab_id: u64,
    ctx: &AppContext,
) -> Result<(ViewHandle<Workspace>, ViewHandle<PaneGroup>), ErrorBody> {
    sorted_workspaces(ctx)
        .into_iter()
        .find_map(|(_, workspace)| {
            let found = workspace
                .as_ref(ctx)
                .tab_views()
                .find(|group| entity_number(group.id()) == tab_id)
                .cloned();
            found.map(|group| (workspace.clone(), group))
        })
        .ok_or_else(|| not_found(format!("no tab with tab_id {tab_id}")))
}

fn set_title(
    params: SetTitleParams,
    ctx: &mut ModelContext<ForkControlHost>,
) -> Result<(), ErrorBody> {
    let (workspace, pane_group) = match (params.tab_id, params.pane_id) {
        (Some(tab_id), _) => find_tab(tab_id, ctx)?,
        (None, Some(pane_id)) => {
            let location = find_pane(pane_id, ctx)?;
            (location.workspace, location.pane_group)
        }
        (None, None) => {
            return Err(ErrorBody::new(
                ErrorCode::BadRequest,
                "tab_id or pane_id is required",
            ));
        }
    };
    let title = params
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty());
    pane_group.update(ctx, |group, ctx| match title {
        Some(title) => group.set_title(title, ctx),
        None => group.clear_title(ctx),
    });
    // PaneGroup::set_title focuses the renamed tab's pane; hand focus back to the active tab.
    workspace.update(ctx, |workspace, ctx| {
        workspace.focus_active_tab(ctx);
        ctx.notify();
    });
    Ok(())
}

fn target_window(ctx: &AppContext) -> Option<(WindowId, ViewHandle<Workspace>)> {
    let windows = WindowManager::as_ref(ctx);
    let preferred = windows
        .active_window()
        .or_else(|| windows.frontmost_window_id());
    let registry = WorkspaceRegistry::as_ref(ctx);
    preferred
        .and_then(|window_id| registry.get(window_id, ctx).map(|ws| (window_id, ws)))
        .or_else(|| sorted_workspaces(ctx).into_iter().next())
}

fn open_tab(
    params: OpenTabParams,
    ctx: &mut ModelContext<ForkControlHost>,
) -> Result<OpenTabResult, ErrorBody> {
    let cwd = PathBuf::from(&params.cwd);
    if !cwd.is_absolute() || !cwd.is_dir() {
        return Err(ErrorBody::new(
            ErrorCode::BadRequest,
            format!("cwd must be an existing absolute directory: {}", params.cwd),
        ));
    }
    let title = params
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_owned);
    let options = NewTerminalOptions {
        initial_directory: Some(cwd),
        hide_homepage: true,
        ..Default::default()
    };

    let (window_id, pane_group) = match params.window {
        WindowTarget::New => {
            let (window_id, _root) = open_new_with_workspace_source(
                NewWorkspaceSource::Session {
                    options: Box::new(options),
                    initial_team_uid: None,
                },
                ctx,
            );
            let workspace = WorkspaceRegistry::as_ref(ctx)
                .get(window_id, ctx)
                .ok_or_else(|| {
                    ErrorBody::new(ErrorCode::Internal, "new window has no workspace")
                })?;
            let pane_group = workspace.as_ref(ctx).active_tab_pane_group().clone();
            if let Some(title) = &title {
                pane_group.update(ctx, |group, ctx| group.set_title(title, ctx));
            }
            (window_id, pane_group)
        }
        WindowTarget::Current => {
            let (window_id, workspace) = target_window(ctx).ok_or_else(|| {
                ErrorBody::new(
                    ErrorCode::Unavailable,
                    "no Warp window is open; use window \"new\"",
                )
            })?;
            let previous_tab = workspace.as_ref(ctx).active_tab_pane_group().id();
            let pane_group = workspace.update(ctx, |workspace, ctx| {
                workspace.add_tab_with_pane_layout(
                    PanesLayout::SingleTerminal(Box::new(options)),
                    Arc::new(HashMap::new()),
                    title.clone(),
                    ctx,
                );
                let pane_group = workspace.active_tab_pane_group().clone();
                if !params.focus {
                    workspace.activate_tab_by_pane_group_id(previous_tab, ctx);
                }
                pane_group
            });
            (window_id, pane_group)
        }
    };

    let terminal = pane_group
        .as_ref(ctx)
        .active_session_view(ctx)
        .ok_or_else(|| ErrorBody::new(ErrorCode::Internal, "new tab has no terminal"))?;
    if let Some(command) = params.command.as_deref().filter(|c| !c.trim().is_empty()) {
        terminal.update(ctx, |terminal, ctx| {
            terminal.execute_command_or_set_pending(command, ctx)
        });
    }
    if params.focus {
        ctx.windows().show_window_and_focus_app(window_id);
    }
    let shell_pid = terminal
        .as_ref(ctx)
        .model
        .lock()
        .shell_process_info()
        .map(|shell| shell.pid);
    Ok(OpenTabResult {
        window_id: window_number(window_id),
        tab_id: entity_number(pane_group.id()),
        pane_id: entity_number(terminal.id()),
        shell_pid,
    })
}

fn send_input(
    params: SendInputParams,
    ctx: &mut ModelContext<ForkControlHost>,
) -> Result<SendInputResult, ErrorBody> {
    let location = find_pane(params.pane_id, ctx)?;
    let (activity, is_alt_screen, bracketed) = {
        let terminal = location.terminal.as_ref(ctx);
        let activity = match terminal.session_command_context(ctx) {
            CommandContext::RunningCommand { .. } => PaneActivity::RunningCommand,
            CommandContext::RunningAIBlock { .. } => PaneActivity::WarpAgentRunning,
            _ => PaneActivity::AtPrompt,
        };
        let mut model = terminal.model.lock();
        (
            activity,
            model.is_alt_screen_active(),
            model.needs_bracketed_paste(),
        )
    };
    let route = route_input(activity, is_alt_screen, params.allow_shell)?;

    match route {
        InputRoute::Pty => {
            let to_agent = params.submit
                && !params.text.is_empty()
                && submit_to_cli_agent(&location, &params.text, ctx)?;
            if !to_agent {
                write_to_pty(&location, &params, bracketed, ctx)?;
            }
        }
        InputRoute::InputEditor => {
            location.terminal.update(ctx, |terminal, ctx| {
                if params.submit {
                    terminal.execute_command_or_set_pending(&params.text, ctx);
                } else {
                    terminal
                        .input()
                        .update(ctx, |input, ctx| input.system_insert(&params.text, ctx));
                }
            });
        }
    }
    Ok(SendInputResult {
        delivered_to: route,
    })
}

fn pane_busy() -> ErrorBody {
    ErrorBody::new(
        ErrorCode::PaneBusy,
        "a Warp agent controls this pane's input",
    )
}

/// Returns false when the pane has no CLI agent session Warp recognizes.
#[cfg(feature = "local_tty")]
fn submit_to_cli_agent(
    location: &PaneLocation,
    text: &str,
    ctx: &mut ModelContext<ForkControlHost>,
) -> Result<bool, ErrorBody> {
    if CLIAgentSessionsModel::as_ref(ctx)
        .session(location.terminal.id())
        .is_none()
    {
        return Ok(false);
    }
    let agent_in_control = location
        .terminal
        .as_ref(ctx)
        .model
        .lock()
        .block_list()
        .active_block()
        .is_agent_in_control();
    if agent_in_control {
        return Err(pane_busy());
    }
    let text = text.to_owned();
    location.terminal.update(ctx, |terminal, ctx| {
        terminal.submit_text_to_cli_agent_pty(text, ctx)
    });
    Ok(true)
}

#[cfg(not(feature = "local_tty"))]
fn submit_to_cli_agent(
    _location: &PaneLocation,
    _text: &str,
    _ctx: &mut ModelContext<ForkControlHost>,
) -> Result<bool, ErrorBody> {
    Ok(false)
}

fn write_to_pty(
    location: &PaneLocation,
    params: &SendInputParams,
    bracketed: bool,
    ctx: &mut ModelContext<ForkControlHost>,
) -> Result<(), ErrorBody> {
    let bytes = encode_pty_text(&params.text, params.mode, bracketed);
    let delay = submit_delay(params.mode == InputMode::Paste && bracketed && !bytes.is_empty());
    let submit = params.submit;
    let delivered = location.terminal.update(ctx, |terminal, ctx| {
        if bytes.is_empty() {
            return !submit || terminal.write_user_bytes_to_pty(b"\r".to_vec(), ctx);
        }
        if !terminal.write_user_bytes_to_pty(bytes, ctx) {
            return false;
        }
        if submit {
            ctx.spawn(Timer::after(delay), |terminal, _, ctx| {
                terminal.write_user_bytes_to_pty(b"\r".to_vec(), ctx);
            });
        }
        true
    });
    if delivered { Ok(()) } else { Err(pane_busy()) }
}

pub(super) fn handle(
    request: Request,
    procs: Option<ProcessTable>,
    ctx: &mut ModelContext<ForkControlHost>,
) -> Result<Value, ErrorBody> {
    match request {
        Request::Ping => to_value(ping()),
        Request::List => {
            let procs = procs.unwrap_or_else(ProcessTable::snapshot);
            to_value(list(&procs, ctx))
        }
        Request::FindByPid(params) => {
            let procs = procs.unwrap_or_else(ProcessTable::snapshot);
            find_by_pid(params.pid, &procs, ctx).and_then(to_value)
        }
        Request::Focus(params) => focus(params.pane_id, ctx).map(|()| json!({})),
        Request::SetTitle(params) => set_title(params, ctx).map(|()| json!({})),
        Request::OpenTab(params) => open_tab(params, ctx).and_then(to_value),
        Request::SendInput(params) => send_input(params, ctx).and_then(to_value),
    }
}

fn ping() -> PingResult {
    PingResult {
        api_version: API_VERSION,
        app_version: ChannelState::app_version().map(str::to_owned),
        channel: format!("{:?}", ChannelState::channel()).to_lowercase(),
        pid: std::process::id(),
    }
}

fn to_value<T: serde::Serialize>(value: T) -> Result<Value, ErrorBody> {
    serde_json::to_value(value)
        .map_err(|e| ErrorBody::new(ErrorCode::Internal, format!("serialize: {e}")))
}
