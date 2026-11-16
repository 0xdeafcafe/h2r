//! Signed-host discovery sessions, projected from the event log.
use crate::{
    app::{ApiError, Live, conflict, forbidden, invalid, not_found, now, sign_error, unauthorized},
    model::{State, nonce_key},
    store::{NewEvent, RecordedEvent},
};
use axum::{
    body::{Bytes, to_bytes},
    extract::{ConnectInfo, Path, Query, Request, State as HttpState},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use live_proto::{
    Author, Page, lxm,
    session::{self, Endpoint, NewSession, Session},
    sign::{MAX_EVENT_SKEW_S, SignedEvent, parse_time},
};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    net::{IpAddr, SocketAddr},
    sync::Arc,
};
type Shared = HttpState<Arc<Live>>;

impl Live {
    pub async fn create_session(
        &self,
        bearer: Option<&str>,
        new: NewSession,
        observed: Option<IpAddr>,
    ) -> Result<Session, ApiError> {
        let author = self.authenticate(bearer, lxm::SESSIONS_WRITE)?;
        self.rate_limit(&author, lxm::SESSIONS_WRITE)?;
        let Author::Account { xuid, .. } = author else {
            return Err(forbidden("a session host must be an account"));
        };
        new.validate().map_err(invalid)?;
        self.check_session_map(&new.flags, new.hopper_id, new.map_id.as_ref())?;
        let seen = observed.and_then(|ip| match ip {
            IpAddr::V4(v) => Some(v),
            _ => None,
        });
        // A listed session draws every browsing console's discovery probes
        // to its address, so off-loopback it is the address the host
        // connected from, whatever was declared. A declared address is kept
        // otherwise, and lists show the session only if that address is a
        // local-network one (Endpoint::listable, docs/live/live-custom-games.md).
        let declared = match seen {
            Some(v) if new.visibility.is_listed() && !v.is_loopback() => None,
            _ => new.addr,
        };
        let addr = declared
            .or(seen)
            .ok_or_else(|| invalid("an IPv4 endpoint is required"))?;
        if addr.is_unspecified() || addr.is_multicast() || addr.is_broadcast() || new.port > 65529 {
            return Err(invalid("invalid gameplay endpoint"));
        }
        let _guard = self.session_writes.lock().await;
        let tag = {
            let s = self.model.read();
            if s.sessions
                .values()
                .filter(|v| v.host_xuid == xuid && v.state == "open")
                .count()
                >= 32
            {
                return Err(conflict("host already advertises 32 sessions"));
            }
            s.accounts
                .get(&xuid)
                .ok_or_else(|| not_found("host account is missing"))?
                .gamertag
                .to_string()
        };
        let id = loop {
            let mut bytes = [0; 8];
            getrandom::fill(&mut bytes).map_err(|_| invalid("random source unavailable"))?;
            bytes[0] = (bytes[0] & 15) | 0x80;
            let id = live_proto::sign::hex(&bytes);
            if !self.model.read().sessions.contains_key(&id) {
                break id;
            }
        };
        let time = time::OffsetDateTime::now_utc();
        let at = time
            .format(&time::format_description::well_known::Rfc3339)
            .expect("time formats");
        let item = Session {
            session_id: id.clone(),
            host: author.to_string(),
            host_xuid: xuid,
            host_gamertag: tag,
            xnkid: id.clone(),
            endpoint: Endpoint {
                addr,
                port: new.port,
                source: if declared.is_some() {
                    "declared"
                } else {
                    "observed"
                }
                .into(),
            },
            advertisement: new,
            since: at,
            last_activity: time.unix_timestamp(),
            state: "open".into(),
            players: BTreeMap::new(),
        };
        let stream = session::stream(&id);
        let event = self.server_event(
            stream.clone(),
            session::CREATED,
            serde_json::to_value(&item).expect("serializes"),
        );
        let recorded = self.store.append(vec![event], &[(stream, 0)]).await?;
        self.wait_applied(recorded[0].position).await?;
        Ok(item)
    }

    pub async fn append_session_events(
        &self,
        bearer: Option<&str>,
        id: &str,
        jws: &str,
    ) -> Result<u64, ApiError> {
        let author = self.authenticate(bearer, lxm::SESSIONS_WRITE)?;
        self.rate_limit(&author, lxm::SESSIONS_WRITE)?;
        let (parsed, bodies) = SignedEvent::open_batch(jws).map_err(sign_error)?;
        let first = &bodies[0];
        if first.author != author.to_string() {
            return Err(forbidden("event author is not the token issuer"));
        }
        let key = self
            .key_for(&author, &first.kid)
            .ok_or_else(|| unauthorized("unknown or revoked session key"))?;
        parsed.verify(&key).map_err(sign_error)?;
        let _guard = self.session_writes.lock().await;
        let mut item = self
            .model
            .read()
            .sessions
            .get(id)
            .cloned()
            .ok_or_else(|| not_found("no such session"))?;
        if item.host != author.to_string() {
            return Err(forbidden("only the host writes this session"));
        }
        if item.state != "open" {
            return Err(conflict("session is closed"));
        }
        let clock = now();
        let stream = session::stream(id);
        let mut nonces = BTreeSet::new();
        let mut changed = Vec::new();
        for (i, b) in bodies.iter().enumerate() {
            if b.stream != stream {
                return Err(forbidden("event stream is not this session"));
            }
            if !matches!(
                b.kind.as_str(),
                session::UPDATED | session::DELETED | session::JOINED | session::LEFT
            ) {
                return Err(forbidden("console cannot write this session event"));
            }
            let at = parse_time(&b.at)
                .ok_or_else(|| invalid("invalid event time"))?
                .unix_timestamp();
            if (at - clock).abs() > MAX_EVENT_SKEW_S {
                return Err(invalid("session event time is stale"));
            }
            let nonce = nonce_key(&b.author, &b.nonce);
            if !nonces.insert(nonce.clone()) || self.model.nonces.contains(&nonce, clock) {
                return Err(conflict("nonce already used"));
            }
            if matches!(b.kind.as_str(), session::JOINED | session::LEFT) {
                let p: session::Player = serde_json::from_value(b.data.clone())
                    .map_err(|_| invalid("invalid player"))?;
                if !self.model.read().accounts.contains_key(&p.xuid)
                    || p.slot.as_ref().is_some_and(|s| s.len() > 64)
                {
                    return Err(invalid("unknown player or invalid slot"));
                }
            }
            if b.kind == session::UPDATED
                && let Some(id) = b.data.get("map_id").and_then(|v| v.as_str())
            {
                let id = id.parse().map_err(|_| invalid("invalid map_id"))?;
                let ad = &item.advertisement;
                self.check_session_map(&ad.flags, ad.hopper_id, Some(&id))?;
            }
            if b.kind == session::DELETED && i + 1 != bodies.len() {
                return Err(invalid("SessionDeleted must be last"));
            }
            let did = item.apply(&b.kind, &b.data, &b.at, at).map_err(invalid)?;
            if did {
                changed.push(i);
            }
        }
        for b in &bodies {
            let at = parse_time(&b.at).unwrap().unix_timestamp();
            if !self.model.nonces.insert_new(
                nonce_key(&b.author, &b.nonce),
                at + MAX_EVENT_SKEW_S,
                clock,
            ) {
                return Err(conflict("nonce already used"));
            }
        }
        if changed.is_empty() {
            return Ok(self.position());
        }
        let events = changed
            .into_iter()
            .map(|i| NewEvent {
                jws: jws.into(),
                jws_index: Some(i),
                body: bodies[i].clone(),
            })
            .collect();
        let recorded = self.store.append(events, &[]).await?;
        let pos = recorded.last().expect("nonempty").position;
        self.wait_applied(pos).await?;
        Ok(pos)
    }

    pub(crate) async fn sweep_sessions(&self, clock: i64) {
        let _guard = self.session_writes.lock().await;
        let ids: Vec<_> = self
            .model
            .read()
            .sessions
            .values()
            .filter(|v| v.state == "open" && v.last_activity + self.policy().session_idle_s < clock)
            .map(|v| v.session_id.clone())
            .collect();
        if ids.is_empty() {
            return;
        }
        let events = ids
            .iter()
            .map(|id| self.server_event(session::stream(id), session::EXPIRED, json!({})))
            .collect();
        match self.store.append(events, &[]).await {
            Ok(rows) => {
                let _ = self.wait_applied(rows.last().unwrap().position).await;
            }
            Err(e) => eprintln!("live: session sweep: {e}"),
        }
    }
}
pub(crate) fn project(s: &mut State, e: &RecordedEvent, at: i64) {
    let b = &e.body;
    let Some(id) = b.stream.strip_prefix("session-") else {
        return;
    };
    if b.kind == session::CREATED {
        if let Ok(item) = serde_json::from_value::<Session>(b.data.clone()) {
            s.sessions.insert(id.into(), item);
        }
    } else if let Some(item) = s.sessions.get_mut(id) {
        let _ = item.apply(&b.kind, &b.data, &b.at, at);
    }
}
pub(crate) async fn create(HttpState(live): Shared, req: Request) -> Result<Response, ApiError> {
    let headers = req.headers().clone();
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0);
    let observed = live.policy().public.client_ip(peer, &headers);
    let bytes = to_bytes(req.into_body(), crate::http::MAX_BODY)
        .await
        .map_err(|_| invalid("session request too large"))?;
    let new = serde_json::from_slice(&bytes).map_err(|_| invalid("invalid session request"))?;
    let item = live
        .create_session(crate::http::bearer(&headers), new, observed)
        .await?;
    Ok(crate::http::json(
        StatusCode::CREATED,
        &json!({"session_id":item.session_id,"session":item,"position":live.position()}),
    ))
}
pub(crate) async fn append(
    HttpState(live): Shared,
    Path(id): Path<String>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Result<Response, ApiError> {
    if !session::valid_id(&id) {
        return Err(invalid("bad session id"));
    }
    let jws = std::str::from_utf8(&bytes).map_err(|_| invalid("expected batch JWS"))?;
    let pos = live
        .append_session_events(crate::http::bearer(&headers), &id, jws)
        .await?;
    Ok(crate::http::json(
        StatusCode::CREATED,
        &json!({"position":pos}),
    ))
}
pub(crate) async fn detail(
    HttpState(live): Shared,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    live.authorize_read(crate::http::bearer(&headers))?;
    let state = live.model.read();
    let item = state
        .sessions
        .get(&id)
        .ok_or_else(|| not_found("no such session"))?;
    Ok(crate::http::json(
        StatusCode::OK,
        &json!({"session":item,"position":live.position()}),
    ))
}
pub(crate) async fn list(
    HttpState(live): Shared,
    headers: HeaderMap,
    Query(q): Query<BTreeMap<String, String>>,
) -> Result<Response, ApiError> {
    let viewer = live.read_caller(crate::http::bearer(&headers))?;
    let limit = q
        .get("limit")
        .map(|v| v.parse::<usize>())
        .transpose()
        .map_err(|_| invalid("invalid limit"))?
        .unwrap_or(100);
    if !(1..=1000).contains(&limit) {
        return Err(invalid("session limit is 1..1000"));
    }
    let after = q.get("cursor").map(String::as_str).unwrap_or("");
    let hopper = q
        .get("hopper")
        .map(|v| v.parse::<u32>())
        .transpose()
        .map_err(|_| invalid("invalid hopper"))?;
    let attrs: Option<BTreeMap<u32, session::AttributeValue>> = q
        .get("attributes")
        .map(|s| serde_json::from_str(s))
        .transpose()
        .map_err(|_| invalid("invalid attribute filter"))?;
    if let Some(attrs) = &attrs {
        session::validate_attributes(attrs).map_err(invalid)?;
    }
    let state = live.model.read();
    let mut items: Vec<_> = state
        .sessions
        .values()
        .filter(|s| {
            s.session_id.as_str() > after
                && q.get("state").map_or(s.state == "open", |v| *v == s.state)
                && hopper.is_none_or(|id| s.advertisement.hopper_id == Some(id))
                && q.get("map")
                    .is_none_or(|v| s.advertisement.map.as_ref() == Some(v))
                && q.get("host")
                    .is_none_or(|v| *v == s.host || *v == s.host_gamertag)
                && attrs.as_ref().is_none_or(|a| {
                    a.iter()
                        .all(|(k, v)| s.advertisement.attributes.get(k) == Some(v))
                })
                && q.get("visibility")
                    .is_none_or(|v| v == s.advertisement.visibility.as_str())
                && s.listed_for(viewer, |a, b| state.is_friends(a, b))
        })
        .take(limit + 1)
        .cloned()
        .collect();
    let next = if items.len() > limit {
        items.truncate(limit);
        items.last().map(|s| s.session_id.clone())
    } else {
        None
    };
    Ok(crate::http::json(
        StatusCode::OK,
        &Page {
            items,
            next,
            position: live.position(),
        },
    ))
}
