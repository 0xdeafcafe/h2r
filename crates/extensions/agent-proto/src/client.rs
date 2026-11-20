//! A blocking client: one connection, calls answered in order, events kept
//! for [`Client::next_event`].

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::{Endpoint, Event, Request, RpcError, ServerLine};

/// The connection under a [`Client`].
enum Conn {
    #[cfg(unix)]
    Unix(UnixStream),
    Tcp(TcpStream),
}

impl Conn {
    fn try_clone(&self) -> std::io::Result<Conn> {
        Ok(match self {
            #[cfg(unix)]
            Conn::Unix(s) => Conn::Unix(s.try_clone()?),
            Conn::Tcp(s) => Conn::Tcp(s.try_clone()?),
        })
    }

    fn set_read_timeout(&self, t: Option<Duration>) -> std::io::Result<()> {
        match self {
            #[cfg(unix)]
            Conn::Unix(s) => s.set_read_timeout(t),
            Conn::Tcp(s) => s.set_read_timeout(t),
        }
    }
}

/// Wakes a blocking reader when its owner disconnects, including during a
/// subscription handshake or in the middle of a binary frame.
pub struct ConnectionShutdown(Conn);

impl ConnectionShutdown {
    pub fn shutdown(&self) -> std::io::Result<()> {
        match &self.0 {
            #[cfg(unix)]
            Conn::Unix(s) => s.shutdown(Shutdown::Both),
            Conn::Tcp(s) => s.shutdown(Shutdown::Both),
        }
    }
}

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            #[cfg(unix)]
            Conn::Unix(s) => s.read(buf),
            Conn::Tcp(s) => s.read(buf),
        }
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            #[cfg(unix)]
            Conn::Unix(s) => s.write(buf),
            Conn::Tcp(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            #[cfg(unix)]
            Conn::Unix(s) => s.flush(),
            Conn::Tcp(s) => s.flush(),
        }
    }
}

#[derive(Debug)]
pub enum CallError {
    Io(std::io::Error),
    /// The server sent something that isn't a response or an event.
    Protocol(String),
    Rpc(RpcError),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::Io(e) => write!(f, "{e}"),
            CallError::Protocol(e) => write!(f, "protocol: {e}"),
            CallError::Rpc(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for CallError {}

impl From<std::io::Error> for CallError {
    fn from(e: std::io::Error) -> Self {
        CallError::Io(e)
    }
}

pub struct Client {
    out: Conn,
    lines: BufReader<Conn>,
    next_id: u64,
    events: VecDeque<Event>,
}

fn connect_tcp(address: &str, timeout: Duration) -> std::io::Result<TcpStream> {
    use std::io::{Error, ErrorKind};
    let start = Instant::now();
    let mut last = Error::new(ErrorKind::AddrNotAvailable, "no TCP address resolved");
    for addr in address.to_socket_addrs()? {
        let remaining = timeout.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            return Err(Error::new(ErrorKind::TimedOut, "TCP connect timed out"));
        }
        match TcpStream::connect_timeout(&addr, remaining) {
            Ok(socket) => return Ok(socket),
            Err(error) => last = error,
        }
    }
    Err(last)
}

impl Client {
    /// Connects to a Unix socket at `path`, or to TCP when `path` reads
    /// `tcp://HOST:PORT` ([`Endpoint::parse`]).
    pub fn connect(path: &Path) -> std::io::Result<Client> {
        Self::connect_to(&Endpoint::from(path.to_path_buf()))
    }

    pub fn connect_to(endpoint: &Endpoint) -> std::io::Result<Client> {
        Self::connect_with_timeout(endpoint, None)
    }

    /// Connect with a time budget across the resolved TCP addresses.
    /// Unix sockets keep their normal local connect behavior.
    pub fn connect_with_timeout(
        endpoint: &Endpoint,
        timeout: Option<Duration>,
    ) -> std::io::Result<Client> {
        let s = match endpoint {
            #[cfg(unix)]
            Endpoint::Unix(p) => Conn::Unix(UnixStream::connect(p)?),
            #[cfg(not(unix))]
            Endpoint::Unix(p) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    format!("{}: no Unix sockets here; use tcp://HOST:PORT", p.display()),
                ));
            }
            Endpoint::Tcp(a) => {
                let s = match timeout {
                    None => TcpStream::connect(a.as_str())?,
                    Some(timeout) => connect_tcp(a, timeout)?,
                };
                s.set_nodelay(true)?;
                Conn::Tcp(s)
            }
        };
        Ok(Client {
            out: s.try_clone()?,
            lines: BufReader::new(s),
            next_id: 0,
            events: VecDeque::new(),
        })
    }

    /// No answer within `t` is an I/O error (`None`: wait forever).
    pub fn set_timeout(&self, t: Option<Duration>) -> std::io::Result<()> {
        self.lines.get_ref().set_read_timeout(t)
    }

    pub fn shutdown_handle(&self) -> std::io::Result<ConnectionShutdown> {
        self.out.try_clone().map(ConnectionShutdown)
    }

    /// After the live subscription's JSON reply the same socket carries
    /// binary frames. Keep any bytes read ahead with the response line.
    pub fn into_live_reader(self) -> impl Read + Send {
        self.lines
    }

    /// Sends a request and waits for its response; events that arrive
    /// first are kept.
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, CallError> {
        self.next_id += 1;
        let id = self.next_id;
        let line = serde_json::to_string(&Request::new(id, method, params))
            .map_err(|e| CallError::Protocol(e.to_string()))?;
        self.out.write_all(line.as_bytes())?;
        self.out.write_all(b"\n")?;
        loop {
            match self.read()? {
                ServerLine::Notification(n) => self.events.push_back(n.params),
                ServerLine::Response(r) if r.id == id => {
                    return match (r.result, r.error) {
                        (_, Some(e)) => Err(CallError::Rpc(e)),
                        (Some(v), None) => Ok(v),
                        (None, None) => Ok(Value::Null),
                    };
                }
                ServerLine::Response(r) => {
                    // A parse error has a null id: it answers what we sent.
                    if let (Value::Null, Some(e)) = (&r.id, r.error) {
                        return Err(CallError::Rpc(e));
                    }
                    return Err(CallError::Protocol(format!(
                        "response to {} while waiting for {id}",
                        r.id
                    )));
                }
            }
        }
    }

    /// The next event (kept from earlier calls first).
    pub fn next_event(&mut self) -> Result<Event, CallError> {
        if let Some(e) = self.events.pop_front() {
            return Ok(e);
        }
        match self.read()? {
            ServerLine::Notification(n) => Ok(n.params),
            ServerLine::Response(r) => {
                Err(CallError::Protocol(format!("unexpected response {}", r.id)))
            }
        }
    }

    fn read(&mut self) -> Result<ServerLine, CallError> {
        let mut line = String::new();
        if self.lines.read_line(&mut line)? == 0 {
            return Err(CallError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "the game closed the connection",
            )));
        }
        serde_json::from_str(&line).map_err(|e| CallError::Protocol(format!("{e}: {line}")))
    }
}
