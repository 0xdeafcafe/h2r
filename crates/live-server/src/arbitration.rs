//! Ranked-game arbitration (docs/live/live-matchmaking.md §8.4, live-api.md
//! "Arbitration"). A console's Live HLE posts each `XOnlineArbitration`
//! call as the signed-in account; the server writes it, server-signed, on
//! the round's `round-<session>-<round>` stream with that account's xuid.
//! Nobody else writes those streams. [`Live::rank_game`] ranks a game's
//! humans only if their own account registered its round.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use axum::extract::{Path as UrlPath, State as HttpState};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use live_proto::arbitration::{
    self, EXTENDED, Extend, MAX_PROCEDURES, REGISTERED, REPORTED, Register, Registered, Report,
    Reported,
};
use live_proto::{Author, Xuid, lxm};
use serde_json::json;

use crate::app::{ApiError, Live, conflict, forbidden, invalid, not_found};
use crate::model::State;
use crate::store::RecordedEvent;

/// One round as the log has it.
#[derive(Debug, Default, Clone)]
pub struct Round {
    /// Who registered it, and where in the log.
    pub registered: BTreeMap<Xuid, u64>,
    pub reported: BTreeSet<Xuid>,
}

/// Every session's rounds: session id -> round id -> round.
#[derive(Debug, Default)]
pub struct Rounds {
    pub sessions: HashMap<String, BTreeMap<String, Round>>,
}

impl Rounds {
    pub fn round(&self, session: &str, round: &str) -> Option<&Round> {
        self.sessions.get(session)?.get(round)
    }

    /// The round a game in `session` hosted by `host` was played as: of
    /// the rounds `host` registered there that have not ranked a game
    /// (`used`, `{session}-{round}`), the one it registered last. `None`
    /// when there is none.
    pub fn for_game(
        &self,
        session: &str,
        host: Xuid,
        used: &BTreeSet<String>,
    ) -> Option<(String, &Round)> {
        self.sessions
            .get(session)?
            .iter()
            .filter(|(id, _)| !used.contains(&format!("{session}-{id}")))
            .filter_map(|(id, r)| Some((*r.registered.get(&host)?, id, r)))
            .max_by_key(|(at, _, _)| *at)
            .map(|(_, id, r)| (id.clone(), r))
    }
}

pub(crate) fn project(s: &mut State, e: &RecordedEvent) {
    let b = &e.body;
    let (session, round, xuid) = match b.kind.as_str() {
        REGISTERED => match serde_json::from_value::<Registered>(b.data.clone()) {
            Ok(d) => (d.session_id, d.round_id, d.xuid),
            Err(_) => return,
        },
        REPORTED => match serde_json::from_value::<Reported>(b.data.clone()) {
            Ok(d) => (d.session_id, d.round_id, d.xuid),
            Err(_) => return,
        },
        _ => return,
    };
    let r = s
        .arbitration
        .sessions
        .entry(session)
        .or_default()
        .entry(round)
        .or_default();
    if b.kind == REGISTERED {
        r.registered.entry(xuid).or_insert(e.position);
    } else {
        r.reported.insert(xuid);
    }
}

/// What a console posts.
pub enum Call {
    Register(Register),
    Extend(Extend),
    Report(Report),
}

impl Live {
    /// `POST /v1/arbitration/{session}/{round}/{register|extend|report}` by
    /// an account with a `games.write` token. Registering twice, or
    /// reporting twice, writes nothing; extending or reporting a round the
    /// account did not register is a conflict. The event's position, or
    /// `None` when nothing was written.
    pub async fn arbitrate(
        &self,
        bearer: Option<&str>,
        session: &str,
        round: &str,
        call: Call,
    ) -> Result<Option<u64>, ApiError> {
        let author = self.authenticate(bearer, lxm::GAMES_WRITE)?;
        let Author::Account { xuid, .. } = author else {
            return Err(forbidden("arbitration is an account's own"));
        };
        self.rate_limit(&author, lxm::GAMES_WRITE)?;
        if !arbitration::valid_id(session) || !arbitration::valid_id(round) {
            return Err(not_found("no such round"));
        }
        let _guard = self.round_writes.lock().await;
        let (registered, reported) = {
            let s = self.model.read();
            let r = s.arbitration.round(session, round);
            (
                r.is_some_and(|r| r.registered.contains_key(&xuid)),
                r.is_some_and(|r| r.reported.contains(&xuid)),
            )
        };
        let (session_id, round_id) = (session.to_string(), round.to_string());
        let (kind, data) = match call {
            Call::Register(_) if registered => return Ok(None),
            Call::Register(c) => (
                REGISTERED,
                serde_json::to_value(Registered {
                    session_id,
                    round_id,
                    xuid,
                    duration: c.duration,
                    flags: c.flags,
                }),
            ),
            _ if !registered => return Err(conflict("register the round first")),
            Call::Extend(c) => (
                EXTENDED,
                serde_json::to_value(arbitration::Extended {
                    session_id,
                    round_id,
                    xuid,
                    duration: c.duration,
                }),
            ),
            Call::Report(_) if reported => return Ok(None),
            Call::Report(c) => {
                if c.procedures.len() > MAX_PROCEDURES {
                    return Err(invalid(format!(
                        "a report has at most {MAX_PROCEDURES} procedures"
                    )));
                }
                (
                    REPORTED,
                    serde_json::to_value(Reported {
                        session_id,
                        round_id,
                        xuid,
                        flags: c.flags,
                        procedures: c.procedures,
                    }),
                )
            }
        };
        let event = self.server_event(
            arbitration::stream(session, round),
            kind,
            data.expect("serializes"),
        );
        let recorded = self.store.append(vec![event], &[]).await?;
        self.wait_applied(recorded[0].position).await?;
        Ok(Some(recorded[0].position))
    }
}

// ---------- route handler ----------

pub(crate) async fn post_round(
    HttpState(live): HttpState<Arc<Live>>,
    headers: HeaderMap,
    UrlPath((session, round, action)): UrlPath<(String, String, String)>,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    fn body_of<T: serde::de::DeserializeOwned>(b: &[u8]) -> Result<T, ApiError> {
        serde_json::from_slice(b).map_err(|e| invalid(format!("body: {e}")))
    }
    let call = match action.as_str() {
        "register" => Call::Register(body_of(&body)?),
        "extend" => Call::Extend(body_of(&body)?),
        "report" => Call::Report(body_of(&body)?),
        _ => return Err(not_found("no such arbitration call")),
    };
    let p = live
        .arbitrate(crate::http::bearer(&headers), &session, &round, call)
        .await?;
    Ok(match p {
        Some(p) => crate::http::json(
            StatusCode::CREATED,
            &json!({"ok": true, "position": p, "changed": true}),
        ),
        None => crate::http::json(StatusCode::OK, &json!({"ok": true, "changed": false})),
    })
}
