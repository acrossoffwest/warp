use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::protocol::{
    Envelope, ErrorBody, ErrorCode, Request, error_response, ok_response, parse_request,
};

pub type Handler = Arc<dyn Fn(Request) -> Result<Value, ErrorBody> + Send + Sync>;

/// `sun_path` is 104 bytes on macOS (108 on Linux), including the NUL.
const MAX_SOCKET_PATH_LEN: usize = 103;

pub struct Server {
    path: PathBuf,
    stop: Arc<AtomicBool>,
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
        if path.exists() {
            if UnixStream::connect(path).is_ok() {
                bail!("another process is already serving {}", path.display());
            }
            std::fs::remove_file(path)
                .with_context(|| format!("removing stale socket {}", path.display()))?;
        }
        let listener =
            UnixListener::bind(path).with_context(|| format!("binding {}", path.display()))?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("chmod 0600 {}", path.display()))?;

        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let accept_thread = std::thread::Builder::new()
            .name("fork-control-accept".into())
            .spawn(move || accept_loop(listener, handler, thread_stop))
            .context("spawning accept thread")?;

        Ok(Server {
            path: path.to_owned(),
            stop,
            accept_thread: Some(accept_thread),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = UnixStream::connect(&self.path);
        if let Some(thread) = self.accept_thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

fn accept_loop(listener: UnixListener, handler: Handler, stop: Arc<AtomicBool>) {
    for stream in listener.incoming() {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let Ok(stream) = stream else { continue };
        if !peer_is_same_user(&stream) {
            log::warn!("fork_control: rejected a connection from another user");
            continue;
        }
        let handler = handler.clone();
        let _ = std::thread::Builder::new()
            .name("fork-control-conn".into())
            .spawn(move || serve_connection(stream, handler));
    }
}

fn serve_connection(stream: UnixStream, handler: Handler) {
    let Ok(read_half) = stream.try_clone() else { return };
    let mut writer = stream;
    for line in BufReader::new(read_half).lines() {
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
