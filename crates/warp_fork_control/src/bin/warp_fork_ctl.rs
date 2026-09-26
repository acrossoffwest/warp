#[cfg(not(unix))]
fn main() {
    eprintln!("warp-fork-ctl is only supported on macOS and Linux");
    std::process::exit(2);
}

#[cfg(unix)]
fn main() {
    if let Err(error) = run() {
        eprintln!("warp-fork-ctl: {error:#}");
        std::process::exit(2);
    }
}

#[cfg(unix)]
fn run() -> anyhow::Result<()> {
    use anyhow::Context;
    use clap::Parser;
    use serde_json::Value;
    use warp_fork_control::cli::{Cli, Command};
    use warp_fork_control::protocol::ListResult;
    use warp_fork_control::{client, paths};

    let cli = Cli::parse();
    let socket = match cli.socket.clone() {
        Some(socket) => socket,
        None => paths::default_socket_path(cli.profile.as_deref())
            .context("could not determine the socket path; pass --socket")?,
    };
    let (method, params) = cli.command.to_request()?;
    let response = client::call(&socket, &method, params)?;

    if response["ok"] != Value::Bool(true) {
        eprintln!(
            "{}: {}",
            response["error"]["code"].as_str().unwrap_or("error"),
            response["error"]["message"].as_str().unwrap_or("")
        );
        std::process::exit(1);
    }

    match cli.command {
        Command::List { json: false } => {
            let list: ListResult = serde_json::from_value(response["result"].clone())?;
            print_table(&list);
        }
        _ => println!("{}", serde_json::to_string_pretty(&response["result"])?),
    }
    Ok(())
}

#[cfg(unix)]
fn print_table(list: &warp_fork_control::protocol::ListResult) {
    println!(
        "{:>7} {:>7} {:>4} {:>7} {:>7} {:>3} {:>3} {:<24} {:<18} CWD",
        "PANE", "TAB", "WIN", "SHELL", "FG", "ALT", "FOC", "TITLE", "COMMAND"
    );
    let dash = || "-".to_string();
    for pane in &list.panes {
        println!(
            "{:>7} {:>7} {:>4} {:>7} {:>7} {:>3} {:>3} {:<24.24} {:<18.18} {}",
            pane.pane_id,
            pane.tab_id,
            pane.window_id,
            pane.shell_pid.map_or_else(dash, |pid| pid.to_string()),
            pane.foreground_pgid
                .map_or_else(dash, |pid| pid.to_string()),
            if pane.is_alt_screen { "yes" } else { "" },
            if pane.is_focused { "*" } else { "" },
            pane.title,
            pane.foreground_command.clone().unwrap_or_else(dash),
            pane.cwd.clone().unwrap_or_else(dash),
        );
    }
}
