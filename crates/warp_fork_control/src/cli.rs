use std::io::Read;
use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};

#[derive(Debug, Parser)]
#[command(name = "warp-fork-ctl", about = "Drive this Warp fork over its local fork-control socket")]
pub struct Cli {
    /// Socket path (default: $WARP_FORK_CONTROL_SOCKET or the per-profile default).
    #[arg(long, global = true)]
    pub socket: Option<PathBuf>,
    /// Data profile of the target Warp instance (dev builds, WARP_DATA_PROFILE).
    #[arg(long, global = true, env = "WARP_DATA_PROFILE")]
    pub profile: Option<String>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// API and app version.
    Ping,
    /// All terminal panes.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Focus a pane and bring Warp to the front.
    Focus { pane_id: u64 },
    /// Open a new tab in a directory, optionally running a command.
    Open {
        #[arg(long)]
        cwd: PathBuf,
        #[arg(long)]
        cmd: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        new_window: bool,
        #[arg(long)]
        no_focus: bool,
    },
    /// Set (or --clear) a custom tab title. The id is a tab_id, or a pane_id with --pane.
    Title {
        id: u64,
        title: Option<String>,
        #[arg(long)]
        pane: bool,
        #[arg(long)]
        clear: bool,
    },
    /// Send text to a pane. Use "-" to read the text from stdin.
    Send {
        pane_id: u64,
        text: String,
        #[arg(long)]
        submit: bool,
        #[arg(long)]
        allow_shell: bool,
        #[arg(long)]
        keys: bool,
    },
    /// Find the pane whose process tree contains a pid.
    FindPid { pid: u32 },
}

impl Command {
    pub fn to_request(&self) -> Result<(String, Value)> {
        let request = match self {
            Command::Ping => ("ping", json!({})),
            Command::List { .. } => ("list", json!({})),
            Command::Focus { pane_id } => ("focus", json!({"pane_id": pane_id})),
            Command::Open { cwd, cmd, title, new_window, no_focus } => {
                let cwd = std::path::absolute(cwd)?;
                let mut params = json!({
                    "cwd": cwd.to_string_lossy(),
                    "window": if *new_window { "new" } else { "current" },
                    "focus": !no_focus,
                });
                if let Some(cmd) = cmd {
                    params["command"] = json!(cmd);
                }
                if let Some(title) = title {
                    params["title"] = json!(title);
                }
                ("open_tab", params)
            }
            Command::Title { id, title, pane, clear } => {
                let title = match (title, clear) {
                    (_, true) => Value::Null,
                    (Some(title), false) => json!(title),
                    (None, false) => bail!("pass a title or --clear"),
                };
                let key = if *pane { "pane_id" } else { "tab_id" };
                ("set_title", json!({ key: id, "title": title }))
            }
            Command::Send { pane_id, text, submit, allow_shell, keys } => {
                let text = if text == "-" {
                    let mut buffer = String::new();
                    std::io::stdin().read_to_string(&mut buffer)?;
                    buffer
                } else {
                    text.clone()
                };
                (
                    "send_input",
                    json!({
                        "pane_id": pane_id,
                        "text": text,
                        "submit": submit,
                        "allow_shell": allow_shell,
                        "mode": if *keys { "keys" } else { "paste" },
                    }),
                )
            }
            Command::FindPid { pid } => ("find_by_pid", json!({"pid": pid})),
        };
        Ok((request.0.to_string(), request.1))
    }
}

#[cfg(test)]
#[path = "cli_tests.rs"]
mod tests;
