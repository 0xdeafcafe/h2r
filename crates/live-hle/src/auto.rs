//! Co-op test automation (`h2 coop test --auto`, docs/live/live-coop.md
//! section 12): the host invites the named friends to its advertised lobby
//! by itself, and a guest accepts an invitation from the named host by
//! itself, as if each had pressed through the friends list. Off unless
//! `H2_COOP_AUTO_INVITE` or `H2_COOP_AUTO_ACCEPT` names someone.
//! `H2_COOP_AUTO_ACCEPT_DELAY` (seconds) holds the accept back that long
//! after the invitation was sent, so `h2 coop test --late` has a guest
//! arrive once the host is in the level (join in progress, section 14).
//!
//! The decisions are here, pure; `State` asks them after each pump of the
//! invitation tasks and does what they say through the same requests the
//! game's own friends list makes.

use std::collections::BTreeSet;

use live_proto::{Xuid, mailbox::Invite};

use crate::Friend;

/// The host's side: gamertags to invite. The guest's side: the gamertag
/// whose invitation to accept.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Auto {
    invite: Vec<String>,
    accept_from: Option<String>,
    /// Seconds after an invitation was sent before it is accepted.
    accept_delay: i64,
    /// (session, xuid) pairs already invited.
    invited: BTreeSet<(String, Xuid)>,
    /// Invitation ids already answered.
    accepted: BTreeSet<u64>,
}

impl Auto {
    pub fn from_env() -> Self {
        Self::from_values(
            std::env::var("H2_COOP_AUTO_INVITE").ok().as_deref(),
            std::env::var("H2_COOP_AUTO_ACCEPT").ok().as_deref(),
        )
        .accept_after(std::env::var("H2_COOP_AUTO_ACCEPT_DELAY").ok().as_deref())
    }

    /// Holds accepts back `secs` seconds (a whole number) after the
    /// invitation was sent. Anything else means at once.
    pub fn accept_after(mut self, secs: Option<&str>) -> Self {
        self.accept_delay = secs
            .and_then(|s| s.trim().parse::<u32>().ok())
            .map_or(0, i64::from);
        self
    }

    /// `invite` is a comma-separated list of gamertags.
    pub fn from_values(invite: Option<&str>, accept_from: Option<&str>) -> Self {
        let invite = invite
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(String::from)
            .collect();
        let accept_from = accept_from
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(String::from);
        Auto {
            invite,
            accept_from,
            ..Default::default()
        }
    }

    pub fn is_on(&self) -> bool {
        !self.invite.is_empty() || self.accept_from.is_some()
    }

    /// The friends to invite now to `session`: named, online, and not yet
    /// invited to it. Marks them invited.
    pub fn invite_now(&mut self, session: &str, friends: &[Friend]) -> Vec<Friend> {
        let mut out = Vec::new();
        for f in friends {
            if !f.online
                || !self
                    .invite
                    .iter()
                    .any(|t| t.eq_ignore_ascii_case(&f.gamertag))
            {
                continue;
            }
            if self.invited.insert((session.to_string(), f.xuid)) {
                out.push(f.clone());
            }
        }
        out
    }

    /// Forgets an invitation that could not be sent, so the next pump tries
    /// again.
    pub fn invite_failed(&mut self, session: &str, xuid: Xuid) {
        self.invited.remove(&(session.to_string(), xuid));
    }

    /// The invitation to accept at `now` (unix seconds): the newest from the
    /// named host not yet answered and sent at least the delay ago. Marks it
    /// answered.
    pub fn accept_now(&mut self, pending: &[Invite], now: i64) -> Option<Invite> {
        let from = self.accept_from.as_ref()?;
        let v = pending
            .iter()
            .filter(|v| v.gamertag.eq_ignore_ascii_case(from) && v.answer.is_none())
            .filter(|v| now >= v.sent_at.saturating_add(self.accept_delay))
            .filter(|v| !self.accepted.contains(&v.id))
            .max_by_key(|v| v.sent_at)?
            .clone();
        self.accepted.insert(v.id);
        Some(v)
    }

    /// Forgets an answer that could not be sent.
    pub fn accept_failed(&mut self, id: u64) {
        self.accepted.remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn friend(x: u64, tag: &str, online: bool) -> Friend {
        Friend {
            xuid: Xuid(x),
            gamertag: tag.into(),
            online,
            playing: false,
            activity: None,
        }
    }

    fn invite(id: u64, tag: &str, at: i64) -> Invite {
        Invite {
            id,
            from: Xuid(1),
            to: Xuid(2),
            gamertag: tag.into(),
            session_id: "8000000000000001".into(),
            sent_at: at,
            answer: None,
        }
    }

    #[test]
    fn off_without_names() {
        assert!(!Auto::from_values(None, None).is_on());
        assert!(!Auto::from_values(Some(" , "), Some(" ")).is_on());
        assert!(Auto::from_values(Some("Bravo"), None).is_on());
        assert!(Auto::from_values(None, Some("Alpha")).is_on());
    }

    #[test]
    fn the_host_invites_each_named_online_friend_once_per_session() {
        let mut a = Auto::from_values(Some("bravo, Charlie"), None);
        let friends = [
            friend(2, "Bravo", true),
            friend(3, "Charlie", false),
            friend(4, "Delta", true),
        ];
        let first = a.invite_now("s1", &friends);
        assert_eq!(first, vec![friend(2, "Bravo", true)]);
        assert!(a.invite_now("s1", &friends).is_empty(), "once");
        // Charlie comes online.
        let later = [friend(2, "Bravo", true), friend(3, "Charlie", true)];
        assert_eq!(a.invite_now("s1", &later), vec![friend(3, "Charlie", true)]);
        // A new lobby: invited again.
        assert_eq!(a.invite_now("s2", &later).len(), 2);
        // A failed send is tried again.
        a.invite_failed("s2", Xuid(2));
        assert_eq!(a.invite_now("s2", &later), vec![friend(2, "Bravo", true)]);
    }

    #[test]
    fn the_guest_accepts_the_newest_invitation_from_the_named_host_once() {
        let mut a = Auto::from_values(None, Some("alpha"));
        let mut answered = invite(9, "Alpha", 50);
        answered.answer = Some(live_proto::mailbox::Answer::Decline);
        let pending = [
            invite(1, "Alpha", 10),
            invite(2, "Alpha", 20),
            invite(3, "Mallory", 30),
            answered,
        ];
        assert_eq!(a.accept_now(&pending, 100).map(|v| v.id), Some(2));
        assert_eq!(a.accept_now(&pending, 100).map(|v| v.id), Some(1));
        assert_eq!(a.accept_now(&pending, 100), None);
        a.accept_failed(2);
        assert_eq!(a.accept_now(&pending, 100).map(|v| v.id), Some(2));
        // No name, nothing accepted.
        assert_eq!(Auto::default().accept_now(&pending, 100), None);
    }

    #[test]
    fn a_late_guest_waits_out_the_delay_after_the_invitation() {
        let pending = [invite(1, "Alpha", 1_000)];
        let mut a = Auto::from_values(None, Some("Alpha")).accept_after(Some("90"));
        assert_eq!(a.accept_now(&pending, 1_000), None);
        assert_eq!(a.accept_now(&pending, 1_089), None);
        assert_eq!(a.accept_now(&pending, 1_090).map(|v| v.id), Some(1));
        // Unset or not a number: at once.
        for d in [None, Some(""), Some("soon"), Some("-5")] {
            let mut a = Auto::from_values(None, Some("Alpha")).accept_after(d);
            assert_eq!(
                a.accept_now(&pending, 1_000).map(|v| v.id),
                Some(1),
                "{d:?}"
            );
        }
    }
}
