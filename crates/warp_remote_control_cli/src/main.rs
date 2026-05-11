mod protocol;

use std::io::{Read, Write};
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use interprocess::local_socket::LocalSocketStream;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use protocol::{RemoteControlRequest, RemoteControlResponse, SplitDirection, SERVICE_ID};

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(name = "warp-remote-control", about = "External control for a running Warp instance")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Send a Ping to the running Warp and print 'pong' on success.
    Ping,
    /// Split the focused Warp pane and run a shell command in the new pane.
    Split {
        /// Command to execute in the new pane (raw shell input).
        #[arg(long)]
        command: String,
        /// Direction to split.
        #[arg(long, value_enum)]
        direction: DirectionArg,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum DirectionArg {
    Right,
    Down,
    Left,
    Up,
}

impl From<DirectionArg> for SplitDirection {
    fn from(d: DirectionArg) -> Self {
        match d {
            DirectionArg::Right => SplitDirection::Right,
            DirectionArg::Down => SplitDirection::Down,
            DirectionArg::Left => SplitDirection::Left,
            DirectionArg::Up => SplitDirection::Up,
        }
    }
}

// ---------------------------------------------------------------------------
// Wire protocol — mirrors crates/ipc/src/protocol.rs
//
// Frame format:
//   [8 bytes big-endian usize  — payload length]
//   [N bytes bincode payload   — the typed message]
//
// The ipc crate uses `usize::to_be_bytes()` which on a 64-bit host is 8 bytes.
// We hardcode 8 bytes here to be explicit.
//
// bincode config: default (little-endian, u64 sequence lengths, fixed int).
// ---------------------------------------------------------------------------

/// Mirrors `ipc::protocol::Request`.
#[derive(Serialize, Deserialize, Debug, Clone)]
struct Request {
    id: Uuid,
    service_id: String,
    bytes: Vec<u8>,
}

impl Request {
    fn new(request: &RemoteControlRequest) -> Result<Self> {
        let bytes = bincode::serialize(request).context("serialize request payload")?;
        Ok(Self {
            id: Uuid::new_v4(),
            service_id: SERVICE_ID.to_owned(),
            bytes,
        })
    }
}

/// Mirrors `ipc::protocol::Response`.
#[derive(Serialize, Deserialize, Debug, Clone)]
enum Response {
    Success {
        request_id: Uuid,
        service_id: String,
        bytes: Vec<u8>,
    },
    Failure {
        request_id: Uuid,
        error_message: String,
    },
}

fn send_message<M: Serialize>(stream: &mut LocalSocketStream, message: &M) -> Result<()> {
    let payload = bincode::serialize(message).context("serialize framed message")?;
    // 8-byte big-endian length prefix — matches ipc crate's usize::to_be_bytes() on 64-bit.
    let header = (payload.len() as u64).to_be_bytes();
    stream.write_all(&header).context("write frame header")?;
    stream.write_all(&payload).context("write frame payload")?;
    stream.flush().context("flush stream")?;
    Ok(())
}

fn recv_message<M: for<'de> Deserialize<'de>>(stream: &mut LocalSocketStream) -> Result<M> {
    let mut header = [0u8; 8];
    stream
        .read_exact(&mut header)
        .context("read frame header")?;
    let payload_len = u64::from_be_bytes(header) as usize;
    let mut payload = vec![0u8; payload_len];
    stream
        .read_exact(&mut payload)
        .context("read frame payload")?;
    let message: M = bincode::deserialize(&payload).context("deserialize framed message")?;
    Ok(message)
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

fn socket_address_path() -> Result<PathBuf> {
    let base = dirs::data_local_dir()
        .or_else(dirs::home_dir)
        .context("no data dir available")?;
    Ok(base.join("dev.warp.Warp").join("remote_control.addr"))
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    let exit_code = match run() {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e:#}");
            if e.chain().any(|c| c.to_string().contains("Warp is not running")) {
                2
            } else if e.chain().any(|c| {
                let s = c.to_string();
                s.contains("Connection refused")
                    || s.contains("could not connect")
                    || s.contains("No such file or directory")
                    || s.contains("connect to server")
            }) {
                3
            } else {
                1
            }
        }
    };
    std::process::exit(exit_code);
}

fn run() -> Result<()> {
    let cli = Cli::parse();

    // Read the socket path written by the Warp server on startup.
    let addr_path = socket_address_path()?;
    let socket_path = std::fs::read_to_string(&addr_path).with_context(|| {
        format!(
            "Warp is not running? Couldn't read {}",
            addr_path.display()
        )
    })?;
    let socket_path = socket_path.trim().to_owned();

    // Connect to the Warp IPC Unix domain socket.
    let mut stream = LocalSocketStream::connect(socket_path.as_str())
        .with_context(|| format!("could not connect to Warp IPC at {socket_path}"))?;

    // Build the request for the chosen subcommand.
    let rpc_request = match &cli.command {
        Cmd::Ping => RemoteControlRequest::Ping,
        Cmd::Split { command, direction } => RemoteControlRequest::SplitActivePaneAndRun {
            command: command.clone(),
            direction: (*direction).into(),
        },
    };

    let wire_request = Request::new(&rpc_request)?;
    let request_id = wire_request.id;
    send_message(&mut stream, &wire_request)?;

    // Read the response frame.
    let response: Response = recv_message(&mut stream)?;

    // Validate that the response echoes our request ID, then extract the payload.
    let response_bytes = match response {
        Response::Success {
            request_id: rid,
            bytes,
            ..
        } => {
            if rid != request_id {
                return Err(anyhow!(
                    "response request_id mismatch: expected {request_id}, got {rid}"
                ));
            }
            bytes
        }
        Response::Failure {
            request_id: rid,
            error_message,
        } => {
            if rid != request_id {
                return Err(anyhow!(
                    "response request_id mismatch: expected {request_id}, got {rid}"
                ));
            }
            return Err(anyhow!("IPC framework error: {error_message}"));
        }
    };

    // Deserialize the typed response and act on it.
    let typed_response: RemoteControlResponse =
        bincode::deserialize(&response_bytes).context("deserialize response payload")?;

    match typed_response {
        RemoteControlResponse::Pong => {
            println!("pong");
            Ok(())
        }
        RemoteControlResponse::Ok => Ok(()),
        RemoteControlResponse::Error { message } => {
            eprintln!("{message}");
            std::process::exit(1);
        }
    }
}
