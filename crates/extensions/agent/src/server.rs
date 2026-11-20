//! The listener: a Unix socket, or TCP for a game reached over the network
//! (the Xbox app; docs/runtime/xbox.md, "Remote control"). One thread
//! accepts; two per connection (lines in, lines out). Connection threads
//! only queue; the game thread answers.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;

use agent_proto::Endpoint;

use crate::Agent;

/// Lines queued for a client before it counts as not reading and is
/// dropped.
const OUT_QUEUE: usize = 4096;

/// Longest request line accepted (1 MiB); longer closes the connection.
const MAX_LINE: usize = 1 << 20;

/// One accepted connection, whatever carries it.
trait Stream: Read + Write + Send + Sized + 'static {
    fn try_clone(&self) -> std::io::Result<Self>;
    fn shutdown(&self);
    fn write_timeout(&self, timeout: std::time::Duration) -> std::io::Result<()>;
}

#[cfg(unix)]
impl Stream for UnixStream {
    fn try_clone(&self) -> std::io::Result<Self> {
        UnixStream::try_clone(self)
    }
    fn write_timeout(&self, timeout: std::time::Duration) -> std::io::Result<()> {
        self.set_write_timeout(Some(timeout))
    }
    fn shutdown(&self) {
        let _ = UnixStream::shutdown(self, std::net::Shutdown::Both);
    }
}

impl Stream for TcpStream {
    fn try_clone(&self) -> std::io::Result<Self> {
        TcpStream::try_clone(self)
    }
    fn write_timeout(&self, timeout: std::time::Duration) -> std::io::Result<()> {
        self.set_write_timeout(Some(timeout))
    }
    fn shutdown(&self) {
        let _ = TcpStream::shutdown(self, std::net::Shutdown::Both);
    }
}

/// What the server listens on, for [`Server::endpoint`] and the wake-up
/// connection at drop.
enum Bound {
    #[cfg(unix)]
    Unix(PathBuf),
    Tcp(std::net::SocketAddr),
}

/// The listening socket. Dropping it stops accepting (and removes a Unix
/// socket's file); open connections end when the game exits.
pub struct Server {
    bound: Bound,
    stop: Arc<AtomicBool>,
}

impl Server {
    /// Listens at `path`: a Unix socket, or TCP when it reads
    /// `tcp://HOST:PORT` ([`Endpoint::parse`]).
    pub fn listen(path: &Path, agent: Agent) -> std::io::Result<Server> {
        Self::listen_on(&Endpoint::from(path.to_path_buf()), agent)
    }

    /// Listens on `endpoint`. A Unix socket file left by a run that's gone
    /// is replaced; one that something still answers on is an error. A
    /// TCP port in use is an error.
    pub fn listen_on(endpoint: &Endpoint, agent: Agent) -> std::io::Result<Server> {
        let stop = Arc::new(AtomicBool::new(false));
        let s = Arc::clone(&stop);
        let bound = match endpoint {
            #[cfg(unix)]
            Endpoint::Unix(path) => {
                let listener = bind_unix(path)?;
                thread::Builder::new()
                    .name("agent-accept".into())
                    .spawn(move || accept(listener.incoming(), &s, agent))?;
                Bound::Unix(path.clone())
            }
            #[cfg(not(unix))]
            Endpoint::Unix(path) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    format!(
                        "{}: no Unix sockets here; listen on tcp://HOST:PORT",
                        path.display()
                    ),
                ));
            }
            Endpoint::Tcp(addr) => {
                let listener = TcpListener::bind(addr.as_str())?;
                let local = listener.local_addr()?;
                thread::Builder::new()
                    .name("agent-accept".into())
                    .spawn(move || {
                        let incoming = listener.incoming().map(|r| {
                            r.inspect(|s| {
                                let _ = s.set_nodelay(true);
                            })
                        });
                        accept(incoming, &s, agent)
                    })?;
                Bound::Tcp(local)
            }
        };
        Ok(Server { bound, stop })
    }

    /// Where it listens; for TCP, the bound address (the real port when
    /// asked for port 0).
    pub fn endpoint(&self) -> Endpoint {
        match &self.bound {
            #[cfg(unix)]
            Bound::Unix(p) => Endpoint::Unix(p.clone()),
            Bound::Tcp(a) => Endpoint::Tcp(a.to_string()),
        }
    }

    /// The Unix socket's path, or the TCP address as `tcp://HOST:PORT`.
    pub fn path(&self) -> PathBuf {
        match &self.bound {
            #[cfg(unix)]
            Bound::Unix(p) => p.clone(),
            Bound::Tcp(_) => PathBuf::from(self.endpoint().to_string()),
        }
    }
}

#[cfg(unix)]
fn bind_unix(path: &Path) -> std::io::Result<UnixListener> {
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        use std::os::unix::fs::FileTypeExt;
        if !meta.file_type().is_socket() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!(
                    "{}: a file that isn't a socket is there; not replacing it",
                    path.display()
                ),
            ));
        }
        if UnixStream::connect(path).is_ok() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                format!("{}: another game is listening there", path.display()),
            ));
        }
        std::fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path)?;
    // The agent can drive the game: only this user may connect.
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

fn accept<S: Stream>(
    incoming: impl Iterator<Item = std::io::Result<S>>,
    stop: &AtomicBool,
    agent: Agent,
) {
    for stream in incoming {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        match stream {
            Ok(stream) => serve(stream, agent.clone()),
            Err(e) => eprintln!("agent: accept: {e}"),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Wake the accept loop so it sees the flag.
        match &self.bound {
            #[cfg(unix)]
            Bound::Unix(p) => {
                let _ = UnixStream::connect(p);
                let _ = std::fs::remove_file(p);
            }
            Bound::Tcp(a) => {
                let mut to = *a;
                if to.ip().is_unspecified() {
                    to.set_ip(match to {
                        std::net::SocketAddr::V4(_) => std::net::Ipv4Addr::LOCALHOST.into(),
                        std::net::SocketAddr::V6(_) => std::net::Ipv6Addr::LOCALHOST.into(),
                    });
                }
                let _ = TcpStream::connect_timeout(&to, std::time::Duration::from_secs(1));
            }
        }
    }
}

pub(crate) enum Output {
    Line(String),
    Upgrade { line: String, frames: crate::live::Frames },
}

fn serve<S: Stream>(stream: S, agent: Agent) {
    let Ok(mut out) = stream.try_clone() else {
        return;
    };
    let (tx, rx) = mpsc::sync_channel::<Output>(OUT_QUEUE);
    let conn = agent.connect_socket(tx);
    let _ = thread::Builder::new()
        .name(format!("agent-out-{conn}"))
        .spawn(move || {
            for msg in rx {
                let (line, upgrade) = match msg {
                    Output::Line(line) => (line, None),
                    Output::Upgrade { line, frames } => (line, Some(frames)),
                };
                if out
                    .write_all(line.as_bytes())
                    .and_then(|_| out.write_all(b"\n"))
                    .is_err()
                {
                    break;
                }
                if let Some(frames) = upgrade {
                    if out.write_timeout(crate::live::CLOSE_INTERVAL).is_ok() {
                        let _ = frames.write_to(&mut out);
                    }
                    break;
                }
            }
            // Dropped (or done): close both ways, which also ends the reader.
            out.shutdown();
        });
    let _ = thread::Builder::new()
        .name(format!("agent-in-{conn}"))
        .spawn(move || {
            let mut r = BufReader::new(stream);
            let mut line = String::new();
            loop {
                line.clear();
                match (&mut r).take(MAX_LINE as u64 + 1).read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(n) if n > MAX_LINE => break,
                    Ok(_) => agent.submit(conn, &line),
                }
            }
            // Dropping the connection's sender ends the writer thread.
            agent.disconnect(conn);
        });
}
