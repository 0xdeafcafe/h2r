//! The bot roster (docs/game/bots.md §9.1): which bots a host runs, as the
//! server tells it, in Clankermatch's `MatchProposed` and in the answer to
//! `POST /games/bots`. Bots are accounts: every entry has an xuid the
//! server issued, and the host never changes a bot's identity, rank or
//! skill.
//!
//! A host without a server (System Link, offline) makes its own roster with
//! [`local_xuid`]s, a range no server account is ever in.

use serde::{Deserialize, Serialize};

use crate::{Gamertag, Xuid};

/// The most bots in one roster: a 16-player session with one human.
pub const MAX_BOTS: usize = 15;

/// The high 16 bits of a host's own bot xuids. Server accounts carry
/// `0x0009` (`live_server::generate_xuid`), so no account is ever in this
/// range, and a server refuses it in a game record.
pub const LOCAL_XUID_MARK: u64 = 0x000e_0000_0000_0000;

/// A host-made bot xuid: the local mark, then `n`.
pub fn local_xuid(n: u32) -> Xuid {
    Xuid(LOCAL_XUID_MARK | u64::from(n))
}

/// Whether `x` is a host-made bot xuid.
pub fn is_local_xuid(x: Xuid) -> bool {
    x.0 >> 48 == LOCAL_XUID_MARK >> 48
}

/// The high 16 bits of the server's bot accounts: a range of their own,
/// apart from human accounts' `0x0009`.
pub const SERVER_XUID_MARK: u64 = 0x000b_0000_0000_0000;

/// Whether `x` is in the server's bot range.
pub fn is_server_xuid(x: Xuid) -> bool {
    x.0 >> 48 == SERVER_XUID_MARK >> 48
}

/// The mark a bot's name carries in the game: U+2699 GEAR and a space
/// before its gamertag (docs/game/bots.md §8.2.2). It is not part of the
/// account's name: a record names the bot without it ([`unmarked_units`]),
/// and the site draws its own from the bot flag.
pub const BOT_MARK: char = '\u{2699}';

/// A name's UTF-16 units without a leading [`BOT_MARK`] and the space
/// after it. No player can type the gear, so only a bot's name has it.
pub fn unmarked_units(units: &[u16]) -> &[u16] {
    const MARK: u16 = BOT_MARK as u16;
    match units {
        [MARK, 0x20, rest @ ..] | [MARK, rest @ ..] => rest,
        _ => units,
    }
}

/// `BotIssued` on [`bot_stream`]: the server made a bot account. Data:
/// [`Issued`].
pub const ISSUED: &str = "BotIssued";
/// `BotsRostered` on [`roster_stream`]: the server gave a session's host a
/// roster. Data: [`Rostered`].
pub const ROSTERED: &str = "BotsRostered";
/// Every event type of this module; only the server writes them.
pub const KINDS: [&str; 2] = [ISSUED, ROSTERED];

pub fn bot_stream(xuid: Xuid) -> String {
    format!("bot-{xuid}")
}

pub fn roster_stream(session_id: &str) -> String {
    format!("bots-{session_id}")
}

/// `BotIssued` data: a bot account and how it looks. A persistent bot keeps
/// it for good; a made-up one is issued for one roster.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Issued {
    pub xuid: Xuid,
    pub gamertag: Gamertag,
    pub persistent: bool,
    pub emblem: Emblem,
    pub colors: Colors,
    pub personality: Personality,
}

/// `BotsRostered` data: the bots a session's host may run, from this
/// event on (a later one replaces it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rostered {
    pub session_id: String,
    pub host: Xuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playlist: Option<String>,
    pub bots: Vec<RosteredBot>,
}

/// One bot of a [`Rostered`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RosteredBot {
    pub xuid: Xuid,
    pub persistent: bool,
    /// The level the roster showed: the bot's own (persistent) or the
    /// humans' (made up).
    pub level: u8,
    pub team: Option<u8>,
    pub skill: f32,
}

/// `POST /v1/games/bots`: a host asking for bots for its session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RosterRequest {
    /// The host's Live session (16 lowercase hex digits): the roster is
    /// filed under it, and the game record names it.
    pub session_id: String,
    /// The playlist id (`NewGame.playlist_id`) in matchmaking; absent in a
    /// custom game.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playlist: Option<String>,
    pub count: u32,
    /// 0 to 1; absent: the server picks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill: Option<f32>,
    /// A team game: the server may set teams.
    #[serde(default)]
    pub teams: bool,
    /// The host's previous session (16 hex digits), when the game handed
    /// its lobby to this one (Halo 2 deletes the lobby's session and
    /// advertises a new one as a game starts and ends): the server rosters
    /// the bots of that session's roster again, so they keep their
    /// accounts and names ([`CARRY_WINDOW_S`], docs/game/bots.md §9.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carry_from: Option<String>,
}

/// The first two bytes of every phantom bot's machine id (the host's
/// `bots::phantom::MACHINE_PREFIX`): `C1` has the multicast bit set, so no
/// console's Ethernet address starts with it. A remote player on such a
/// machine is a phantom bot, whichever one.
pub const PHANTOM_MACHINE_PREFIX: [u8; 2] = [0xc1, 0xa4];

/// Whether `machine` is a phantom bot's.
pub fn is_phantom_machine(machine: [u8; 6]) -> bool {
    machine[..2] == PHANTOM_MACHINE_PREFIX
}

/// How long after the host's previous session ended (its last activity:
/// the delete) a new session may carry its roster.
pub const CARRY_WINDOW_S: i64 = 120;

/// The roster: the bots one session runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Roster {
    pub match_id: String,
    pub bots: Vec<RosterBot>,
}

/// One bot in a roster.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RosterBot {
    pub xuid: Xuid,
    pub gamertag: Gamertag,
    /// A persistent bot (ranked, the same account every game), or one made
    /// up for this match.
    pub persistent: bool,
    pub emblem: Emblem,
    pub colors: Colors,
    /// What the lobby shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank: Option<BotRank>,
    /// A team index, or `None` for the host to balance.
    pub team: Option<u8>,
    /// 0 to 1: Easy 0, Normal 0.33, Heroic 0.67, Legendary 1.
    pub skill: f32,
    pub personality: Personality,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Emblem {
    pub foreground: u8,
    pub background: u8,
    pub colors: [u8; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Colors {
    pub primary: u8,
    pub secondary: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotRank {
    pub level: u8,
    pub playlist: String,
}

/// A bot's quirks, each 0 to 1. A persistent bot's are fixed, seed
/// included, so it plays the same way every game.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Personality {
    pub seed: u64,
    pub aggression: f32,
    pub caution: f32,
    pub objective: f32,
    pub teamwork: f32,
    /// 0 never taunts (docs/game/bots.md §3.5).
    pub taunt: f32,
}

impl Default for Personality {
    fn default() -> Self {
        Personality {
            seed: 0,
            aggression: 0.5,
            caution: 0.5,
            objective: 0.5,
            teamwork: 0.5,
            taunt: 0.3,
        }
    }
}

/// The named difficulties' skills (§9.1).
pub const EASY: f32 = 0.0;
pub const NORMAL: f32 = 0.33;
pub const HEROIC: f32 = 0.67;
pub const LEGENDARY: f32 = 1.0;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RosterError {
    #[error("a roster has at most {MAX_BOTS} bots")]
    TooMany,
    #[error("bot {0}: xuid twice in the roster")]
    DuplicateXuid(Xuid),
    #[error("bot {0}: {1} must be 0 to 1")]
    OutOfRange(Xuid, &'static str),
    #[error("bot {0}: a server roster cannot carry a host-made xuid")]
    LocalXuid(Xuid),
}

fn unit(x: Xuid, name: &'static str, v: f32) -> Result<(), RosterError> {
    if (0.0..=1.0).contains(&v) {
        Ok(())
    } else {
        Err(RosterError::OutOfRange(x, name))
    }
}

impl Roster {
    /// The rules of §9.1 a host checks before running a server's roster:
    /// at most [`MAX_BOTS`], each xuid once and none host-made, every
    /// 0-to-1 value in range.
    pub fn check(&self) -> Result<(), RosterError> {
        if self.bots.len() > MAX_BOTS {
            return Err(RosterError::TooMany);
        }
        for (i, b) in self.bots.iter().enumerate() {
            let x = b.xuid;
            if self.bots[..i].iter().any(|o| o.xuid == x) {
                return Err(RosterError::DuplicateXuid(x));
            }
            if is_local_xuid(x) {
                return Err(RosterError::LocalXuid(x));
            }
            unit(x, "skill", b.skill)?;
            let p = &b.personality;
            unit(x, "aggression", p.aggression)?;
            unit(x, "caution", p.caution)?;
            unit(x, "objective", p.objective)?;
            unit(x, "teamwork", p.teamwork)?;
            unit(x, "taunt", p.taunt)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"{
      "match_id": "m1",
      "bots": [
        {
          "xuid": "0009000000000abc",
          "gamertag": "Rusty Lugnut",
          "persistent": true,
          "emblem": { "foreground": 12, "background": 3, "colors": [1, 7, 0] },
          "colors": { "primary": 4, "secondary": 9 },
          "rank": { "level": 17, "playlist": "team-slayer" },
          "team": 1,
          "skill": 0.62,
          "personality": { "seed": 1234567, "aggression": 0.5, "caution": 0.5,
                           "objective": 0.7, "teamwork": 0.5, "taunt": 0.3 }
        },
        {
          "xuid": "0009000000000abd",
          "gamertag": "Bolt",
          "persistent": false,
          "emblem": { "foreground": 0, "background": 0, "colors": [0, 0, 0] },
          "colors": { "primary": 0, "secondary": 0 },
          "team": null,
          "skill": 0,
          "personality": { "seed": 1, "aggression": 1, "caution": 0,
                           "objective": 0, "teamwork": 0, "taunt": 0 }
        }
      ]
    }"#;

    #[test]
    fn the_documented_roster_parses_and_checks() {
        let r: Roster = serde_json::from_str(DOC).unwrap();
        assert_eq!(r.bots.len(), 2);
        assert_eq!(r.bots[0].gamertag.as_str(), "Rusty Lugnut");
        assert_eq!(r.bots[0].team, Some(1));
        assert_eq!(r.bots[1].team, None);
        assert_eq!(r.bots[1].rank, None);
        r.check().unwrap();
        let back: Roster = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn a_bad_gamertag_is_refused_at_parse() {
        let bad = DOC.replace("Rusty Lugnut", "Rusty  Lugnut");
        assert!(serde_json::from_str::<Roster>(&bad).is_err());
    }

    #[test]
    fn check_refuses_duplicates_ranges_local_xuids_and_size() {
        let r: Roster = serde_json::from_str(DOC).unwrap();
        let mut d = r.clone();
        d.bots[1].xuid = d.bots[0].xuid;
        assert_eq!(d.check(), Err(RosterError::DuplicateXuid(d.bots[0].xuid)));
        let mut s = r.clone();
        s.bots[0].skill = 1.5;
        assert_eq!(
            s.check(),
            Err(RosterError::OutOfRange(s.bots[0].xuid, "skill"))
        );
        let mut t = r.clone();
        t.bots[1].personality.taunt = -0.1;
        assert_eq!(
            t.check(),
            Err(RosterError::OutOfRange(t.bots[1].xuid, "taunt"))
        );
        let mut l = r.clone();
        l.bots[0].xuid = local_xuid(3);
        assert_eq!(l.check(), Err(RosterError::LocalXuid(local_xuid(3))));
        let mut big = r.clone();
        big.bots = (0..16)
            .map(|i| {
                let mut b = r.bots[1].clone();
                b.xuid = Xuid(0x0009_0000_0000_1000 + i);
                b
            })
            .collect();
        assert_eq!(big.check(), Err(RosterError::TooMany));
    }

    #[test]
    fn the_three_xuid_ranges_are_apart() {
        let human = Xuid(0x0009_1234_5678_9abc);
        let server = Xuid(SERVER_XUID_MARK | 5);
        assert!(is_server_xuid(server) && !is_local_xuid(server));
        assert!(!is_server_xuid(human) && !is_server_xuid(local_xuid(1)));
        assert_eq!(bot_stream(server), "bot-000b000000000005");
        assert_eq!(roster_stream("0123456789abcdef"), "bots-0123456789abcdef");
    }

    #[test]
    fn local_xuids_never_look_like_accounts() {
        assert!(is_local_xuid(local_xuid(0)));
        assert!(is_local_xuid(local_xuid(u32::MAX)));
        assert!(!is_local_xuid(Xuid(0x0009_ffff_ffff_ffff)));
        assert!(!is_local_xuid(Xuid(0x0009_0000_0000_0001)));
    }

    #[test]
    fn the_request_defaults() {
        let r: RosterRequest =
            serde_json::from_str(r#"{"session_id": "0123456789abcdef", "count": 4}"#).unwrap();
        assert_eq!(
            r,
            RosterRequest {
                session_id: "0123456789abcdef".into(),
                playlist: None,
                count: 4,
                skill: None,
                teams: false,
                carry_from: None,
            }
        );
        let r: RosterRequest = serde_json::from_str(
            r#"{"session_id": "0123456789abcdef", "count": 4, "carry_from": "fedcba9876543210"}"#,
        )
        .unwrap();
        assert_eq!(r.carry_from.as_deref(), Some("fedcba9876543210"));
        assert!(
            !serde_json::to_string(&RosterRequest {
                carry_from: None,
                ..r
            })
            .unwrap()
            .contains("carry_from"),
            "absent when not carrying"
        );
    }

    #[test]
    fn a_bots_name_loses_its_mark() {
        let u = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        assert_eq!(
            unmarked_units(&u("\u{2699} Clanker 3")),
            &u("Clanker 3")[..]
        );
        assert_eq!(
            unmarked_units(&u("\u{2699}Fourteen chars")),
            &u("Fourteen chars")[..]
        );
        assert_eq!(unmarked_units(&u("Sneaky Pete")), &u("Sneaky Pete")[..]);
        assert_eq!(unmarked_units(&u("A \u{2699}")), &u("A \u{2699}")[..]);
        assert!(unmarked_units(&[]).is_empty());
    }
}
