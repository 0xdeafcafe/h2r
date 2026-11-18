//! `GET /v1/stream` on the hub's hibernatable WebSockets. Each socket is
//! a `live_server::stream::StreamSession`, the same session axum's driver
//! runs on a machine; here one pump fans the hub's feeds out to all of
//! them, and the runtime hands in what readers send
//! (`websocket_message`).
//!
//! Each socket carries its session's lasting state as its attachment
//! ([`Attachment`]). After the object wakes from hibernation (a new
//! instance, sockets still open), the first message or feed event finds
//! no session for a socket and rebuilds it from that.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use live_server::Live;
use live_server::stream::{Out, Resume, StreamFilter, StreamSession};
use serde::{Deserialize, Serialize};
use worker::{State, WebSocket};

/// How long a reader has to send its auth message (as on a machine).
const AUTH_TIMEOUT_MS: f64 = 10_000.0;

/// What a socket keeps across hibernation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub id: u64,
    pub resume: Resume,
}

struct Entry {
    ws: WebSocket,
    /// When this instance took the socket (ms since the epoch): a reader
    /// that owes an auth message has `AUTH_TIMEOUT` from then.
    since: f64,
    /// Taken out while an await works on it.
    session: Option<StreamSession>,
}

#[derive(Default)]
pub struct Sockets {
    entries: RefCell<HashMap<u64, Entry>>,
    next: Cell<u64>,
}

/// Sends a session's messages. `false`: it closed the socket.
fn send(ws: &WebSocket, out: Vec<Out>) -> bool {
    for o in out {
        let sent = match o {
            Out::Text(t) => ws.send_with_str(&*t),
            Out::Binary(b) => ws.send_with_bytes(&b),
            Out::Close(code, reason) => {
                let _ = ws.close(Some(code), Some(reason));
                return false;
            }
        };
        if sent.is_err() {
            return false;
        }
    }
    true
}

/// Brings a session that was out of the pump's reach (being opened or
/// caught up across an await) level with the log, so that once it is put
/// back, with no await between, the pump's next event is the next one it
/// needs.
async fn settle(session: &mut StreamSession, mut out: Vec<Out>) -> Result<Vec<Out>, String> {
    while session.behind() {
        out.extend(session.catch_up().await?);
    }
    Ok(out)
}

async fn open(session: &mut StreamSession) -> Result<Vec<Out>, String> {
    let out = session.open().await?;
    settle(session, out).await
}

fn attach(ws: &WebSocket, id: u64, session: &StreamSession) {
    let _ = ws.serialize_attachment(Attachment {
        id,
        resume: session.resume(),
    });
}

impl Sockets {
    /// A new reader: accepted for hibernation, let in at once unless it
    /// owes an auth message.
    pub async fn accept(
        &self,
        state: &State,
        live: Arc<Live>,
        ws: WebSocket,
        filter: StreamFilter,
    ) -> worker::Result<()> {
        state.accept_web_socket(&ws);
        let id = self.next.get() + 1;
        self.next.set(id);
        let mut session = StreamSession::new(live, filter);
        if !session.awaits_auth() {
            let out = open(&mut session).await.map_err(worker::Error::RustError)?;
            if !send(&ws, out) {
                return Ok(());
            }
        }
        attach(&ws, id, &session);
        self.entries.borrow_mut().insert(
            id,
            Entry {
                ws,
                since: js_sys::Date::now(),
                session: Some(session),
            },
        );
        Ok(())
    }

    /// The socket's session id, rebuilding its entry from the attachment
    /// if this instance has not seen it (after a wake).
    fn adopt(&self, live: &Arc<Live>, ws: &WebSocket) -> Option<u64> {
        let a: Attachment = ws.deserialize_attachment().ok().flatten()?;
        let mut entries = self.entries.borrow_mut();
        if let std::collections::hash_map::Entry::Vacant(slot) = entries.entry(a.id) {
            self.next.set(self.next.get().max(a.id));
            let mut session = StreamSession::resumed(live.clone(), a.resume);
            // A packed reader's game streams start again at keyframes.
            let rejoin = session.rejoin();
            if !send(ws, rejoin.into_iter().collect()) {
                return None;
            }
            slot.insert(Entry {
                ws: ws.clone(),
                since: js_sys::Date::now(),
                session: Some(session),
            });
        }
        Some(a.id)
    }

    /// Every open socket this instance was handed before it slept.
    pub fn adopt_all(&self, live: &Arc<Live>, state: &State) {
        for ws in state.get_websockets() {
            self.adopt(live, &ws);
        }
    }

    /// A text message from a reader.
    pub async fn message(&self, live: &Arc<Live>, ws: WebSocket, text: &str) {
        let Some(id) = self.adopt(live, &ws) else {
            return;
        };
        let Some(mut session) = self.take(id) else {
            return;
        };
        let keep = match session.on_text(text) {
            Ok(true) => match open(&mut session).await {
                Ok(out) => send(&ws, out),
                Err(e) => send(&ws, vec![Out::Close(1011, e)]),
            },
            Ok(false) => true,
            Err(close) => send(&ws, vec![close]),
        };
        self.put(id, &ws, session, keep);
    }

    pub fn closed(&self, ws: &WebSocket) {
        if let Ok(Some(a)) = ws.deserialize_attachment::<Attachment>() {
            self.entries.borrow_mut().remove(&a.id);
        }
    }

    fn take(&self, id: u64) -> Option<StreamSession> {
        self.entries.borrow_mut().get_mut(&id)?.session.take()
    }

    fn put(&self, id: u64, ws: &WebSocket, session: StreamSession, keep: bool) {
        let mut entries = self.entries.borrow_mut();
        if !keep {
            entries.remove(&id);
            return;
        }
        attach(ws, id, &session);
        if let Some(e) = entries.get_mut(&id) {
            e.session = Some(session);
        }
    }

    /// Runs `f` on every open session, synchronously; drops the ones whose
    /// socket closed.
    fn each(&self, mut f: impl FnMut(&mut StreamSession) -> Vec<Out>, save: bool) {
        let mut entries = self.entries.borrow_mut();
        entries.retain(|id, e| {
            let Some(session) = e.session.as_mut() else {
                return true;
            };
            let out = f(session);
            if out.is_empty() {
                return true;
            }
            let keep = send(&e.ws, out);
            if keep && save {
                attach(&e.ws, *id, session);
            }
            keep
        });
    }

    /// Closes readers that owe an auth message and have not sent it in
    /// time.
    fn close_silent(&self) {
        let now = js_sys::Date::now();
        self.entries.borrow_mut().retain(|_, e| {
            let late = now - e.since >= AUTH_TIMEOUT_MS
                && e.session.as_ref().is_some_and(StreamSession::awaits_auth);
            if late {
                send(&e.ws, vec![StreamSession::auth_timed_out()]);
            }
            !late
        });
    }

    /// Every session's catch-up from the store, one at a time.
    async fn catch_up_all(&self) {
        let ids: Vec<u64> = self.entries.borrow().keys().copied().collect();
        for id in ids {
            let Some(mut session) = self.take(id) else {
                continue;
            };
            let Some(ws) = self.entries.borrow().get(&id).map(|e| e.ws.clone()) else {
                continue;
            };
            let keep = match settle(&mut session, Vec::new()).await {
                Ok(out) => send(&ws, out),
                Err(e) => send(&ws, vec![Out::Close(1011, e)]),
            };
            self.put(id, &ws, session, keep);
        }
    }
}

/// Fans the hub's feeds out to its sockets until the object goes away:
/// events, JSON frames, packed ticks, and once a second, expired tokens.
pub async fn pump(live: Arc<Live>, sockets: Rc<Sockets>) {
    use tokio::sync::broadcast::error::RecvError;
    let mut feed = live.feed();
    let mut frames = live.frames();
    let mut ticks = live.packed().subscribe_all();
    let mut every = live_server::rt::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            event = feed.recv() => match event {
                Ok(e) => sockets.each(|s| s.on_event(&e).into_iter().collect(), true),
                Err(RecvError::Lagged(_)) => sockets.catch_up_all().await,
                Err(RecvError::Closed) => return,
            },
            frame = frames.recv() => match frame {
                Ok(f) => sockets.each(|s| s.on_frame(&f).into_iter().collect(), false),
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return,
            },
            tick = ticks.recv() => match tick {
                Ok(t) => sockets.each(|s| s.on_tick(&t).into_iter().collect(), false),
                Err(RecvError::Lagged(_)) => sockets.each(|s| s.rejoin().into_iter().collect(), false),
                Err(RecvError::Closed) => return,
            },
            _ = every.tick() => {
                let now = live_server::app::now();
                sockets.each(|s| s.expired(now).into_iter().collect(), false);
                sockets.close_silent();
            }
        }
    }
}
