//! Teams: signed account events on team streams, one deterministic projection,
//! and read endpoints. Writes are serialized by Live across validation/append.
use crate::{
    app::{ApiError, Live, invalid, not_found},
    model::State,
    store::RecordedEvent,
};
use axum::{
    extract::{Path, Query, State as HttpState},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use live_proto::{
    Author, ErrorCode, Gamertag, Page, Xuid,
    sign::EventBody,
    team::{Action, Identity, TeamError},
};
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc};

pub(crate) fn error(e: TeamError) -> ApiError {
    let code = match e {
        TeamError::Privileges | TeamError::NoAdmin | TeamError::NotMember => ErrorCode::Forbidden,
        TeamError::NotFound | TeamError::UserNotFound => ErrorCode::NotFound,
        TeamError::Invalid => ErrorCode::Invalid,
        _ => ErrorCode::Conflict,
    };
    ApiError::new(code, format!("team:{e:?}"))
}
pub(crate) fn actor(s: &State, author: &str) -> Result<Identity, ApiError> {
    let Some(Author::Account { xuid, .. }) = Author::parse(author) else {
        return Err(crate::app::forbidden("team writes require an account"));
    };
    let account = s
        .accounts
        .get(&xuid)
        .ok_or_else(|| not_found("no such account"))?;
    Ok(Identity {
        xuid,
        gamertag: account.gamertag.to_string(),
    })
}
pub(crate) fn validate(s: &State, b: &EventBody) -> Result<(), ApiError> {
    let action: Action =
        serde_json::from_value(b.data.clone()).map_err(|_| invalid("invalid team action"))?;
    if action.kind() != b.kind || b.stream != format!("team-{}", action.id()) {
        return Err(invalid("team action, kind and stream do not agree"));
    }
    let actor = actor(s, &b.author)?;
    if let Action::Recruit { target, .. } = &action {
        let account = s
            .accounts
            .get(&target.xuid)
            .ok_or_else(|| error(TeamError::UserNotFound))?;
        if account.gamertag.as_str() != target.gamertag {
            return Err(invalid("recruit identity does not match the account"));
        }
    }
    s.teams.clone().apply(&actor, &action, &b.at).map_err(error)
}
pub(crate) fn project(s: &mut State, e: &RecordedEvent) {
    let Ok(action) = serde_json::from_value::<Action>(e.body.data.clone()) else {
        return;
    };
    let Ok(actor) = actor(s, &e.body.author) else {
        return;
    };
    let _ = s.teams.apply(&actor, &action, &e.body.at);
}
fn id(text: &str) -> Result<Xuid, ApiError> {
    Xuid::parse(text).map_err(|_| invalid("team/account ids are sixteen hexadecimal digits"))
}
fn tag(text: &str) -> Result<Gamertag, ApiError> {
    Gamertag::new(text).map_err(|_| invalid("invalid gamertag"))
}
type Shared = HttpState<Arc<Live>>;
pub(crate) async fn list(
    HttpState(live): Shared,
    headers: HeaderMap,
    Query(q): Query<BTreeMap<String, String>>,
) -> Result<Response, ApiError> {
    live.authorize_read(crate::http::bearer(&headers))?;
    let after = q
        .get("cursor")
        .map(|s| id(s))
        .transpose()?
        .unwrap_or(Xuid(0));
    let limit = q
        .get("limit")
        .map(|s| s.parse::<usize>())
        .transpose()
        .map_err(|_| invalid("invalid team limit"))?
        .unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(invalid("team limit is 1..100"));
    }
    let s = live.model.read();
    let mut items: Vec<_> = s
        .teams
        .teams
        .values()
        .filter(|t| !t.deleted && t.id > after)
        .take(limit + 1)
        .cloned()
        .collect();
    let next = if items.len() > limit {
        items.truncate(limit);
        items.last().map(|t| t.id.to_string())
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
pub(crate) async fn detail(
    HttpState(live): Shared,
    headers: HeaderMap,
    Path(raw): Path<String>,
) -> Result<Response, ApiError> {
    live.authorize_read(crate::http::bearer(&headers))?;
    let s = live.model.read();
    let team = s.teams.get(id(&raw)?).map_err(error)?;
    Ok(crate::http::json(
        StatusCode::OK,
        &json!({"team":team,"position":live.position()}),
    ))
}
pub(crate) async fn account_teams(
    HttpState(live): Shared,
    headers: HeaderMap,
    Path(raw): Path<String>,
) -> Result<Response, ApiError> {
    live.authorize_read(crate::http::bearer(&headers))?;
    let s = live.model.read();
    let account = s
        .account_by_tag(&tag(&raw)?)
        .ok_or_else(|| not_found("no such account"))?;
    Ok(crate::http::json(
        StatusCode::OK,
        &Page {
            items: s.teams.of(account.xuid),
            next: None,
            position: live.position(),
        },
    ))
}
pub(crate) async fn account_teams_by_id(
    HttpState(live): Shared,
    headers: HeaderMap,
    Path(raw): Path<String>,
) -> Result<Response, ApiError> {
    live.authorize_read(crate::http::bearer(&headers))?;
    let s = live.model.read();
    let uid = id(&raw)?;
    if !s.accounts.contains_key(&uid) {
        return Err(not_found("no such account"));
    }
    Ok(crate::http::json(
        StatusCode::OK,
        &Page {
            items: s.teams.of(uid),
            next: None,
            position: live.position(),
        },
    ))
}
pub(crate) async fn account_identity(
    HttpState(live): Shared,
    headers: HeaderMap,
    Path(raw): Path<String>,
) -> Result<Response, ApiError> {
    live.authorize_read(crate::http::bearer(&headers))?;
    let s = live.model.read();
    let uid = id(&raw)?;
    let account = s
        .accounts
        .get(&uid)
        .ok_or_else(|| not_found("no such account"))?;
    Ok(crate::http::json(
        StatusCode::OK,
        &Identity {
            xuid: uid,
            gamertag: account.gamertag.to_string(),
        },
    ))
}
