//! `GET /v1/ingest`, the game's socket, on the hub's hibernatable
//! WebSockets: each socket runs a `live_server::ingest::IngestSession`, the
//! same session axum's driver runs on a machine.
//!
//! A socket's messages are handled one at a time and in order: each
//! session sits behind an async lock, since a message's work awaits the
//! store and the runtime may hand in the next message meanwhile.
//!
//! The session's author and token end are kept as the socket's attachment
//! ([`IngestAttachment`]), so a socket that outlives a hibernation goes on
//! after the wake. The packed decoder is not kept: the sender's next
//! keyframe picks its frames up again.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use live_server::Live;
use live_server::ingest::{IngestResume, IngestSession};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use worker::{State, WebSocket};

use crate::sockets::send;

/// How long a game has to send its auth message (as on a machine).
const AUTH_TIMEOUT_MS: f64 = 10_000.0;

/// What an ingest socket keeps across hibernation. Its fields differ from
/// a stream socket's, so each kind reads only its own.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestAttachment {
    pub ingest: u64,
    pub session: IngestResume,
}

struct Entry {
    ws: WebSocket,
    since: f64,
    session: Rc<Mutex<IngestSession>>,
}

#[derive(Default)]
pub struct Ingests {
    entries: RefCell<HashMap<u64, Entry>>,
    next: Cell<u64>,
}

pub enum Message<'a> {
    Text(&'a str),
    Binary(&'a [u8]),
}

fn attach(ws: &WebSocket, id: u64, session: &IngestSession) {
    let _ = ws.serialize_attachment(IngestAttachment {
        ingest: id,
        session: session.resume(),
    });
}

/// The socket's attachment, if it is an ingest socket.
pub fn attachment(ws: &WebSocket) -> Option<IngestAttachment> {
    ws.deserialize_attachment().ok().flatten()
}

impl Ingests {
    pub fn accept(&self, state: &State, live: Arc<Live>, ws: WebSocket) {
        state.accept_web_socket(&ws);
        let id = self.next.get() + 1;
        self.next.set(id);
        let session = IngestSession::new(live);
        attach(&ws, id, &session);
        self.entries.borrow_mut().insert(
            id,
            Entry {
                ws,
                since: js_sys::Date::now(),
                session: Rc::new(Mutex::new(session)),
            },
        );
    }

    /// The socket's session, rebuilt from its attachment after a wake.
    fn session(
        &self,
        live: &Arc<Live>,
        ws: &WebSocket,
        a: IngestAttachment,
    ) -> Rc<Mutex<IngestSession>> {
        let mut entries = self.entries.borrow_mut();
        self.next.set(self.next.get().max(a.ingest));
        entries
            .entry(a.ingest)
            .or_insert_with(|| Entry {
                ws: ws.clone(),
                since: js_sys::Date::now(),
                session: Rc::new(Mutex::new(IngestSession::resumed(live.clone(), a.session))),
            })
            .session
            .clone()
    }

    pub async fn message(
        &self,
        live: &Arc<Live>,
        ws: WebSocket,
        a: IngestAttachment,
        msg: Message<'_>,
    ) {
        let id = a.ingest;
        let session = self.session(live, &ws, a);
        let mut session = session.lock().await;
        let before = session.resume();
        let out = match msg {
            Message::Text(t) => session.on_text(t).await,
            Message::Binary(b) => session.on_binary(b).await,
        };
        if !send(&ws, out) {
            self.entries.borrow_mut().remove(&id);
            return;
        }
        // Only an auth message changes what is kept.
        if session.resume() != before {
            attach(&ws, id, &session);
        }
    }

    pub fn closed(&self, id: u64) {
        self.entries.borrow_mut().remove(&id);
    }

    /// Expired tokens, and games that owe an auth message past the
    /// deadline; from the hub's alarm.
    pub fn check(&self) {
        let now_ms = js_sys::Date::now();
        let now = live_server::app::now();
        self.entries.borrow_mut().retain(|_, e| {
            // A session busy with a message is checked next time.
            let Ok(s) = e.session.try_lock() else {
                return true;
            };
            let close = if s.awaits_auth() {
                (now_ms - e.since >= AUTH_TIMEOUT_MS).then(IngestSession::auth_timed_out)
            } else {
                s.expired(now)
            };
            match close {
                Some(c) => {
                    send(&e.ws, vec![c]);
                    false
                }
                None => true,
            }
        });
    }

    /// When [`Ingests::check`] next has something to do (ms since the
    /// epoch).
    pub fn next_due_ms(&self) -> Option<f64> {
        self.entries
            .borrow()
            .values()
            .filter_map(|e| {
                let s = e.session.try_lock().ok()?;
                if s.awaits_auth() {
                    Some(e.since + AUTH_TIMEOUT_MS)
                } else {
                    s.deadline().map(|d| d as f64 * 1000.0)
                }
            })
            .min_by(f64::total_cmp)
    }
}
