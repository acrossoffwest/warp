use std::collections::HashMap;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::net::Shutdown;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::protocol::{
    Envelope, ErrorBody, ErrorCode, Request, error_response, ok_response, parse_request,
};

pub type Handler = Arc<dyn Fn(Request) -> Result<Value, ErrorBody> + Send + Sync>;

/// `sun_path` is 104 bytes on macOS (108 on Linux), including the NUL.
const MAX_SOCKET_PATH_LEN: usize = 103;

const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(50);

type Connections = Arc<Mutex<HashMap<u64, UnixStream>>>;

pub struct Server {
    path: PathBuf,
    dev: u64,
    ino: u64,
    stop: Arc<AtomicBool>,
    connections: Connections,
    accept_thread: Option<JoinHandle<()>>,
}

impl Server {
    pub fn start(path: &Path, handler: Handler) -> Result<Server> {
        if path.as_os_str().len() > MAX_SOCKET_PATH_LEN {
            bail!("socket path is too long for a Unix socket: {}", path.display());
        }
        let dir = path.parent().context("socket path has no parent directory")?;
        if !dir.exists() {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)
                .with_context(|| format!("creating {}", dir.display()))?;
        }
        match std::fs::symlink_metadata(path) {
            Ok(metadata) => {
                if !metadata.file_type().is_socket() {
                    bail!("refusing to remove non-socket file at {}", path.display());
                }
                if UnixStream::connect(path).is_ok() {
                    bail!("another process is already serving {}", path.display());
                }
                std::fs::remove_file(path)
                    .with_context(|| format!("removing stale socket {}", path.display()))?;
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("inspecting {}", path.display()));
            }
        }
        let listener =
            UnixListener::bind(path).with_context(|| format!("binding {}", path.display()))?;
        listener
            .set_nonblocking(true)
            .with_context(|| format!("setting {} non-blocking", path.display()))?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("chmod 0600 {}", path.display()))?;
        let metadata = std::fs::symlink_metadata(path)
            .with_context(|| format!("inspecting {}", path.display()))?;
        let (dev, ino) = (metadata.dev(), metadata.ino());

        let stop = Arc::new(AtomicBool::new(false));
        let connections: Connections = Arc::new(Mutex::new(HashMap::new()));
        let thread_stop = stop.clone();
        let thread_connections = connections.clone();
        let accept_thread = std::thread::Builder::new()
            .name("fork-control-accept".into())
            .spawn(move || accept_loop(listener, handler, thread_stop, thread_connections))
            .context("spawning accept thread")?;

        Ok(Server {
            path: path.to_owned(),
            dev,
            ino,
            stop,
            connections,
            accept_thread: Some(accept_thread),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    #[cfg(test)]
    fn live_connection_count(&self) -> usize {
        self.connections.lock().map(|c| c.len()).unwrap_or(0)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Ok(connections) = self.connections.lock() {
            for stream in connections.values() {
                let _ = stream.shutdown(Shutdown::Both);
            }
        }
        if let Some(thread) = self.accept_thread.take() {
            let _ = thread.join();
        }
        if let Ok(metadata) = std::fs::symlink_metadata(&self.path)
            && metadata.dev() == self.dev
            && metadata.ino() == self.ino
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn accept_loop(
    listener: UnixListener,
    handler: Handler,
    stop: Arc<AtomicBool>,
    connections: Connections,
) {
    let mut next_id: u64 = 0;
    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        match listener.accept() {
            Ok((stream, _addr)) => {
                if let Err(error) = stream.set_nonblocking(false) {
                    log::warn!("fork_control: failed to set connection blocking: {error}");
                    continue;
                }
                if !peer_is_same_user(&stream) {
                    log::warn!("fork_control: rejected a connection from another user");
                    continue;
                }
                let Ok(registered) = stream.try_clone() else { continue };
                let id = next_id;
                next_id += 1;
                if let Ok(mut connections) = connections.lock() {
                    connections.insert(id, registered);
                }
                if stop.load(Ordering::SeqCst) {
                    if let Ok(mut connections) = connections.lock()
                        && let Some(registered) = connections.remove(&id)
                    {
                        let _ = registered.shutdown(Shutdown::Both);
                    }
                    continue;
                }
                let handler = handler.clone();
                let conn_stop = stop.clone();
                let conn_connections = connections.clone();
                let spawned = std::thread::Builder::new()
                    .name("fork-control-conn".into())
                    .spawn(move || {
                        serve_connection(stream, handler, conn_stop);
                        if let Ok(mut connections) = conn_connections.lock() {
                            connections.remove(&id);
                        }
                    });
                if let Err(error) = spawned {
                    log::warn!("fork_control: failed to spawn connection thread: {error}");
                    if let Ok(mut connections) = connections.lock() {
                        connections.remove(&id);
                    }
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(ACCEPT_POLL_INTERVAL);
            }
            Err(error) => {
                log::warn!("fork_control: accept failed: {error}");
                std::thread::sleep(ACCEPT_POLL_INTERVAL);
            }
        }
    }
}

fn serve_connection(stream: UnixStream, handler: Handler, stop: Arc<AtomicBool>) {
    let Ok(read_half) = stream.try_clone() else { return };
    let mut writer = stream;
    for line in BufReader::new(read_half).lines() {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let response = handle_line(&line, &handler);
        if writeln!(writer, "{response}").is_err() {
            break;
        }
    }
}

pub fn handle_line(line: &str, handler: &Handler) -> Value {
    match parse_request(line) {
        Err((id, error)) => {
            log::warn!("fork_control: rejected request: {}", error.message);
            error_response(id, &error)
        }
        Ok(Envelope { id, request }) => {
            match catch_unwind(AssertUnwindSafe(|| handler(request))) {
                Ok(Ok(result)) => ok_response(id, result),
                Ok(Err(error)) => error_response(id, &error),
                Err(_) => {
                    log::error!("fork_control: request handler panicked");
                    error_response(
                        id,
                        &ErrorBody::new(ErrorCode::Internal, "request handler panicked"),
                    )
                }
            }
        }
    }
}

fn peer_is_same_user(stream: &UnixStream) -> bool {
    // SAFETY: geteuid has no preconditions.
    let euid = unsafe { libc::geteuid() };
    peer_uid(stream).is_some_and(|uid| uid == euid)
}

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
fn peer_uid(stream: &UnixStream) -> Option<u32> {
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: the fd is a live Unix socket owned by `stream`; uid/gid are valid out-pointers.
    let rc = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    (rc == 0).then_some(uid)
}

#[cfg(target_os = "linux")]
fn peer_uid(stream: &UnixStream) -> Option<u32> {
    // SAFETY: ucred is plain data; getsockopt writes at most `len` bytes into it.
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    (rc == 0).then_some(cred.uid)
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "linux"
)))]
fn peer_uid(_stream: &UnixStream) -> Option<u32> {
    None
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
