//! `GET /v1/ingest`: the game's WebSocket (docs/live-api.md, Streams). The
//! first message is `{kind: "auth", token}` with `lxm: games.write`; then
//! `frame`, `events` (one batch JWS) and `ping` messages, and binary
//! messages of packed frames (docs/live-telemetry.md, Packed frames). Each `events`
//! batch is answered with `appended` or `error`; frames are answered only
//! when refused. The socket closes when the token expires unless the client
//! sends a fresh `auth`.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use live_proto::sign::{TOKEN_LEEWAY_S, Token};
use live_proto::{Author, ClientMessage, ErrorCode, IngestReply, lxm, pack};

use crate::app::{ApiError, Live, now};
use crate::games::reply_error;
use crate::http::{
    AUTH_TIMEOUT, CLOSE_UNAUTHORIZED, MAX_WS_MESSAGE, auth_message, close, send_json,
};

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

fn author_of(token_text: &str) -> Option<Author> {
    let ClientMessage::Auth { token } = serde_json::from_str(token_text).ok()? else {
        return None;
    };
    Author::parse(&Token::parse(&token).ok()?.claims.iss)
}

async fn run(live: Arc<Live>, mut socket: WebSocket) -> Result<(), String> {
    let first = tokio::time::timeout(AUTH_TIMEOUT, socket.recv()).await;
    let Ok(Some(Ok(Message::Text(text)))) = first else {
        close(
            &mut socket,
            CLOSE_UNAUTHORIZED,
            "send an auth message first",
        )
        .await;
        return Ok(());
    };
    let (mut expires, author) = match auth_message(&live, &text, lxm::GAMES_WRITE) {
        Ok(exp) => (exp, author_of(&text).expect("authenticated")),
        Err(e) => {
            close(&mut socket, CLOSE_UNAUTHORIZED, &e.message).await;
            return Ok(());
        }
    };
    let mut decoder = pack::Decoder::new();
    loop {
        let wait = Duration::from_secs((expires + TOKEN_LEEWAY_S - now()).max(0) as u64);
        let msg = tokio::select! {
            m = socket.recv() => m,
            _ = tokio::time::sleep(wait) => {
                close(&mut socket, CLOSE_UNAUTHORIZED, "token expired").await;
                return Ok(());
            }
        };
        let text = match msg {
            Some(Ok(Message::Text(text))) => text,
            Some(Ok(Message::Binary(bytes))) => {
                // Packed frames (docs/live-telemetry.md, Packed frames): a
                // container of this socket's stream. Only refusals reply.
                for reply in ingest_packed(&live, &author, &mut decoder, &bytes).await {
                    send_json(&mut socket, &reply_error(&reply))
                        .await
                        .map_err(|e| e.to_string())?;
                }
                continue;
            }
            Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return Ok(()),
            Some(Ok(_)) => continue,
        };
        let reply = match serde_json::from_str::<ClientMessage>(&text) {
            Ok(ClientMessage::Auth { .. }) => match auth_message(&live, &text, lxm::GAMES_WRITE) {
                // A fresh token must be the same author's.
                Ok(exp) if author_of(&text).as_ref() == Some(&author) => {
                    expires = exp;
                    None
                }
                Ok(_) => Some(Err(ApiError::new(
                    ErrorCode::Forbidden,
                    "a socket keeps its author",
                ))),
                Err(e) => {
                    close(&mut socket, CLOSE_UNAUTHORIZED, &e.message).await;
                    return Ok(());
                }
            },
            Ok(ClientMessage::Frame { frame }) => {
                live.ingest_frame(&author, *frame).await.err().map(Err)
            }
            Ok(ClientMessage::Events { game_id, jws }) => Some(
                live.append_game_batch(&author, &game_id, &jws)
                    .await
                    .map(|position| IngestReply::Appended { game_id, position }),
            ),
            Ok(ClientMessage::Ping) => live.ping_as(&author).err().map(Err),
            Err(e) => Some(Err(ApiError::new(
                ErrorCode::Invalid,
                format!("message: {e}"),
            ))),
        };
        let reply = match reply {
            None => continue,
            Some(Ok(r)) => r,
            Some(Err(e)) => reply_error(&e),
        };
        send_json(&mut socket, &reply)
            .await
            .map_err(|e| e.to_string())?;
    }
}

/// Decodes and ingests a binary message's packed frames, oldest first;
/// returns the refusals. A frame that does not decode (a gap after a
/// refused or lost frame) is refused until the sender's next keyframe.
async fn ingest_packed(
    live: &Live,
    author: &Author,
    decoder: &mut pack::Decoder,
    bytes: &[u8],
) -> Vec<ApiError> {
    let mut refusals = Vec::new();
    for record in pack::records(bytes) {
        let decoded = record.and_then(|r| decoder.decode(r));
        let result = match decoded {
            Ok(d) => live.ingest_frame(author, d.frame).await.map(|_| ()),
            Err(e) => Err(ApiError::new(ErrorCode::Invalid, format!("frame: {e}"))),
        };
        if let Err(e) = result {
            refusals.push(e);
        }
    }
    refusals
}
