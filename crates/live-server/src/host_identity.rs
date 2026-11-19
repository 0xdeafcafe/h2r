//! Lease-scoped service credentials. A publisher is separate from an account
//! or an operator, including on a resumed Worker ingest socket.
use std::fmt;

use live_proto::{
    Author, Xuid, game,
    hosts::{HostLease, HostState},
    lxm,
    sign::{Jws, KeyPair, PublicKey, TYP_TOKEN, Token},
};
use serde::{Deserialize, Serialize};

use crate::{
    app::{ApiError, Live, forbidden, now, unauthorized},
    identity::ServerIdentity,
    model::{Game, State},
};

pub const HOST_REGISTER: &str = "hosts.register";
pub const HOST_REPORT: &str = "hosts.report";
pub const SESSIONS_PUBLISH: &str = "sessions.publish";
pub const TELEMETRY_INGEST: &str = "telemetry.ingest";
const SCOPES: [&str; 4] = [
    HOST_REGISTER,
    HOST_REPORT,
    SESSIONS_PUBLISH,
    TELEMETRY_INGEST,
];

#[derive(Serialize, Deserialize)]
pub(crate) struct HostClaims {
    iss: String,
    aud: String,
    sub: String,
    lxm: Vec<String>,
    iat: i64,
    exp: i64,
    // Signed reservation details are available even after the QoS queue expires.
    pub(crate) assignment_id: String,
    pub(crate) roster: Vec<Xuid>,
    pub(crate) hopper: Option<u32>,
    map: String,
    map_id: Option<String>,
    // A lease-specific Ed25519 seed for the existing event-batch JWS protocol.
    // This stays inside the private boot credential; never log the claims.
    event_key: String,
}

fn event_key(identity: &ServerIdentity, subject: &str) -> KeyPair {
    KeyPair::from_secret(identity.derive(&format!("h2live lease event key v1/{subject}")))
}

impl HostClaims {
    fn issue(identity: &ServerIdentity, lease: &HostLease, clock: i64) -> String {
        let sub = format!("host:{}", lease.id);
        let claims = Self {
            event_key: event_key(identity, &sub).secret_b64(),
            iss: identity.author(),
            aud: identity.id.clone(),
            sub,
            lxm: SCOPES.map(String::from).into(),
            iat: clock,
            exp: clock + i64::from(lease.max_match_s) + 300,
            assignment_id: lease.assignment_id.clone(),
            roster: lease.roster.clone(),
            hopper: lease.hopper,
            map: lease.map.clone(),
            map_id: lease.map_id.clone(),
        };
        identity.key.sign_json(TYP_TOKEN, &claims)
    }

    fn verify(
        identity: &ServerIdentity,
        raw: &str,
        scope: &str,
        clock: i64,
    ) -> Result<Self, ApiError> {
        let bad = || unauthorized("invalid host token");
        let jwt = Jws::parse(raw).map_err(|_| bad())?;
        if jwt.kid() != Some(identity.key.kid()) {
            return Err(bad());
        }
        jwt.verify(identity.key.public()).map_err(|_| bad())?;
        let claims: Self = serde_json::from_value(jwt.json_payload(TYP_TOKEN).map_err(|_| bad())?)
            .map_err(|_| bad())?;
        if claims.aud != identity.id
            || claims.iss != identity.author()
            || claims.iat > clock
            || claims.exp <= clock
            || lease_id(&claims.sub).is_none()
        {
            return Err(bad());
        }
        if !SCOPES.contains(&scope) || !claims.lxm.iter().any(|s| s == scope) {
            return Err(forbidden("host token does not grant this scope"));
        }
        Ok(claims)
    }

    /// Stopped leases cannot open fresh games or advertise another lobby.
    pub(crate) fn require_active(&self, live: &Live) -> Result<(), ApiError> {
        let id = lease_id(&self.sub).expect("verified");
        if live
            .host_view(id)
            .is_some_and(|v| !matches!(v.state, HostState::Ended | HostState::Failed))
        {
            Ok(())
        } else {
            Err(forbidden("host lease is no longer active"))
        }
    }

    /// The game-start request may omit reservation fields a playerless title
    /// cannot discover through a local profile. A supplied binding must agree.
    pub(crate) fn bind_game(&self, state: &State, new: &mut game::NewGame) -> Result<(), ApiError> {
        if new.mode != "multiplayer"
            || new.map != self.map
            || new.map_id.map(|id| id.to_string()) != self.map_id
        {
            return Err(forbidden("game is outside the host lease"));
        }
        if let Some(sim) = &new.sim {
            return if self.assignment_id == game::sim_game_id(&sim.run_id) {
                Ok(())
            } else {
                Err(forbidden("simulation belongs to another lease"))
            };
        }
        let session = state
            .sessions
            .values()
            .find(|s| s.host == self.sub && s.state == "open")
            .ok_or_else(|| forbidden("publish the lease session before its game"))?;
        if new
            .session_id
            .as_ref()
            .is_some_and(|id| *id != session.session_id)
        {
            return Err(forbidden("game names another session"));
        }
        new.session_id = Some(session.session_id.clone());
        if let Some(a) = &session.matchmaking {
            if !self.owns_assignment(a) {
                return Err(forbidden("game belongs to another assignment"));
            }
            let playlist = a.hopper.to_string();
            if new.playlist_id.as_ref().is_some_and(|p| p != &playlist) {
                return Err(forbidden("game names another hopper"));
            }
            new.playlist_id = Some(playlist);
            if !game::is_matchmade(new.kind()) {
                new.kind = Some("matchmaking".into());
            }
        } else if !self.assignment_id.starts_with("custom-") || game::is_matchmade(new.kind()) {
            return Err(forbidden("game needs its assigned session"));
        }
        Ok(())
    }

    pub(crate) fn session_assignment<'a>(
        &'a self,
        new: &live_proto::session::NewSession,
        header: Option<&str>,
    ) -> Result<Option<&'a str>, ApiError> {
        if new.is_coop()
            || new.map.as_ref().is_some_and(|m| m != &self.map)
            || new.map_id.map(|id| id.to_string()) != self.map_id
            || header.is_some_and(|id| id != self.assignment_id)
        {
            return Err(forbidden("session is outside the host lease"));
        }
        if self.assignment_id.starts_with("custom-") {
            if header.is_some() || new.hopper_id.is_some() || new.flags != "custom" {
                return Err(forbidden("custom lease needs a custom session"));
            }
            return Ok(None);
        }
        Ok(Some(&self.assignment_id))
    }

    pub(crate) fn owns_assignment(&self, a: &live_proto::matchmaking::Assignment) -> bool {
        a.id == self.assignment_id
            && a.host_lease.as_deref() == lease_id(&self.sub)
            && self.hopper == Some(a.hopper)
            && self.matches_roster(&a.members)
    }

    pub(crate) fn matches_roster(&self, members: &[Xuid]) -> bool {
        members.len() == self.roster.len()
            && self.roster.iter().all(|x| members.contains(x))
            && members.iter().all(|x| self.roster.contains(x))
    }
}

fn lease_id(subject: &str) -> Option<&str> {
    subject
        .strip_prefix("host:")
        .filter(|id| id.starts_with("hl-") && game::valid_id(id))
}

/// Only game ingest and session publication accept this additional principal.
/// Generic account/administrator authentication cannot return it.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Publisher {
    Peer(Author),
    Host(String), // full verified subject
}

impl fmt::Display for Publisher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Peer(a) => a.fmt(f),
            Self::Host(subject) => f.write_str(subject),
        }
    }
}

impl Publisher {
    pub(crate) fn from_attachment(subject: &str) -> Option<Self> {
        Author::parse(subject)
            .map(Self::Peer)
            .or_else(|| lease_id(subject).map(|_| Self::Host(subject.into())))
    }

    pub(crate) fn peer(&self) -> Option<&Author> {
        match self {
            Self::Peer(a) => Some(a),
            Self::Host(_) => None,
        }
    }

    pub(crate) fn xuid(&self) -> Option<Xuid> {
        self.peer().and_then(crate::usage::author_xuid)
    }

    pub(crate) fn may_write(&self, game: &Game) -> bool {
        match self {
            Self::Peer(a) => lease_id(&game.started.host).is_none() && game.admits(a),
            Self::Host(subject) => subject == &game.started.host,
        }
    }

    /// A lease host has no XUID. A signed bot XUID still needs a
    /// server-issued persistent bot and a seat on this assignment.
    pub(crate) fn check_players<'a>(
        &self,
        state: &State,
        game_host: Option<Xuid>,
        session: Option<&str>,
        before: u64,
        players: impl IntoIterator<Item = &'a game::Player>,
    ) -> Result<(), String> {
        let Self::Host(subject) = self else {
            return crate::bots::check_players(state, game_host, session, before, players);
        };
        let reservation = session
            .and_then(|id| state.sessions.get(id))
            .filter(|s| &s.host == subject)
            .and_then(|s| s.matchmaking.as_ref());
        for player in players {
            if player.xuid == Some(Xuid(0)) {
                return Err("the host has no player XUID".into());
            }
            if player.is_bot()
                && let Some(x) = player.xuid
                && reservation.is_some_and(|a| a.members.contains(&x))
                && state.bots.bot(x).is_some_and(|bot| bot.persistent)
            {
                continue;
            }
            crate::bots::check_players(state, None, session, before, [player])?;
        }
        Ok(())
    }

    pub(crate) fn host_lease_id(&self) -> Option<&str> {
        match self {
            Self::Host(subject) => lease_id(subject),
            Self::Peer(_) => None,
        }
    }

    pub(crate) fn key(&self, live: &Live, kid: &str) -> Option<PublicKey> {
        match self {
            Self::Peer(a) => live.key_for(a, kid),
            Self::Host(subject) => {
                let key = event_key(&live.identity, subject);
                (key.kid() == kid).then(|| key.public().clone())
            }
        }
    }
}

impl Live {
    pub fn issue_host_token(&self, lease: &HostLease) -> String {
        HostClaims::issue(&self.identity, lease, now())
    }

    /// The caller compares the returned lease id to the resource in the URL.
    pub fn verify_host_token(&self, bearer: Option<&str>, scope: &str) -> Result<String, ApiError> {
        let raw = bearer.ok_or_else(|| unauthorized("expected a host bearer token"))?;
        let claims = HostClaims::verify(&self.identity, raw, scope, now())?;
        Ok(lease_id(&claims.sub).expect("verified").to_owned())
    }

    pub(crate) fn publisher(
        &self,
        bearer: Option<&str>,
        scope: &str,
    ) -> Result<(Publisher, Option<HostClaims>, i64), ApiError> {
        let raw = bearer.ok_or_else(|| unauthorized("expected a bearer token"))?;
        // Account tokens have a single string lxm. Authenticate them through
        // the existing replay, ban and device checks exactly once.
        if let Ok(token) = Token::parse(raw) {
            let method = match scope {
                TELEMETRY_INGEST => lxm::GAMES_WRITE,
                SESSIONS_PUBLISH => lxm::SESSIONS_WRITE,
                _ => return Err(forbidden("unknown publisher scope")),
            };
            let peer = self.authenticate(bearer, method)?;
            return Ok((Publisher::Peer(peer), None, token.claims.exp));
        }
        let claims = HostClaims::verify(&self.identity, raw, scope, now())?;
        let exp = claims.exp;
        Ok((Publisher::Host(claims.sub.clone()), Some(claims), exp))
    }
}

/// GameStarted is server-authored; GameEnded must actually come from the
/// lease key. Both facts, and unidentified seats, survive a log replay.
pub(crate) fn ranked_host_game(
    game: &Game,
    session: &live_proto::session::Session,
    assignment: &live_proto::matchmaking::Assignment,
    lease: &str,
) -> bool {
    lease_id(&game.started.host) == Some(lease)
        && session.host == game.started.host
        && game.started.game.session_id.as_deref() == Some(session.session_id.as_str())
        && game
            .started
            .game
            .playlist_id
            .as_ref()
            .is_some_and(|p| p == &assignment.hopper.to_string())
        && game.ended_by_host
        && game.all_players_identified
        && game.ended.as_ref().is_some_and(|(end, _)| {
            end.players
                .iter()
                .all(|score| game.players.iter().any(|p| p.index == score.index))
        })
        && game
            .xuids
            .iter()
            .all(|x| x.0 != 0 && assignment.members.contains(x))
}

#[cfg(test)]
mod tests;
