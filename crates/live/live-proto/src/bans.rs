//! Account bans (docs/live/live-admin.md "Bans"): an admin bars an account
//! from writing to the service, for a while or until lifted. Both events
//! are server-signed, on the banned account's [`stream`]; admins, the
//! operator and the banned player read them, nobody else.
//!
//! A ban in force refuses every token the account presents for a write
//! scope with [`crate::ErrorCode::Banned`]; reads still work, so the player
//! can see the notice. A ban with an end date stops being in force when
//! the date passes, with nothing written.
use serde::{Deserialize, Serialize};

use crate::Xuid;

/// `AccountBanned`: an admin banned an account (server-signed). Data:
/// [`Banned`]. A newer ban of the same account replaces the older.
pub const ACCOUNT_BANNED: &str = "AccountBanned";
/// `AccountUnbanned`: an admin lifted a ban (server-signed). Data:
/// [`Unbanned`].
pub const ACCOUNT_UNBANNED: &str = "AccountUnbanned";

/// Every event type of this module; only the server's own endpoints write
/// them.
pub const KINDS: [&str; 2] = [ACCOUNT_BANNED, ACCOUNT_UNBANNED];

/// The stream an account's bans are on.
pub fn stream(target: Xuid) -> String {
    format!("bans-{target}")
}

/// The longest reason or note kept, in characters.
pub const MAX_TEXT: usize = 500;

/// Token scopes a banned account may still use: reading, its sign-in
/// code secret, its sign-ins (seeing them and signing others out), its
/// cloud saves and its own logs.
pub const STILL_ALLOWED: [&str; 6] = [
    crate::lxm::READ,
    crate::lxm::RECOVERY_READ,
    crate::lxm::SIGNINS_READ,
    crate::lxm::SIGNINS_WRITE,
    crate::lxm::STORAGE_READ,
    crate::lxm::LOG_READ,
];

/// Whether a ban refuses tokens for `method`.
pub fn bars(method: &str) -> bool {
    !STILL_ALLOWED.contains(&method)
}

/// `AccountBanned` data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Banned {
    pub target: Xuid,
    /// What the player is told.
    pub reason: String,
    /// When it ends (RFC 3339); none for a ban until lifted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
    /// The player report it answers, by log position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<u64>,
    /// The admin; none when the operator did it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Xuid>,
}

/// `AccountUnbanned` data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unbanned {
    pub target: Xuid,
    /// The ban lifted, by log position.
    pub ban: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Xuid>,
}

/// The body of `POST /v1/admin/bans/{xuid}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BanRequest {
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<u64>,
}

/// The body of `POST /v1/admin/bans/{xuid}/lift`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiftRequest {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// A ban as the player sees it (`GET /v1/accounts/by-xuid/{xuid}/ban`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BanNotice {
    /// The ban's log position.
    pub id: u64,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
    /// When it was made (RFC 3339).
    pub at: String,
}

/// `GET /v1/accounts/by-xuid/{xuid}/ban`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BanStatus {
    pub banned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ban: Option<BanNotice>,
}

/// `GET /v1/accounts/{who}/commendations`: how many players commended the
/// account, by kind (from the game's player-details screen or the web).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Commendations {
    pub xuid: Xuid,
    pub good_attitude: u64,
    pub good_session: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lxm;

    #[test]
    fn bans_bar_writes_and_leave_reads() {
        for m in [
            lxm::EVENTS_WRITE,
            lxm::SESSIONS_WRITE,
            lxm::SESSIONS_RELAY,
            lxm::REPORTS_WRITE,
            lxm::STORAGE_WRITE,
            lxm::RECOVERY_WRITE,
            lxm::CONTENT_WRITE,
            lxm::LOG_WRITE,
            "anything.new",
        ] {
            assert!(bars(m), "{m}");
        }
        for m in [
            lxm::READ,
            lxm::RECOVERY_READ,
            lxm::SIGNINS_READ,
            lxm::SIGNINS_WRITE,
            lxm::STORAGE_READ,
            lxm::LOG_READ,
        ] {
            assert!(!bars(m), "{m}");
        }
    }

    #[test]
    fn wire_shapes() {
        let r: BanRequest = serde_json::from_str(r#"{"reason":"cheating"}"#).unwrap();
        assert_eq!(
            (r.reason.as_str(), r.until, r.report),
            ("cheating", None, None)
        );
        assert!(serde_json::from_str::<BanRequest>(r#"{"reason":"x","days":3}"#).is_err());
        assert!(serde_json::from_str::<BanRequest>(r#"{}"#).is_err());
        let b = Banned {
            target: Xuid(0x0009_0000_0000_0001),
            reason: "r".into(),
            until: None,
            report: None,
            by: None,
        };
        assert_eq!(
            serde_json::to_value(&b).unwrap(),
            serde_json::json!({"target": "0009000000000001", "reason": "r"})
        );
        assert_eq!(stream(b.target), "bans-0009000000000001");
    }
}
