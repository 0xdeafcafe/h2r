//! Recipient-owned invitation answers; invitations may only advertise the sender's open lobby.
//! Friends' text messages share the mailbox: the recipient reads and deletes them.
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
    Author, Xuid, lxm,
    mailbox::{self, Answer, Answered, Invite, Mailbox, Sent, TextDeleted, TextMessage, TextSent},
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
    if b.kind == mailbox::TEXT_SENT {
        let d: TextSent =
            serde_json::from_value(b.data.clone()).map_err(|_| invalid("invalid message"))?;
        if d.from != author || d.from == recipient {
            return Err(forbidden(
                "sender must be the signing account and message another account",
            ));
        }
        if !mailbox::valid_text(&d.text) {
            return Err(invalid("message text is empty, too long or has control characters"));
        }
        if !s.is_friends(author, recipient) {
            return Err(forbidden("only friends may message each other"));
        }
        if s.texts.get(&recipient).is_some_and(|v| v.len() >= mailbox::TEXT_BOX_MAX) {
            return Err(conflict("mailbox full"));
        }
    } else if b.kind == mailbox::TEXT_DELETED {
        let d: TextDeleted =
            serde_json::from_value(b.data.clone()).map_err(|_| invalid("invalid deletion"))?;
        if d.by != recipient || author != recipient {
            return Err(forbidden("only the recipient deletes a message"));
        }
        if !s.texts.get(&recipient).is_some_and(|v| v.contains_key(&d.message_id)) {
            return Err(invalid("unknown message"));
        }
    } else if b.kind == mailbox::SENT {
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
    if e.body.kind == mailbox::TEXT_SENT {
        if let Ok(d) = serde_json::from_value::<TextSent>(e.body.data.clone())
            && let Some(a) = s.accounts.get(&d.from)
        {
            let message = TextMessage {
                id: e.position,
                from: d.from,
                to,
                gamertag: a.gamertag.to_string(),
                text: d.text,
                sent_at: at,
            };
            s.texts.entry(to).or_default().insert(e.position, message);
        }
    } else if e.body.kind == mailbox::TEXT_DELETED {
        if let Ok(d) = serde_json::from_value::<TextDeleted>(e.body.data.clone())
            && let Some(v) = s.texts.get_mut(&to)
        {
            v.remove(&d.message_id);
        }
    } else if e.body.kind == mailbox::SENT {
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
    let messages: Vec<_> = state
        .texts
        .get(&xuid)
        .into_iter()
        .flat_map(|v| v.values())
        .rev()
        .take(mailbox::TEXT_BOX_MAX)
        .cloned()
        .collect();
    Ok(crate::http::json(
        StatusCode::OK,
        &Mailbox {
            items,
            messages,
            next: None,
            position: live.position(),
        },
    ))
}
