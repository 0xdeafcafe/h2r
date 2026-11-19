//! `GET /v1/stream` on the hub's hibernatable WebSockets. Each socket is
//! a `live_server::stream::StreamSession`, the same session axum's driver
//! runs on a machine; here one pump fans the hub's feeds out to all of
//! them, and the runtime hands in what readers send
//! (`websocket_message`).
//!
//! A socket's messages are handled one at a time and in order: its
//! session sits behind an async lock, since opening or catching up awaits
//! the store and the runtime may hand in the next message meanwhile. The
//! pump skips a session that is busy, and the session is caught up to the
//! log ([`settle`]) before it is let go.
//!
//! Nothing here runs on a timer: token expiry and the auth deadline are
//! checked from the hub's alarm ([`Sockets::check`], due at
//! [`Sockets::next_due_ms`]).
//!
//! Each socket carries its session's lasting state as its attachment
//! ([`Attachment`]), written when that state changes (let in, a new
//! token), not for every event. Its place in the log is not kept up to
//! date there: every open session is level with the log whenever the hub
//! is idle, which is the only time it hibernates, so a session rebuilt
//! after a wake starts at the log's head ([`Sockets::adopt`]).
//!
//! The code is generic over [`Socket`], so it runs and is tested on this
//! machine too; the Worker's is `worker::WebSocket`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use live_server::Live;
use live_server::stream::{Out, Resume, StreamFilter, StreamSession};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

/// How long a reader has to send its auth message (as on a machine).
const AUTH_TIMEOUT_MS: f64 = 10_000.0;

/// What a socket keeps across hibernation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attachment {
    pub id: u64,
    pub resume: Resume,
}

/// A WebSocket as the stream sockets use it.
pub trait Socket: Clone {
    /// `false`: it could not be sent (the socket is gone).
    fn send_text(&self, text: &str) -> bool;
    fn send_binary(&self, bytes: &[u8]) -> bool;
    fn close(&self, code: u16, reason: String);
    fn attach(&self, a: &Attachment);
    /// Its attachment, if it is a stream socket.
    fn attachment(&self) -> Option<Attachment>;
}

/// Milliseconds since the epoch.
pub fn now_ms() -> f64 {
    #[cfg(target_arch = "wasm32")]
    return js_sys::Date::now();
    #[cfg(not(target_arch = "wasm32"))]
    return std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0);
}

/// Sends a session's messages. `false`: it closed the socket.
pub fn send<S: Socket>(ws: &S, out: Vec<Out>) -> bool {
    for o in out {
        let sent = match o {
            Out::Text(t) => ws.send_text(&t),
            Out::Binary(b) => ws.send_binary(&b),
            Out::Close(code, reason) => {
                ws.close(code, reason);
                return false;
            }
        };
        if !sent {
            return false;
        }
    }
    true
}

/// Brings a session that was out of the pump's reach (busy across an
/// await) level with the log, so that once it is let go, with no await
/// between, the pump's next event is the next one it needs.
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

struct Entry<S> {
    ws: S,
    /// When this instance took the socket (ms since the epoch): a reader
    /// that owes an auth message has `AUTH_TIMEOUT` from then.
    since: f64,
    session: Rc<Mutex<StreamSession>>,
}

pub struct Sockets<S> {
    entries: RefCell<HashMap<u64, Entry<S>>>,
    next: Cell<u64>,
    /// Attachments written, for the measurements and the tests.
    attached: Cell<u64>,
}

impl<S> Default for Sockets<S> {
    fn default() -> Self {
        Sockets {
            entries: RefCell::new(HashMap::new()),
            next: Cell::new(0),
            attached: Cell::new(0),
        }
    }
}

impl<S: Socket> Sockets<S> {
    fn attach(&self, ws: &S, id: u64, session: &StreamSession) {
        self.attached.set(self.attached.get() + 1);
        ws.attach(&Attachment {
            id,
            resume: session.resume(),
        });
    }

    /// Attachments written so far.
    pub fn attached(&self) -> u64 {
        self.attached.get()
    }

    /// Open stream sockets.
    pub fn len(&self) -> usize {
        self.entries.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// A new reader (already accepted for hibernation by the caller): let
    /// in at once unless it owes an auth message.
    pub async fn accept(&self, live: Arc<Live>, ws: S, filter: StreamFilter) {
        let id = self.next.get() + 1;
        self.next.set(id);
        let session = Rc::new(Mutex::new(StreamSession::new(live, filter)));
        // In the map before any await, locked: a message that comes while
        // the reader is let in waits its turn.
        let held = session.clone();
        let mut guard = held.lock().await;
        self.entries.borrow_mut().insert(
            id,
            Entry {
                ws: ws.clone(),
                since: now_ms(),
                session,
            },
        );
        if !guard.awaits_auth() {
            let keep = match open(&mut guard).await {
                Ok(out) => send(&ws, out),
                Err(e) => send(&ws, vec![Out::Close(1011, e)]),
            };
            if !keep {
                self.entries.borrow_mut().remove(&id);
                return;
            }
        }
        self.attach(&ws, id, &guard);
    }

    /// The socket's session, rebuilt from its attachment if this instance
    /// has not seen it (after a wake). An open session starts at the log's
    /// head: it was level with the log when the hub went idle.
    pub fn adopt(&self, live: &Arc<Live>, ws: &S) -> Option<(u64, Rc<Mutex<StreamSession>>)> {
        let a = ws.attachment()?;
        let mut entries = self.entries.borrow_mut();
        if let Some(e) = entries.get(&a.id) {
            return Some((a.id, e.session.clone()));
        }
        self.next.set(self.next.get().max(a.id));
        let mut resume = a.resume;
        if let Some(last) = resume.last.as_mut() {
            *last = (*last).max(live.position());
        }
        let mut session = StreamSession::resumed(live.clone(), resume);
        // A packed reader's game streams start again at keyframes.
        let rejoin = session.rejoin();
        if !send(ws, rejoin.into_iter().collect()) {
            return None;
        }
        let session = Rc::new(Mutex::new(session));
        entries.insert(
            a.id,
            Entry {
                ws: ws.clone(),
                since: now_ms(),
                session: session.clone(),
            },
        );
        Some((a.id, session))
    }

    /// Every open socket this instance was handed before it slept.
    pub fn adopt_all(&self, live: &Arc<Live>, sockets: impl IntoIterator<Item = S>) {
        for ws in sockets {
            self.adopt(live, &ws);
        }
    }

    /// A text message from a reader. Messages from one socket are handled
    /// in the order they came, each after the last has finished.
    pub async fn message(&self, live: &Arc<Live>, ws: S, text: &str) {
        let Some((id, session)) = self.adopt(live, &ws) else {
            return;
        };
        let mut session = session.lock().await;
        let before = session.resume();
        let keep = match session.on_text(text) {
            Ok(true) => match open(&mut session).await {
                Ok(out) => send(&ws, out),
                Err(e) => send(&ws, vec![Out::Close(1011, e)]),
            },
            Ok(false) => true,
            Err(close) => send(&ws, vec![close]),
        };
        if !keep {
            self.entries.borrow_mut().remove(&id);
            return;
        }
        if session.resume() != before {
            self.attach(&ws, id, &session);
        }
    }

    pub fn closed(&self, ws: &S) {
        if let Some(a) = ws.attachment() {
            self.entries.borrow_mut().remove(&a.id);
        }
    }

    /// Runs `f` on every session not busy with a message, synchronously;
    /// drops the ones whose socket closed. A busy one is caught up before
    /// it is let go.
    fn each(&self, mut f: impl FnMut(&mut StreamSession) -> Vec<Out>) {
        let mut entries = self.entries.borrow_mut();
        entries.retain(|_, e| {
            let Ok(mut session) = e.session.try_lock() else {
                return true;
            };
            let out = f(&mut session);
            out.is_empty() || send(&e.ws, out)
        });
    }

    /// Closes readers that owe an auth message and have not sent it in
    /// time.
    fn close_silent(&self) {
        let now = now_ms();
        self.entries.borrow_mut().retain(|_, e| {
            let late = now - e.since >= AUTH_TIMEOUT_MS
                && e.session.try_lock().is_ok_and(|s| s.awaits_auth());
            if late {
                send(&e.ws, vec![StreamSession::auth_timed_out()]);
            }
            !late
        });
    }

    /// The timed checks, run from the hub's alarm: expired tokens, and
    /// readers that owe an auth message past the deadline.
    pub fn check(&self) {
        let now = live_server::app::now();
        self.each(|s| s.expired(now).into_iter().collect());
        self.close_silent();
    }

    /// When [`Sockets::check`] next has something to do (ms since the
    /// epoch), for the hub's alarm.
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

    /// Every session's catch-up from the store, one at a time.
    async fn catch_up_all(&self) {
        let all: Vec<_> = self
            .entries
            .borrow()
            .iter()
            .map(|(id, e)| (*id, e.ws.clone(), e.session.clone()))
            .collect();
        for (id, ws, session) in all {
            let mut session = session.lock().await;
            let keep = match settle(&mut session, Vec::new()).await {
                Ok(out) => send(&ws, out),
                Err(e) => send(&ws, vec![Out::Close(1011, e)]),
            };
            if !keep {
                self.entries.borrow_mut().remove(&id);
            }
        }
    }
}

/// Fans the hub's feeds out to its sockets until the object goes away:
/// events, JSON frames and packed ticks. It sets no timer (the timed
/// checks run from the hub's alarm), so it does not keep the object from
/// hibernating.
pub async fn pump<S: Socket>(live: Arc<Live>, sockets: Rc<Sockets<S>>) {
    use tokio::sync::broadcast::error::RecvError;
    let mut feed = live.feed();
    let mut frames = live.frames();
    let mut ticks = live.packed().subscribe_all();
    loop {
        tokio::select! {
            event = feed.recv() => match event {
                Ok(e) => sockets.each(|s| s.on_event(&e).into_iter().collect()),
                Err(RecvError::Lagged(_)) => sockets.catch_up_all().await,
                Err(RecvError::Closed) => return,
            },
            frame = frames.recv() => match frame {
                Ok(f) => sockets.each(|s| s.on_frame(&f).into_iter().collect()),
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return,
            },
            tick = ticks.recv() => match tick {
                Ok(t) => sockets.each(|s| s.on_tick(&t).into_iter().collect()),
                Err(RecvError::Lagged(_)) => sockets.each(|s| s.rejoin().into_iter().collect()),
                Err(RecvError::Closed) => return,
            },
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl Socket for worker::WebSocket {
    fn send_text(&self, text: &str) -> bool {
        self.send_with_str(text).is_ok()
    }
    fn send_binary(&self, bytes: &[u8]) -> bool {
        self.send_with_bytes(bytes).is_ok()
    }
    fn close(&self, code: u16, reason: String) {
        let _ = worker::WebSocket::close(self, Some(code), Some(reason));
    }
    fn attach(&self, a: &Attachment) {
        let _ = self.serialize_attachment(a);
    }
    fn attachment(&self) -> Option<Attachment> {
        self.deserialize_attachment().ok().flatten()
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use live_proto::sign::{AccountRequest, KeyPair, issue_token};
    use live_server::store::{
        EventStore, MemoryStore, NewEvent, Query, RecordedEvent, StoreError, Wake,
    };

    /// A memory store whose reads take a turn of the runtime, as a real
    /// store's do, so another message can arrive while one is being read.
    struct Slow(MemoryStore);

    #[async_trait]
    impl EventStore for Slow {
        async fn append(
            &self,
            events: Vec<NewEvent>,
            expect: &[(String, u64)],
        ) -> Result<Vec<RecordedEvent>, StoreError> {
            self.0.append(events, expect).await
        }
        async fn read(&self, query: &Query) -> Result<Vec<RecordedEvent>, StoreError> {
            tokio::task::yield_now().await;
            self.0.read(query).await
        }
        async fn head(&self) -> Result<u64, StoreError> {
            self.0.head().await
        }
        async fn put_blob(&self, sha256: &str, bytes: &[u8]) -> Result<(), StoreError> {
            self.0.put_blob(sha256, bytes).await
        }
        async fn blob(&self, sha256: &str) -> Result<Option<Vec<u8>>, StoreError> {
            self.0.blob(sha256).await
        }
        async fn load_checkpoint(&self, name: &str) -> Result<Option<u64>, StoreError> {
            self.0.load_checkpoint(name).await
        }
        async fn save_checkpoint(&self, name: &str, position: u64) -> Result<(), StoreError> {
            self.0.save_checkpoint(name, position).await
        }
        async fn wake(&self) -> Result<Box<dyn Wake>, StoreError> {
            self.0.wake().await
        }
        async fn blob_names(&self) -> Result<Vec<String>, StoreError> {
            self.0.blob_names().await
        }
        async fn checkpoints(&self) -> Result<Vec<(String, u64)>, StoreError> {
            self.0.checkpoints().await
        }
    }

    #[derive(Default)]
    struct Seen {
        sent: Vec<serde_json::Value>,
        closed: Option<u16>,
        attachment: Option<Attachment>,
    }

    #[derive(Clone, Default)]
    struct Fake(Rc<RefCell<Seen>>);

    impl Socket for Fake {
        fn send_text(&self, text: &str) -> bool {
            let mut s = self.0.borrow_mut();
            s.sent.push(serde_json::from_str(text).unwrap());
            s.closed.is_none()
        }
        fn send_binary(&self, _: &[u8]) -> bool {
            true
        }
        fn close(&self, code: u16, _: String) {
            self.0.borrow_mut().closed = Some(code);
        }
        fn attach(&self, a: &Attachment) {
            self.0.borrow_mut().attachment = Some(a.clone());
        }
        fn attachment(&self) -> Option<Attachment> {
            self.0.borrow().attachment.clone()
        }
    }

    impl Fake {
        fn kinds(&self) -> Vec<String> {
            let s = self.0.borrow();
            s.sent
                .iter()
                .map(|m| m["kind"].as_str().unwrap_or("?").to_string())
                .collect()
        }
    }

    struct Server {
        live: Arc<Live>,
        dir: std::path::PathBuf,
    }

    impl Drop for Server {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    async fn server(private: bool) -> Server {
        let dir = std::env::temp_dir().join(format!(
            "h2live-sockets-{}-{}",
            std::process::id(),
            live_proto::sign::random_id()
        ));
        let identity = live_server::ServerIdentity::open(&dir, Some("live.test")).unwrap();
        let policy = live_server::Policy {
            private,
            ..Default::default()
        };
        let live = Live::start(Arc::new(Slow(MemoryStore::new())), identity, policy)
            .await
            .unwrap();
        Server { live, dir }
    }

    /// An account's `read` token, and its key and author for more.
    async fn account(live: &Live, name: &str) -> (KeyPair, String) {
        let key = KeyPair::generate();
        let req = AccountRequest::sign(&key, name, None, "live.test", live_server::app::now());
        let created = live.create_account(&req).await.unwrap();
        (key, created.id)
    }

    fn auth(key: &KeyPair, author: &str, token: Option<&str>) -> String {
        let token = token.map_or_else(
            || issue_token(key, author, "live.test", "read", live_server::app::now()),
            str::to_string,
        );
        serde_json::json!({"kind": "auth", "token": token}).to_string()
    }

    /// Two messages close together: the second used to find the session
    /// taken out by the first (busy reading the store) and was lost.
    #[tokio::test]
    async fn a_sockets_messages_are_each_handled_in_order() {
        let s = server(true).await;
        let (key, author) = account(&s.live, "Alpha").await;
        let sockets = Sockets::<Fake>::default();
        let ws = Fake::default();
        sockets
            .accept(s.live.clone(), ws.clone(), StreamFilter::default())
            .await;
        assert!(ws.kinds().is_empty(), "a private stream waits for a token");

        let first = auth(&key, &author, None);
        let second = auth(&key, &author, Some("not-a-token"));
        tokio::join!(
            sockets.message(&s.live, ws.clone(), &first),
            sockets.message(&s.live, ws.clone(), &second),
        );
        // Let in and caught up, then the bad refresh closes it.
        assert_eq!(ws.kinds().last().map(String::as_str), Some("caught_up"));
        assert_eq!(
            ws.0.borrow().closed,
            Some(live_server::stream::CLOSE_UNAUTHORIZED)
        );
        assert!(sockets.is_empty());
    }

    /// A reader's attachment is written when it is let in, not per event;
    /// after a wake its session starts at the log's head.
    #[tokio::test]
    async fn attachments_are_written_on_change_and_a_wake_starts_at_the_head() {
        let s = server(false).await;
        let sockets = Rc::new(Sockets::<Fake>::default());
        let ws = Fake::default();
        sockets
            .accept(s.live.clone(), ws.clone(), StreamFilter { packed: true, v8: true, ..Default::default() })
            .await;
        assert_eq!(sockets.attached(), 1);
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                tokio::task::spawn_local(pump(s.live.clone(), sockets.clone()));
                for name in ["Alpha", "Bravo", "Charlie"] {
                    account(&s.live, name).await;
                }
                // The pump has had its turn once the events are applied.
                for _ in 0..10 {
                    tokio::task::yield_now().await;
                }
            })
            .await;
        let events = ws.kinds().iter().filter(|k| *k == "event").count();
        assert!(events >= 3, "{:?}", ws.kinds());
        assert_eq!(sockets.attached(), 1, "none for the events");
        let saved = ws.attachment().unwrap();
        assert!(saved.resume.last.unwrap() < s.live.position());

        // A new instance (after a wake) takes the socket at the head.
        let woke = Sockets::<Fake>::default();
        let (_, session) = woke.adopt(&s.live, &ws).unwrap();
        let session = session.lock().await;
        assert_eq!(session.resume().last, Some(s.live.position()));
        assert!(session.filter().packed && session.filter().v8);
        assert!(!session.behind());
    }
}
