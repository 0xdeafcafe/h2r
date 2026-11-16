//! The relay path: co-op datagrams through the Live server.
//!
//! `GET /v1/sessions/{id}/relay` is a WebSocket (docs/live-api.md, "Co-op
//! relay"). The first message is `{"kind":"auth","token":...}` with a token
//! for `sessions.relay`; after that every binary message is one datagram,
//! handed to the session's other member. The server never looks inside one.
//!
//! [`RelayLink`] runs the socket on a thread of its own, so `send` and
//! `recv` never block the game.

use std::io::ErrorKind;
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::link::Link;

/// How long the socket thread waits for the server before it looks at the
/// outgoing queue again: the most a send can be held up.
const TURN: Duration = Duration::from_millis(1);

pub struct RelayLink {
    out: Sender<Vec<u8>>,
    inbound: Receiver<Vec<u8>>,
    closed: Arc<AtomicBool>,
    notes: Arc<Mutex<Vec<String>>>,
}

impl RelayLink {
    /// Connects to `url` (`ws://host/v1/sessions/<id>/relay`) and
    /// authenticates with `token`. Fails when the server cannot be reached;
    /// a refused token closes the link afterwards ([`RelayLink::closed`]).
    pub fn connect(url: &str, token: &str) -> Result<RelayLink, String> {
        let (mut socket, _) = tungstenite::connect(url).map_err(|e| format!("relay {url}: {e}"))?;
        let auth = format!("{{\"kind\":\"auth\",\"token\":{}}}", json_string(token));
        socket
            .send(Message::text(auth))
            .map_err(|e| format!("relay auth: {e}"))?;
        tcp(&mut socket)
            .set_read_timeout(Some(TURN))
            .map_err(|e| format!("relay socket: {e}"))?;
        let (out, out_rx) = channel::<Vec<u8>>();
        let (in_tx, inbound) = channel::<Vec<u8>>();
        let closed = Arc::new(AtomicBool::new(false));
        let notes = Arc::new(Mutex::new(Vec::new()));
        let (c, n) = (Arc::clone(&closed), Arc::clone(&notes));
        std::thread::Builder::new()
            .name("coop-relay".into())
            .spawn(move || {
                run(socket, out_rx, in_tx, &n);
                c.store(true, Ordering::Relaxed);
            })
            .map_err(|e| format!("relay thread: {e}"))?;
        Ok(RelayLink {
            out,
            inbound,
            closed,
            notes,
        })
    }

    /// The server closed the socket (a refused token, the session ended, a
    /// broken rule) or the connection dropped.
    pub fn closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    /// The text messages the server has sent (`{"kind":"peer",...}`, and the
    /// close reason), oldest first.
    pub fn take_notes(&self) -> Vec<String> {
        std::mem::take(&mut *self.notes.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn tcp(socket: &mut WebSocket<MaybeTlsStream<TcpStream>>) -> &mut TcpStream {
    match socket.get_mut() {
        MaybeTlsStream::Plain(tcp) => tcp,
        MaybeTlsStream::Rustls(tls) => &mut tls.sock,
        // The only other variants are TLS backends this build does not enable.
        _ => unreachable!("an unknown stream kind"),
    }
}

fn run(
    mut socket: WebSocket<MaybeTlsStream<TcpStream>>,
    out: Receiver<Vec<u8>>,
    inbound: Sender<Vec<u8>>,
    notes: &Mutex<Vec<String>>,
) {
    let note = |s: String| notes.lock().unwrap_or_else(|e| e.into_inner()).push(s);
    loop {
        loop {
            match out.try_recv() {
                Ok(d) => {
                    if let Err(e) = socket.send(Message::binary(d))
                        && !would_block(&e)
                    {
                        note(format!("closed: {e}"));
                        return;
                    }
                }
                Err(TryRecvError::Empty) => break,
                // The link was dropped.
                Err(TryRecvError::Disconnected) => {
                    let _ = socket.close(None);
                    let _ = socket.flush();
                    return;
                }
            }
        }
        match socket.read() {
            Ok(Message::Binary(d)) => {
                if inbound.send(d.to_vec()).is_err() {
                    return;
                }
            }
            Ok(Message::Text(t)) => note(t.to_string()),
            Ok(Message::Close(frame)) => {
                note(match frame {
                    Some(f) => format!("closed: {} {}", u16::from(f.code), f.reason),
                    None => "closed".into(),
                });
                return;
            }
            Ok(_) => {}
            Err(e) if would_block(&e) => {}
            Err(e) => {
                note(format!("closed: {e}"));
                return;
            }
        }
    }
}

fn would_block(e: &tungstenite::Error) -> bool {
    matches!(e, tungstenite::Error::Io(e)
        if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut))
}

impl Link for RelayLink {
    fn send(&mut self, datagram: &[u8]) {
        let _ = self.out.send(datagram.to_vec());
    }

    fn recv(&mut self) -> Option<Vec<u8>> {
        self.inbound.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_quoted_as_json() {
        assert_eq!(json_string("a.b-c_d"), "\"a.b-c_d\"");
        assert_eq!(json_string("a\"b\\c\n"), "\"a\\\"b\\\\c\\u000a\"");
    }
}
