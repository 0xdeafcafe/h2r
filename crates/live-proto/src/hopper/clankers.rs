//! Clanker hoppers (docs/game/bots.md §8, §9.4): the playlists Clankermatch
//! plays, where every other player is a bot the host runs.
//!
//! The mark is not in the game's file: every descriptor byte is a game
//! field. The server keeps it beside the descriptors it writes (the default
//! lineup's [`super::defaults::Playlist::clankers`]) and serves it with the
//! hopper list, by descriptor index, as [`ClankerHoppers`] on
//! `GET /v1/hoppers/clankers`. The console's HLE reads it for the Live
//! menu's Clankermatch row (which lists only these hoppers, and OptiMatch
//! only the others), for the search (a clanker hopper finds no session, so
//! the console hosts) and for the lobby fill (how many bots, how many
//! teams).

use serde::{Deserialize, Serialize};

use super::layout::{Descriptor, Rules};

/// The path, under `/v1`, the marks are served on.
pub const PATH: &str = "/hoppers/clankers";

/// A playlist's clanker mark in the server's hopper source.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clankers {
    /// The playlist id (`+0x08`) of the human hopper whose standing this
    /// one shares; `None`: its own (§9.4 item 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ranks_as: Option<u32>,
}

/// One clanker hopper as served.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClankerHopper {
    /// The descriptor index (0..16), as sessions and searches name the
    /// hopper (docs/live/live-matchmaking.md §6.7).
    pub hopper_id: u32,
    /// The descriptor's playlist id (`+0x08`).
    pub playlist_id: u32,
    /// The most players a game holds: the human and the bots.
    pub players: u32,
    /// The teams; 0 for free for all.
    pub teams: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ranks_as: Option<u32>,
}

impl ClankerHopper {
    /// The mark for descriptor `hopper_id` with these rules.
    pub fn new(hopper_id: u32, playlist_id: u32, rules: &Rules, mark: Clankers) -> Self {
        let (players, teams) = size_of(rules);
        ClankerHopper {
            hopper_id,
            playlist_id,
            players,
            teams,
            ranks_as: mark.ranks_as,
        }
    }

    /// The bots a lobby of `humans` fills up to: the rest of the most
    /// players. Never negative.
    pub fn bots_for(&self, humans: u32) -> u32 {
        self.players.saturating_sub(humans)
    }
}

/// The most players and the teams (0 for free for all) of a hopper's
/// rules.
pub fn size_of(rules: &Rules) -> (u32, u32) {
    match *rules {
        Rules::FreeForAll { players, .. } => (players.1, 0),
        Rules::UnrankedTeams {
            teams, team_size, ..
        }
        | Rules::RankedTeams {
            teams, team_size, ..
        }
        | Rules::Clan {
            teams, team_size, ..
        } => (teams.saturating_mul(team_size.1), teams),
    }
}

/// `GET /v1/hoppers/clankers`: the clanker hoppers of the served file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClankerHoppers {
    #[serde(default)]
    pub hoppers: Vec<ClankerHopper>,
}

impl ClankerHoppers {
    /// The mark of descriptor `hopper_id`, if it is a clanker hopper.
    pub fn get(&self, hopper_id: u32) -> Option<&ClankerHopper> {
        self.hoppers.iter().find(|h| h.hopper_id == hopper_id)
    }

    pub fn contains(&self, hopper_id: u32) -> bool {
        self.get(hopper_id).is_some()
    }

    /// The default lineup's marks ([`super::defaults::playlists`]), kept
    /// only where `served` (the descriptors of the file the server serves)
    /// has, at the same index, a descriptor with the same playlist id: a
    /// published file that moved or replaced a clanker playlist loses its
    /// mark rather than marking a human hopper.
    pub fn of_defaults(served: &[Descriptor]) -> Self {
        let hoppers = super::defaults::playlists()
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                let mark = p.clankers?;
                let d = served.get(i)?;
                if d.playlist_id != p.playlist_id {
                    return None;
                }
                // The published descriptor can resize a playlist in place.
                let hopper = ClankerHopper::new(i as u32, d.playlist_id, &d.rules()?, mark);
                (1..=16).contains(&hopper.players).then_some(hopper)
            })
            .collect();
        ClankerHoppers { hoppers }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_come_from_the_rules() {
        let ffa = Rules::FreeForAll {
            ranked: true,
            players: (2, 8),
        };
        assert_eq!(size_of(&ffa), (8, 0));
        let team = Rules::UnrankedTeams {
            teams: 2,
            team_size: (2, 4),
            uneven: false,
        };
        assert_eq!(size_of(&team), (8, 2));
        let h = ClankerHopper::new(9, 10, &team, Clankers::default());
        assert_eq!(h.bots_for(1), 7);
        assert_eq!(h.bots_for(9), 0);
    }

    #[test]
    fn fill_size_uses_the_served_playlist_rules() {
        let file = super::super::HopperFile::default_set();
        let mut served: Vec<_> = (0..super::super::HOPPERS)
            .map(|i| file.descriptor(i).unwrap())
            .collect();
        let i = ClankerHoppers::of_defaults(&served).hoppers[0].hopper_id;
        served[i as usize].set_rules(&Rules::FreeForAll {
            ranked: false,
            players: (9, 12),
        });
        let mark = ClankerHoppers::of_defaults(&served);
        assert_eq!(mark.get(i).unwrap().bots_for(1), 11);
    }

    #[test]
    fn marks_round_trip_and_look_up_by_index() {
        let set = ClankerHoppers {
            hoppers: vec![ClankerHopper {
                hopper_id: 8,
                playlist_id: 9,
                players: 8,
                teams: 0,
                ranks_as: None,
            }],
        };
        let json = serde_json::to_string(&set).unwrap();
        assert_eq!(
            json,
            r#"{"hoppers":[{"hopper_id":8,"playlist_id":9,"players":8,"teams":0}]}"#
        );
        let back: ClankerHoppers = serde_json::from_str(&json).unwrap();
        assert_eq!(back, set);
        assert!(back.contains(8));
        assert!(!back.contains(0));
        assert_eq!(
            serde_json::from_str::<ClankerHoppers>("{}").unwrap(),
            ClankerHoppers::default()
        );
    }
}
