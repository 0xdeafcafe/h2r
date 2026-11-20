//! Account bans (docs/live/live-api.md "Bans", docs/live/live-admin.md
//! "Bans"): an admin bans an account, for a while or until lifted, and
//! while the ban is in force [`Live::authenticate`] refuses the account's
//! tokens for every write scope ([`live_proto::bans::bars`]) with
//! `banned` (403). Signing in, sessions, the relay and reports all go
//! through it. Reads still work, so the player can read their notice.
//!
//! Both events are server-signed (`AccountBanned`, `AccountUnbanned`, on
//! the account's `bans-<xuid>` stream) and private: admins, the operator
//! and the banned player read them. A ban with an end date stops being in
//! force when the date passes, judged when a token is used; nothing is
//! written then. A new ban of an account replaces the one before. Admins
//! can't be banned.
//!
//! Also here: a player's commendation counts, public, from the player
//! reports' two positive kinds.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};

use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use live_proto::bans::{
    self, ACCOUNT_BANNED, ACCOUNT_UNBANNED, BanNotice, BanRequest, BanStatus, Banned,
    Commendations, LiftRequest, MAX_TEXT, Unbanned,
};
use live_proto::maps::Who;
use live_proto::{Author, ErrorCode, Xuid, lxm};
use serde::Serialize;
use serde_json::json;
use time::format_description::well_known::Rfc3339;

use crate::app::{ApiError, Live, conflict, forbidden, invalid, not_found, now};
use crate::dispatch::Subscriber;
use crate::store::RecordedEvent;

type ApiResult<T> = Result<T, ApiError>;

/// A ban, as the log says.
#[derive(Debug, Clone, PartialEq)]
pub struct Ban {
    pub position: u64,
    pub data: Banned,
    /// When it ends, in Unix seconds; none until lifted.
    pub until: Option<i64>,
    pub at: String,
    pub lifted: Option<Lift>,
    /// The ban that replaced it.
    pub replaced_by: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Lift {
    pub note: String,
    pub by: Option<Xuid>,
    pub at: String,
}

/// Where a ban stands at a moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BanState {
    Active,
    Expired,
    Lifted,
    Replaced,
}

impl Ban {
    pub fn state(&self, now: i64) -> BanState {
        if self.lifted.is_some() {
            BanState::Lifted
        } else if self.replaced_by.is_some() {
            BanState::Replaced
        } else if self.until.is_some_and(|u| u <= now) {
            BanState::Expired
        } else {
            BanState::Active
        }
    }
}

fn unix(rfc3339: &str) -> Option<i64> {
    time::OffsetDateTime::parse(rfc3339, &Rfc3339)
        .ok()
        .map(|t| t.unix_timestamp())
}

/// Every ban, by position.
#[derive(Debug, Default)]
pub struct BansState {
    pub bans: BTreeMap<u64, Ban>,
    /// Each account's latest ban.
    latest: HashMap<Xuid, u64>,
}

impl BansState {
    /// The account's ban in force at `now`, if any.
    pub fn in_force(&self, xuid: Xuid, now: i64) -> Option<&Ban> {
        self.latest
            .get(&xuid)
            .and_then(|p| self.bans.get(p))
            .filter(|b| b.state(now) == BanState::Active)
    }

    fn apply(&mut self, e: &RecordedEvent) {
        let b = &e.body;
        match b.kind.as_str() {
            ACCOUNT_BANNED => {
                let Ok(d) = serde_json::from_value::<Banned>(b.data.clone()) else {
                    return;
                };
                if let Some(old) = self.latest.insert(d.target, e.position)
                    && let Some(old) = self.bans.get_mut(&old)
                {
                    old.replaced_by = Some(e.position);
                }
                self.bans.insert(
                    e.position,
                    Ban {
                        position: e.position,
                        until: d.until.as_deref().and_then(unix),
                        data: d,
                        at: b.at.clone(),
                        lifted: None,
                        replaced_by: None,
                    },
                );
            }
            ACCOUNT_UNBANNED => {
                if let Ok(d) = serde_json::from_value::<Unbanned>(b.data.clone())
                    && let Some(ban) = self.bans.get_mut(&d.ban)
                    && ban.data.target == d.target
                    && ban.lifted.is_none()
                {
                    ban.lifted = Some(Lift {
                        note: d.note,
                        by: d.by,
                        at: b.at.clone(),
                    });
                }
            }
            _ => {}
        }
    }
}

/// The bans' projection.
#[derive(Default)]
pub struct Bans {
    state: RwLock<BansState>,
}

impl Bans {
    pub fn read(&self) -> std::sync::RwLockReadGuard<'_, BansState> {
        self.state.read().unwrap()
    }
}

impl Subscriber for Bans {
    fn name(&self) -> &str {
        "bans"
    }

    fn apply(&self, event: &RecordedEvent) {
        if bans::KINDS.contains(&event.body.kind.as_str()) {
            self.state.write().unwrap().apply(event);
        }
    }
}

/// The error a banned account's write gets.
pub fn banned_error(ban: &Ban) -> ApiError {
    let message = match &ban.data.until {
        Some(u) => format!("this account is banned until {u}: {}", ban.data.reason),
        None => format!("this account is banned: {}", ban.data.reason),
    };
    ApiError::new(ErrorCode::Banned, message)
}

/// One ban in `GET /v1/admin/bans`.
#[derive(Debug, Serialize)]
pub struct AdminBan {
    pub id: u64,
    pub target: Who,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<u64>,
    /// The admin's gamertag; none for the operator.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    pub at: String,
    pub state: BanState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifted: Option<AdminLift>,
}

#[derive(Debug, Serialize)]
pub struct AdminLift {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    pub at: String,
}

/// `GET /v1/admin/bans`.
#[derive(Debug, Serialize)]
pub struct AdminBans {
    /// Newest first.
    pub items: Vec<AdminBan>,
    pub position: u64,
}

fn text(field: &str, raw: &str) -> ApiResult<String> {
    let t = raw.trim();
    if t.chars().count() > MAX_TEXT || t.chars().any(|c| c.is_control() && c != '\n') {
        return Err(invalid(format!(
            "{field} is at most {MAX_TEXT} characters, without control characters"
        )));
    }
    Ok(t.to_string())
}

impl Live {
    /// An admin (or the operator) with a `reports.moderate` token: the
    /// admin's XUID, none for the operator.
    fn ban_moderator(&self, bearer: Option<&str>) -> ApiResult<Option<Xuid>> {
        let author = self.authenticate(bearer, lxm::REPORTS_MODERATE)?;
        crate::admin::require_admin(self, &author)
            .map_err(|_| forbidden("bans need the operator or an admin"))?;
        self.rate_limit(&author, lxm::REPORTS_MODERATE)?;
        Ok(match author {
            Author::Account { xuid, .. } => Some(xuid),
            Author::Server(_) => None,
        })
    }

    fn ban_target(&self, target: &str) -> ApiResult<Xuid> {
        let target = Xuid::parse(target).map_err(|_| not_found("no such account"))?;
        if !self.model.read().accounts.contains_key(&target) {
            return Err(not_found("no such account"));
        }
        Ok(target)
    }

    /// `POST /v1/admin/bans/{xuid}`: the ban's position.
    pub async fn ban_account(
        &self,
        bearer: Option<&str>,
        target: &str,
        req: BanRequest,
    ) -> ApiResult<u64> {
        let by = self.ban_moderator(bearer)?;
        let target = self.ban_target(target)?;
        if self.admins.is_admin_xuid(target, &self.model.read()) {
            return Err(forbidden("admins can't be banned"));
        }
        let reason = text("a reason", &req.reason)?;
        if reason.is_empty() {
            return Err(invalid("a ban needs a reason"));
        }
        let until = match &req.until {
            None => None,
            Some(u) => {
                let t = time::OffsetDateTime::parse(u.trim(), &Rfc3339)
                    .map_err(|_| invalid("until is an RFC 3339 time"))?;
                if t.unix_timestamp() <= now() {
                    return Err(invalid("until is in the past"));
                }
                let t = t.to_offset(time::UtcOffset::UTC);
                Some(t.format(&Rfc3339).expect("formats"))
            }
        };
        if let Some(r) = req.report
            && self
                .reports
                .read()
                .reports
                .get(&r)
                .is_none_or(|r| r.data.target != target)
        {
            return Err(invalid("no such report against that player"));
        }
        let _guard = self.ban_writes.lock().await;
        let data = Banned {
            target,
            reason,
            until,
            report: req.report,
            by,
        };
        let event = self.server_event(
            bans::stream(target),
            ACCOUNT_BANNED,
            serde_json::to_value(&data).expect("serializes"),
        );
        let recorded = self.store.append(vec![event], &[]).await?;
        self.wait_applied(recorded[0].position).await?;
        Ok(recorded[0].position)
    }

    /// `POST /v1/admin/bans/{xuid}/lift`: the lift's position.
    pub async fn lift_ban(&self, bearer: Option<&str>, target: &str, note: &str) -> ApiResult<u64> {
        let by = self.ban_moderator(bearer)?;
        let target = self.ban_target(target)?;
        let note = text("a note", note)?;
        let _guard = self.ban_writes.lock().await;
        let ban = self
            .bans
            .read()
            .in_force(target, now())
            .map(|b| b.position)
            .ok_or_else(|| conflict("that account is not banned"))?;
        let data = Unbanned {
            target,
            ban,
            note,
            by,
        };
        let event = self.server_event(
            bans::stream(target),
            ACCOUNT_UNBANNED,
            serde_json::to_value(&data).expect("serializes"),
        );
        let recorded = self.store.append(vec![event], &[]).await?;
        self.wait_applied(recorded[0].position).await?;
        Ok(recorded[0].position)
    }

    /// `GET /v1/admin/bans?state=active|all`.
    pub fn admin_bans(&self, bearer: Option<&str>, state: Option<&str>) -> ApiResult<AdminBans> {
        self.ban_moderator(bearer)?;
        let active_only = match state.unwrap_or("active") {
            "active" => true,
            "all" => false,
            _ => return Err(invalid("state is active or all")),
        };
        let position = self.position();
        let now = now();
        let s = self.bans.read();
        let items = s
            .bans
            .values()
            .rev()
            .filter(|b| !active_only || b.state(now) == BanState::Active)
            .map(|b| AdminBan {
                id: b.position,
                target: self.who_is(b.data.target),
                reason: b.data.reason.clone(),
                until: b.data.until.clone(),
                report: b.data.report,
                by: b.data.by.map(|x| self.who_is(x).gamertag),
                at: b.at.clone(),
                state: b.state(now),
                lifted: b.lifted.as_ref().map(|l| AdminLift {
                    note: l.note.clone(),
                    by: l.by.map(|x| self.who_is(x).gamertag),
                    at: l.at.clone(),
                }),
            })
            .collect();
        Ok(AdminBans { items, position })
    }

    /// `GET /v1/accounts/by-xuid/{xuid}/ban`: for the player themselves or
    /// an admin, with a `read` token.
    pub fn ban_status(&self, bearer: Option<&str>, target: &str) -> ApiResult<BanStatus> {
        let author = self.authenticate(bearer, lxm::READ)?;
        let target = self.ban_target(target)?;
        let mine = matches!(author, Author::Account { xuid, .. } if xuid == target);
        if !mine && !self.admins.is_admin(&author, &self.model.read()) {
            return Err(forbidden("only the player and admins read a ban"));
        }
        let s = self.bans.read();
        let ban = s.in_force(target, now()).map(|b| BanNotice {
            id: b.position,
            reason: b.data.reason.clone(),
            until: b.data.until.clone(),
            at: b.at.clone(),
        });
        Ok(BanStatus {
            banned: ban.is_some(),
            ban,
        })
    }

    /// `GET /v1/accounts/{who}/commendations`.
    pub fn commendations(&self, bearer: Option<&str>, who: &str) -> ApiResult<Commendations> {
        self.authorize_read(bearer)?;
        let xuid = crate::ranks::xuid_of(&self.model.read(), who)
            .ok_or_else(|| not_found("no such account"))?;
        let mut c = Commendations {
            xuid,
            good_attitude: 0,
            good_session: 0,
        };
        for r in self.reports.read().reports.values() {
            if r.data.target != xuid {
                continue;
            }
            match r.data.kind {
                live_proto::reports::Kind::GoodAttitude => c.good_attitude += 1,
                live_proto::reports::Kind::GoodSession => c.good_session += 1,
                _ => {}
            }
        }
        Ok(c)
    }
}

// ---------- route handlers ----------

type Shared = State<Arc<Live>>;

pub(crate) async fn post_ban(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath(xuid): UrlPath<String>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let bytes = body.map_err(|e| invalid(e.body_text()))?;
    let req: BanRequest =
        serde_json::from_slice(&bytes).map_err(|e| invalid(format!("ban: {e}")))?;
    let position = live
        .ban_account(crate::http::bearer(&headers), &xuid, req)
        .await?;
    Ok(crate::http::json(
        StatusCode::CREATED,
        &json!({"ok": true, "position": position}),
    ))
}

pub(crate) async fn post_lift(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath(xuid): UrlPath<String>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let b: LiftRequest = crate::reports::body_json(body)?;
    let position = live
        .lift_ban(crate::http::bearer(&headers), &xuid, &b.note)
        .await?;
    Ok(crate::http::json(
        StatusCode::OK,
        &json!({"ok": true, "position": position}),
    ))
}

pub(crate) async fn get_admin_bans(
    State(live): Shared,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<Response> {
    let view = live.admin_bans(
        crate::http::bearer(&headers),
        q.get("state").map(String::as_str),
    )?;
    Ok(crate::http::json(StatusCode::OK, &view))
}

pub(crate) async fn get_ban(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath(xuid): UrlPath<String>,
) -> ApiResult<Response> {
    let view = live.ban_status(crate::http::bearer(&headers), &xuid)?;
    Ok(crate::http::json(StatusCode::OK, &view))
}

pub(crate) async fn get_commendations(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath(who): UrlPath<String>,
) -> ApiResult<Response> {
    let view = live.commendations(crate::http::bearer(&headers), &who)?;
    Ok(crate::http::json(StatusCode::OK, &view))
}
