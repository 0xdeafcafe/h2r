//! The router harness shared by the integration tests.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use http_body_util::BodyExt;
use live_proto::sign::{self, AccountRequest, EventBody, KeyPair, issue_token};
use live_proto::{Xuid, lxm, streams, types};
use live_server::store::{EventStore, MemoryStore};
use live_server::{Live, Policy, ServerIdentity, http};
use serde_json::{Value, json};
use tower::ServiceExt;

pub fn temp_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "h2live-http-{}-{}",
        std::process::id(),
        sign::random_id()
    ))
}

pub fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

pub struct Harness {
    pub live: Arc<Live>,
    pub app: Router,
    /// A second copy of the server key: the operator.
    pub operator: KeyPair,
    pub id: String,
    pub dir: PathBuf,
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub struct Resp {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Resp {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|_| panic!("not JSON: {}", String::from_utf8_lossy(&self.body)))
    }
    pub fn error(&self) -> String {
        self.json()["error"].as_str().unwrap_or("").to_string()
    }
}

/// An account this test created, with its key.
pub struct User {
    pub key: KeyPair,
    pub xuid: Xuid,
    pub author: String,
}

pub async fn start(store: Arc<dyn EventStore>, policy: Policy) -> Harness {
    start_with(store, policy, live_server::app::Timers::Spawned).await
}

#[allow(dead_code)]
pub async fn start_driven(store: Arc<dyn EventStore>, policy: Policy) -> Harness {
    start_with(store, policy, live_server::app::Timers::Driven).await
}

pub async fn start_with(
    store: Arc<dyn EventStore>,
    policy: Policy,
    timers: live_server::app::Timers,
) -> Harness {
    let dir = temp_dir();
    let identity = ServerIdentity::open(&dir, Some("live.test")).unwrap();
    let live = Live::start_with(store, identity, policy, timers)
        .await
        .unwrap();
    Harness {
        app: http::router(live.clone()),
        live,
        operator: ServerIdentity::load(&dir).unwrap().key,
        id: "live.test".into(),
        dir,
    }
}

pub async fn harness() -> Harness {
    start(Arc::new(MemoryStore::new()), Policy::default()).await
}

impl Harness {
    pub async fn call(
        &self,
        method: Method,
        uri: &str,
        headers: &[(&str, String)],
        body: impl Into<Body>,
    ) -> Resp {
        let mut req = Request::builder().method(method).uri(uri);
        for (k, v) in headers {
            req = req.header(*k, v);
        }
        let resp = self
            .app
            .clone()
            .oneshot(req.body(body.into()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = resp
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec();
        Resp {
            status,
            headers,
            body,
        }
    }

    pub async fn get(&self, uri: &str) -> Resp {
        self.call(Method::GET, uri, &[], Body::empty()).await
    }

    pub fn token(&self, key: &KeyPair, iss: &str, method: &str) -> String {
        issue_token(key, iss, &self.id, method, now())
    }

    pub async fn create(&self, gamertag: &str) -> User {
        let key = KeyPair::generate();
        let r = self
            .call(
                Method::POST,
                "/v1/accounts",
                &[],
                AccountRequest::sign(&key, gamertag, None, "live.test", now()),
            )
            .await;
        assert_eq!(
            r.status,
            StatusCode::CREATED,
            "{}",
            String::from_utf8_lossy(&r.body)
        );
        let body = r.json();
        let xuid = Xuid::parse(body["xuid"].as_str().unwrap()).unwrap();
        User {
            key,
            xuid,
            author: body["id"].as_str().unwrap().into(),
        }
    }

    pub fn event(&self, user: &User, stream: &str, kind: &str, data: Value) -> String {
        EventBody::new(
            &user.key,
            &user.author,
            stream,
            kind,
            data,
            time::OffsetDateTime::now_utc(),
        )
        .sign(&user.key)
    }

    pub async fn post_event(&self, user: &User, jws: String) -> Resp {
        let auth = format!(
            "Bearer {}",
            self.token(&user.key, &user.author, lxm::EVENTS_WRITE)
        );
        self.call(Method::POST, "/v1/events", &[("authorization", auth)], jws)
            .await
    }

    pub async fn sign_in(&self, user: &User) -> Resp {
        let jws = self.event(
            user,
            &streams::presence(user.xuid),
            types::SIGNED_IN,
            json!({"xuid": user.xuid, "state": "online"}),
        );
        self.post_event(user, jws).await
    }
}

impl Harness {
    pub fn bearer(&self, user: &User, method: &str) -> (&'static str, String) {
        (
            "authorization",
            format!("Bearer {}", self.token(&user.key, &user.author, method)),
        )
    }

    /// `POST /v1/games` as `user`; the new game's id.
    pub async fn start_game(&self, user: &User, game: Value) -> String {
        let r = self
            .call(
                Method::POST,
                "/v1/games",
                &[self.bearer(user, lxm::GAMES_WRITE)],
                game.to_string(),
            )
            .await;
        assert_eq!(
            r.status,
            StatusCode::CREATED,
            "{}",
            String::from_utf8_lossy(&r.body)
        );
        r.json()["game_id"].as_str().unwrap().to_string()
    }

    /// A batch JWS of `events` on `game_id` by `user`.
    pub fn batch(&self, user: &User, game_id: &str, events: &[(&str, Value)]) -> String {
        let at = time::OffsetDateTime::now_utc();
        let bodies: Vec<EventBody> = events
            .iter()
            .map(|(kind, data)| {
                EventBody::new(
                    &user.key,
                    &user.author,
                    &format!("game-{game_id}"),
                    kind,
                    data.clone(),
                    at,
                )
            })
            .collect();
        sign::sign_batch(&user.key, &bodies)
    }

    pub async fn post_batch(&self, user: &User, game_id: &str, jws: String) -> Resp {
        let uri = format!("/v1/games/{game_id}/events");
        self.call(
            Method::POST,
            &uri,
            &[self.bearer(user, lxm::GAMES_WRITE)],
            jws,
        )
        .await
    }

    pub async fn put_outline(&self, user: &User, map: &str, outline: &Value) -> Resp {
        let uri = format!("/v1/maps/{map}/outline");
        self.call(
            Method::PUT,
            &uri,
            &[self.bearer(user, lxm::GAMES_WRITE)],
            outline.to_string(),
        )
        .await
    }

    pub async fn ping(&self, user: &User) -> Resp {
        self.call(
            Method::POST,
            "/v1/presence/ping",
            &[self.bearer(user, lxm::EVENTS_WRITE)],
            Body::empty(),
        )
        .await
    }

    /// `POST /v1/presence/ping/batch` with each user's `events.write`
    /// token, in order.
    pub async fn ping_batch(&self, users: &[&User]) -> Resp {
        let tokens: Vec<String> = users
            .iter()
            .map(|u| self.token(&u.key, &u.author, lxm::EVENTS_WRITE))
            .collect();
        self.call(
            Method::POST,
            "/v1/presence/ping/batch",
            &[],
            json!({"tokens": tokens}).to_string(),
        )
        .await
    }

    /// `POST /v1/presence/ping/batch` with raw tokens (bad ones included).
    pub async fn ping_batch_raw(&self, body: Value) -> Resp {
        self.call(
            Method::POST,
            "/v1/presence/ping/batch",
            &[],
            body.to_string(),
        )
        .await
    }
}

/// A two-player team game; player 0 is `host`'s account.
pub fn two_player_game(host: &User) -> Value {
    json!({
        "console": "Den", "map": "lockout", "mode": "multiplayer", "variant": "Team Slayer",
        "team_play": true, "score_limit": 50, "tick": 0,
        "players": [
            {"index": 0, "name": "Host", "xuid": host.xuid, "team": 0, "local": true},
            {"index": 1, "name": "Guest", "team": 1}
        ]
    })
}

/// Every event on `/v1/stream` from the start, as `auth` (a read token).
pub async fn streamed(addr: std::net::SocketAddr, token: Option<String>) -> Vec<Value> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let q = if token.is_some() {
        "?from=0&auth=1"
    } else {
        "?from=0"
    };
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/v1/stream{q}"))
        .await
        .unwrap();
    if let Some(t) = token {
        ws.send(Message::Text(
            json!({"kind": "auth", "token": t}).to_string().into(),
        ))
        .await
        .unwrap();
    }
    let mut out = Vec::new();
    loop {
        let msg = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
            .await
            .expect("a message in time")
            .unwrap()
            .unwrap();
        let Message::Text(t) = msg else { continue };
        let v: Value = serde_json::from_str(&t).unwrap();
        match v["kind"].as_str() {
            Some("event") => out.push(v["event"].clone()),
            Some("caught_up") => return out,
            _ => {}
        }
    }
}
