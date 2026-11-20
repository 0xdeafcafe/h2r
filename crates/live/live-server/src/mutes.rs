//! The mute list (docs/live/live-api.md "Mute list", docs/live/live-voice.md
//! §3.6): the players an account muted, from the game's player-details
//! screen (`XOnlineMutelistAdd` and `Remove` in the Live HLE) or the web.
//! The game builds its voice mute matrix from it, both ways.
//!
//! Each change is a server-signed `PlayerMuted` or `PlayerUnmuted` event on
//! the muting account's `mutes-<xuid>` stream. They are private: the account
//! itself, admins and the operator read them. Muting a player already muted,
//! or unmuting one who is not, appends nothing. An account mutes at most
//! [`MAX`] players, the size of the game's list.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use live_proto::mutes::{self, MAX, MuteEntry, MuteList, Muted, PLAYER_MUTED, PLAYER_UNMUTED};
use live_proto::{Author, Xuid, lxm};
use serde_json::json;

use crate::app::{ApiError, Live, conflict, forbidden, invalid, not_found};
use crate::dispatch::Subscriber;
use crate::store::RecordedEvent;

type ApiResult<T> = Result<T, ApiError>;

/// Every account's mutes, oldest first.
#[derive(Debug, Default)]
pub struct MutesState {
    lists: HashMap<Xuid, Vec<Xuid>>,
}

impl MutesState {
    /// The players `by` muted, oldest first.
    pub fn of(&self, by: Xuid) -> &[Xuid] {
        self.lists.get(&by).map(Vec::as_slice).unwrap_or_default()
    }

    pub fn has(&self, by: Xuid, target: Xuid) -> bool {
        self.of(by).contains(&target)
    }

    fn apply(&mut self, e: &RecordedEvent) {
        let Ok(d) = serde_json::from_value::<Muted>(e.body.data.clone()) else {
            return;
        };
        let list = self.lists.entry(d.by).or_default();
        match e.body.kind.as_str() {
            PLAYER_MUTED if !list.contains(&d.target) => list.push(d.target),
            PLAYER_UNMUTED => list.retain(|x| *x != d.target),
            _ => {}
        }
    }
}

/// The mute lists' projection.
#[derive(Default)]
pub struct Mutes {
    state: RwLock<MutesState>,
}

impl Mutes {
    pub fn read(&self) -> std::sync::RwLockReadGuard<'_, MutesState> {
        self.state.read().unwrap()
    }
}

impl Subscriber for Mutes {
    fn name(&self) -> &str {
        "mutes"
    }

    fn apply(&self, event: &RecordedEvent) {
        if mutes::KINDS.contains(&event.body.kind.as_str()) {
            self.state.write().unwrap().apply(event);
        }
    }
}

impl Live {
    fn mute_owner(&self, xuid: &str) -> ApiResult<Xuid> {
        let x = Xuid::parse(xuid).map_err(|_| not_found("no such account"))?;
        if !self.model.read().accounts.contains_key(&x) {
            return Err(not_found("no such account"));
        }
        Ok(x)
    }

    /// `GET /v1/accounts/by-xuid/{xuid}/mutes`: with a `read` token of the
    /// account itself or an admin.
    pub fn mute_list(&self, bearer: Option<&str>, xuid: &str) -> ApiResult<MuteList> {
        let author = self.authenticate(bearer, lxm::READ)?;
        let owner = self.mute_owner(xuid)?;
        let mine = matches!(author, Author::Account { xuid, .. } if xuid == owner);
        if !mine && !self.admins.is_admin(&author, &self.model.read()) {
            return Err(forbidden("a mute list is its owner's"));
        }
        let position = self.position();
        let items = self
            .mutes
            .read()
            .of(owner)
            .iter()
            .map(|x| MuteEntry {
                xuid: *x,
                gamertag: self.who_is(*x).gamertag,
            })
            .collect();
        Ok(MuteList { items, position })
    }

    /// `PUT` (mute) or `DELETE` (unmute)
    /// `/v1/accounts/by-xuid/{xuid}/mutes/{target}`, by the account itself
    /// with a `mutes.write` token: the event's position, or None when
    /// nothing changed.
    pub async fn set_mute(
        &self,
        bearer: Option<&str>,
        xuid: &str,
        target: &str,
        mute: bool,
    ) -> ApiResult<Option<u64>> {
        let author = self.authenticate(bearer, lxm::MUTES_WRITE)?;
        let owner = self.mute_owner(xuid)?;
        if !matches!(author, Author::Account { xuid, .. } if xuid == owner) {
            return Err(forbidden("only the account changes its mute list"));
        }
        self.rate_limit(&author, lxm::MUTES_WRITE)?;
        let target = Xuid::parse(target).map_err(|_| not_found("no such account"))?;
        if mute {
            if target == owner {
                return Err(invalid("a player can't mute themselves"));
            }
            if !self.model.read().accounts.contains_key(&target) {
                return Err(not_found("no such account"));
            }
        }
        let _guard = self.mute_writes.lock().await;
        {
            let s = self.mutes.read();
            if s.has(owner, target) == mute {
                return Ok(None);
            }
            if mute && s.of(owner).len() >= MAX {
                return Err(conflict(format!("a mute list holds at most {MAX} players")));
            }
        }
        let data = Muted { by: owner, target };
        let event = self.server_event(
            mutes::stream(owner),
            if mute { PLAYER_MUTED } else { PLAYER_UNMUTED },
            serde_json::to_value(data).expect("serializes"),
        );
        let recorded = self.store.append(vec![event], &[]).await?;
        self.wait_applied(recorded[0].position).await?;
        Ok(Some(recorded[0].position))
    }
}

// ---------- route handlers ----------

type Shared = State<Arc<Live>>;

pub(crate) async fn get_mutes(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath(xuid): UrlPath<String>,
) -> ApiResult<Response> {
    let view = live.mute_list(crate::http::bearer(&headers), &xuid)?;
    Ok(crate::http::json(StatusCode::OK, &view))
}

fn answer(position: Option<u64>, created: StatusCode) -> Response {
    match position {
        Some(p) => crate::http::json(
            created,
            &json!({"ok": true, "position": p, "changed": true}),
        ),
        None => crate::http::json(StatusCode::OK, &json!({"ok": true, "changed": false})),
    }
}

pub(crate) async fn put_mute(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath((xuid, target)): UrlPath<(String, String)>,
) -> ApiResult<Response> {
    let p = live
        .set_mute(crate::http::bearer(&headers), &xuid, &target, true)
        .await?;
    Ok(answer(p, StatusCode::CREATED))
}

pub(crate) async fn delete_mute(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath((xuid, target)): UrlPath<(String, String)>,
) -> ApiResult<Response> {
    let p = live
        .set_mute(crate::http::bearer(&headers), &xuid, &target, false)
        .await?;
    Ok(answer(p, StatusCode::OK))
}
