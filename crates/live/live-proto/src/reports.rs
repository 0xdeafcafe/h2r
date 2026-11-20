//! Player reports (docs/live/live-api.md "Player reports"): what a player
//! tells the admins about another player, from the game's player-details
//! screens (`XOnlineFeedbackSend`) or the web, and what the admins did with
//! it. Both events are server-signed, on the reported player's
//! [`stream`], and only admins and the operator read them.
use serde::{Deserialize, Serialize};

use crate::Xuid;

/// `PlayerReported`: an account reported another (server-signed). Data:
/// [`Reported`].
pub const PLAYER_REPORTED: &str = "PlayerReported";
/// `ReportReviewed`: an admin dismissed a report or closed it with a note
/// (server-signed). Data: [`Reviewed`].
pub const REPORT_REVIEWED: &str = "ReportReviewed";

/// Every event type of this module; only the server's own endpoints write
/// them.
pub const KINDS: [&str; 2] = [PLAYER_REPORTED, REPORT_REVIEWED];

/// The stream a player's reports are on.
pub fn stream(target: Xuid) -> String {
    format!("reports-{target}")
}

/// The longest reason or note kept, in characters.
pub const MAX_TEXT: usize = 500;

/// What a report says about the player: the title's feedback types, by
/// the public XDK's `XONLINE_FEEDBACK_TYPE` values. Halo 2 offers ten of
/// them (docs/live/live-boundary.md, "XOnlineFeedbackSend"); the public
/// header's names for 10, 11 and 12 are not confirmed here, so they keep
/// their numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// 0, `NEG_NICKNAME`: an offensive gamertag.
    Gamertag,
    /// 1, `NEG_GAMEPLAY`: unsporting play.
    Gameplay,
    /// 2, `NEG_SCREAMING`.
    Screaming,
    /// 3, `NEG_HARASSMENT`.
    Harassment,
    /// 4, `NEG_LEWDNESS`.
    Lewdness,
    /// 5, `POS_ATTITUDE`: a commendation.
    GoodAttitude,
    /// 6, `POS_SESSION`: a commendation.
    GoodSession,
    /// 10, negative, name unconfirmed.
    #[serde(rename = "type_10")]
    Type10,
    /// 11, negative, name unconfirmed.
    #[serde(rename = "type_11")]
    Type11,
    /// 12, negative, name unconfirmed.
    #[serde(rename = "type_12")]
    Type12,
}

impl Kind {
    /// Every kind, in the order of the game's list (its reason index 0..9).
    pub const GAME_ORDER: [Kind; 10] = [
        Kind::GoodAttitude,
        Kind::GoodSession,
        Kind::Gamertag,
        Kind::Gameplay,
        Kind::Screaming,
        Kind::Harassment,
        Kind::Lewdness,
        Kind::Type12,
        Kind::Type11,
        Kind::Type10,
    ];

    /// The kind of an `XONLINE_FEEDBACK_TYPE` value; None for the values
    /// Halo 2 never sends (7, 8, 9 and anything past 12).
    pub fn from_xdk(value: u32) -> Option<Kind> {
        Some(match value {
            0 => Kind::Gamertag,
            1 => Kind::Gameplay,
            2 => Kind::Screaming,
            3 => Kind::Harassment,
            4 => Kind::Lewdness,
            5 => Kind::GoodAttitude,
            6 => Kind::GoodSession,
            10 => Kind::Type10,
            11 => Kind::Type11,
            12 => Kind::Type12,
            _ => return None,
        })
    }

    /// Its `XONLINE_FEEDBACK_TYPE` value.
    pub fn xdk(self) -> u32 {
        match self {
            Kind::Gamertag => 0,
            Kind::Gameplay => 1,
            Kind::Screaming => 2,
            Kind::Harassment => 3,
            Kind::Lewdness => 4,
            Kind::GoodAttitude => 5,
            Kind::GoodSession => 6,
            Kind::Type10 => 10,
            Kind::Type11 => 11,
            Kind::Type12 => 12,
        }
    }

    /// A complaint, for the admins' queue; the two commendations are not.
    pub fn negative(self) -> bool {
        !matches!(self, Kind::GoodAttitude | Kind::GoodSession)
    }
}

/// `PlayerReported` data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reported {
    /// The player reported.
    pub target: Xuid,
    /// Who reported them.
    pub by: Xuid,
    pub kind: Kind,
    /// The reporter's words (the web asks; the game sends none).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    /// The game the reporter was in, when the game knew one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game_id: Option<String>,
}

/// The body of `POST /v1/accounts/by-xuid/{xuid}/report`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportRequest {
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game_id: Option<String>,
}

/// What an admin did with a report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Review {
    /// Nothing to act on.
    Dismiss,
    /// Closed with a note kept on the player's record.
    Note,
}

impl std::str::FromStr for Review {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "dismiss" => Ok(Review::Dismiss),
            "note" => Ok(Review::Note),
            _ => Err(()),
        }
    }
}

/// `ReportReviewed` data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reviewed {
    /// The report's log position.
    pub report: u64,
    pub action: Review,
    /// Required for a note; optional for a dismissal.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// The admin; none when the operator did it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Xuid>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdk_values_round_trip_and_the_game_list_maps_as_the_title_does() {
        for k in Kind::GAME_ORDER {
            assert_eq!(Kind::from_xdk(k.xdk()), Some(k));
        }
        // The title's table at 0x0008d3d0, by reason index.
        let table = [5, 6, 0, 1, 2, 3, 4, 12, 11, 10];
        for (i, v) in table.into_iter().enumerate() {
            assert_eq!(Kind::GAME_ORDER[i].xdk(), v, "reason {i}");
        }
        for v in [7, 8, 9, 13, u32::MAX] {
            assert_eq!(Kind::from_xdk(v), None, "{v}");
        }
        let negative: Vec<bool> = Kind::GAME_ORDER.iter().map(|k| k.negative()).collect();
        assert_eq!(
            negative,
            [false, false, true, true, true, true, true, true, true, true]
        );
    }

    #[test]
    fn wire_names() {
        assert_eq!(serde_json::to_value(Kind::Type10).unwrap(), "type_10");
        assert_eq!(
            serde_json::to_value(Kind::GoodAttitude).unwrap(),
            "good_attitude"
        );
        let r: ReportRequest = serde_json::from_str(r#"{"kind":"harassment"}"#).unwrap();
        assert_eq!(
            (r.kind, r.reason.as_str(), r.game_id),
            (Kind::Harassment, "", None)
        );
        assert!(serde_json::from_str::<ReportRequest>(r#"{"kind":"nope"}"#).is_err());
        assert!(serde_json::from_str::<ReportRequest>(r#"{"kind":"gameplay","x":1}"#).is_err());
        assert_eq!("note".parse::<Review>(), Ok(Review::Note));
        assert!("ban".parse::<Review>().is_err());
    }
}
