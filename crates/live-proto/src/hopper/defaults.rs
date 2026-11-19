//! The playlists a server offers with no hopper file published: a classic
//! Halo 2 lineup built from nothing but the format ([`super::layout`]) and
//! numbers set here. No bytes come from a disc or a real hopper file.
//!
//! What the game itself does is the model. Its compiled-in defaults
//! (`0x001940d0`, used when no file was ever downloaded) build seven
//! hoppers the same way: a descriptor per hopper (`0x00193c70`), the
//! start-wait table from a base wait and the player range
//! ([`super::layout::start_waits`]), the skill ceiling
//! ([`super::layout::default_skill_ceiling`]), status 0, and entries whose
//! variants are the stock record of a game type plus a few changes
//! (`0x00212f40`), with settings 12..14 cleared (`0x00193010`). The
//! timings here are the game's (30 s base wait and 150 s search timeout
//! for small games, 45 s and 300 s for big team games); the playlists,
//! their sizes, maps and weights are this project's choice.
//!
//! After the eight human playlists come three clanker playlists
//! (docs/game/bots.md §8, §9.4 item 1): Clankermatch hoppers, where every
//! other player is a bot the host runs. Their descriptors are ordinary ones;
//! what makes them clanker hoppers is [`Playlist::clankers`], which the
//! server keeps beside the file and serves as
//! [`super::clankers::ClankerHoppers`].
//!
//! Map pack maps are left out: an entry whose map id the console does not
//! have marks the whole hopper as an error (`0x00194660`), and the format
//! has no per-entry "only when installed" flag.

use super::clankers::Clankers;
use super::layout::{Descriptor, Entry, PLAYLIST_IDS, Rules, default_skill_ceiling, start_waits};
use super::variant::{self as v, GameType, VariantRecord};
use super::{
    BLOCK, BLOCKS_AT, DESCRIPTOR, HOPPERS, HopperError, HopperFile, MAX_DESCRIPTION, MAX_TITLE,
};

/// The retail multiplayer levels by map id: the ids the game's level list
/// (the globals tag, `matg +0x178`, element 0, block `+0x10`, stride
/// `0xc64`) gives each map, which is what an entry's `+0x04` must match.
pub const RETAIL_MAPS: [(u32, &str); 12] = [
    (10, "Ivory Tower"),
    (20, "Midship"),
    (30, "Zanzibar"),
    (40, "Waterworks"),
    (50, "Lockout"),
    (60, "Burial Mounds"),
    (70, "Colossus"),
    (80, "Ascension"),
    (100, "Beaver Creek"),
    (110, "Coagulation"),
    (120, "Foundation"),
    (800, "Headlong"),
];

pub const IVORY_TOWER: u32 = 10;
pub const MIDSHIP: u32 = 20;
pub const ZANZIBAR: u32 = 30;
pub const WATERWORKS: u32 = 40;
pub const LOCKOUT: u32 = 50;
pub const BURIAL_MOUNDS: u32 = 60;
pub const COLOSSUS: u32 = 70;
pub const ASCENSION: u32 = 80;
pub const BEAVER_CREEK: u32 = 100;
pub const COAGULATION: u32 = 110;
pub const FOUNDATION: u32 = 120;
pub const HEADLONG: u32 = 800;

/// The retail map ids, for [`HopperFile::problems`].
pub fn retail_map_ids() -> Vec<u32> {
    RETAIL_MAPS.iter().map(|(id, _)| *id).collect()
}

/// The game's base wait and search timeout for small games, ms.
const SMALL_WAIT: u32 = 30_000;
const SMALL_TIMEOUT: u32 = 150_000;
/// And for big team games.
const BIG_WAIT: u32 = 45_000;
const BIG_TIMEOUT: u32 = 300_000;

/// The matchmaking variants the default playlists use: the stock record of
/// the type, then each one's changes.
pub mod variants {
    use super::*;

    fn mm(game: GameType, name: &str, team: bool) -> VariantRecord {
        let mut r = VariantRecord::stock(game, name);
        r.set_team(team);
        r
    }

    /// Free for all Slayer, a six-minute limit.
    pub fn rumble_slayer() -> VariantRecord {
        let mut r = mm(GameType::Slayer, "Rumble Slayer", false);
        r.put_i32(v::TIME_LIMIT, 360);
        r.for_matchmaking()
    }

    pub fn team_slayer() -> VariantRecord {
        mm(GameType::Slayer, "Team Slayer", true).for_matchmaking()
    }

    /// Team Slayer with sniper rifles: no vehicles or turrets, no motion
    /// sensor, a sniper loadout (the item numbers the game's own Snipers
    /// variant uses).
    pub fn team_snipers() -> VariantRecord {
        let mut r = mm(GameType::Slayer, "Team Snipers", true);
        r.set_flag(v::flag::MOTION_SENSOR, false);
        r.put_u8(v::WEAPONS_ON_MAP, 12);
        r.put_u8(v::STARTING_WEAPON, 10);
        r.put_u8(v::SECONDARY_WEAPON, 4);
        for at in [v::LIGHT_VEHICLE, v::SECOND_LIGHT_VEHICLE] {
            r.put_u8(at, 7);
        }
        for at in [v::HEAVY_VEHICLE, v::FLYING_VEHICLE, v::SECOND_HEAVY_VEHICLE] {
            r.put_u8(at, 4);
        }
        for at in [v::PRIMARY_TURRET, v::SECONDARY_TURRET] {
            r.put_u8(at, 6);
        }
        r.for_matchmaking()
    }

    pub fn oddball() -> VariantRecord {
        let mut r = mm(GameType::Oddball, "Oddball", false);
        r.put_i32(v::TIME_LIMIT, 360);
        r.for_matchmaking()
    }

    pub fn king_of_the_hill() -> VariantRecord {
        let mut r = mm(GameType::King, "King of the Hill", false);
        r.put_i32(v::TIME_LIMIT, 360);
        r.for_matchmaking()
    }

    /// CTF with the type's two extra flags on, as the game's Classic CTF.
    pub fn classic_ctf() -> VariantRecord {
        let mut r = mm(GameType::Ctf, "Classic CTF", true);
        let f = r.u32_at(v::TYPE_AREA);
        r.put_u32(v::TYPE_AREA, f | 0xc);
        r.for_matchmaking()
    }

    /// Two rounds, one score each, three minutes a round.
    pub fn assault() -> VariantRecord {
        let mut r = mm(GameType::Assault, "Assault", true);
        r.put_i32(v::ROUNDS, 2);
        r.put_i32(v::SCORE_TO_WIN, 1);
        r.put_i32(v::TIME_LIMIT, 180);
        r.put_i32(v::TYPE_AREA + 0x14, 1);
        r.for_matchmaking()
    }

    pub fn territories() -> VariantRecord {
        mm(GameType::Territories, "Territories", true).for_matchmaking()
    }

    pub fn team_king() -> VariantRecord {
        let mut r = mm(GameType::King, "Team King", true);
        r.put_i32(v::SETTING_25, 0);
        r.put_i32(v::RESPAWN_TIME, 10);
        r.for_matchmaking()
    }

    pub fn teamball() -> VariantRecord {
        let mut r = mm(GameType::Oddball, "Teamball", true);
        r.put_i32(v::SETTING_25, 0);
        r.put_i32(v::RESPAWN_TIME, 10);
        r.for_matchmaking()
    }
}

/// The built-in matchmaking variants by key: what an entry can be given
/// without copying another entry's 0x130 bytes (the playlist builders'
/// variant picker, `variant` in the admin PUT body). Each is the stock
/// record of its game type with this module's changes; none comes from a
/// disc.
pub fn builtin_variants() -> Vec<(&'static str, VariantRecord)> {
    use variants::*;
    vec![
        ("rumble_slayer", rumble_slayer()),
        ("team_slayer", team_slayer()),
        ("team_snipers", team_snipers()),
        ("oddball", oddball()),
        ("teamball", teamball()),
        ("king_of_the_hill", king_of_the_hill()),
        ("team_king", team_king()),
        ("classic_ctf", classic_ctf()),
        ("assault", assault()),
        ("territories", territories()),
    ]
}

/// The built-in variant named `key` ([`builtin_variants`]).
pub fn builtin_variant(key: &str) -> Option<VariantRecord> {
    builtin_variants()
        .into_iter()
        .find_map(|(k, v)| (k == key).then_some(v))
}

/// The key of a built-in variant with exactly these bytes, if any.
pub fn builtin_key(v: &VariantRecord) -> Option<&'static str> {
    builtin_variants()
        .into_iter()
        .find_map(|(k, b)| (b == *v).then_some(k))
}

/// The default lineup's human playlists, offered as templates for a new
/// playlist: each gives the descriptor (type, team and party sizes, waits,
/// timeout, skill ceiling) a playlist of its kind needs to pass the game's
/// validator, which the editors' raw fields cannot build. The clanker
/// playlists are left out: their mark is kept by playlist id and index
/// ([`super::clankers`]), which a copy elsewhere would not have.
pub fn templates() -> Vec<Playlist> {
    playlists()
        .into_iter()
        .filter(|p| p.clankers.is_none())
        .collect()
}

/// The template whose default playlist id is `playlist_id`.
pub fn template(playlist_id: u32) -> Option<Playlist> {
    templates()
        .into_iter()
        .find(|p| p.playlist_id == playlist_id)
}

/// One entry of a default playlist: `(variant, map id, weight)`.
pub type DefaultEntry = (fn() -> VariantRecord, u32, u32);

/// One default playlist, as written into the file.
pub struct Playlist {
    pub playlist_id: u32,
    pub title: &'static str,
    pub description: &'static str,
    pub rules: Rules,
    pub base_wait_ms: u32,
    pub search_timeout_ms: u32,
    pub entries: Vec<DefaultEntry>,
    /// A clanker hopper's mark (docs/game/bots.md §9.4 item 1): not a game
    /// field, kept beside the descriptor and served on
    /// `GET /v1/hoppers/clankers`. `None` for a human hopper.
    pub clankers: Option<Clankers>,
}

impl Playlist {
    /// The descriptor this playlist writes.
    pub fn descriptor(&self) -> Descriptor {
        let mut d = Descriptor {
            playlist_id: self.playlist_id,
            skill_ceiling: default_skill_ceiling(),
            search_timeout_ms: self.search_timeout_ms,
            ..Descriptor::default()
        };
        d.set_rules(&self.rules);
        d.set_text(self.title, self.description);
        let (teams, size) = match self.rules {
            Rules::FreeForAll { players, .. } => (1, players),
            Rules::UnrankedTeams {
                teams, team_size, ..
            }
            | Rules::RankedTeams {
                teams, team_size, ..
            }
            | Rules::Clan {
                teams, team_size, ..
            } => (teams, team_size),
        };
        d.start_wait_ms = start_waits(self.base_wait_ms, teams, size);
        d
    }

    pub fn entries(&self) -> Vec<Entry> {
        self.entries
            .iter()
            .map(|(variant, map_id, weight)| Entry {
                map_id: *map_id,
                weight: *weight,
                map_name: String::new(),
                variant: variant(),
            })
            .collect()
    }
}

fn team(
    teams: u32,
    size: (u32, u32),
    party: (u32, u32),
    party_as_team: Option<(u32, u32)>,
) -> Rules {
    Rules::RankedTeams {
        teams,
        team_size: size,
        max_difference: 0,
        party,
        party_as_team,
    }
}

/// The default lineup, in hopper order. Playlist ids are 1.. in order.
pub fn playlists() -> Vec<Playlist> {
    use variants::*;
    let small_ffa = [
        LOCKOUT,
        MIDSHIP,
        IVORY_TOWER,
        BEAVER_CREEK,
        ASCENSION,
        FOUNDATION,
    ];
    let small_team = [
        LOCKOUT,
        MIDSHIP,
        IVORY_TOWER,
        BEAVER_CREEK,
        ASCENSION,
        COLOSSUS,
        FOUNDATION,
    ];
    let big = [COAGULATION, HEADLONG, ZANZIBAR, WATERWORKS, BURIAL_MOUNDS];
    let on = |v: fn() -> VariantRecord, maps: &[u32], w: u32| {
        maps.iter().map(|m| (v, *m, w)).collect::<Vec<_>>()
    };
    vec![
        Playlist {
            playlist_id: 1,
            title: "Rumble Pit",
            description: "Every player for themselves, up to 8. Slayer, with some Oddball and King of the Hill.",
            rules: Rules::FreeForAll {
                ranked: true,
                players: (4, 8),
            },
            base_wait_ms: SMALL_WAIT,
            search_timeout_ms: SMALL_TIMEOUT,
            entries: [
                on(rumble_slayer, &small_ffa, 10),
                on(oddball, &[LOCKOUT, MIDSHIP], 3),
                on(king_of_the_hill, &[IVORY_TOWER, ASCENSION], 3),
            ]
            .concat(),
            clankers: None,
        },
        Playlist {
            playlist_id: 2,
            title: "Team Slayer",
            description: "Four against four Slayer on the small and medium maps.",
            rules: team(2, (3, 4), (1, 4), Some((3, 4))),
            base_wait_ms: SMALL_WAIT,
            search_timeout_ms: SMALL_TIMEOUT,
            entries: [
                on(team_slayer, &small_team, 10),
                on(team_slayer, &[ZANZIBAR], 4),
            ]
            .concat(),
            clankers: None,
        },
        Playlist {
            playlist_id: 3,
            title: "Team Skirmish",
            description: "Two against two. Bring a partner or find one here.",
            rules: team(2, (2, 2), (1, 2), Some((2, 2))),
            base_wait_ms: SMALL_WAIT,
            search_timeout_ms: SMALL_TIMEOUT,
            entries: [
                on(
                    team_slayer,
                    &[LOCKOUT, MIDSHIP, IVORY_TOWER, BEAVER_CREEK, FOUNDATION],
                    10,
                ),
                on(teamball, &[MIDSHIP, LOCKOUT], 3),
            ]
            .concat(),
            clankers: None,
        },
        Playlist {
            playlist_id: 4,
            title: "Team Snipers",
            description: "Four against four with sniper rifles and no vehicles.",
            rules: team(2, (3, 4), (1, 4), None),
            base_wait_ms: SMALL_WAIT,
            search_timeout_ms: SMALL_TIMEOUT,
            entries: on(
                team_snipers,
                &[
                    ZANZIBAR,
                    COAGULATION,
                    BURIAL_MOUNDS,
                    ASCENSION,
                    COLOSSUS,
                    LOCKOUT,
                    HEADLONG,
                ],
                10,
            ),
            clankers: None,
        },
        Playlist {
            playlist_id: 5,
            title: "Big Team Battle",
            description: "Up to eight a side on the big maps: Slayer, CTF, Assault and Territories.",
            rules: team(2, (5, 8), (1, 5), None),
            base_wait_ms: BIG_WAIT,
            search_timeout_ms: BIG_TIMEOUT,
            entries: [
                on(team_slayer, &big, 10),
                on(
                    classic_ctf,
                    &[COAGULATION, ZANZIBAR, HEADLONG, WATERWORKS],
                    6,
                ),
                on(assault, &[ZANZIBAR, COAGULATION, BURIAL_MOUNDS], 4),
                on(territories, &[HEADLONG, WATERWORKS], 4),
            ]
            .concat(),
            clankers: None,
        },
        Playlist {
            playlist_id: 6,
            title: "Team Objective",
            description: "Four against four: Capture the Flag, Assault, Territories, King and Oddball.",
            rules: team(2, (3, 4), (1, 4), Some((3, 4))),
            base_wait_ms: SMALL_WAIT,
            search_timeout_ms: SMALL_TIMEOUT,
            entries: [
                on(
                    classic_ctf,
                    &[BEAVER_CREEK, MIDSHIP, COLOSSUS, ASCENSION],
                    6,
                ),
                on(assault, &[MIDSHIP, BEAVER_CREEK], 4),
                on(territories, &[LOCKOUT, ASCENSION, COLOSSUS], 4),
                on(team_king, &[IVORY_TOWER, MIDSHIP], 3),
                on(teamball, &[LOCKOUT, BEAVER_CREEK], 3),
            ]
            .concat(),
            clankers: None,
        },
        Playlist {
            playlist_id: 7,
            title: "Free for All",
            description: "Unranked free for all, 2 to 8 players. Parties welcome.",
            rules: Rules::FreeForAll {
                ranked: false,
                players: (2, 8),
            },
            base_wait_ms: SMALL_WAIT,
            search_timeout_ms: SMALL_TIMEOUT,
            entries: [
                on(rumble_slayer, &small_ffa, 10),
                on(oddball, &[MIDSHIP], 3),
                on(king_of_the_hill, &[ASCENSION], 3),
            ]
            .concat(),
            clankers: None,
        },
        Playlist {
            playlist_id: 8,
            title: "Unranked Teams",
            description: "Unranked team games for parties of any size, 2 to 4 a side.",
            rules: Rules::UnrankedTeams {
                teams: 2,
                team_size: (2, 4),
                uneven: true,
            },
            base_wait_ms: SMALL_WAIT,
            search_timeout_ms: SMALL_TIMEOUT,
            entries: [
                on(team_slayer, &small_team, 10),
                on(classic_ctf, &[BEAVER_CREEK, MIDSHIP], 4),
                on(team_king, &[IVORY_TOWER], 3),
                on(teamball, &[LOCKOUT], 3),
            ]
            .concat(),
            clankers: None,
        },
        // Clankermatch (docs/game/bots.md §8): one human (or a party) and
        // bots up to the most players. All three are unranked hoppers: the
        // descriptor's rules are what the game's own search and lobby read,
        // and a ranked type's party and team-size rules (ranked teams need
        // 2..=size a team and keep parties within limits; ranked free for
        // all is solo) are made for filling a game with humans. An
        // unranked type lets a lone human host and the bots fill every
        // other seat, with uneven teams while they join. The server still
        // ranks a Clankermatch game (bots.md §8.2 item 1, by its own kind)
        // from the game record, not from the hopper type. Each keeps its
        // own standing (`ranks_as: None`) until a test of the validator
        // and playlist UI says duplicate playlist ids are fine (§9.4).
        Playlist {
            playlist_id: 9,
            title: "Clanker Slayer",
            description: "Free for all Slayer, up to 8 players, where every other player is a bot.",
            rules: Rules::FreeForAll {
                ranked: false,
                players: (2, 8),
            },
            base_wait_ms: SMALL_WAIT,
            search_timeout_ms: SMALL_TIMEOUT,
            entries: on(rumble_slayer, &small_ffa, 10),
            clankers: Some(Clankers::default()),
        },
        Playlist {
            playlist_id: 10,
            title: "Clanker Teams",
            description: "Four against four Team Slayer. Bots fill every seat your party does not.",
            rules: Rules::UnrankedTeams {
                teams: 2,
                team_size: (1, 4),
                uneven: true,
            },
            base_wait_ms: SMALL_WAIT,
            search_timeout_ms: SMALL_TIMEOUT,
            entries: on(team_slayer, &small_team, 10),
            clankers: Some(Clankers::default()),
        },
        Playlist {
            playlist_id: 11,
            title: "Clanker Doubles",
            description: "Two against two Team Slayer. Bots fill every seat your party does not.",
            rules: Rules::UnrankedTeams {
                teams: 2,
                team_size: (1, 2),
                uneven: true,
            },
            base_wait_ms: SMALL_WAIT,
            search_timeout_ms: SMALL_TIMEOUT,
            entries: on(
                team_slayer,
                &[LOCKOUT, MIDSHIP, IVORY_TOWER, BEAVER_CREEK, FOUNDATION],
                10,
            ),
            clankers: Some(Clankers::default()),
        },
    ]
}

impl HopperFile {
    /// Whether hopper `i` is an unused slot: type 0, which the game reads
    /// as no hopper (every reader goes through the validator, which refuses
    /// it). The default set leaves hoppers 11..15 so.
    pub fn is_unused(&self, i: usize) -> Result<bool, HopperError> {
        Ok(self.descriptor(i)?.hopper_type == 0)
    }

    /// The lowest playlist id (1..=100) no other used hopper has; the id is
    /// the base of the playlist's stats ids, so it must be unique.
    pub fn free_playlist_id(&self, i: usize) -> Option<u32> {
        let taken: Vec<u32> = (0..HOPPERS)
            .filter(|&j| j != i)
            .filter_map(|j| self.descriptor(j).ok())
            .filter(|d| d.hopper_type != 0)
            .map(|d| d.playlist_id)
            .collect();
        PLAYLIST_IDS.into_iter().find(|id| !taken.contains(id))
    }

    /// Makes hopper `i` a playlist of `template`'s kind: its whole
    /// descriptor (title and description included), status 0, with a
    /// playlist id no other hopper uses. The entries are left as they are:
    /// write at least one before publishing.
    pub fn create_from(&mut self, i: usize, template: &Playlist) -> Result<(), HopperError> {
        let mut d = template.descriptor();
        d.playlist_id = self.free_playlist_id(i).ok_or(HopperError::NoPlaylistId)?;
        self.set_descriptor(i, &d)
    }

    /// Makes hopper `i` an unused slot again: its descriptor and data block
    /// all zero, as [`HopperFile::default_set`] leaves the slots it does not
    /// fill.
    pub fn clear_hopper(&mut self, i: usize) -> Result<(), HopperError> {
        if i >= HOPPERS {
            return Err(HopperError::Hopper(i));
        }
        let image = self.image_mut();
        image[i * DESCRIPTOR..(i + 1) * DESCRIPTOR].fill(0);
        let block = BLOCKS_AT + i * BLOCK;
        image[block..block + BLOCK].fill(0);
        Ok(())
    }

    /// A complete file built from scratch: [`playlists`] in hoppers
    /// 0.., every other byte zero (unused hopper slots are all zero, which
    /// the game reads as no hopper).
    pub fn default_set() -> HopperFile {
        let mut file = HopperFile::blank();
        for (i, p) in playlists().iter().enumerate() {
            debug_assert!(p.title.len() <= MAX_TITLE && p.description.len() <= MAX_DESCRIPTION);
            file.set_descriptor(i, &p.descriptor())
                .expect("the default descriptors fit");
            file.set_entries(i, &p.entries())
                .expect("the default entries fit");
        }
        file
    }
}

#[cfg(test)]
mod tests {
    use super::super::layout::{HopperType, check_descriptor};
    use super::super::{BLOCK, BLOCKS_AT, DESCRIPTOR, HOPPERS, MAX_FILE};
    use super::*;

    #[test]
    fn the_default_set_passes_every_check_the_game_makes() {
        let file = HopperFile::default_set();
        assert_eq!(file.problems(&retail_map_ids()), vec![]);
        let lineup = playlists();
        let mut ids = Vec::new();
        for (i, p) in lineup.iter().enumerate() {
            let d = file.descriptor(i).unwrap();
            assert_eq!(check_descriptor(&d), vec![], "{}", p.title);
            assert_eq!(d.status, 0, "the game checks the maps itself");
            assert_eq!(d.rules(), Some(p.rules));
            assert_eq!(d.titles.iter().filter(|t| *t == p.title).count(), 9);
            assert!(p.title.len() <= MAX_TITLE && p.description.len() <= MAX_DESCRIPTION);
            ids.push(d.playlist_id);
            // Unused fields are zero.
            assert_eq!((d.base_wait_ms, d.min_rank, d.list_any_rank), (0, 0, false));
            let entries = file.entries(i).unwrap();
            assert_eq!(entries, p.entries());
            assert!((1..=100).contains(&entries.len()));
            for e in &entries {
                assert!(e.map_name.is_empty());
                assert_eq!(e.variant.problems(), vec![]);
                let teams = d.kind().unwrap().teams();
                assert_eq!(e.variant.team(), teams, "{} on {}", p.title, e.map_id);
            }
            // The old view agrees.
            let h = file.hopper(i).unwrap();
            assert_eq!(h.record_count as usize, entries.len());
            assert_eq!(h.name, "");
            assert_eq!(file.text(i).unwrap().title, p.title);
        }
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), lineup.len(), "playlist ids are unique");
        // The rest of the image is zero.
        let img = file.image();
        assert!(
            img[lineup.len() * DESCRIPTOR..BLOCKS_AT]
                .iter()
                .all(|b| *b == 0)
        );
        assert!(
            img[BLOCKS_AT + lineup.len() * BLOCK..]
                .iter()
                .all(|b| *b == 0)
        );
        for i in 0..lineup.len() {
            let block = BLOCKS_AT + i * BLOCK;
            assert!(img[block..block + 0x44].iter().all(|b| *b == 0));
        }
        assert!(lineup.len() <= HOPPERS);
    }

    #[test]
    fn the_lineup_is_the_classic_one_with_a_ranked_split() {
        let lineup = playlists();
        let titles: Vec<&str> = lineup.iter().map(|p| p.title).collect();
        assert_eq!(
            titles,
            [
                "Rumble Pit",
                "Team Slayer",
                "Team Skirmish",
                "Team Snipers",
                "Big Team Battle",
                "Team Objective",
                "Free for All",
                "Unranked Teams",
                "Clanker Slayer",
                "Clanker Teams",
                "Clanker Doubles"
            ]
        );
        let kinds: Vec<HopperType> = lineup.iter().map(|p| p.rules.hopper_type()).collect();
        assert!(kinds[..6].iter().all(|k| k.ranked()));
        assert!(kinds[6..].iter().all(|k| !k.ranked()));
        // The human playlists first, then the clanker ones.
        let marked: Vec<bool> = lineup.iter().map(|p| p.clankers.is_some()).collect();
        assert_eq!(marked, (0..11).map(|i| i >= 8).collect::<Vec<bool>>());
        let ids: Vec<u32> = lineup.iter().map(|p| p.playlist_id).collect();
        assert_eq!(ids, (1..=11).collect::<Vec<u32>>());
        let file = HopperFile::default_set();
        let d = file.descriptor(2).unwrap();
        assert_eq!(
            (d.min_players(), d.max_players()),
            (Some(4), Some(4)),
            "2v2"
        );
        let d = file.descriptor(4).unwrap();
        assert_eq!((d.min_players(), d.max_players()), (Some(10), Some(16)));
        assert_eq!(d.search_timeout_ms, 300_000);
        assert_eq!(d.start_wait(10), Some(45_000));
        let d = file.descriptor(0).unwrap();
        assert_eq!(d.max_party(), Some(1), "ranked free for all is solo");
        // Every map is a retail one; every game type the lineup names shows.
        let maps = retail_map_ids();
        let mut engines: Vec<u32> = Vec::new();
        for p in &lineup {
            for e in p.entries() {
                assert!(maps.contains(&e.map_id));
                engines.push(e.variant.engine());
            }
        }
        engines.sort();
        engines.dedup();
        assert_eq!(engines, [1, 2, 3, 4, 8, 9]);
    }

    #[test]
    fn the_default_file_round_trips_byte_for_byte_and_fits_the_buffer() {
        let file = HopperFile::default_set();
        let built = file.build_checked().unwrap();
        assert!(built.len() <= MAX_FILE, "{} bytes", built.len());
        let back = HopperFile::parse(&built).unwrap();
        assert!(back.image() == file.image());
        assert_eq!(back.build(), built);
        assert_eq!(HopperFile::default_set().build(), built, "deterministic");
        // Rewriting every decoded field from what was read changes nothing.
        let mut again = back.clone();
        for i in 0..HOPPERS {
            again
                .set_descriptor(i, &back.descriptor(i).unwrap())
                .unwrap();
            again.set_entries(i, &back.entries(i).unwrap()).unwrap();
        }
        assert!(again.image() == file.image());
    }

    #[test]
    fn the_variants_are_the_stock_records_with_their_changes() {
        use variants::*;
        let ts = team_snipers();
        assert_eq!(ts.name(), "Team Snipers");
        assert!(ts.team());
        assert_eq!(ts.flags() & v::flag::MOTION_SENSOR, 0);
        assert_eq!(ts.u8_at(v::STARTING_WEAPON), 10);
        let rs = rumble_slayer();
        assert!(!rs.team());
        assert_eq!(
            (rs.i32_at(v::SCORE_TO_WIN), rs.i32_at(v::TIME_LIMIT)),
            (25, 360)
        );
        let a = assault();
        assert_eq!((a.i32_at(v::ROUNDS), a.i32_at(v::SCORE_TO_WIN)), (2, 1));
        assert_eq!(classic_ctf().u32_at(v::TYPE_AREA), 0xe);
        for r in [
            team_slayer(),
            oddball(),
            king_of_the_hill(),
            territories(),
            team_king(),
            teamball(),
        ] {
            assert_eq!(r.problems(), vec![], "{}", r.name());
            assert_eq!(r.i32_at(v::SETTING_14), 0);
        }
    }

    #[test]
    fn the_built_in_variants_are_keyed_clean_and_cover_the_lineup() {
        let all = builtin_variants();
        for (k, v) in &all {
            assert_eq!(v.problems(), vec![], "{k}");
            assert_eq!(builtin_variant(k).as_ref(), Some(v));
            assert_eq!(builtin_key(v), Some(*k));
        }
        let mut keys: Vec<&str> = all.iter().map(|(k, _)| *k).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), all.len(), "keys are unique");
        assert_eq!(builtin_variant("no_such_variant"), None);
        // Every default entry carries one of them, so a template's entries
        // can be named by key.
        for p in playlists() {
            for e in p.entries() {
                assert!(builtin_key(&e.variant).is_some(), "{}", p.title);
            }
        }
    }

    #[test]
    fn a_template_fills_an_unused_slot_the_game_takes_and_clearing_empties_it() {
        let mut file = HopperFile::default_set();
        let n = playlists().len();
        assert!(file.is_unused(n).unwrap());
        assert!(!file.is_unused(0).unwrap());
        let t = template(2).unwrap();
        assert_eq!(t.title, "Team Slayer");
        file.create_from(n, &t).unwrap();
        file.set_entries(n, &t.entries()[..1]).unwrap();
        let d = file.descriptor(n).unwrap();
        assert_eq!(check_descriptor(&d), vec![]);
        assert_eq!(d.playlist_id, 12, "the lowest id no other playlist has");
        assert_eq!(d.kind(), Some(t.rules.hopper_type()));
        assert_eq!(file.problems(&retail_map_ids()), vec![]);
        assert!(!file.is_unused(n).unwrap());
        // Clearing makes it the all-zero slot it was.
        file.clear_hopper(n).unwrap();
        assert!(file.image() == HopperFile::default_set().image());
        assert_eq!(
            file.clear_hopper(HOPPERS),
            Err(HopperError::Hopper(HOPPERS))
        );
        // Only the human playlists are templates.
        assert!(template(9).is_none(), "clanker playlists are not templates");
        assert_eq!(templates().len(), 8);
    }
}
