//! `GET /v1/stream` as a transport-neutral session (docs/live/live-api.md,
//! Streams): what to send a reader for each thing that happens, whatever
//! carries the messages. Two drivers carry them:
//! - axum's WebSocket on a machine (`http::run_stream`), one task a socket;
//! - the Worker's hibernatable WebSockets in the `LiveHub` Durable Object
//!   (crates/live/live-worker), which fans the hub's feeds out to every session.
//!
//! A session's lasting state ([`Resume`]: the filter, the reader's place,
//! who reads) is plain data, so a Durable Object can keep it on the socket
//! and rebuild the session after it wakes.

use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use live_proto::sign::TOKEN_LEEWAY_S;
use live_proto::{ClientMessage, ErrorCode, StreamMessage};
use serde::{Deserialize, Serialize};

use crate::app::{ApiError, Live};
use crate::games::{LiveFrame, WsText};
use crate::packed::{Follow, GameBatch, Tick};
use crate::sessions::Viewer;
use crate::store::{Query, RecordedEvent};

/// WebSocket close code for a missing, bad or expired token.
pub const CLOSE_UNAUTHORIZED: u16 = 4401;

/// Events a catch-up reads at a time.
const PAGE: usize = 500;

/// What the reader asked for in the query string.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamFilter {
    /// Replay from after this position (default: the head).
    pub from: Option<u64>,
    /// `?stream=`: events of one stream only.
    pub stream: Option<String>,
    pub types: Vec<String>,
    pub exclude: Vec<String>,
    /// `?games=`: live frames only for these games (default: all).
    pub games: Vec<String>,
    /// `?frames=packed`: live frames as packed binary messages
    /// (docs/live/live-telemetry.md, Packed frames); JSON otherwise.
    pub packed: bool,
    #[serde(default)]
    pub v8: bool,
    /// `?auth=1`: the first message is a `read` token, on a public server
    /// too, so the reader sees what that account may (a private server
    /// always asks for one).
    pub auth: bool,
}

fn invalid(m: &str) -> ApiError {
    ApiError::new(ErrorCode::Invalid, m)
}

/// A comma-separated list from the first of `keys` present.
fn list(p: &HashMap<String, String>, keys: &[&str]) -> Vec<String> {
    keys.iter()
        .find_map(|k| p.get(*k))
        .map(|v| {
            v.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

impl StreamFilter {
    /// From the query string's parameters.
    pub fn from_params(p: &HashMap<String, String>) -> Result<Self, ApiError> {
        let from = p
            .get("from")
            .filter(|v| !v.is_empty())
            .map(|v| {
                v.parse::<u64>()
                    .map_err(|_| invalid("from must be a position"))
            })
            .transpose()?;
        Ok(StreamFilter {
            from,
            stream: p.get("stream").filter(|s| !s.is_empty()).cloned(),
            types: list(p, &["types", "type"]),
            exclude: list(p, &["exclude"]),
            games: list(p, &["games"]),
            packed: match p.get("frames").map(String::as_str) {
                None | Some("") | Some("json") => false,
                Some("packed") => true,
                Some(_) => return Err(invalid("frames must be json or packed")),
            },
            v8: match p.get("frame_version").map(String::as_str) {
                None | Some("7") => false,
                Some("8") => true,
                Some(_) => return Err(invalid("frame_version must be 7 or 8")),
            },
            auth: matches!(p.get("auth").map(String::as_str), Some("1" | "true")),
        })
    }

    pub fn query(&self, after: u64, limit: usize) -> Query {
        Query {
            after,
            limit,
            stream: self.stream.clone(),
            types: self.types.clone(),
            exclude: self.exclude.clone(),
        }
    }

    fn shows_game(&self, game_id: &str) -> bool {
        if crate::sim_runs::is_sim_id(game_id) {
            return self.games.iter().any(|g| g == game_id);
        }
        self.games.is_empty() || self.games.iter().any(|g| g == game_id)
    }
}

/// A message for the reader.
#[derive(Debug, Clone, PartialEq)]
pub enum Out {
    Text(WsText),
    Binary(Bytes),
    /// Close the socket with this code and reason; nothing follows.
    Close(u16, String),
}

impl Out {
    fn json(msg: &StreamMessage) -> Out {
        let text = serde_json::to_string(msg).expect("serializes");
        Out::Text(text.into())
    }
}

/// What a session keeps between messages, as plain data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resume {
    pub filter: StreamFilter,
    /// The last position the reader has been brought past (`None`: still
    /// waiting for its auth message, so nothing replayed yet).
    pub last: Option<u64>,
    pub viewer: Viewer,
    /// A private server's token end (unix seconds): the socket closes then
    /// unless a fresh token comes.
    pub expires: Option<i64>,
}

pub struct StreamSession {
    live: Arc<Live>,
    filter: StreamFilter,
    probe: Query,
    viewer: Viewer,
    expires: Option<i64>,
    /// `None` until the reader is let in (after its auth message, when one
    /// is due) and caught up.
    last: Option<u64>,
    follow: Follow,
    registered: bool,
}

impl Drop for StreamSession {
    fn drop(&mut self) {
        if self.registered {
            self.live
                .packed()
                .reader(&self.filter.games, self.filter.v8, false);
        }
    }
}

impl StreamSession {
    pub fn new(live: Arc<Live>, filter: StreamFilter) -> Self {
        let probe = filter.query(0, 1);
        StreamSession {
            live,
            filter,
            probe,
            viewer: Viewer::Anonymous,
            expires: None,
            last: None,
            follow: Follow::default(),
            registered: false,
        }
    }

    /// A session as it was ([`StreamSession::resume`]). Packed readers
    /// pick their game streams up again from the last keyframes
    /// ([`StreamSession::rejoin`]).
    pub fn resumed(live: Arc<Live>, r: Resume) -> Self {
        let mut s = StreamSession::new(live, r.filter);
        s.viewer = r.viewer;
        s.expires = r.expires;
        s.last = r.last;
        s
    }

    pub fn resume(&self) -> Resume {
        Resume {
            filter: self.filter.clone(),
            last: self.last,
            viewer: self.viewer,
            expires: self.expires,
        }
    }

    pub fn filter(&self) -> &StreamFilter {
        &self.filter
    }

    /// Whether the first message must be a token before anything is sent.
    pub fn awaits_auth(&self) -> bool {
        self.last.is_none() && (self.live.policy().private || self.filter.auth)
    }

    /// Whether the reader is let in and caught up: it hears the feeds.
    pub fn is_open(&self) -> bool {
        self.last.is_some()
    }

    /// Whether the log has moved past where an open reader was brought
    /// (events appended while it was being caught up).
    pub fn behind(&self) -> bool {
        self.last.is_some_and(|last| last < self.live.position())
    }

    /// When a private stream's token runs out (unix seconds, leeway
    /// included); `None` for a public one.
    pub fn deadline(&self) -> Option<i64> {
        self.expires.map(|exp| exp + TOKEN_LEEWAY_S)
    }

    /// Lets the reader in: replays from its `from` (default: the head) and
    /// says `caught_up`; a packed reader also gets its games' frames since
    /// their keyframes. Call once [`StreamSession::awaits_auth`] is false,
    /// after subscribing to the feeds (so nothing falls between).
    pub async fn open(&mut self) -> Result<Vec<Out>, String> {
        let mut last = self.filter.from.unwrap_or_else(|| self.live.position());
        let mut out = Vec::new();
        self.read_after(&mut last, &mut out).await?;
        self.last = Some(last);
        out.push(Out::json(&StreamMessage::CaughtUp { position: last }));
        if self.filter.packed {
            out.extend(self.rejoin());
        }
        Ok(out)
    }

    fn sees_event(&self, e: &RecordedEvent) -> bool {
        !crate::sim_runs::is_sim_stream(&e.body.stream)
            || self.filter.stream.as_deref() == Some(e.body.stream.as_str())
    }

    /// Every stored event after `last` that the filter keeps, then `last`
    /// moved to at least the position the read model had reached.
    async fn read_after(&self, last: &mut u64, out: &mut Vec<Out>) -> Result<(), String> {
        let target = self.live.position();
        loop {
            let batch = self
                .live
                .store()
                .read(&self.filter.query(*last, PAGE))
                .await
                .map_err(|e| e.to_string())?;
            for e in &batch {
                // A hidden event is skipped, but the reader's place moves
                // past it.
                if self.sees_event(e)
                    && let Some(w) = self.live.wire_for(e, self.viewer)
                {
                    out.push(Out::json(&StreamMessage::Event { event: Box::new(w) }));
                }
                *last = e.position;
            }
            if batch.len() < PAGE {
                *last = (*last).max(target);
                return Ok(());
            }
        }
    }

    /// After the reader fell behind the feed: what it missed, from the
    /// store.
    pub async fn catch_up(&mut self) -> Result<Vec<Out>, String> {
        let Some(mut last) = self.last else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        self.read_after(&mut last, &mut out).await?;
        self.last = Some(last);
        Ok(out)
    }

    /// A text message from the reader: its auth message, or on a private
    /// stream a fresh token. `Ok(true)`: the reader was just let in, so
    /// call [`StreamSession::open`]. `Err`: close with it.
    pub fn on_text(&mut self, text: &str) -> Result<bool, Out> {
        let waiting = self.awaits_auth();
        if !waiting && self.expires.is_none() {
            // Public streams ignore what clients send.
            return Ok(false);
        }
        let parsed = match serde_json::from_str::<ClientMessage>(text) {
            Ok(ClientMessage::Auth { token }) => self.live.read_viewer_until(&token),
            _ => Err(ApiError::new(
                ErrorCode::Unauthorized,
                "expected {\"kind\":\"auth\",\"token\":...}",
            )),
        };
        match parsed {
            Ok((viewer, exp)) => {
                self.viewer = viewer;
                // A public server's reader keeps the socket past its token;
                // a private one must send fresh tokens.
                self.expires = (self.live.policy().private
                    || self
                        .filter
                        .games
                        .iter()
                        .any(|id| crate::sim_runs::is_sim_id(id))
                    || self
                        .filter
                        .stream
                        .as_deref()
                        .is_some_and(crate::sim_runs::is_sim_stream))
                .then_some(exp);
                Ok(waiting)
            }
            Err(e) => Err(Out::Close(CLOSE_UNAUTHORIZED, e.message)),
        }
    }

    /// The close for a reader that never sent its auth message in time.
    pub fn auth_timed_out() -> Out {
        Out::Close(CLOSE_UNAUTHORIZED, "send an auth message first".into())
    }

    /// `Some` when a private stream's token has run out at `now`.
    pub fn expired(&self, now: i64) -> Option<Out> {
        self.deadline()
            .filter(|d| now >= *d)
            .map(|_| Out::Close(CLOSE_UNAUTHORIZED, "token expired".into()))
    }

    /// A new event from the feed.
    pub fn on_event(&mut self, e: &RecordedEvent) -> Option<Out> {
        let last = self.last.as_mut()?;
        if e.position <= *last {
            return None;
        }
        *last = e.position;
        (self.probe.matches(e) && self.sees_event(e))
            .then(|| self.live.wire_for(e, self.viewer))
            .flatten()
            .map(|w| Out::json(&StreamMessage::Event { event: Box::new(w) }))
    }

    /// A live frame, for a JSON reader.
    pub fn on_frame(&self, f: &LiveFrame) -> Option<Out> {
        (self.is_open()
            && !self.filter.packed
            && self.filter.shows_game(&f.frame.game_id)
            && self.live.frame_shows(f, self.viewer))
        .then(|| Out::Text(f.json_message()))
    }

    /// A tick of every game's packed frames, for a packed reader.
    pub fn on_tick(&mut self, tick: &Tick) -> Option<Out> {
        if !self.is_open() || !self.filter.packed {
            return None;
        }
        if tick
            .games
            .first()
            .is_none_or(|b| (b.version == 8) != self.filter.v8)
        {
            return None;
        }
        let ids = tick.games.iter().map(|b| b.game_id.as_str());
        let shown: Vec<bool> = self
            .live
            .games_show(ids, self.viewer)
            .into_iter()
            .zip(&tick.games)
            .map(|(shows, b)| shows && self.filter.shows_game(&b.game_id))
            .collect();
        self.follow.take_tick(tick, &shown).map(Out::Binary)
    }

    /// One game's packed batch, for a packed reader of that game.
    pub fn on_game_batch(&mut self, b: &GameBatch) -> Option<Out> {
        if !self.is_open()
            || !self.filter.packed
            || (b.version == 8) != self.filter.v8
            || !self.filter.shows_game(&b.game_id)
        {
            return None;
        }
        if self.live.games_show([b.game_id.as_str()], self.viewer)[0] {
            self.follow.take_batch(b).map(Out::Binary)
        } else {
            None
        }
    }

    /// A packed reader's games' frames since their last keyframes that it
    /// does not have yet, in one message: on opening, and after it fell
    /// behind (a slow reader skips some, and picks its streams up again).
    pub fn rejoin(&mut self) -> Option<Out> {
        if !self.is_open() || !self.filter.packed {
            return None;
        }
        if !self.registered {
            self.live
                .packed()
                .reader(&self.filter.games, self.filter.v8, true);
            self.registered = true;
        }
        let mut out = Vec::new();
        self.live.join_packed(
            &self.filter.games,
            self.filter.v8,
            self.viewer,
            &mut self.follow,
            &mut out,
        );
        (!out.is_empty()).then(|| Out::Binary(out.into()))
    }
}

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;
    use live_proto::sign::{AccountRequest, KeyPair};

    async fn live(private: bool) -> (Arc<Live>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "h2live-stream-{}-{}",
            std::process::id(),
            live_proto::sign::random_id()
        ));
        let identity = crate::ServerIdentity::open(&dir, Some("live.test")).unwrap();
        let policy = crate::Policy {
            private,
            ..crate::Policy::default()
        };
        let live = Live::start(Arc::new(crate::store::MemoryStore::new()), identity, policy)
            .await
            .unwrap();
        (live, dir)
    }

    async fn sign_up(live: &Live, name: &str) {
        let req = AccountRequest::sign(
            &KeyPair::generate(),
            name,
            None,
            "live.test",
            crate::app::now(),
        );
        live.create_account(&req).await.unwrap();
    }

    fn text(o: &Out) -> serde_json::Value {
        match o {
            Out::Text(t) => serde_json::from_str(t).unwrap(),
            other => panic!("not text: {other:?}"),
        }
    }

    fn params(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn filters_read_the_query_string() {
        let f = StreamFilter::from_params(&params(&[
            ("from", "12"),
            ("type", "a, b,"),
            ("exclude", "c"),
            ("games", "g1"),
            ("frames", "packed"),
            ("auth", "1"),
        ]))
        .unwrap();
        assert_eq!(f.from, Some(12));
        assert_eq!(f.types, ["a", "b"]);
        assert_eq!(f.exclude, ["c"]);
        assert_eq!(f.games, ["g1"]);
        assert!(f.packed && f.auth && !f.v8);
        let v8 = StreamFilter::from_params(&params(&[("frames", "packed"), ("frame_version", "8")])).unwrap();
        assert!(v8.packed && v8.v8);
        let mut attachment = serde_json::to_value(&f).unwrap();
        attachment.as_object_mut().unwrap().remove("v8");
        assert_eq!(serde_json::from_value::<StreamFilter>(attachment).unwrap(), f);
        assert!(StreamFilter::from_params(&params(&[("frame_version", "9")])).is_err());
        assert_eq!(
            StreamFilter::from_params(&params(&[])).unwrap(),
            StreamFilter::default()
        );
        assert!(StreamFilter::from_params(&params(&[("from", "x")])).is_err());
        assert!(StreamFilter::from_params(&params(&[("frames", "xml")])).is_err());
    }

    #[tokio::test]
    async fn a_session_replays_follows_and_resumes_where_it_was() {
        let (live, dir) = live(false).await;
        sign_up(&live, "Alpha").await;
        let head = live.position();
        assert!(head > 0);

        // From the start: the log so far, then caught_up at the head.
        let filter = StreamFilter {
            from: Some(0),
            ..StreamFilter::default()
        };
        let mut s = StreamSession::new(live.clone(), filter);
        assert!(!s.awaits_auth() && !s.is_open());
        let out = s.open().await.unwrap();
        let last = text(out.last().unwrap());
        assert_eq!(last["kind"], "caught_up");
        assert_eq!(last["position"], head);
        assert!(out.len() > 1, "replayed the sign-up");
        assert!(!s.behind());

        // New events from the feed, each once.
        let mut feed = live.feed();
        sign_up(&live, "Bravo").await;
        assert!(s.behind());
        let e = feed.recv().await.unwrap();
        let sent = s.on_event(&e).expect("an event");
        assert_eq!(text(&sent)["kind"], "event");
        assert_eq!(s.on_event(&e), None, "not twice");

        // Its place survives as plain data (a socket attachment).
        let saved: Resume =
            serde_json::from_str(&serde_json::to_string(&s.resume()).unwrap()).unwrap();
        assert_eq!(saved, s.resume());
        assert_eq!(saved.last, Some(e.position));
        let mut back = StreamSession::resumed(live.clone(), saved);
        assert!(back.is_open());
        assert_eq!(back.on_event(&e), None, "already sent before the wake");
        // What it missed while it was not following comes from the store.
        while let Ok(e) = feed.try_recv() {
            let _ = s.on_event(&e);
        }
        let before = back.resume().last.unwrap();
        sign_up(&live, "Charlie").await;
        let missed = back.catch_up().await.unwrap();
        assert!(!missed.is_empty());
        assert!(back.resume().last.unwrap() > before);
        assert!(!back.behind());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_private_session_waits_for_its_token() {
        let (live, dir) = live(true).await;
        sign_up(&live, "Alpha").await;
        let mut s = StreamSession::new(live.clone(), StreamFilter::default());
        assert!(s.awaits_auth());
        // Nothing reaches a reader before it is let in.
        let mut feed = live.feed();
        sign_up(&live, "Bravo").await;
        let e = feed.recv().await.unwrap();
        assert_eq!(s.on_event(&e), None);
        assert_eq!(s.rejoin(), None);
        assert_eq!(s.expired(i64::MAX), None, "no token yet");

        // Still waiting after a wake.
        let back = StreamSession::resumed(live.clone(), s.resume());
        assert!(back.awaits_auth());

        match s.on_text("{\"kind\":\"hello\"}") {
            Err(Out::Close(code, _)) => assert_eq!(code, CLOSE_UNAUTHORIZED),
            other => panic!("expected a close, got {other:?}"),
        }
        match s.on_text("{\"kind\":\"auth\",\"token\":\"not-a-token\"}") {
            Err(Out::Close(code, _)) => assert_eq!(code, CLOSE_UNAUTHORIZED),
            other => panic!("expected a close, got {other:?}"),
        }
        assert_eq!(
            StreamSession::auth_timed_out(),
            Out::Close(CLOSE_UNAUTHORIZED, "send an auth message first".into())
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_private_token_runs_out() {
        let (live, dir) = live(true).await;
        let s = StreamSession::resumed(
            live,
            Resume {
                filter: StreamFilter::default(),
                last: Some(0),
                viewer: Viewer::Anonymous,
                expires: Some(1_000),
            },
        );
        assert_eq!(s.deadline(), Some(1_000 + TOKEN_LEEWAY_S));
        assert_eq!(s.expired(1_000), None, "the leeway");
        assert!(matches!(
            s.expired(1_000 + TOKEN_LEEWAY_S),
            Some(Out::Close(CLOSE_UNAUTHORIZED, _))
        ));
        let _ = std::fs::remove_dir_all(dir);
    }
}
