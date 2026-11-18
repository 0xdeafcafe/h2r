//! Signed-host discovery sessions, projected from the event log.
use crate::ConnectInfo;
use crate::{
    app::{ApiError, Live, conflict, forbidden, invalid, not_found, now, sign_error, unauthorized},
    model::{State, nonce_key},
    store::{NewEvent, RecordedEvent},
};
use axum::{
    body::{Bytes, to_bytes},
    extract::{Path, Query, Request, State as HttpState},
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
        // The batch's events share its JWS.
        let jws: std::sync::Arc<str> = jws.into();
        let events = changed
            .into_iter()
            .map(|i| NewEvent {
                jws: std::sync::Arc::clone(&jws),
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
/// Who reads a list of finished games: what a co-op game's session
/// visibility is judged against (docs/live/live-api.md, "Co-op games").
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Viewer {
    /// No bearer: only public games.
    Anonymous,
    Account(live_proto::Xuid),
    /// The operator or an admin account: every game.
    Admin,
}

impl Live {
    /// The reader behind a `read` bearer (checked even on a public server,
    /// as [`Live::read_caller`] does); the operator and admin accounts are
    /// [`Viewer::Admin`].
    pub fn read_viewer(&self, bearer: Option<&str>) -> Result<Viewer, ApiError> {
        if bearer.is_none() {
            self.authorize_read(None)?;
            return Ok(Viewer::Anonymous);
        }
        let author = self.authenticate(bearer, lxm::READ)?;
        Ok(self.viewer_of(&author))
    }

    /// The reader behind a `read` token sent on a stream, and the token's
    /// expiry. The token is checked (and its `jti` spent) once.
    pub fn read_viewer_until(&self, token: &str) -> Result<(Viewer, i64), ApiError> {
        let author = self.authenticate(Some(token), lxm::READ)?;
        let exp = live_proto::sign::Token::parse(token)
            .map(|t| t.claims.exp)
            .unwrap_or(0);
        Ok((self.viewer_of(&author), exp))
    }

    /// [`wire_for`] against the current read model.
    pub fn wire_for(&self, e: &RecordedEvent, viewer: Viewer) -> Option<live_proto::WireEvent> {
        if viewer == Viewer::Admin {
            return Some(e.to_wire());
        }
        wire_for(&self.model.read(), e, viewer)
    }

    /// Whether a live frame's game shows to `viewer`.
    pub fn frame_shows(&self, f: &crate::games::LiveFrame, viewer: Viewer) -> bool {
        viewer == Viewer::Admin || game_shows_to(&self.model.read(), &f.frame.game_id, viewer)
    }

    /// Whether each of `games` shows to `viewer`, under one read of the
    /// model.
    pub fn games_show<'a>(
        &self,
        games: impl IntoIterator<Item = &'a str>,
        viewer: Viewer,
    ) -> Vec<bool> {
        if viewer == Viewer::Admin {
            return games.into_iter().map(|_| true).collect();
        }
        let s = self.model.read();
        games
            .into_iter()
            .map(|id| game_shows_to(&s, id, viewer))
            .collect()
    }

    fn viewer_of(&self, author: &Author) -> Viewer {
        if self.admins.is_admin(author, &self.model.read()) {
            return Viewer::Admin;
        }
        match author {
            Author::Account { xuid, .. } => Viewer::Account(*xuid),
            Author::Server(_) => Viewer::Admin,
        }
    }
}

/// Whether `viewer` may read a log event (`GET /v1/events`, `/v1/stream`):
/// a game's events follow [`crate::model::Game::shows_to`], a session's
/// follow its current visibility with [`finished_shows_to`]'s rule (who was
/// in it counts), and every other event is public.
pub fn event_shows_to(s: &State, stream: &str, viewer: Viewer) -> bool {
    if viewer == Viewer::Admin {
        return true;
    }
    if let Some(id) = stream.strip_prefix("game-") {
        return game_shows_to(s, id, viewer);
    }
    if let Some(id) = stream.strip_prefix("session-") {
        return session_shows_to(s, id, viewer);
    }
    true
}

/// What presence says instead of a hidden game's or session's activity.
pub const GENERIC_ACTIVITY: &str = "In a game";

/// The open friends-only and invite-only sessions `xuid` hosts or is on.
pub fn private_sessions_of(s: &State, xuid: live_proto::Xuid) -> Vec<String> {
    s.sessions
        .values()
        .filter(|x| {
            x.state == "open"
                && !x.advertisement.visibility.is_public()
                && (x.host_xuid == xuid || x.players.contains_key(&xuid))
        })
        .map(|x| x.session_id.clone())
        .collect()
}

/// Whether `viewer` may see the details of `xuid`'s presence: its game id
/// (the game shows to it) and its activity (every private session the
/// player was in shows to it). The player and admins always may.
pub fn presence_detail_shows(
    s: &State,
    xuid: live_proto::Xuid,
    game_id: Option<&str>,
    sessions: &[String],
    viewer: Viewer,
) -> bool {
    if viewer == Viewer::Admin || viewer == Viewer::Account(xuid) {
        return true;
    }
    game_id.is_none_or(|g| game_shows_to(s, g, viewer))
        && sessions.iter().all(|id| session_shows_to(s, id, viewer))
}

/// A presence's game id and activity as `viewer` may see them: hidden
/// ones become no game id and [`GENERIC_ACTIVITY`].
pub fn presence_for(
    s: &State,
    xuid: live_proto::Xuid,
    game_id: Option<&String>,
    activity: Option<&String>,
    sessions: &[String],
    viewer: Viewer,
) -> (Option<String>, Option<String>) {
    if presence_detail_shows(s, xuid, game_id.map(String::as_str), sessions, viewer) {
        return (game_id.cloned(), activity.cloned());
    }
    let busy = game_id.is_some() || activity.is_some();
    (None, busy.then(|| GENERIC_ACTIVITY.to_string()))
}

/// A log event as `viewer` reads it: None when hidden ([`event_shows_to`]);
/// a presence event whose details the reader may not see has them replaced
/// ([`presence_for`]), its signature withheld (`jws` empty: it signed the
/// real data) and `redacted: true` in its data.
pub fn wire_for(s: &State, e: &RecordedEvent, viewer: Viewer) -> Option<live_proto::WireEvent> {
    if !event_shows_to(s, &e.body.stream, viewer) {
        return None;
    }
    let mut w = e.to_wire();
    let b = &e.body;
    if viewer == Viewer::Admin
        || !matches!(
            b.kind.as_str(),
            live_proto::types::SIGNED_IN | live_proto::types::PRESENCE_CHANGED
        )
    {
        return Some(w);
    }
    let Some(xuid) = b
        .stream
        .strip_prefix("presence-")
        .and_then(|x| live_proto::Xuid::parse(x).ok())
    else {
        return Some(w);
    };
    let text = |k: &str| b.data[k].as_str().map(str::to_string);
    let (game_id, activity) = (text("game_id"), text("activity"));
    let sessions = s
        .presence_context
        .get(&e.position)
        .map(Vec::as_slice)
        .unwrap_or_default();
    if presence_detail_shows(s, xuid, game_id.as_deref(), sessions, viewer) {
        return Some(w);
    }
    if let Some(d) = w.data.as_object_mut() {
        d.remove("game_id");
        d.remove("session_id");
        if game_id.is_some() || activity.is_some() {
            d.insert("activity".into(), GENERIC_ACTIVITY.into());
        }
        d.insert("redacted".into(), true.into());
    }
    w.jws = String::new();
    w.jws_index = None;
    Some(w)
}

/// [`crate::model::Game::shows_to`] by id; an unknown game is public.
pub fn game_shows_to(s: &State, id: &str, viewer: Viewer) -> bool {
    s.games
        .get(id)
        .is_none_or(|g| g.shows_to(viewer, |a, b| s.is_friends(a, b)))
}

/// A session's events: public ones to everyone; a friends-only or
/// invite-only one to its host, whoever is or was in it, the host's
/// friends (friends-only) and admins.
pub fn session_shows_to(s: &State, id: &str, viewer: Viewer) -> bool {
    let Some(item) = s.sessions.get(id) else {
        return true;
    };
    let played = |x: live_proto::Xuid| {
        item.players.contains_key(&x)
            || s.coop_joined
                .get(id)
                .is_some_and(|j| j.iter().any(|(y, _)| *y == x))
            || s.coop_games
                .values()
                .any(|g| g.session_id == id && g.guests.contains(&x))
    };
    finished_shows_to(
        item.advertisement.visibility,
        item.host_xuid,
        played,
        viewer,
        |a, b| s.is_friends(a, b),
    )
}

/// Whether a finished game from a co-op session with this `visibility`,
/// hosted by `host`, shows to `viewer`. Public: to everyone. Friends: to
/// whoever played in it (`played`) and the host's friends. Invite: only to
/// whoever played in it. Admins see every game.
pub fn finished_shows_to(
    visibility: session::Visibility,
    host: live_proto::Xuid,
    played: impl Fn(live_proto::Xuid) -> bool,
    viewer: Viewer,
    friends: impl Fn(live_proto::Xuid, live_proto::Xuid) -> bool,
) -> bool {
    let v = match viewer {
        Viewer::Admin => return true,
        _ if visibility.is_public() => return true,
        Viewer::Anonymous => return false,
        Viewer::Account(v) => v,
    };
    v == host || played(v) || (visibility == session::Visibility::Friends && friends(host, v))
}

/// The co-op session a Live co-op campaign game (`online: true`) was played
/// in, found and recorded when the game starts: who hosted it and how it was
/// listed then.
#[derive(Debug, Clone, PartialEq)]
pub struct CoopLink {
    pub session_id: String,
    pub host_xuid: live_proto::Xuid,
    pub visibility: session::Visibility,
}

impl CoopLink {
    /// The open co-op session hosted by, or else joined by, one of `xuids`.
    pub fn find(s: &State, xuids: &[live_proto::Xuid]) -> Option<CoopLink> {
        let open = || {
            s.sessions
                .values()
                .filter(|x| x.state == "open" && x.advertisement.flags == session::COOP)
        };
        open()
            .find(|x| xuids.contains(&x.host_xuid))
            .or_else(|| open().find(|x| x.players.keys().any(|p| xuids.contains(p))))
            .map(|x| CoopLink {
                session_id: x.session_id.clone(),
                host_xuid: x.host_xuid,
                visibility: x.advertisement.visibility,
            })
    }
}

/// A finished co-op campaign game, derived from its session's events: a
/// `coop` session that closed after at least one guest joined.
#[derive(Debug, Clone, PartialEq)]
pub struct CoopGame {
    pub session_id: String,
    pub host_xuid: live_proto::Xuid,
    pub campaign: session::Campaign,
    /// The session's visibility when it closed: who the game shows to
    /// ([`finished_shows_to`]).
    pub visibility: session::Visibility,
    /// Every guest that ever joined, in join order.
    pub guests: Vec<live_proto::Xuid>,
    /// The first guest's join time.
    pub started: String,
    pub ended: String,
    /// `closed`, `host_left` or `expired`.
    pub end_reason: String,
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
        let open = item.state == "open";
        let was = item.advertisement.visibility;
        if item.apply(&b.kind, &b.data, &b.at, at).is_err() || !open {
            return;
        }
        // A running co-op game follows its session's visibility; one that
        // ended keeps what it had.
        let now = item.advertisement.visibility;
        if now != was {
            for g in s.games.values_mut().filter(|g| g.ended.is_none()) {
                if let Some(c) = g.coop.as_mut().filter(|c| c.session_id == id) {
                    c.visibility = now;
                }
            }
        }
        let Some(item) = s.sessions.get(id) else {
            return;
        };
        let coop = item.advertisement.flags == session::COOP;
        if coop {
            let joined = s.coop_joined.entry(id.into()).or_default();
            for xuid in item.players.keys() {
                if !joined.iter().any(|(x, _)| x == xuid) {
                    joined.push((*xuid, b.at.clone()));
                }
            }
        }
        if item.state != "open" {
            let joined = s.coop_joined.remove(id).unwrap_or_default();
            let (Some(campaign), Some((_, started))) =
                (item.advertisement.campaign.clone(), joined.first().cloned())
            else {
                return;
            };
            if !coop {
                return;
            }
            let end_reason = if b.kind == session::EXPIRED {
                "expired".to_string()
            } else {
                b.data["reason"].as_str().unwrap_or("closed").to_string()
            };
            s.coop_games.insert(
                e.position,
                CoopGame {
                    session_id: id.into(),
                    host_xuid: item.host_xuid,
                    campaign,
                    visibility: item.advertisement.visibility,
                    guests: joined.into_iter().map(|(x, _)| x).collect(),
                    started,
                    ended: b.at.clone(),
                    end_reason,
                },
            );
        }
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
    // The title's OptiMatch search (procedure 1, decoded by the HLE): filter
    // on playlist, level window and party room, closest mean level first.
    let optimatch: Option<live_proto::optimatch::Query> = q
        .get("optimatch")
        .map(|s| serde_json::from_str(s))
        .transpose()
        .map_err(|_| invalid("invalid optimatch query"))?;
    let state = live.model.read();
    let listed = state.sessions.values().filter(|s| {
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
            && q.get("flags").is_none_or(|v| *v == s.advertisement.flags)
            && q.get("visibility")
                .is_none_or(|v| v == s.advertisement.visibility.as_str())
            && s.listed_for(viewer, |a, b| state.is_friends(a, b))
    });
    if let Some(query) = optimatch {
        // Ordered by match quality, so one page: no cursor.
        let items: Vec<_> =
            live_proto::optimatch::select(&query, listed, |s| &s.advertisement.attributes, limit)
                .into_iter()
                .cloned()
                .collect();
        return Ok(crate::http::json(
            StatusCode::OK,
            &Page {
                items,
                next: None,
                position: live.position(),
            },
        ));
    }
    let mut items: Vec<_> = listed.take(limit + 1).cloned().collect();
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
/// `GET /v1/coop/games`: finished co-op campaign games, newest first. The
/// cursor is the log position of the closing event; `?player=` narrows to
/// games a XUID or gamertag hosted or joined.
pub(crate) async fn coop_games(
    HttpState(live): Shared,
    headers: HeaderMap,
    Query(q): Query<BTreeMap<String, String>>,
) -> Result<Response, ApiError> {
    let viewer = live.read_viewer(crate::http::bearer(&headers))?;
    let limit = q
        .get("limit")
        .filter(|v| !v.is_empty())
        .map(|v| v.parse::<usize>())
        .transpose()
        .map_err(|_| invalid("invalid limit"))?
        .unwrap_or(100)
        .clamp(1, 1000);
    let before = q
        .get("cursor")
        .filter(|v| !v.is_empty())
        .map(|v| v.parse::<u64>())
        .transpose()
        .map_err(|_| invalid("bad cursor"))?;
    let position = live.position();
    let s = live.model.read();
    let player = match q.get("player").filter(|v| !v.is_empty()) {
        None => None,
        Some(v) => Some(match live_proto::Xuid::parse(v) {
            Ok(x) => x,
            Err(_) => {
                let tag = live_proto::Gamertag::new(v).map_err(|_| invalid("invalid player"))?;
                match s.account_by_tag(&tag) {
                    Some(a) => a.xuid,
                    // An unknown gamertag has played nothing.
                    None => {
                        return Ok(crate::http::json(
                            StatusCode::OK,
                            &Page::<serde_json::Value> {
                                items: vec![],
                                next: None,
                                position,
                            },
                        ));
                    }
                }
            }
        }),
    };
    let who = |x: &live_proto::Xuid| {
        json!({
            "xuid": x,
            "gamertag": s.accounts.get(x).map(|a| a.gamertag.to_string()),
        })
    };
    let rows: Vec<_> = s
        .coop_games
        .iter()
        .rev()
        .filter(|(p, _)| before.is_none_or(|b| **p < b))
        .filter(|(_, g)| player.is_none_or(|x| g.host_xuid == x || g.guests.contains(&x)))
        .filter(|(_, g)| {
            finished_shows_to(
                g.visibility,
                g.host_xuid,
                |x| g.guests.contains(&x),
                viewer,
                |a, b| s.is_friends(a, b),
            )
        })
        .take(limit)
        .collect();
    let next = (rows.len() == limit)
        .then(|| rows.last().map(|(p, _)| p.to_string()))
        .flatten();
    let items = rows
        .iter()
        .map(|(_, g)| {
            let players: Vec<_> = std::iter::once(&g.host_xuid)
                .chain(&g.guests)
                .map(&who)
                .collect();
            json!({
                "session_id": g.session_id,
                "host": who(&g.host_xuid),
                "players": players,
                "campaign": g.campaign,
                "started": g.started,
                "ended": g.ended,
                "end_reason": g.end_reason,
                "visibility": g.visibility.as_str(),
            })
        })
        .collect();
    Ok(crate::http::json(
        StatusCode::OK,
        &Page {
            items,
            next,
            position,
        },
    ))
}
