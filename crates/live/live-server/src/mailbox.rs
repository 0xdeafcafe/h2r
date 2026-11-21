//! Recipient-owned invitation answers; invitations may only advertise the sender's open lobby.
use crate::{
    app::{ApiError, Live, conflict, forbidden, invalid},
    model::State,
    sessions::pack_assignment_allows,
    store::RecordedEvent,
};
use axum::{
    extract::{Path, State as HttpState},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use live_proto::{
    Author, Page, Xuid, lxm,
    mailbox::{self, Answer, Answered, Invite, Sent},
    sign::EventBody,
};
use std::sync::Arc;
pub(crate) fn validate(s: &State, b: &EventBody) -> Result<(), ApiError> {
    let recipient = b
        .stream
        .strip_prefix("mailbox-")
        .and_then(|v| Xuid::parse(v).ok())
        .ok_or_else(|| invalid("invalid mailbox stream"))?;
    if b.stream != mailbox::stream(recipient) || !s.accounts.contains_key(&recipient) {
        return Err(invalid("unknown mailbox"));
    }
    let Author::Account { xuid: author, .. } =
        Author::parse(&b.author).ok_or_else(|| forbidden("account required"))?
    else {
        return Err(forbidden("account required"));
    };
    if b.kind == mailbox::SENT {
        let d: Sent =
            serde_json::from_value(b.data.clone()).map_err(|_| invalid("invalid invite"))?;
        if d.from != author || d.from == recipient {
            return Err(forbidden(
                "sender must be the signing account and invite another account",
            ));
        }
        let session = s
            .sessions
            .get(&d.session_id)
            .ok_or_else(|| invalid("unknown session"))?;
        if session.state != "open"
            || session.host_xuid != author
            || !s.presence.contains_key(&author)
        {
            return Err(forbidden(
                "only a signed-in host invites to an open session",
            ));
        }
        if !pack_assignment_allows(session, recipient) {
            return Err(forbidden("recipient has no seat on this Map Pack assignment"));
        }
        if s.mailboxes
            .get(&recipient)
            .into_iter()
            .flat_map(|v| v.values())
            .any(|v| v.from == author && v.answer == Some(Answer::Block))
        {
            return Err(forbidden(
                "recipient blocked game invitations from this account",
            ));
        }
        let pending: Vec<_> = s
            .mailboxes
            .get(&recipient)
            .into_iter()
            .flat_map(|v| v.values())
            .filter(|v| {
                v.answer.is_none()
                    && s.sessions
                        .get(&v.session_id)
                        .is_some_and(|s| s.state == "open" && pack_assignment_allows(s, recipient))
            })
            .collect();
        if pending.len() >= 100
            || pending
                .iter()
                .any(|v| v.from == author && v.session_id == d.session_id)
        {
            return Err(conflict("mailbox full or invite already pending"));
        }
    } else {
        let d: Answered =
            serde_json::from_value(b.data.clone()).map_err(|_| invalid("invalid answer"))?;
        if d.by != recipient || author != recipient {
            return Err(forbidden("only the recipient answers"));
        }
        let invite = s
            .mailboxes
            .get(&recipient)
            .and_then(|v| v.get(&d.invite_id))
            .ok_or_else(|| invalid("unknown invite"))?;
        if invite.session_id != d.session_id || invite.answer.is_some() {
            return Err(conflict("invite is not pending"));
        }
        if d.answer == Answer::Accept {
            let session = s.sessions
                .get(&d.session_id)
                .filter(|v| v.state == "open")
                .ok_or_else(|| conflict("session is closed"))?;
            if !pack_assignment_allows(session, recipient) {
                return Err(forbidden("recipient has no seat on this Map Pack assignment"));
            }
        }
    }
    Ok(())
}
pub(crate) fn project(s: &mut State, e: &RecordedEvent, at: i64) {
    let Some(to) = e
        .body
        .stream
        .strip_prefix("mailbox-")
        .and_then(|v| Xuid::parse(v).ok())
    else {
        return;
    };
    if e.body.kind == mailbox::SENT {
        if let Ok(d) = serde_json::from_value::<Sent>(e.body.data.clone())
            && let Some(a) = s.accounts.get(&d.from)
        {
            s.mailboxes.entry(to).or_default().insert(
                e.position,
                Invite {
                    id: e.position,
                    from: d.from,
                    to,
                    gamertag: a.gamertag.to_string(),
                    session_id: d.session_id,
                    sent_at: at,
                    answer: None,
                },
            );
        }
    } else if let Ok(d) = serde_json::from_value::<Answered>(e.body.data.clone())
        && let Some(v) = s
            .mailboxes
            .get_mut(&to)
            .and_then(|v| v.get_mut(&d.invite_id))
    {
        v.answer = Some(d.answer);
    }
}
pub(crate) async fn read(
    HttpState(live): HttpState<Arc<Live>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let xuid = Xuid::parse(&id).map_err(|_| invalid("invalid recipient"))?;
    let author = live.authenticate(crate::http::bearer(&headers), lxm::READ)?;
    if !matches!(author,Author::Account{xuid:uid,..} if uid==xuid) {
        return Err(forbidden("only the mailbox owner reads it"));
    }
    let state = live.model.read();
    let items: Vec<_> = state
        .mailboxes
        .get(&xuid)
        .into_iter()
        .flat_map(|v| v.values())
        .filter(|v| {
            v.answer.is_none()
                && state
                    .sessions
                    .get(&v.session_id)
                    .is_some_and(|s| s.state == "open" && pack_assignment_allows(s, xuid))
        })
        .rev()
        .take(100)
        .cloned()
        .collect();
    Ok(crate::http::json(
        StatusCode::OK,
        &Page {
            items,
            next: None,
            position: live.position(),
        },
    ))
}
