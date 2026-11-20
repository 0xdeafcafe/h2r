//! The studio's socket: a local Unix socket only (never TCP), readable and
//! writable by the user alone. One thread accepts; one per connection reads
//! lines, queues each request and writes its answer. Nothing here touches
//! the [`App`](crate::App): whoever owns it answers the queue
//! ([`super::Api::pump`]), so a window answers between frames and the
//! user sees each edit as it lands.

use std::io::{BufRead, BufReader, Read, Write};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
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
}

impl Listener {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Listens at `path` and queues requests on the returned receiver. A
/// socket file left by an editor that's gone is replaced; one something
/// still answers on is an error, as is anything that isn't a socket.
#[cfg(unix)]
pub fn listen(path: &Path) -> std::io::Result<(Listener, mpsc::Receiver<Incoming>)> {
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    let invalid = |m: String| std::io::Error::new(std::io::ErrorKind::InvalidInput, m);
    if path.to_str().is_some_and(|s| s.starts_with("tcp:")) {
        return Err(invalid(format!(
            "{}: the studio listens on a local Unix socket only",
            path.display()
        )));
    }
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if !meta.file_type().is_socket() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("{}: a file that isn't a socket is there", path.display()),
            ));
        }
        if UnixStream::connect(path).is_ok() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                format!("{}: another editor is listening there", path.display()),
            ));
        }
        std::fs::remove_file(path)?;
    }
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let s = Arc::clone(&stop);
    thread::Builder::new()
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
        },
        rx,
    ))
}

#[cfg(not(unix))]
pub fn listen(path: &Path) -> std::io::Result<(Listener, mpsc::Receiver<Incoming>)> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        format!("{}: no Unix sockets here", path.display()),
    ))
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        #[cfg(unix)]
        let _ = UnixStream::connect(&self.path);
        let _ = std::fs::remove_file(&self.path);
    }
}

/// One connection: a line in, its answer out, in order.
#[cfg(unix)]
fn serve(conn: UnixStream, tx: mpsc::Sender<Incoming>) {
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
                let (rtx, rrx) = mpsc::channel();
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
        };
        if let Some(r) = reply {
            let Ok(mut text) = serde_json::to_string(&r) else {
                return;
            };
            text.push('\n');
            if out.write_all(text.as_bytes()).is_err() {
                return;
            }
        }
    }
}
