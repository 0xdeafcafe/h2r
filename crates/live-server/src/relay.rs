//! `GET /v1/sessions/{id}/relay`: the co-op relay (docs/live/live-api.md, "Co-op
//! relay"; docs/live/live-coop.md). A WebSocket that hands each binary message
//! from one member of a co-op session to the other, for two machines that
//! cannot reach each other directly.
//!
//! - The first message is `{kind: "auth", token}` with `lxm: sessions.relay`.
//! - The author must be a member: the session's host, or an account whose
//!   invitation to this session was accepted. The session must be open and
//!   advertised with `flags: "coop"`.
//! - A room holds two members. A member that connects again replaces its
//!   old socket; a third account is refused.
//! - A binary message is at most [`MAX_RELAY_MESSAGE`] bytes and a socket
//!   may send `Policy::relay_rate` messages a second; breaking either
//!   closes the socket. The server never reads a message's contents, never
//!   stores one, and drops it when the other member is absent or slow: it
//!   is a datagram path, and the co-op protocol repeats what was lost.
//! - Each member is told when the other arrives or leaves:
//!   `{kind: "peer", present}`.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use axum::body::Bytes;
#[cfg(feature = "native")]
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::Response;
use live_proto::mailbox::Answer;
use live_proto::sign::{TOKEN_LEEWAY_S, Token};
use live_proto::{Author, ClientMessage, ErrorCode, Xuid, lxm};
use tokio::sync::mpsc;

use crate::app::{ApiError, Live, now};
#[cfg(feature = "native")]
use crate::http::close;
use crate::http::{AUTH_TIMEOUT, CLOSE_UNAUTHORIZED, MAX_WS_MESSAGE, auth_message};

/// The largest relayed message: a co-op datagram is under 900 bytes
/// (`coop::wire::MAX_DATAGRAM`).
pub const MAX_RELAY_MESSAGE: usize = 1200;
/// The session flag that makes a session a co-op one.
pub const COOP_FLAG: &str = "coop";
/// Messages waiting for a slow member before new ones are dropped.
const QUEUE: usize = 256;
/// How often an open socket's membership is checked again.
const RECHECK: Duration = Duration::from_secs(5);

/// Close codes, beside [`CLOSE_UNAUTHORIZED`].
pub const CLOSE_FORBIDDEN: u16 = 4403;
/// Replaced by the same account's newer socket, or the room is full.
pub const CLOSE_CONFLICT: u16 = 4409;
pub const CLOSE_TOO_LARGE: u16 = 4413;
pub const CLOSE_RATE: u16 = 4429;

enum Out {
    Data(Bytes),
    Peer(bool),
    Replaced,
}

struct Member {
    xuid: Xuid,
    socket: u64,
    tx: mpsc::Sender<Out>,
}

/// The open relay rooms, by session id.
#[derive(Default)]
pub struct Relays {
    rooms: Mutex<HashMap<String, Vec<Member>>>,
    next_socket: AtomicU64,
    /// Messages handed on, for the tests and the admin page.
    forwarded: AtomicU64,
}

impl Relays {
    pub fn forwarded(&self) -> u64 {
        self.forwarded.load(Ordering::Relaxed)
    }

    /// Sessions with a relay socket open.
    pub fn rooms(&self) -> usize {
        self.rooms.lock().unwrap().len()
    }

    /// Adds a member's socket. `Err` when two other accounts hold the room.
    fn join(&self, session: &str, xuid: Xuid, tx: mpsc::Sender<Out>) -> Result<u64, ()> {
        let socket = self.next_socket.fetch_add(1, Ordering::Relaxed);
        let mut rooms = self.rooms.lock().unwrap();
        let room = rooms.entry(session.to_string()).or_default();
        if let Some(old) = room.iter_mut().find(|m| m.xuid == xuid) {
            let _ = old.tx.try_send(Out::Replaced);
            old.socket = socket;
            old.tx = tx;
        } else if room.len() >= 2 {
            return Err(());
        } else {
            room.push(Member { xuid, socket, tx });
        }
        // Tell each side about the other.
        if let [a, b] = room.as_slice() {
            let _ = a.tx.try_send(Out::Peer(true));
            let _ = b.tx.try_send(Out::Peer(true));
        }
        Ok(socket)
    }

    fn leave(&self, session: &str, socket: u64) {
        let mut rooms = self.rooms.lock().unwrap();
        let Some(room) = rooms.get_mut(session) else {
            return;
        };
        let before = room.len();
        room.retain(|m| m.socket != socket);
        if room.len() < before {
            for m in room.iter() {
                let _ = m.tx.try_send(Out::Peer(false));
            }
        }
        if room.is_empty() {
            rooms.remove(session);
        }
    }

    /// Hands `data` to the other member, or drops it.
    fn forward(&self, session: &str, from: u64, data: Bytes) {
        let rooms = self.rooms.lock().unwrap();
        let other = rooms
            .get(session)
            .and_then(|room| room.iter().find(|m| m.socket != from));
        if let Some(m) = other
            && m.tx.try_send(Out::Data(data)).is_ok()
        {
            self.forwarded.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl Live {
    /// Whether `xuid` may use the relay of `session`.
    pub(crate) fn relay_member(&self, session: &str, xuid: Xuid) -> Result<(), ApiError> {
        let s = self.model.read();
        let Some(found) = s.sessions.get(session) else {
            return Err(ApiError::new(ErrorCode::NotFound, "no such session"));
        };
        if found.state != "open" || found.advertisement.flags != COOP_FLAG {
            return Err(ApiError::new(
                ErrorCode::Forbidden,
                "the relay is for open co-op sessions",
            ));
        }
        let invited = || {
            s.mailboxes.get(&xuid).is_some_and(|box_| {
                box_.values()
                    .any(|i| i.session_id == session && i.answer == Some(Answer::Accept))
            })
        };
        if found.host_xuid == xuid || invited() {
            Ok(())
        } else {
            Err(ApiError::new(
                ErrorCode::Forbidden,
                "not a member of this session",
            ))
        }
    }
}

/// The type of a relay ticket's JWS.
pub const TYP_RELAY_TICKET: &str = "h2live-relay-ticket";
/// How long a relay ticket lasts. A relay asks the hub for a new one when
/// its ticket has run out, so membership is checked again this often.
pub const TICKET_S: i64 = 15;

/// What the hub signs when an account may use a session's relay: who, for
/// which session, until when. A relay that is not the hub (the Worker's
/// `CoopRelay` object) checks it with the server's public key instead of
/// reading the read model itself.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Ticket {
    pub session: String,
    pub xuid: Xuid,
    /// The account's author id.
    pub author: String,
    /// The account's token's `exp`: the socket closes then (with the
    /// leeway) unless a fresh token comes.
    pub token_exp: i64,
    /// When this ticket runs out (unix seconds).
    pub exp: i64,
}

impl Ticket {
    /// When the socket's token has run out, leeway included.
    pub fn token_deadline(&self) -> i64 {
        self.token_exp + TOKEN_LEEWAY_S
    }

    /// Opens a ticket signed by `key` for `session`, not yet run out at
    /// `now`.
    pub fn open(
        key: &live_proto::sign::PublicKey,
        compact: &str,
        session: &str,
        now: i64,
    ) -> Result<Ticket, ApiError> {
        let bad = |m: &str| ApiError::new(ErrorCode::Forbidden, format!("relay ticket: {m}"));
        let jws = live_proto::sign::Jws::parse(compact).map_err(|e| bad(&e.to_string()))?;
        jws.verify(key).map_err(|e| bad(&e.to_string()))?;
        let payload = jws
            .json_payload(TYP_RELAY_TICKET)
            .map_err(|e| bad(&e.to_string()))?;
        let t: Ticket = serde_json::from_value(payload).map_err(|e| bad(&e.to_string()))?;
        if t.session != session {
            return Err(bad("for another session"));
        }
        if t.exp < now {
            return Err(bad("run out"));
        }
        Ok(t)
    }
}

/// `POST /v1/sessions/{id}/relay/ticket`'s answer.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TicketReply {
    pub ticket: String,
    pub exp: i64,
}

impl Live {
    fn sign_ticket(&self, t: &Ticket) -> TicketReply {
        TicketReply {
            ticket: self.identity.key.sign_json(TYP_RELAY_TICKET, t),
            exp: t.exp,
        }
    }

    /// A ticket for the relay of `session`, for the account behind a
    /// `sessions.relay` token (its `jti` is spent here).
    pub fn relay_ticket(
        &self,
        bearer: Option<&str>,
        session: &str,
    ) -> Result<TicketReply, ApiError> {
        let raw = bearer.ok_or_else(|| {
            ApiError::new(
                ErrorCode::Unauthorized,
                "this needs Authorization: Bearer <token>",
            )
        })?;
        let token_exp = self.authenticate_until(raw, lxm::SESSIONS_RELAY)?;
        let author = Author::parse(
            &Token::parse(raw)
                .map_err(|_| ApiError::new(ErrorCode::Unauthorized, "token"))?
                .claims
                .iss,
        );
        let Some(Author::Account { xuid, .. }) = author.clone() else {
            return Err(ApiError::new(
                ErrorCode::Forbidden,
                "the relay is for accounts",
            ));
        };
        self.relay_member(session, xuid)?;
        Ok(self.sign_ticket(&Ticket {
            session: session.to_string(),
            xuid,
            author: author.expect("parsed").to_string(),
            token_exp,
            exp: now() + TICKET_S,
        }))
    }

    /// A new ticket for the holder of one that has (nearly) run out, if it
    /// is still a member and its token has not run out.
    pub fn renew_relay_ticket(
        &self,
        compact: &str,
        session: &str,
    ) -> Result<TicketReply, ApiError> {
        let now = now();
        // A ticket that has just run out may be renewed: the relay asks
        // when it finds one past its time.
        let old = Ticket::open(self.identity.key.public(), compact, session, now - TICKET_S)?;
        if old.token_deadline() < now {
            return Err(ApiError::new(ErrorCode::Unauthorized, "token expired"));
        }
        self.relay_member(session, old.xuid)?;
        Ok(self.sign_ticket(&Ticket {
            exp: now + TICKET_S,
            ..old
        }))
    }
}

#[cfg(feature = "native")]
pub async fn relay(
    State(live): State<Arc<Live>>,
    Path(session): Path<String>,
    ws: Result<WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
) -> Result<Response, ApiError> {
    let ws = ws.map_err(|_| ApiError::new(ErrorCode::Invalid, "the relay is a WebSocket"))?;
    Ok(ws
        .max_message_size(MAX_WS_MESSAGE)
        .on_upgrade(move |socket| run(live, session, socket)))
}

fn account(token_text: &str) -> Option<(Author, Xuid)> {
    let ClientMessage::Auth { token } = serde_json::from_str(token_text).ok()? else {
        return None;
    };
    let author = Author::parse(&Token::parse(&token).ok()?.claims.iss)?;
    match author {
        Author::Account { xuid, .. } => Some((author, xuid)),
        Author::Server(_) => None,
    }
}

/// A token bucket for one socket.
struct Bucket {
    tokens: f64,
    at: Instant,
    per_sec: f64,
    burst: f64,
}

impl Bucket {
    fn new((per_sec, burst): (f64, u32)) -> Bucket {
        Bucket {
            tokens: burst as f64,
            at: Instant::now(),
            per_sec,
            burst: burst as f64,
        }
    }

    fn allow(&mut self) -> bool {
        let now = Instant::now();
        self.tokens = (self.tokens + now.duration_since(self.at).as_secs_f64() * self.per_sec)
            .min(self.burst);
        self.at = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(feature = "native")]
async fn run(live: Arc<Live>, session: String, mut socket: WebSocket) {
    let first = tokio::time::timeout(AUTH_TIMEOUT, socket.recv()).await;
    let Ok(Some(Ok(Message::Text(text)))) = first else {
        close(
            &mut socket,
            CLOSE_UNAUTHORIZED,
            "send an auth message first",
        )
        .await;
        return;
    };
    let mut expires = match auth_message(&live, &text, lxm::SESSIONS_RELAY) {
        Ok(exp) => exp,
        Err(e) => return close(&mut socket, CLOSE_UNAUTHORIZED, &e.message).await,
    };
    let Some((author, xuid)) = account(&text) else {
        return close(&mut socket, CLOSE_FORBIDDEN, "the relay is for accounts").await;
    };
    if let Err(e) = live.relay_member(&session, xuid) {
        return close(&mut socket, CLOSE_FORBIDDEN, &e.message).await;
    }
    let (tx, mut rx) = mpsc::channel(QUEUE);
    let Ok(id) = live.relays.join(&session, xuid, tx) else {
        return close(&mut socket, CLOSE_CONFLICT, "the session's relay is full").await;
    };
    let (code, reason) = pump(
        &live,
        &session,
        id,
        &author,
        xuid,
        &mut expires,
        &mut socket,
        &mut rx,
    )
    .await;
    live.relays.leave(&session, id);
    if let Some(code) = code {
        close(&mut socket, code, &reason).await;
    }
}

#[cfg(feature = "native")]
/// Runs one member's socket until it ends; the close code to send, if any.
#[allow(clippy::too_many_arguments)]
async fn pump(
    live: &Live,
    session: &str,
    id: u64,
    author: &Author,
    xuid: Xuid,
    expires: &mut i64,
    socket: &mut WebSocket,
    rx: &mut mpsc::Receiver<Out>,
) -> (Option<u16>, String) {
    let mut bucket = Bucket::new(live.policy().relay_rate);
    let mut recheck = tokio::time::interval(RECHECK);
    recheck.tick().await;
    let stop = |code, reason: &str| (Some(code), reason.to_string());
    loop {
        let token_left = Duration::from_secs((*expires + TOKEN_LEEWAY_S - now()).max(0) as u64);
        tokio::select! {
            msg = socket.recv() => match msg {
                Some(Ok(Message::Binary(data))) => {
                    if data.len() > MAX_RELAY_MESSAGE {
                        return stop(CLOSE_TOO_LARGE, "message too large");
                    }
                    if !bucket.allow() {
                        return stop(CLOSE_RATE, "slow down");
                    }
                    live.relays.forward(session, id, data);
                }
                // A fresh token keeps the socket open; it must be the same account's.
                Some(Ok(Message::Text(text))) => match auth_message(live, &text, lxm::SESSIONS_RELAY) {
                    Ok(exp) if account(&text).is_some_and(|(a, _)| &a == author) => *expires = exp,
                    Ok(_) => return stop(CLOSE_FORBIDDEN, "a socket keeps its author"),
                    Err(e) => return stop(CLOSE_UNAUTHORIZED, &e.message),
                },
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return (None, String::new()),
                Some(Ok(_)) => {}
            },
            out = rx.recv() => {
                let sent = match out {
                    Some(Out::Data(data)) => socket.send(Message::Binary(data)).await,
                    Some(Out::Peer(present)) => {
                        let text = serde_json::json!({"kind": "peer", "present": present}).to_string();
                        socket.send(Message::Text(text.into())).await
                    }
                    Some(Out::Replaced) | None => {
                        return stop(CLOSE_CONFLICT, "replaced by a newer socket");
                    }
                };
                if sent.is_err() {
                    return (None, String::new());
                }
            }
            _ = recheck.tick() => {
                if let Err(e) = live.relay_member(session, xuid) {
                    return stop(CLOSE_FORBIDDEN, &e.message);
                }
            }
            _ = tokio::time::sleep(token_left) => return stop(CLOSE_UNAUTHORIZED, "token expired"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xuid(n: u64) -> Xuid {
        Xuid::parse(&format!("{:016x}", 0x0009_0000_0000_0000u64 + n)).unwrap()
    }

    #[test]
    fn a_room_holds_two_accounts_and_forwards_between_them() {
        let relays = Relays::default();
        let (ta, mut ra) = mpsc::channel(8);
        let (tb, mut rb) = mpsc::channel(8);
        let a = relays.join("s", xuid(1), ta).unwrap();
        assert!(ra.try_recv().is_err(), "alone: nothing to hear");
        let b = relays.join("s", xuid(2), tb).unwrap();
        assert!(matches!(ra.try_recv(), Ok(Out::Peer(true))));
        assert!(matches!(rb.try_recv(), Ok(Out::Peer(true))));
        let (tc, _rc) = mpsc::channel(8);
        assert!(relays.join("s", xuid(3), tc).is_err(), "a third account");

        relays.forward("s", a, Bytes::from_static(b"to b"));
        assert!(matches!(rb.try_recv(), Ok(Out::Data(d)) if &d[..] == b"to b"));
        assert!(ra.try_recv().is_err(), "never back to the sender");
        relays.forward("other", a, Bytes::from_static(b"nowhere"));
        assert_eq!(relays.forwarded(), 1);

        // The same account again replaces its socket.
        let (tb2, mut rb2) = mpsc::channel(8);
        let b2 = relays.join("s", xuid(2), tb2).unwrap();
        assert!(matches!(rb.try_recv(), Ok(Out::Replaced)));
        assert!(matches!(rb2.try_recv(), Ok(Out::Peer(true))));
        relays.leave("s", b); // the old socket's exit changes nothing
        relays.forward("s", a, Bytes::from_static(b"again"));
        assert!(matches!(rb2.try_recv(), Ok(Out::Data(_))));

        relays.leave("s", b2);
        assert!(matches!(ra.try_recv(), Ok(Out::Peer(true))));
        assert!(matches!(ra.try_recv(), Ok(Out::Peer(false))));
        relays.leave("s", a);
        assert_eq!(relays.rooms(), 0);
    }

    #[test]
    fn a_slow_member_loses_messages_instead_of_holding_them() {
        let relays = Relays::default();
        let (ta, _ra) = mpsc::channel(8);
        let (tb, mut rb) = mpsc::channel(2);
        let a = relays.join("s", xuid(1), ta).unwrap();
        relays.join("s", xuid(2), tb).unwrap();
        for _ in 0..10 {
            relays.forward("s", a, Bytes::from_static(b"x"));
        }
        // One slot went to the peer notice, one to a message.
        assert_eq!(relays.forwarded(), 1);
        assert!(matches!(rb.try_recv(), Ok(Out::Peer(true))));
        assert!(matches!(rb.try_recv(), Ok(Out::Data(_))));
        assert!(rb.try_recv().is_err());
    }

    #[test]
    fn the_bucket_allows_its_burst_then_its_rate() {
        let mut b = Bucket::new((1000.0, 3));
        assert!((0..3).all(|_| b.allow()));
        assert!(!b.allow());
        std::thread::sleep(Duration::from_millis(5));
        assert!(b.allow());
    }
}
