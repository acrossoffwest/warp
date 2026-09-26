use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::{Value, json};

pub fn call(socket: &Path, method: &str, params: Value) -> Result<Value> {
    let mut stream = UnixStream::connect(socket)
        .with_context(|| format!("connecting to {} (is Warp running?)", socket.display()))?;
    stream.set_read_timeout(Some(Duration::from_secs(15)))?;
    let request = json!({"id": 1, "method": method, "params": params});
    writeln!(stream, "{request}")?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    serde_json::from_str(&line).context("invalid response from Warp")
}
