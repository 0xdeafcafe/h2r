//! `GET /v1/ingest`: the game's WebSocket (docs/live/live-api.md, Streams). The
//! first message is `{kind: "auth", token}` or `{kind: "auth_batched", token}`
//! with `games.write` for accounts or `telemetry.ingest` for a leased host. It then accepts
//! `frame`, private training `sim_combat`, `events` (one batch JWS) and `ping` messages, and binary
//! messages of packed frames (docs/live/live-telemetry.md, Packed frames). Each `events`
//! batch is answered with `appended` or `error`; frames are answered only
//! when refused. The socket closes when the token expires unless the client
//! sends a fresh `auth`.
//!
//! [`IngestSession`] is the socket's logic with no socket under it, as
//! [`crate::stream::StreamSession`] is the stream's: axum's task drives it
//! here, and the Worker's `LiveHub` drives it on hibernatable WebSockets.

use std::sync::Arc;

use live_proto::sign::TOKEN_LEEWAY_S;
use live_proto::{ClientMessage, ErrorCode, IngestReply, MAX_INGEST_BATCH_FRAMES, pack};
use serde::{Deserialize, Serialize};

use crate::app::{ApiError, Live};
use crate::games::reply_error;
use crate::host_identity::{Publisher, TELEMETRY_INGEST}; // dh-identity
use crate::http::MAX_WS_MESSAGE;
use crate::stream::{CLOSE_UNAUTHORIZED, Out};

fn json(reply: &IngestReply) -> Out {
    Out::Text(serde_json::to_string(reply).expect("serializes").into())
}

/// What an ingest socket keeps between messages, as plain data: who it
/// writes as, and when its token ends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestResume {
    /// `None` until the auth message.
    pub author: Option<String>,
    pub expires: i64,
    #[serde(default)]
    pub batch_frames: bool,
}

pub struct IngestSession {
    live: Arc<Live>,
    /// Who the socket writes as, and its token's `exp`; `None` until the
    /// auth message.
    author: Option<(Publisher, i64)>,
    batch_frames: bool,
    /// The packed frames' decoder. Not kept across a wake: a resumed
    /// socket's frames are refused until the sender's next keyframe, as
    /// after any gap.
    decoder: pack::Decoder,
}

impl IngestSession {
    pub fn new(live: Arc<Live>) -> Self {
        IngestSession {
            live,
            author: None,
            batch_frames: false,
            decoder: pack::Decoder::new(),
        }
    }

    pub fn resumed(live: Arc<Live>, r: IngestResume) -> Self {
        let mut s = IngestSession::new(live);
        s.author = r
            .author
            .as_deref()
            .and_then(Publisher::from_attachment)
            .map(|a| (a, r.expires));
        s.batch_frames = r.batch_frames;
        s
    }

    pub fn resume(&self) -> IngestResume {
        IngestResume {
            author: self.author.as_ref().map(|(a, _)| a.to_string()),
            expires: self.author.as_ref().map_or(0, |(_, e)| *e),
            batch_frames: self.batch_frames,
        }
    }

    /// Whether the first message, the auth message, is still owed.
    pub fn awaits_auth(&self) -> bool {
        self.author.is_none()
    }

    /// When the token runs out (unix seconds, leeway included).
    pub fn deadline(&self) -> Option<i64> {
        self.author.as_ref().map(|(publisher, exp)| {
            if publisher.peer().is_some() {
                exp + TOKEN_LEEWAY_S
            } else {
                *exp
            }
        })
    }

    /// `Some` when the token has run out at `now`.
    pub fn expired(&self, now: i64) -> Option<Out> {
        self.deadline()
            .filter(|d| now >= *d)
            .map(|_| Out::Close(CLOSE_UNAUTHORIZED, "token expired".into()))
    }

    pub fn auth_timed_out() -> Out {
        Out::Close(CLOSE_UNAUTHORIZED, "send an auth message first".into())
    }

    /// A text message: the auth message, a fresh token, a frame, an
    /// events batch or a ping. What to send back; a close ends it.
    pub async fn on_text(&mut self, text: &str) -> Vec<Out> {
        if text.len() > MAX_WS_MESSAGE {
            return vec![Out::Close(1009, "message too large".into())];
        }
        if let Some(out) = self.expired(crate::app::now()) {
            return vec![out];
        }
        let message = serde_json::from_str::<ClientMessage>(text);
        if let Ok(ClientMessage::Auth { token } | ClientMessage::AuthBatched { token }) = &message {
            return match self.live.publisher(Some(token), TELEMETRY_INGEST) {
                Ok((author, _, exp)) => {
                    if self.author.as_ref().is_some_and(|(old, _)| *old != author) {
                        return vec![json(&reply_error(&crate::app::forbidden(
                            "a socket keeps its author",
                        )))];
                    }
                    self.author = Some((author, exp));
                    self.batch_frames = matches!(&message, Ok(ClientMessage::AuthBatched { .. }));
                    Vec::new()
                }
                Err(e) => vec![Out::Close(CLOSE_UNAUTHORIZED, e.message)],
            };
        }
        let Some((author, _)) = &self.author else {
            return vec![Self::auth_timed_out()];
        };
        let live = &self.live;
        let reply = match message {
            Ok(ClientMessage::Auth { .. } | ClientMessage::AuthBatched { .. }) => {
                unreachable!("handled above")
            }
            Ok(ClientMessage::Frame { frame }) => {
                live.meter(
                    author.xuid(),
                    crate::usage::metric::UP_FRAMES,
                    text.len() as u64,
                )
                .await;
                return live
                    .ingest_frame_from_socket(author, *frame, false, self.batch_frames)
                    .await
                    .err()
                    .map(|e| self.frame_error(&e))
                    .into_iter()
                    .collect();
            }
            Ok(ClientMessage::SimCombat { game_id, sample }) => {
                live.meter(author.xuid(), crate::usage::metric::UP_FRAMES, text.len() as u64).await;
                live.ingest_sim_combat(author, &game_id, *sample).err().map(Err)
            }
            Ok(ClientMessage::Events { game_id, jws }) => Some(
                live.append_publisher_batch(author, &game_id, &jws)
                    .await
                    .map(|position| IngestReply::Appended { game_id, position }),
            ),
            Ok(ClientMessage::Ping) => match author {
                Publisher::Peer(a) => live.ping_as(a).err().map(Err),
                Publisher::Host(subject) => {
                    live.model
                        .touch_service_sessions(subject, crate::app::now());
                    None
                }
            },
            Err(e) => Some(Err(ApiError::new(
                ErrorCode::Invalid,
                format!("message: {e}"),
            ))),
        };
        match reply {
            None => Vec::new(),
            Some(Ok(r)) => vec![json(&r)],
            Some(Err(e)) => vec![json(&reply_error(&e))],
        }
    }

    /// A binary message: packed frames (docs/live/live-telemetry.md, Packed
    /// frames), a container of this socket's stream, oldest first. Only
    /// refusals reply. A frame that does not decode (a gap after a refused
    /// or lost frame) is refused until the sender's next keyframe.
    pub async fn on_binary(&mut self, bytes: &[u8]) -> Vec<Out> {
        if bytes.len() > MAX_WS_MESSAGE {
            return vec![Out::Close(1009, "message too large".into())];
        }
        if let Some(out) = self.expired(crate::app::now()) {
            return vec![out];
        }
        let Some((author, _)) = &self.author else {
            return vec![Self::auth_timed_out()];
        };
        if self.batch_frames && pack::records(bytes).nth(MAX_INGEST_BATCH_FRAMES).is_some() {
            return vec![self.frame_error(&ApiError::new(
                ErrorCode::TooLarge,
                format!("frame: a batch carries at most {MAX_INGEST_BATCH_FRAMES} records"),
            ))];
        }
        self.live
            .meter(
                author.xuid(),
                crate::usage::metric::UP_FRAMES,
                bytes.len() as u64,
            )
            .await;
        let mut out = Vec::new();
        for record in pack::records(bytes) {
            let decoded = record.and_then(|r| self.decoder.decode(r));
            let result = match decoded {
                Ok(d) => self
                    .live
                    .ingest_frame_from_socket(author, d.frame, d.machine_baseline, self.batch_frames)
                    .await
                    .map(|_| ()),
                Err(e) => Err(ApiError::new(ErrorCode::Invalid, format!("frame: {e}"))),
            };
            if let Err(e) = result {
                out.push(self.frame_error(&e));
            }
        }
        out
    }

    fn frame_error(&self, e: &ApiError) -> Out {
        if self.batch_frames {
            json(&IngestReply::FrameError {
                error: e.code,
                message: e.message.clone(),
            })
        } else {
            json(&reply_error(e))
        }
    }
}

#[cfg(feature = "native")]
pub use native::ingest;

#[cfg(feature = "native")]
mod native {
    use std::sync::Arc;
    use std::time::Duration;

    use axum::extract::State;
    use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
    use axum::response::Response;
    use live_proto::ErrorCode;

    use super::IngestSession;
    use crate::app::{ApiError, Live, now};
    use crate::http::{AUTH_TIMEOUT, MAX_WS_MESSAGE, send_all};

    pub async fn ingest(
        State(live): State<Arc<Live>>,
        ws: Result<WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
    ) -> Result<Response, ApiError> {
        let ws = ws.map_err(|_| ApiError::new(ErrorCode::Invalid, "/v1/ingest is a WebSocket"))?;
        Ok(ws
            .max_message_size(MAX_WS_MESSAGE)
            .on_upgrade(move |socket| async move {
                if let Err(reason) = run(live, socket).await {
                    eprintln!("live: ingest closed: {reason}");
                }
            }))
    }

    async fn run(live: Arc<Live>, mut socket: WebSocket) -> Result<(), String> {
        let mut session = IngestSession::new(live);
        let first = tokio::time::timeout(AUTH_TIMEOUT, socket.recv()).await;
        let Ok(Some(Ok(Message::Text(text)))) = first else {
            send_all(&mut socket, vec![IngestSession::auth_timed_out()]).await?;
            return Ok(());
        };
        if !send_all(&mut socket, session.on_text(&text).await).await? {
            return Ok(());
        }
        loop {
            let deadline = session.deadline().unwrap_or(0);
            let wait = Duration::from_secs((deadline - now()).max(0) as u64);
            let msg = tokio::select! {
                m = socket.recv() => m,
                _ = tokio::time::sleep(wait) => {
                    let out = session.expired(now()).into_iter().collect();
                    send_all(&mut socket, out).await?;
                    return Ok(());
                }
            };
            let out = match msg {
                Some(Ok(Message::Text(text))) => session.on_text(&text).await,
                Some(Ok(Message::Binary(bytes))) => session.on_binary(&bytes).await,
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return Ok(()),
                Some(Ok(_)) => continue,
            };
            if !send_all(&mut socket, out).await? {
                return Ok(());
            }
        }
    }
}
