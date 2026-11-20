//! A co-op relay room as the Worker's `CoopRelay` object keeps it: the two
//! members' sockets, what each may do, and forwarding between them
//! (docs/live/live-coop.md; on a machine, `live_server::relay`). Membership
//! comes from the hub as a signed ticket ([`Ticket`]); the room never reads
//! the read model.
//!
//! Generic over [`RelaySocket`], so it runs and is tested on this machine
//! too; the Worker's is `worker::WebSocket`.

use std::cell::RefCell;

use live_server::relay::{CLOSE_CONFLICT, CLOSE_RATE, CLOSE_TOO_LARGE, MAX_RELAY_MESSAGE, Ticket};
use live_server::stream::CLOSE_UNAUTHORIZED;
use serde::{Deserialize, Serialize};

/// The session id in `/v1/sessions/{id}/relay`.
pub(crate) fn session_of(path: &str) -> Option<&str> {
    let id = path.strip_prefix("/v1/sessions/")?.strip_suffix("/relay")?;
    (!id.is_empty() && !id.contains('/')).then_some(id)
}

/// What a relay socket keeps across hibernation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelayAttachment {
    /// This socket, within its room.
    pub socket: u64,
    pub session: String,
    /// The hub's ticket. Older pending sockets may have no ticket.
    pub ticket: Option<String>,
}

pub trait RelaySocket: Clone {
    fn send_text(&self, text: &str) -> bool;
    fn send_binary(&self, bytes: &[u8]) -> bool;
    fn close(&self, code: u16, reason: &str);
}

/// A token bucket in milliseconds of the caller's clock.
#[derive(Debug, Clone)]
pub struct Bucket {
    tokens: f64,
    at_ms: f64,
    per_sec: f64,
    burst: f64,
}

impl Bucket {
    pub fn new((per_sec, burst): (f64, u32), now_ms: f64) -> Bucket {
        Bucket {
            tokens: burst as f64,
            at_ms: now_ms,
            per_sec,
            burst: burst as f64,
        }
    }

    pub fn allow(&mut self, now_ms: f64) -> bool {
        let gained = (now_ms - self.at_ms).max(0.0) / 1000.0 * self.per_sec;
        self.tokens = (self.tokens + gained).min(self.burst);
        self.at_ms = now_ms;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

struct Member<S> {
    socket: u64,
    ws: S,
    ticket: Ticket,
    bucket: Bucket,
}

/// Why a message was not forwarded: close the sender with this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused(pub u16, pub &'static str);

pub struct Room<S> {
    members: RefCell<Vec<Member<S>>>,
    rate: (f64, u32),
}

fn peer<S: RelaySocket>(ws: &S, present: bool) {
    let text = serde_json::json!({"kind": "peer", "present": present}).to_string();
    ws.send_text(&text);
}

impl<S: RelaySocket> Room<S> {
    pub fn new(rate: (f64, u32)) -> Self {
        Room {
            members: RefCell::new(Vec::new()),
            rate,
        }
    }

    pub fn len(&self) -> usize {
        self.members.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn is_member(&self, socket: u64) -> bool {
        self.members.borrow().iter().any(|m| m.socket == socket)
    }

    /// The member socket's ticket.
    pub fn ticket(&self, socket: u64) -> Option<Ticket> {
        self.members
            .borrow()
            .iter()
            .find(|m| m.socket == socket)
            .map(|m| m.ticket.clone())
    }

    /// Lets a socket in with its ticket. An account that is already in
    /// replaces its old socket (closed); a third account is refused.
    pub fn join(&self, socket: u64, ws: S, ticket: Ticket, now_ms: f64) -> Result<(), Refused> {
        let mut members = self.members.borrow_mut();
        let bucket = Bucket::new(self.rate, now_ms);
        if let Some(old) = members.iter_mut().find(|m| m.ticket.xuid == ticket.xuid) {
            if old.socket != socket {
                old.ws.close(CLOSE_CONFLICT, "replaced by a newer socket");
            }
            *old = Member {
                socket,
                ws,
                ticket,
                bucket,
            };
        } else if members.len() >= 2 {
            return Err(Refused(CLOSE_CONFLICT, "the session's relay is full"));
        } else {
            members.push(Member {
                socket,
                ws,
                ticket,
                bucket,
            });
        }
        if let [a, b] = members.as_slice() {
            peer(&a.ws, true);
            peer(&b.ws, true);
        }
        Ok(())
    }

    /// A member as it was before the object slept: back in the room, with
    /// no notices (both sides already know).
    pub fn restore(&self, socket: u64, ws: S, ticket: Ticket, now_ms: f64) {
        let mut members = self.members.borrow_mut();
        if members.iter().any(|m| m.socket == socket) || members.len() >= 2 {
            return;
        }
        members.push(Member {
            socket,
            ws,
            ticket,
            bucket: Bucket::new(self.rate, now_ms),
        });
    }

    /// A member's fresh ticket (a new token); it must be the same account.
    pub fn refresh(&self, socket: u64, ticket: Ticket) -> Result<(), Refused> {
        let mut members = self.members.borrow_mut();
        let m = members
            .iter_mut()
            .find(|m| m.socket == socket)
            .ok_or(Refused(CLOSE_UNAUTHORIZED, "send an auth message first"))?;
        if m.ticket.xuid != ticket.xuid {
            return Err(Refused(
                live_server::relay::CLOSE_FORBIDDEN,
                "a socket keeps its author",
            ));
        }
        m.ticket = ticket;
        Ok(())
    }

    /// A socket that went away; the other member hears of it.
    pub fn leave(&self, socket: u64) {
        let mut members = self.members.borrow_mut();
        let before = members.len();
        members.retain(|m| m.socket != socket);
        if members.len() < before {
            for m in members.iter() {
                peer(&m.ws, false);
            }
        }
    }

    /// A datagram from a member, for the other, or dropped when there is
    /// none. `Err`: close the sender.
    pub fn forward(&self, from: u64, data: &[u8], now_ms: f64) -> Result<bool, Refused> {
        let mut members = self.members.borrow_mut();
        let sender = members
            .iter_mut()
            .find(|m| m.socket == from)
            .ok_or(Refused(CLOSE_UNAUTHORIZED, "send an auth message first"))?;
        if data.len() > MAX_RELAY_MESSAGE {
            return Err(Refused(CLOSE_TOO_LARGE, "message too large"));
        }
        if !sender.bucket.allow(now_ms) {
            return Err(Refused(CLOSE_RATE, "slow down"));
        }
        if sender.ticket.token_deadline() * 1000 < now_ms as i64 {
            return Err(Refused(CLOSE_UNAUTHORIZED, "token expired"));
        }
        Ok(members
            .iter()
            .find(|m| m.socket != from)
            .is_some_and(|m| m.ws.send_binary(data)))
    }

    /// Members whose token has run out at `now_ms`, closed and gone.
    pub fn expire(&self, now_ms: f64) {
        let gone: Vec<u64> = self
            .members
            .borrow()
            .iter()
            .filter(|m| m.ticket.token_deadline() * 1000 < now_ms as i64)
            .map(|m| {
                m.ws.close(CLOSE_UNAUTHORIZED, "token expired");
                m.socket
            })
            .collect();
        for s in gone {
            self.leave(s);
        }
    }

    /// The earliest token deadline (ms since the epoch), for an alarm.
    pub fn next_deadline_ms(&self) -> Option<i64> {
        self.members
            .borrow()
            .iter()
            .map(|m| m.ticket.token_deadline() * 1000)
            .min()
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use live_proto::Xuid;

    use super::*;

    #[derive(Default)]
    struct Seen {
        text: Vec<String>,
        binary: Vec<Vec<u8>>,
        closed: Option<u16>,
    }

    #[derive(Clone, Default)]
    struct Fake(Rc<RefCell<Seen>>);

    impl RelaySocket for Fake {
        fn send_text(&self, text: &str) -> bool {
            self.0.borrow_mut().text.push(text.into());
            true
        }
        fn send_binary(&self, bytes: &[u8]) -> bool {
            self.0.borrow_mut().binary.push(bytes.to_vec());
            true
        }
        fn close(&self, code: u16, _: &str) {
            self.0.borrow_mut().closed = Some(code);
        }
    }

    fn ticket(n: u64, token_exp: i64) -> Ticket {
        Ticket {
            session: "s".into(),
            xuid: Xuid::parse(&format!("{:016x}", 0x0009_0000_0000_0000u64 + n)).unwrap(),
            author: format!("acct:{n}@live.test"),
            token_exp,
            exp: token_exp,
        }
    }

    const NOW: f64 = 1_000_000.0;
    const LATER: i64 = 2_000;

    #[test]
    fn two_members_hear_of_each_other_and_relay_between_them() {
        let room = Room::new((240.0, 480));
        let (a, b, c) = (Fake::default(), Fake::default(), Fake::default());
        room.join(1, a.clone(), ticket(1, LATER), NOW).unwrap();
        assert!(a.0.borrow().text.is_empty(), "alone: nothing to hear");
        room.join(2, b.clone(), ticket(2, LATER), NOW).unwrap();
        assert_eq!(a.0.borrow().text, [r#"{"kind":"peer","present":true}"#]);
        assert_eq!(b.0.borrow().text, [r#"{"kind":"peer","present":true}"#]);
        assert_eq!(
            room.join(3, c.clone(), ticket(3, LATER), NOW),
            Err(Refused(CLOSE_CONFLICT, "the session's relay is full"))
        );

        assert_eq!(room.forward(1, b"to b", NOW), Ok(true));
        assert_eq!(b.0.borrow().binary, [b"to b".to_vec()]);
        assert!(a.0.borrow().binary.is_empty(), "never back to the sender");
        assert_eq!(
            room.forward(3, b"x", NOW),
            Err(Refused(CLOSE_UNAUTHORIZED, "send an auth message first"))
        );
        assert_eq!(
            room.forward(1, &[0; MAX_RELAY_MESSAGE + 1], NOW),
            Err(Refused(CLOSE_TOO_LARGE, "message too large"))
        );

        // The same account again replaces its socket.
        let b2 = Fake::default();
        room.join(4, b2.clone(), ticket(2, LATER), NOW).unwrap();
        assert_eq!(b.0.borrow().closed, Some(CLOSE_CONFLICT));
        room.leave(2); // the old socket's exit changes nothing
        assert_eq!(room.forward(1, b"again", NOW), Ok(true));
        assert_eq!(b2.0.borrow().binary.len(), 1);
        // A fresh ticket must be the same account's.
        assert!(room.refresh(4, ticket(2, LATER + 60)).is_ok());
        assert!(room.refresh(4, ticket(5, LATER)).is_err());

        room.leave(4);
        assert_eq!(
            a.0.borrow().text.last().unwrap(),
            r#"{"kind":"peer","present":false}"#
        );
        assert_eq!(room.forward(1, b"to nobody", NOW), Ok(false));
        room.leave(1);
        assert!(room.is_empty());
    }

    #[test]
    fn a_fast_sender_and_an_expired_token_are_closed() {
        let room = Room::new((1000.0, 3));
        let (a, b) = (Fake::default(), Fake::default());
        room.join(1, a.clone(), ticket(1, LATER), NOW).unwrap();
        room.join(2, b.clone(), ticket(2, 900), NOW).unwrap();
        assert!((0..3).all(|_| room.forward(1, b"x", NOW).is_ok()));
        assert_eq!(
            room.forward(1, b"x", NOW),
            Err(Refused(CLOSE_RATE, "slow down"))
        );
        assert!(room.forward(1, b"x", NOW + 5.0).is_ok(), "the rate refills");

        // b's token (900 s, plus leeway) has run out by now (1000 s).
        assert_eq!(
            room.next_deadline_ms(),
            Some((900 + live_proto::sign::TOKEN_LEEWAY_S) * 1000)
        );
        assert_eq!(
            room.forward(2, b"x", NOW),
            Err(Refused(CLOSE_UNAUTHORIZED, "token expired"))
        );
        room.expire(NOW);
        assert_eq!(b.0.borrow().closed, Some(CLOSE_UNAUTHORIZED));
        assert_eq!(room.len(), 1);
    }
}
