//! The studio's socket: a local Unix socket only (never TCP), readable and
//! writable by the user alone. One thread accepts; one per connection reads
//! lines, answers process-memory reads, and queues document calls. Nothing here touches
//! the [`App`](crate::App): whoever owns it answers the queue
//! ([`super::Api::pump`]), so a window answers between frames and the
//! user sees each edit as it lands.

use std::io::{BufRead, BufReader, Read, Write};
#[cfg(any(unix, windows))]
use agent_proto::local_socket::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;

use agent_proto::{Request, Response, RpcError};
use serde_json::Value;
use studio_proto::code;

/// Longest request line accepted (16 MiB: a variant's text or a long
/// script line); longer closes the connection.
const MAX_LINE: u64 = 16 << 20;

#[cfg(any(unix, windows))]
static PENDING_REQUESTS: mem_watch::Counter =
    mem_watch::Counter::new("studio.agent.pending_requests");
#[cfg(any(unix, windows))]
static REPLY_BYTES: mem_watch::Counter = mem_watch::Counter::new("studio.agent.reply_buffer_bytes");

/// One request waiting for the app, and where its answer goes.
pub struct Incoming {
    pub request: Request,
    reply: mpsc::Sender<Option<Response>>,
}

impl Incoming {
    /// Sends the answer back to the connection (`None`: a notification).
    pub fn answer(&self, r: Option<Response>) {
        let _ = self.reply.send(r);
    }
}

/// The listening socket. Dropping it stops accepting and removes the file.
pub struct Listener {
    path: PathBuf,
    stop: Arc<AtomicBool>,
    acceptor: Option<thread::JoinHandle<()>>,
}

impl Listener {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Listens at `path` and queues requests on the returned receiver. A
/// socket file left by an editor that's gone is replaced; one something
/// still answers on is an error, as is anything that isn't a socket.
#[cfg(any(unix, windows))]
pub fn listen(path: &Path) -> std::io::Result<(Listener, mpsc::Receiver<Incoming>)> {
    let invalid = |m: String| std::io::Error::new(std::io::ErrorKind::InvalidInput, m);
    if path.to_str().is_some_and(|s| s.starts_with("tcp:")) {
        return Err(invalid(format!(
            "{}: the studio listens on a local Unix socket only",
            path.display()
        )));
    }
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let listener = agent_proto::local_socket::listen(path)?;
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let s = Arc::clone(&stop);
    let acceptor = thread::Builder::new()
        .name("studio-accept".into())
        .spawn(move || {
            for conn in listener.incoming() {
                if s.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(conn) = conn else { continue };
                let tx = tx.clone();
                let _ = thread::Builder::new()
                    .name("studio-conn".into())
                    .spawn(move || serve(conn, tx));
            }
        })?;
    Ok((
        Listener {
            path: path.to_path_buf(),
            stop,
            acceptor: Some(acceptor),
        },
        rx,
    ))
}

#[cfg(not(any(unix, windows)))]
pub fn listen(path: &Path) -> std::io::Result<(Listener, mpsc::Receiver<Incoming>)> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        format!("{}: no Unix sockets here", path.display()),
    ))
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        #[cfg(any(unix, windows))]
        if UnixStream::connect(&self.path).is_ok() && let Some(thread) = self.acceptor.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

/// One connection: a line in, its answer out, in order.
#[cfg(any(unix, windows))]
fn serve(conn: UnixStream, tx: mpsc::Sender<Incoming>) {
    let _connection = mem_watch::agent_activity::connection();
    let _scope = mem_watch::scope("agent.socket");
    let Ok(mut out) = conn.try_clone() else {
        return;
    };
    let mut lines = BufReader::new(conn);
    loop {
        let mut line = String::new();
        match (&mut lines).take(MAX_LINE).read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) if !line.ends_with('\n') && line.len() as u64 >= MAX_LINE => return,
            Ok(_) => {}
        }
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Request>(&line) {
            Err(e) => Some(Response::err(
                Value::Null,
                RpcError::new(code::PARSE, format!("not a request: {e}")),
            )),
            Ok(request) => {
                mem_watch::agent_activity::served();
                if agent_proto::memory_serve::supports(&request.method) {
                    let result = agent_proto::memory_serve::call(&request.method, request.params);
                    request.id.map(|id| match result {
                        Ok(v) => Response::ok(id, v),
                        Err(e) => Response::err(id, e),
                    })
                } else {
                    let (rtx, rrx) = mpsc::channel();
                    let _pending = PENDING_REQUESTS.track(1);
                    if tx
                        .send(Incoming {
                            request,
                            reply: rtx,
                        })
                        .is_err()
                    {
                        return;
                    }
                    match rrx.recv() {
                        Ok(r) => r,
                        Err(_) => return,
                    }
                }
            }
        };
        if let Some(r) = reply {
            let _scope = mem_watch::scope("agent.reply");
            let Ok(mut text) = serde_json::to_string(&r) else {
                return;
            };
            text.push('\n');
            let _reply_buffer = REPLY_BYTES.track(text.capacity() as u64);
            if out.write_all(text.as_bytes()).is_err() {
                return;
            }
        }
    }
}
