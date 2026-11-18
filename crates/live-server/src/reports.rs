//! Player reports (docs/live/live-api.md "Player reports",
//! docs/live/live-admin.md "Player reports"): an account reports another
//! from the game's player-details screens or the web, and admins review
//! the reports in a queue, dismissing them or closing them with a note.
//!
//! Both events are server-signed (`PlayerReported`, `ReportReviewed`, on
//! the reported player's `reports-<xuid>` stream) and private: only admins
//! and the operator read them, in the log or here. The reporter is not told
//! what became of a report, and the reported player is never told of it.
//!
//! A reporter files one report of a kind against a player at a time: a
//! repeat while the first is still open answers 200 with the first's
//! position and appends nothing. The two commendation kinds are never
//! reviewed, so each counts once per pair. Reports are also rate limited
//! per account ([`RATE`]), on top of the server's per-method write limit.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};

use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use live_proto::maps::Who;
use live_proto::reports::{
    self, Kind, MAX_TEXT, PLAYER_REPORTED, REPORT_REVIEWED, ReportRequest, Reported, Review,
    Reviewed,
};
use live_proto::{Author, ErrorCode, Xuid, lxm};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::app::{ApiError, Live, conflict, forbidden, invalid, not_found};
use crate::dispatch::Subscriber;
use crate::store::RecordedEvent;

/// Reports per account: per second, burst. Ten at once, then one every
/// six minutes.
pub const RATE: (f64, u32) = (1.0 / 360.0, 10);

type ApiResult<T> = Result<T, ApiError>;

/// A report, as the log says.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub position: u64,
    pub data: Reported,
    pub at: String,
    pub review: Option<ReviewRecord>,
}

/// What an admin did with a report.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewRecord {
    pub action: Review,
    pub note: String,
    pub by: Option<Xuid>,
    pub at: String,
}

/// The reports, by position.
#[derive(Debug, Default)]
pub struct ReportsState {
    pub reports: BTreeMap<u64, Report>,
    /// Open reports by (reporter, player, kind): the dedupe key.
    open: HashMap<(Xuid, Xuid, Kind), u64>,
}

impl ReportsState {
    /// The open report `by` filed of `kind` against `target`, if any.
    pub fn open_report(&self, by: Xuid, target: Xuid, kind: Kind) -> Option<u64> {
        self.open.get(&(by, target, kind)).copied()
    }

    fn apply(&mut self, e: &RecordedEvent) {
        let b = &e.body;
        match b.kind.as_str() {
            PLAYER_REPORTED => {
                if let Ok(d) = serde_json::from_value::<Reported>(b.data.clone()) {
                    self.open
                        .entry((d.by, d.target, d.kind))
                        .or_insert(e.position);
                    self.reports.insert(
                        e.position,
                        Report {
                            position: e.position,
                            data: d,
                            at: b.at.clone(),
                            review: None,
                        },
                    );
                }
            }
            REPORT_REVIEWED => {
                if let Ok(d) = serde_json::from_value::<Reviewed>(b.data.clone())
                    && let Some(r) = self.reports.get_mut(&d.report)
                    && r.review.is_none()
                {
                    r.review = Some(ReviewRecord {
                        action: d.action,
                        note: d.note,
                        by: d.by,
                        at: b.at.clone(),
                    });
                    let key = (r.data.by, r.data.target, r.data.kind);
                    if self.open.get(&key) == Some(&d.report) {
                        self.open.remove(&key);
                    }
                }
            }
            _ => {}
        }
    }
}

/// The reports' projection.
#[derive(Default)]
pub struct Reports {
    state: RwLock<ReportsState>,
}

impl Reports {
    pub fn read(&self) -> std::sync::RwLockReadGuard<'_, ReportsState> {
        self.state.read().unwrap()
    }
}

impl Subscriber for Reports {
    fn name(&self) -> &str {
        "reports"
    }

    fn apply(&self, event: &RecordedEvent) {
        if reports::KINDS.contains(&event.body.kind.as_str()) {
            self.state.write().unwrap().apply(event);
        }
    }
}

/// Which reports the queue lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    /// Complaints not yet reviewed (the default).
    Open,
    /// Reviewed complaints.
    Closed,
    /// Everything, commendations included.
    All,
}

impl Filter {
    fn parse(s: Option<&str>) -> ApiResult<Self> {
        match s.unwrap_or("open") {
            "open" => Ok(Filter::Open),
            "closed" => Ok(Filter::Closed),
            "all" => Ok(Filter::All),
            _ => Err(invalid("state is open, closed or all")),
        }
    }

    fn keeps(self, r: &Report) -> bool {
        match self {
            Filter::Open => r.data.kind.negative() && r.review.is_none(),
            Filter::Closed => r.data.kind.negative() && r.review.is_some(),
            Filter::All => true,
        }
    }
}

/// One report in `GET /v1/admin/reports`.
#[derive(Debug, Serialize)]
pub struct AdminReport {
    pub id: u64,
    pub target: Who,
    pub by: Who,
    pub kind: Kind,
    pub negative: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game_id: Option<String>,
    pub at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<AdminReview>,
}

#[derive(Debug, Serialize)]
pub struct AdminReview {
    pub action: Review,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// The admin's gamertag; none for the operator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    pub at: String,
}

/// A reported player's record, for every player in the listed reports.
#[derive(Debug, Default, Serialize)]
pub struct PlayerRecord {
    /// Complaints against them, open and closed.
    pub complaints: usize,
    pub open: usize,
    pub commendations: usize,
    /// Admins' notes, oldest first.
    pub notes: Vec<Note>,
}

#[derive(Debug, Serialize)]
pub struct Note {
    pub report: u64,
    pub note: String,
    pub at: String,
}

/// `GET /v1/admin/reports`.
#[derive(Debug, Serialize)]
pub struct AdminReports {
    /// Newest first.
    pub items: Vec<AdminReport>,
    /// By the reported player's XUID.
    pub players: BTreeMap<Xuid, PlayerRecord>,
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
    fn who_is(&self, xuid: Xuid) -> Who {
        Who {
            xuid,
            gamertag: self
                .model
                .read()
                .accounts
                .get(&xuid)
                .map(|a| a.gamertag.to_string())
                .unwrap_or_default(),
        }
    }

    /// `POST /v1/accounts/by-xuid/{xuid}/report`: the report's position,
    /// and whether it was already open (nothing appended).
    pub async fn report_player(
        &self,
        bearer: Option<&str>,
        target: &str,
        req: ReportRequest,
    ) -> ApiResult<(u64, bool)> {
        let author = self.authenticate(bearer, lxm::REPORTS_WRITE)?;
        let Author::Account { xuid: by, .. } = author else {
            return Err(forbidden("reports come from accounts"));
        };
        self.rate_limit(&author, lxm::REPORTS_WRITE)?;
        let target = Xuid::parse(target).map_err(|_| not_found("no such account"))?;
        if target == by {
            return Err(invalid("a player can't report themselves"));
        }
        if !self.model.read().accounts.contains_key(&target) {
            return Err(not_found("no such account"));
        }
        let reason = text("a reason", &req.reason)?;
        if let Some(g) = &req.game_id
            && !live_proto::game::valid_id(g)
        {
            return Err(invalid("game_id is not a game id"));
        }
        let _guard = self.report_writes.lock().await;
        if let Some(p) = self.reports.read().open_report(by, target, req.kind) {
            return Ok((p, true));
        }
        if !self.report_rate.allow(&by.to_string()) {
            return Err(ApiError::new(
                ErrorCode::RateLimited,
                "too many reports; try again later",
            ));
        }
        let data = Reported {
            target,
            by,
            kind: req.kind,
            reason,
            game_id: req.game_id,
        };
        let event = self.server_event(
            reports::stream(target),
            PLAYER_REPORTED,
            serde_json::to_value(&data).expect("serializes"),
        );
        let recorded = self.store.append(vec![event], &[]).await?;
        self.wait_applied(recorded[0].position).await?;
        Ok((recorded[0].position, false))
    }

    /// An admin (or the operator) with a `reports.moderate` token: the
    /// admin's XUID, none for the operator.
    fn report_moderator(&self, bearer: Option<&str>) -> ApiResult<Option<Xuid>> {
        let author = self.authenticate(bearer, lxm::REPORTS_MODERATE)?;
        crate::admin::require_admin(self, &author)
            .map_err(|_| forbidden("reading reports needs the operator or an admin"))?;
        self.rate_limit(&author, lxm::REPORTS_MODERATE)?;
        Ok(match author {
            Author::Account { xuid, .. } => Some(xuid),
            Author::Server(_) => None,
        })
    }

    /// `GET /v1/admin/reports?state=open|closed|all`.
    pub fn admin_reports(
        &self,
        bearer: Option<&str>,
        state: Option<&str>,
    ) -> ApiResult<AdminReports> {
        self.report_moderator(bearer)?;
        let filter = Filter::parse(state)?;
        let position = self.position();
        let s = self.reports.read();
        let items: Vec<AdminReport> = s
            .reports
            .values()
            .rev()
            .filter(|r| filter.keeps(r))
            .map(|r| AdminReport {
                id: r.position,
                target: self.who_is(r.data.target),
                by: self.who_is(r.data.by),
                kind: r.data.kind,
                negative: r.data.kind.negative(),
                reason: r.data.reason.clone(),
                game_id: r.data.game_id.clone(),
                at: r.at.clone(),
                review: r.review.as_ref().map(|v| AdminReview {
                    action: v.action,
                    note: v.note.clone(),
                    by: v.by.map(|x| self.who_is(x).gamertag),
                    at: v.at.clone(),
                }),
            })
            .collect();
        let mut players: BTreeMap<Xuid, PlayerRecord> = items
            .iter()
            .map(|i| (i.target.xuid, PlayerRecord::default()))
            .collect();
        for r in s.reports.values() {
            let Some(p) = players.get_mut(&r.data.target) else {
                continue;
            };
            if !r.data.kind.negative() {
                p.commendations += 1;
                continue;
            }
            p.complaints += 1;
            match &r.review {
                None => p.open += 1,
                Some(v) if v.action == Review::Note => p.notes.push(Note {
                    report: r.position,
                    note: v.note.clone(),
                    at: v.at.clone(),
                }),
                Some(_) => {}
            }
        }
        Ok(AdminReports {
            items,
            players,
            position,
        })
    }

    /// `POST /v1/admin/reports/{id}/{dismiss|note}`.
    pub async fn review_report(
        &self,
        bearer: Option<&str>,
        id: &str,
        action: &str,
        note: &str,
    ) -> ApiResult<u64> {
        let by = self.report_moderator(bearer)?;
        let action: Review = action
            .parse()
            .map_err(|_| invalid("the action is dismiss or note"))?;
        let note = text("a note", note)?;
        if action == Review::Note && note.is_empty() {
            return Err(invalid("a note needs its text"));
        }
        let id: u64 = id.parse().map_err(|_| not_found("no such report"))?;
        let _guard = self.report_writes.lock().await;
        let target = {
            let s = self.reports.read();
            let r = s
                .reports
                .get(&id)
                .ok_or_else(|| not_found("no such report"))?;
            if !r.data.kind.negative() {
                return Err(invalid("commendations are not reviewed"));
            }
            if r.review.is_some() {
                return Err(conflict("that report was already reviewed"));
            }
            r.data.target
        };
        let data = Reviewed {
            report: id,
            action,
            note,
            by,
        };
        let event = self.server_event(
            reports::stream(target),
            REPORT_REVIEWED,
            serde_json::to_value(&data).expect("serializes"),
        );
        let recorded = self.store.append(vec![event], &[]).await?;
        self.wait_applied(recorded[0].position).await?;
        Ok(recorded[0].position)
    }
}

// ---------- route handlers ----------

type Shared = State<Arc<Live>>;

fn body_json<T: serde::de::DeserializeOwned + Default>(
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<T> {
    let bytes = body.map_err(|e| invalid(e.body_text()))?;
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice(&bytes).map_err(|e| invalid(format!("body: {e}")))
}

pub(crate) async fn post_report(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath(xuid): UrlPath<String>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let bytes = body.map_err(|e| invalid(e.body_text()))?;
    let req: ReportRequest =
        serde_json::from_slice(&bytes).map_err(|e| invalid(format!("report: {e}")))?;
    let (position, duplicate) = live
        .report_player(crate::http::bearer(&headers), &xuid, req)
        .await?;
    let status = if duplicate {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok(crate::http::json(
        status,
        &json!({"ok": true, "position": position, "duplicate": duplicate}),
    ))
}

pub(crate) async fn get_admin_reports(
    State(live): Shared,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<Response> {
    let view = live.admin_reports(
        crate::http::bearer(&headers),
        q.get("state").map(String::as_str),
    )?;
    Ok(crate::http::json(StatusCode::OK, &view))
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewBody {
    #[serde(default)]
    note: String,
}

pub(crate) async fn post_review(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath((id, action)): UrlPath<(String, String)>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let b: ReviewBody = body_json(body)?;
    let position = live
        .review_report(crate::http::bearer(&headers), &id, &action, &b.note)
        .await?;
    Ok(crate::http::json(
        StatusCode::OK,
        &json!({"ok": true, "position": position}),
    ))
}
