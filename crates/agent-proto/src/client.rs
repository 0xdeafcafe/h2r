//! A blocking client: one connection, calls answered in order, events kept
//! for [`Client::next_event`].

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use crate::{Event, Request, RpcError, ServerLine};

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
    out: UnixStream,
    lines: BufReader<UnixStream>,
    next_id: u64,
    events: VecDeque<Event>,
}

impl Client {
    pub fn connect(path: &Path) -> std::io::Result<Client> {
        let s = UnixStream::connect(path)?;
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
