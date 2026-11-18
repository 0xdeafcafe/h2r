//! `GET /v1/sessions/{id}/relay` on the Worker: one `CoopRelay` Durable
//! Object per co-op session, so a session's datagrams never pass through
//! the hub (docs/live/live-worker.md, The co-op relay).
//!
//! The client's protocol is the machine's (`live_server::relay`): its
//! first message is `{kind: "auth", token}`. The relay hands that token to
//! the hub (`POST /v1/sessions/{id}/relay/ticket`), which checks it and
//! the account's membership and signs a short ticket; the relay checks the
//! ticket with the server's public key and lets the socket in
//! ([`crate::relayroom`]). A ticket lasts `TICKET_S`; when a member's has
//! run out, the relay renews it with the hub in the background, so
//! membership is checked again that often without holding up datagrams.
//!
//! Sockets are hibernatable: the ticket is kept as the socket's
//! attachment, and the room is rebuilt from them after a wake. The token's
//! end and the auth deadline are checked from the object's alarm.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use live_proto::ClientMessage;
use live_proto::sign::PublicKey;
use live_server::relay::{CLOSE_FORBIDDEN, Ticket, TicketReply};
use live_server::stream::CLOSE_UNAUTHORIZED;
use wasm_bindgen::JsValue;
use worker::{
    DurableObject, Env, Headers, Method, Request, RequestInit, Response, Result, State, Stub,
    WebSocket, WebSocketIncomingMessage, WebSocketPair, durable_object,
};

use crate::bindings::{HUB, HUB_NAME};
use crate::relayroom::{RelayAttachment, RelaySocket, Room};
use crate::settings::Settings;
use crate::sockets::now_ms;

/// How long a socket has to send its auth message.
const AUTH_TIMEOUT_MS: f64 = 10_000.0;

impl RelaySocket for WebSocket {
    fn send_text(&self, text: &str) -> bool {
        self.send_with_str(text).is_ok()
    }
    fn send_binary(&self, bytes: &[u8]) -> bool {
        self.send_with_bytes(bytes).is_ok()
    }
    fn close(&self, code: u16, reason: &str) {
        let _ = WebSocket::close(self, Some(code), Some(reason));
    }
}

/// The session id in `/v1/sessions/{id}/relay`.
pub fn session_of(path: &str) -> Option<&str> {
    let id = path.strip_prefix("/v1/sessions/")?.strip_suffix("/relay")?;
    (!id.is_empty() && !id.contains('/')).then_some(id)
}

fn attachment(ws: &WebSocket) -> Option<RelayAttachment> {
    ws.deserialize_attachment().ok().flatten()
}

fn now_s() -> i64 {
    (now_ms() / 1000.0) as i64
}

/// Asks the hub for a ticket: with the client's token, or to renew one.
async fn ticket_from_hub(
    hub: &Stub,
    key: &PublicKey,
    session: &str,
    token: Option<&str>,
    old: Option<&str>,
) -> std::result::Result<(String, Ticket), (u16, String)> {
    let headers = Headers::new();
    let body = match (token, old) {
        (Some(t), _) => {
            let _ = headers.set("authorization", &format!("Bearer {t}"));
            String::new()
        }
        (None, Some(old)) => {
            let _ = headers.set("content-type", "application/json");
            serde_json::json!({ "ticket": old }).to_string()
        }
        (None, None) => return Err((CLOSE_UNAUTHORIZED, "send an auth message first".into())),
    };
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_headers(headers)
        .with_body(Some(JsValue::from_str(&body)));
    let url = format!("https://hub/v1/sessions/{session}/relay/ticket");
    let fail = |e: worker::Error| (1011, e.to_string());
    let req = Request::new_with_init(&url, &init).map_err(fail)?;
    let mut resp = hub.fetch_with_request(req).await.map_err(fail)?;
    let status = resp.status_code();
    let text = resp.text().await.map_err(fail)?;
    if status != 200 {
        let message = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v["message"].as_str().map(String::from))
            .unwrap_or(text);
        let code = if status == 401 {
            CLOSE_UNAUTHORIZED
        } else {
            CLOSE_FORBIDDEN
        };
        return Err((code, message));
    }
    let reply: TicketReply = serde_json::from_str(&text).map_err(|e| (1011, e.to_string()))?;
    let ticket = Ticket::open(key, &reply.ticket, session, now_s())
        .map_err(|e| (CLOSE_FORBIDDEN, e.message))?;
    Ok((reply.ticket, ticket))
}

#[durable_object]
pub struct CoopRelay {
    state: State,
    env: Env,
    room: Rc<Room<WebSocket>>,
    key: PublicKey,
    next: Cell<u64>,
    /// Whether this instance has taken back the sockets from before a
    /// sleep.
    adopted: Cell<bool>,
    /// Members whose ticket is being renewed.
    renewing: Rc<RefCell<HashSet<u64>>>,
}

impl CoopRelay {
    fn hub(&self) -> Result<Stub> {
        self.env
            .durable_object(HUB)?
            .id_from_name(HUB_NAME)?
            .get_stub()
    }

    fn adopt(&self) {
        if self.adopted.replace(true) {
            return;
        }
        for ws in self.state.get_websockets() {
            let Some(a) = attachment(&ws) else { continue };
            self.next.set(self.next.get().max(a.socket));
            // Its ticket's own time may have passed while asleep: the
            // token's end still holds, and it is renewed on the next
            // datagram.
            if let Some(t) = a.ticket.as_deref()
                && let Ok(ticket) = Ticket::open(&self.key, t, &a.session, 0)
            {
                self.room.restore(a.socket, ws, ticket, now_ms());
            }
        }
    }

    /// The alarm for the next auth deadline or token end, if any.
    async fn schedule(&self) -> Result<()> {
        let sockets = self.state.get_websockets();
        let pending = sockets
            .iter()
            .filter_map(attachment)
            .filter(|a| a.ticket.is_none())
            .map(|a| (a.since + AUTH_TIMEOUT_MS) as i64);
        let due = pending.chain(self.room.next_deadline_ms()).min();
        let storage = self.state.storage();
        match due {
            Some(at) => storage.set_alarm((at - now_ms() as i64).max(250)).await,
            None => storage.delete_alarm().await,
        }
    }

    async fn auth(&self, ws: WebSocket, a: RelayAttachment, text: &str) -> Result<()> {
        let token = match serde_json::from_str::<ClientMessage>(text) {
            Ok(ClientMessage::Auth { token }) => token,
            _ => {
                RelaySocket::close(
                    &ws,
                    CLOSE_UNAUTHORIZED,
                    "expected {\"kind\":\"auth\",\"token\":...}",
                );
                self.room.leave(a.socket);
                return Ok(());
            }
        };
        let got = ticket_from_hub(&self.hub()?, &self.key, &a.session, Some(&token), None).await;
        let (compact, ticket) = match got {
            Ok(t) => t,
            Err((code, why)) => {
                RelaySocket::close(&ws, code, &why);
                self.room.leave(a.socket);
                return Ok(());
            }
        };
        let joined = if self.room.is_member(a.socket) {
            self.room.refresh(a.socket, ticket)
        } else {
            self.room.join(a.socket, ws.clone(), ticket, now_ms())
        };
        match joined {
            Ok(()) => {
                let _ = ws.serialize_attachment(RelayAttachment {
                    ticket: Some(compact),
                    ..a
                });
            }
            Err(r) => {
                RelaySocket::close(&ws, r.0, r.1);
                self.room.leave(a.socket);
            }
        }
        self.schedule().await
    }

    /// Renews a member's run-out ticket in the background; closes the
    /// socket if the hub says no.
    fn renew(&self, ws: WebSocket, a: RelayAttachment) -> Result<()> {
        if !self.renewing.borrow_mut().insert(a.socket) {
            return Ok(());
        }
        let (hub, key, room, renewing) = (
            self.hub()?,
            self.key.clone(),
            self.room.clone(),
            self.renewing.clone(),
        );
        live_server::rt::spawn(async move {
            let got = ticket_from_hub(&hub, &key, &a.session, None, a.ticket.as_deref()).await;
            renewing.borrow_mut().remove(&a.socket);
            match got {
                Ok((compact, ticket)) => {
                    worker::console_log!("relay: renewed {}'s ticket", ticket.xuid);
                    if room.refresh(a.socket, ticket).is_ok() {
                        let _ = ws.serialize_attachment(RelayAttachment {
                            ticket: Some(compact),
                            ..a
                        });
                    }
                }
                Err((code, why)) => {
                    RelaySocket::close(&ws, code, &why);
                    room.leave(a.socket);
                }
            }
        });
        Ok(())
    }
}

impl DurableObject for CoopRelay {
    fn new(state: State, env: Env) -> Self {
        let setting = |name: &str| {
            env.var(name)
                .map(|v| v.to_string())
                .or_else(|_| env.secret(name).map(|s| s.to_string()))
                .ok()
        };
        // The hub refuses to start without settings, so a relay without
        // them never gets a ticket to check.
        let (key, rate) = match Settings::from_env(&setting) {
            Ok(s) => (s.identity.key.public().clone(), s.policy.relay_rate),
            Err(e) => {
                worker::console_error!("relay: settings: {e}");
                (
                    live_proto::sign::KeyPair::generate().public().clone(),
                    (0.0, 0),
                )
            }
        };
        CoopRelay {
            state,
            env,
            room: Rc::new(Room::new(rate)),
            key,
            next: Cell::new(0),
            adopted: Cell::new(false),
            renewing: Rc::new(RefCell::new(HashSet::new())),
        }
    }

    async fn fetch(&self, req: Request) -> Result<Response> {
        self.adopt();
        let path = req.path();
        let Some(session) = session_of(&path) else {
            return Response::error("not found", 404);
        };
        let upgrade = req.headers().get("upgrade")?.unwrap_or_default();
        if !upgrade.eq_ignore_ascii_case("websocket") {
            let e = live_server::ApiError::new(
                live_proto::ErrorCode::Invalid,
                "the relay is a WebSocket",
            );
            return Response::try_from(axum::response::IntoResponse::into_response(e));
        }
        let pair = WebSocketPair::new()?;
        self.state.accept_web_socket(&pair.server);
        let socket = self.next.get() + 1;
        self.next.set(socket);
        pair.server.serialize_attachment(RelayAttachment {
            socket,
            session: session.to_string(),
            since: now_ms(),
            ticket: None,
        })?;
        self.schedule().await?;
        Response::from_websocket(pair.client)
    }

    async fn websocket_message(
        &self,
        ws: WebSocket,
        message: WebSocketIncomingMessage,
    ) -> Result<()> {
        self.adopt();
        let Some(a) = attachment(&ws) else {
            return Ok(());
        };
        match message {
            WebSocketIncomingMessage::String(text) => self.auth(ws, a, &text).await,
            WebSocketIncomingMessage::Binary(data) => {
                match self.room.forward(a.socket, &data, now_ms()) {
                    Ok(_) => {
                        if self.room.ticket(a.socket).is_some_and(|t| t.exp < now_s()) {
                            self.renew(ws, a)?;
                        }
                    }
                    Err(r) => {
                        RelaySocket::close(&ws, r.0, r.1);
                        self.room.leave(a.socket);
                    }
                }
                Ok(())
            }
        }
    }

    async fn websocket_close(
        &self,
        ws: WebSocket,
        _code: usize,
        _reason: String,
        _was_clean: bool,
    ) -> Result<()> {
        self.adopt();
        if let Some(a) = attachment(&ws) {
            self.room.leave(a.socket);
        }
        Ok(())
    }

    async fn websocket_error(&self, ws: WebSocket, _error: worker::Error) -> Result<()> {
        self.adopt();
        if let Some(a) = attachment(&ws) {
            self.room.leave(a.socket);
        }
        Ok(())
    }

    async fn alarm(&self) -> Result<Response> {
        self.adopt();
        let now = now_ms();
        for ws in self.state.get_websockets() {
            if let Some(a) = attachment(&ws)
                && a.ticket.is_none()
                && now - a.since >= AUTH_TIMEOUT_MS
            {
                RelaySocket::close(&ws, CLOSE_UNAUTHORIZED, "send an auth message first");
            }
        }
        self.room.expire(now);
        self.schedule().await?;
        Response::ok("")
    }
}
