//! Fork-only local control API: protocol, transport and CLI.
//! Contract: docs/fork-control-api.md.

pub mod cli;
pub mod input;
pub mod paths;
pub mod pids;
pub mod protocol;
#[cfg(unix)]
pub mod client;
#[cfg(unix)]
pub mod server;
