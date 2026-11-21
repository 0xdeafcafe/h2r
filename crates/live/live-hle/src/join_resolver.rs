//! Resolve a web join as the console's signed-in player. A link has no
//! invitation privileges. Network reads run on the normal Live worker.
use crate::{
    Hle,
    client::{Pending, Reply, Session},
    status::*,
};
use live_proto::{Xuid, mailbox::Invite, session};
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};

pub struct JoinLookup {
    pub(crate) pending: Pending,
    pub(crate) logon: Session,
    pub(crate) controller: usize,
}

pub struct ResolvedJoin {
    pub(crate) invite: Invite,
    pub(crate) logon: Session,
    pub(crate) controller: usize,
}

impl ResolvedJoin {
    pub fn controller(&self) -> u32 {
        self.controller as u32
    }

    pub fn host(&self) -> &str {
        if self.invite.from == Xuid(0) {
            session::DEDICATED_JOIN_HOST
        } else {
            &self.invite.gamertag
        }
    }
}

impl JoinLookup {
    /// Takes the result exactly once. No guest or network I/O.
    pub fn poll(&self) -> Option<Result<ResolvedJoin, String>> {
        Some(match self.pending.take()? {
            Ok(Reply::JoinLink(result)) => result.map(|invite| ResolvedJoin {
                invite,
                logon: self.logon.clone(),
                controller: self.controller,
            }),
            Err(NO_SESSION | NO_USER | INVALID_USER | CONNECTION_LOST) => {
                Err("Sign in to Xbox Live to join this game.".into())
            }
            Err(FRIEND_NOT_FOUND) => Err("This game is no longer available.".into()),
            _ => Err("Could not reach this game on Xbox Live. Try again.".into()),
        })
    }
}

/// Dropping the receipt withdraws an invitation the title has yet to read.
pub struct JoinReceipt(pub(crate) Delivery);
#[derive(Clone, Default)]
pub(crate) struct Delivery {
    state: Arc<AtomicU8>,
}
const WAITING: u8 = 0;
const READ: u8 = 1;
const WITHDRAWN: u8 = 2;

impl Delivery {
    pub(crate) fn take(&self) -> bool {
        self.state
            .compare_exchange(WAITING, READ, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
}

impl JoinReceipt {
    /// The title read the accepted invitation. The host still has to
    /// admit the console to its network session.
    pub fn consumed(&self) -> bool {
        self.0.state.load(Ordering::Acquire) == READ
    }
}
impl Drop for JoinReceipt {
    fn drop(&mut self) {
        let _ =
            self.0
                .state
                .compare_exchange(WAITING, WITHDRAWN, Ordering::AcqRel, Ordering::Acquire);
    }
}

impl Hle {
    pub fn join_link_ready(&self) -> bool {
        self.lock().join_controller().is_some()
    }

    /// The session is resolved on this HLE's configured server, using only
    /// an authenticated player on this console.
    pub fn resolve_join_link(&self, session_id: &str) -> Result<JoinLookup, String> {
        if !session::valid_id(session_id) {
            return Err("This link has an invalid session ID.".into());
        }
        self.lock().resolve_join_link(session_id)
    }

    pub fn accept_join_link(&self, target: ResolvedJoin) -> Result<JoinReceipt, String> {
        self.lock().accept_join_link(target)
    }
}

/// The local accepted record also used when joining through a friend's
/// presence. No mailbox invitation was sent or answered.
pub(crate) fn friend_join(row: session::Session, viewer: Xuid) -> Invite {
    Invite {
        id: 0,
        from: row.host_xuid,
        to: viewer,
        gamertag: row.host_gamertag,
        session_id: row.session_id,
        sent_at: time::OffsetDateTime::now_utc().unix_timestamp(),
        answer: None,
    }
}
